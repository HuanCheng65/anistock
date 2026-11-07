//! Futures orders: placing, matching on the shared layer (`crate::matching`), what a fill does to
//! positions and margin, cancelling, and the opening auction.
//!
//! Cash: an order freezes the initial margin of the part that opens a position plus its commission
//! (`FutOrder::frozen`, part of `cash_frozen`). A fill moves no notional: the opening part's margin passes
//! from the order to the position (`Position::margin`), the closing part's margin is released, and the
//! commission goes to `world`. Gains and losses are paid at the daily settlement.

use super::{Contract, FutOrder, FutTrade, FuturesChange, Position, Tick};
use crate::config::ts_add_minutes;
use crate::engine::{Engine, Placed};
use crate::exchange::{sanity_band, vcm_check, Guard, OrderError, Vcm};
use crate::matching::{self, Book, Incoming, Resting, Venue};
use crate::model::{Kind, OrderStatus, Side};

/// A futures order as the API or a bot submits it.
#[derive(Debug, Clone)]
pub struct FutOrderReq<'a> {
    pub account_id: i64,
    pub contract_id: i64,
    pub side: Side,
    pub kind: Kind,
    pub qty: i64,
    /// limit price in ticks; None for a market order
    pub price: Option<i64>,
    pub day: &'a str,
    pub ts: &'a str,
    /// false during the opening auction: a limit order only rests on the book
    pub match_: bool,
}

impl Engine {
    /// Initial margin per lot (cents): `im_rate` of the notional at the previous settlement, and at least
    /// `im_sds` standard deviations of the forecast of the average.
    pub fn fut_im_lot(&self, c: &Contract) -> i64 {
        let r = &self.futures;
        let by_price = (c.prev_settle as f64 * c.tick_value as f64 * r.im_rate).ceil() as i64;
        let sd = self.st.futures.sd.get(&c.id).copied().unwrap_or(0);
        by_price.max((sd as f64 * c.tick_value as f64 * r.im_sds).ceil() as i64)
    }

    /// Commission reserved per lot at `price`, with one cent for per-fill rounding.
    fn fut_fee_lot(&self, c: &Contract, price: i64) -> i64 {
        self.fees.commission_on(price * c.tick_value) + 1
    }

    /// Place a futures order: validate, freeze margin and commission, match, rest the rest.
    pub fn futures_place(&mut self, req: &FutOrderReq) -> Result<Placed, OrderError> {
        let r = self.do_futures_place(req);
        self.st.futures.reap();
        self.commit();
        r
    }

    pub(crate) fn do_futures_place(&mut self, req: &FutOrderReq) -> Result<Placed, OrderError> {
        if !self.futures.enabled {
            return Err(OrderError::new("期货交易还没有开放"));
        }
        let c = match self.st.futures.contracts.get(&req.contract_id) {
            Some(c) if c.trading() => c.clone(),
            _ => return Err(OrderError::new("这个合约不存在或已停止交易")),
        };
        if req.qty <= 0 {
            return Err(OrderError::new("数量必须是正整数"));
        }
        let (_, hi) = sanity_band(&c, &self.rules);
        let price = match (req.kind, req.price) {
            (Kind::Limit, Some(p)) if p > 0 && p <= hi => Some(p),
            (Kind::Limit, Some(p)) if p > hi => return Err(OrderError::new("价格太离谱了：超出了允许的范围")),
            (Kind::Limit, _) => return Err(OrderError::new("限价单需要价格")),
            (Kind::Market, _) if !req.match_ => return Err(OrderError::new("集合竞价阶段只接受限价单")),
            (Kind::Market, _) => None,
        };
        let opening = req.qty - req.qty.min(self.fut_closable(req.account_id, &c, req.side));
        let frozen = opening * self.fut_im_lot(&c) + req.qty * self.fut_fee_lot(&c, price.unwrap_or(hi));
        if self.st.accounts.get(&req.account_id).is_some_and(|a| a.kind == "player") {
            self.fut_player_check(req, &c, opening, frozen)?; // may finance what free cash does not cover
        }
        if self.available_cash(req.account_id) < frozen {
            return Err(OrderError::new("可用保证金不足"));
        }
        self.add_cash(req.account_id, 0, frozen);

        let f = &mut self.st.futures;
        f.last_order_id += 1;
        let o = FutOrder {
            id: f.last_order_id,
            account_id: req.account_id,
            contract_id: c.id,
            side: req.side,
            kind: req.kind,
            price,
            qty: req.qty,
            filled: 0,
            frozen,
            status: OrderStatus::Open,
            created_at: req.ts.to_string(),
            updated_at: req.ts.to_string(),
        };
        let id = o.id;
        f.insert_order(o.clone());
        self.fut_out(FuturesChange::Order(o.clone()));

        let (filled, notional) = if req.match_ {
            let (lo, hi) = sanity_band(&c, &self.rules);
            let limit = price.unwrap_or(if req.side == Side::Buy { hi } else { lo });
            let incoming = Incoming { id, account: req.account_id, side: req.side, limit, left: req.qty };
            matching::sweep(&mut FuturesVenue { e: self, contract_id: c.id, day: req.day, ts: req.ts }, incoming)
        } else {
            (0, 0)
        };
        let o = &self.st.futures.orders[&id];
        if o.filled == o.qty {
            self.fut_close(id, OrderStatus::Filled, req.ts);
        } else if o.kind == Kind::Market {
            self.fut_close(id, OrderStatus::Cancelled, req.ts); // immediate-or-cancel remainder
        } else {
            self.st.futures.rest(id);
        }
        Ok(Placed {
            order_id: id,
            status: self.st.futures.orders[&id].status,
            filled,
            avg_price: (filled > 0).then(|| crate::exchange::py_round_div(notional, filled)),
        })
    }

    /// Lots an order on `side` can close: the opposite position, less what the account's other open orders
    /// on that side may already close.
    fn fut_closable(&self, account_id: i64, c: &Contract, side: Side) -> i64 {
        let pos = self.st.futures.position(account_id, c.id);
        let against = match side {
            Side::Buy => (-pos).max(0),
            Side::Sell => pos.max(0),
        };
        let pending: i64 =
            self.st.futures.open_orders(account_id, c.id).filter(|o| o.side == side).map(FutOrder::left).sum();
        (against - pending).max(0)
    }

    /// Cancel an open futures order of `account_id`.
    pub fn futures_cancel(&mut self, account_id: i64, order_id: i64, ts: &str) -> Result<(), OrderError> {
        let mine = self
            .st
            .futures
            .orders
            .get(&order_id)
            .is_some_and(|o| o.account_id == account_id && o.status == OrderStatus::Open);
        if !mine {
            return Err(OrderError::new("找不到这笔委托，或它已经结束"));
        }
        self.fut_close(order_id, OrderStatus::Cancelled, ts);
        self.st.futures.reap();
        self.commit();
        Ok(())
    }

    /// End an order: release what it still has frozen and take it off the book.
    pub(crate) fn fut_close(&mut self, id: i64, status: OrderStatus, ts: &str) {
        let f = &mut self.st.futures;
        f.unlist_order(id);
        let o = f.orders.get_mut(&id).unwrap();
        let (account, frozen) = (o.account_id, o.frozen);
        o.status = status;
        o.frozen = 0;
        o.updated_at = ts.to_string();
        let row = o.clone();
        self.add_cash(account, 0, -frozen);
        self.fut_out(FuturesChange::Order(row));
    }

    /// May a futures trade print at `tp` now? The stock rules on the previous settlement price; a new
    /// cooling-off period is recorded on the contract (no news item).
    fn fut_allow(&mut self, contract_id: i64, tp: i64, day: &str, ts: &str) -> bool {
        let window = self.rules.vcm_window_min;
        let f = &self.st.futures;
        let lookback = || {
            let before = ts_add_minutes(ts, -window);
            let (_, ticks) = f.today.get(&contract_id).filter(|(d, _)| d == day)?;
            ticks.iter().rev().find(|t| t.ts.as_str() <= before.as_str()).map(|t| t.price)
        };
        let verdict = vcm_check(&f.contracts[&contract_id], ts, tp, &self.rules, lookback);
        let Vcm::Trigger { low, high, .. } = verdict else { return verdict == Vcm::Allow };
        let c = self.st.futures.contracts.get_mut(&contract_id).unwrap();
        c.start_cooling(ts_add_minutes(ts, self.rules.vcm_cool_min), low, high);
        let row = c.clone();
        self.fut_out(FuturesChange::Contract(row));
        low <= tp && tp <= high
    }

    /// Settle one futures trade of `q` lots at `tp` between two orders.
    fn fut_fill(&mut self, buy_id: i64, sell_id: i64, q: i64, tp: i64, day: &str, ts: &str) {
        let contract_id = self.st.futures.orders[&buy_id].contract_id;
        let f = &mut self.st.futures;
        f.last_trade_id += 1;
        let trade = FutTrade {
            id: f.last_trade_id,
            contract_id,
            price: tp,
            qty: q,
            buyer_id: f.orders[&buy_id].account_id,
            seller_id: f.orders[&sell_id].account_id,
            buy_order: buy_id,
            sell_order: sell_id,
            ts: ts.to_string(),
            day: day.to_string(),
        };
        let today = f.today.entry(contract_id).or_insert_with(|| (day.to_string(), vec![]));
        if today.0 != day {
            *today = (day.to_string(), vec![]);
        }
        today.1.push(Tick { ts: ts.to_string(), price: tp, qty: q });
        let c = f.contracts.get_mut(&contract_id).unwrap();
        c.last_price = tp;
        let c = c.clone();
        self.fut_out(FuturesChange::Trade(trade.clone()));
        self.fut_out(FuturesChange::Contract(c.clone()));
        self.fut_fill_side(&c, buy_id, q, tp, &trade, ts);
        self.fut_fill_side(&c, sell_id, -q, tp, &trade, ts);
    }

    /// One side of a fill: `d` lots (signed) at `tp` into the order's account.
    fn fut_fill_side(&mut self, c: &Contract, order_id: i64, d: i64, tp: i64, trade: &FutTrade, ts: &str) {
        let world = self.special("world").expect("no world account");
        let im_lot = self.fut_im_lot(c);
        let fee_lot = self.fut_fee_lot(c, tp);
        let o = self.st.futures.orders[&order_id].clone();
        let key = (o.account_id, c.id);
        let mut pos = self.st.futures.positions.get(&key).copied().unwrap_or(Position {
            account_id: o.account_id,
            contract_id: c.id,
            qty: 0,
            basis: 0,
            margin: 0,
        });
        let closing = if pos.qty * d < 0 { d.abs().min(pos.qty.abs()) } else { 0 };
        let opening = d.abs() - closing;

        // the closing part's margin is released
        let released = if closing > 0 { pos.margin * closing / pos.qty.abs() } else { 0 };
        pos.margin -= released;
        pos.qty += d;
        pos.basis += d * tp;

        // commission out of the order's reserve
        let fee = self.fees.commission_on(d.abs() * tp * c.tick_value);
        let mut frozen = o.frozen;
        let from_order = fee.min(frozen);
        frozen -= from_order;
        // the opening part's margin passes from the order to the position, keeping the reserve the rest
        // of the order still needs for its commission; a shortfall comes out of free cash if there is any
        let filled = o.filled + d.abs();
        let keep = (o.qty - filled) * fee_lot;
        let need = opening * im_lot;
        let moved = need.min((frozen - keep).max(0));
        frozen -= moved;
        self.add_cash(o.account_id, 0, -released - from_order);
        let extra = (need - moved).min(self.available_cash(o.account_id).max(0));
        self.add_cash(o.account_id, 0, extra);
        pos.margin += moved + extra;
        self.do_transfer(
            o.account_id,
            world,
            fee,
            "futures_commission",
            &trade.day,
            ts,
            Some(c.company_id),
            Some(trade.id),
        );

        let o = self.st.futures.orders.get_mut(&order_id).unwrap();
        o.frozen = frozen;
        o.filled = filled;
        o.updated_at = ts.to_string();
        let (row, done) = (o.clone(), o.filled == o.qty);
        self.fut_out(FuturesChange::Order(row));
        if done {
            self.fut_close(order_id, OrderStatus::Filled, ts);
        }
        self.fut_put_position(pos);
    }

    /// Store a position; a flat one with nothing left to settle is deleted.
    pub(crate) fn fut_put_position(&mut self, p: Position) {
        let key = (p.account_id, p.contract_id);
        if p.qty == 0 && p.basis == 0 && p.margin == 0 {
            if self.st.futures.positions.remove(&key).is_some() {
                self.fut_out(FuturesChange::PositionDelete(key.0, key.1));
            }
        } else {
            self.st.futures.positions.insert(key, p);
            self.fut_out(FuturesChange::Position(p));
        }
    }

    /// The futures opening auction, after the stocks' (`Engine::open_market`): clear each listed contract at
    /// one price, set the day's VCM anchor and reset its cooling-off state.
    pub(crate) fn futures_open(&mut self, day: &str, ts: &str) {
        if !self.futures.enabled {
            return;
        }
        let ids: Vec<i64> = self.st.futures.contracts.values().filter(|c| c.trading()).map(|c| c.id).collect();
        for id in ids {
            let f = &self.st.futures;
            let prev = f.contracts[&id].prev_settle;
            let (p, vol) = f.book(id).auction(|o| f.orders[&o].left(), prev);
            let traded = match p {
                Some(p) => matching::uncross(&mut FuturesVenue { e: self, contract_id: id, day, ts }, p, vol),
                None => 0,
            };
            let c = self.st.futures.contracts.get_mut(&id).unwrap();
            c.open_ref = Some(if traded > 0 { p.unwrap() } else { prev });
            c.vcm_until = None;
            c.vcm_count = 0;
            let row = c.clone();
            self.fut_out(FuturesChange::Contract(row));
        }
        self.st.futures.reap();
    }

    /// Futures invariants: every contract's positions net to zero lots and zero basis.
    pub(crate) fn futures_invariants(&self) -> Result<(), String> {
        let mut net: std::collections::BTreeMap<i64, (i64, i64)> = Default::default();
        for p in self.st.futures.positions.values() {
            let e = net.entry(p.contract_id).or_default();
            e.0 += p.qty;
            e.1 += p.basis;
        }
        match net.into_iter().find(|(_, n)| *n != (0, 0)) {
            Some((c, (q, b))) => Err(format!("futures contract {c} positions do not net out: {q} lots, basis {b}")),
            None => Ok(()),
        }
    }
}

/// One contract's market for the matcher: futures clearing (`Engine::fut_fill`) and the contract's
/// volatility control. Margin is frozen when the order is placed, so the matcher takes what it wants.
struct FuturesVenue<'e, 'a> {
    e: &'e mut Engine,
    contract_id: i64,
    day: &'a str,
    ts: &'a str,
}

impl Venue for FuturesVenue<'_, '_> {
    fn book(&self) -> &Book {
        self.e.st.futures.book(self.contract_id)
    }

    fn resting(&self, id: i64) -> Option<Resting> {
        let o = self.e.st.futures.orders.get(&id).filter(|o| o.status == OrderStatus::Open)?;
        Some(Resting { account: o.account_id, left: o.left() })
    }

    fn allow(&mut self, price: i64) -> bool {
        self.e.fut_allow(self.contract_id, price, self.day, self.ts)
    }

    fn capacity(&mut self, _price: i64, want: i64) -> i64 {
        want
    }

    fn fill(&mut self, buy: i64, sell: i64, qty: i64, price: i64) {
        self.e.fut_fill(buy, sell, qty, price, self.day, self.ts);
    }
}
