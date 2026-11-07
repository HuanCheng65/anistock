//! The futures part of the daily close (docs/v3-futures.md §4.1–4.2):
//!
//! 1. the day's orders expire;
//! 2. the day's heat index is published into the game database (frozen);
//! 3. each contract gets a settlement price: the volume-weighted average of the trades in the session's last
//!    `settle_window_min` minutes, else the market maker's last mid quote, else the previous settlement; a
//!    contract whose period is over and whose index values are all published settles on their average instead;
//! 4. every position is marked to it: losers pay the clearing account, then it pays the winners (the
//!    variation margins sum to zero, so it ends the day at zero), and the initial margin is reset at the new
//!    price;
//! 5. a contract whose period is over but has missing index values stops trading and waits (expiring).

use std::collections::BTreeMap;

use super::{ContractStatus, FutDay, FuturesChange, HeatRow, SettleSource};
use crate::config::ts_add_minutes;
use crate::engine::Engine;
use crate::inputs::HeatIndex;
use crate::model::OrderStatus;

/// What the futures close did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FuturesClose {
    /// contracts settled on their period's average today
    pub settled: Vec<i64>,
    /// cents paid from losers to winners
    pub variation: i64,
    /// cents of losses an account could not pay (covered by world; see `cover_shortfall`)
    pub shortfall: i64,
}

impl Engine {
    /// The futures close of trading day `day`. `session_end` is when the day's session ended (orders created
    /// before it expire); `heat` is the day's published index per work (the inputs file).
    pub(crate) fn futures_close(
        &mut self,
        day: &str,
        ts: &str,
        session_end: &str,
        heat: &BTreeMap<i64, HeatIndex>,
    ) -> FuturesClose {
        let mut out = FuturesClose::default();
        if !self.futures.enabled && self.st.futures.contracts.is_empty() {
            return out;
        }
        self.fut_expire_orders(session_end, ts);
        self.fut_publish_heat(day, heat);
        let ids: Vec<i64> = self.st.futures.contracts.keys().copied().collect();
        for id in ids {
            let c = &self.st.futures.contracts[&id];
            let over = c.last_day.as_str() <= day;
            let (price, source) = match (over, self.fut_period_average(id)) {
                (true, Some(avg)) => (avg, SettleSource::Final),
                _ => self.fut_settlement_price(id, day, session_end),
            };
            let (paid, short) = self.fut_mark(id, price, day, ts);
            out.variation += paid;
            out.shortfall += short;
            self.fut_record_day(id, day, price, source);
            let c = self.st.futures.contracts.get_mut(&id).unwrap();
            c.prev_settle = price;
            c.open_ref = None;
            c.vcm_until = None;
            c.vcm_count = 0;
            if source == SettleSource::Final {
                c.status = ContractStatus::Settled;
                c.final_price = Some(price);
                c.settled_on = Some(day.to_string());
                out.settled.push(id);
            } else if over {
                c.status = ContractStatus::Expiring;
            }
            let row = c.clone();
            self.fut_out(FuturesChange::Contract(row));
            if source == SettleSource::Final {
                self.fut_retire(id);
            }
        }
        out
    }

    /// Orders are good for the day: those placed before the session's end expire.
    fn fut_expire_orders(&mut self, session_end: &str, ts: &str) {
        let mut ids: Vec<i64> = (self.st.futures.orders.values())
            .filter(|o| o.status == OrderStatus::Open && o.created_at.as_str() < session_end)
            .map(|o| o.id)
            .collect();
        ids.sort();
        for id in ids {
            self.fut_close(id, OrderStatus::Cancelled, ts);
        }
        self.st.futures.reap();
    }

    /// Freeze the day's index of every work with contracts into the game database.
    fn fut_publish_heat(&mut self, day: &str, heat: &BTreeMap<i64, HeatIndex>) {
        let works: std::collections::BTreeSet<i64> = self.st.futures.contracts.values().map(|c| c.work_id).collect();
        for w in works {
            let Some(h) = heat.get(&w) else { continue };
            self.st.futures.heat.insert((w, day.to_string()), h.idx);
            self.fut_out(FuturesChange::Heat(HeatRow {
                work_id: w,
                day: day.to_string(),
                idx: h.idx,
                us: h.us,
                cn: h.cn,
                jp: h.jp,
                flags: serde_json::to_string(&h.flags).unwrap_or_else(|_| "{}".into()),
                version: h.version.clone(),
            }));
        }
    }

    /// The average index over the contract's period in ticks, once every day of it is published.
    fn fut_period_average(&self, id: i64) -> Option<i64> {
        let c = &self.st.futures.contracts[&id];
        let f = &self.st.futures;
        let days = f.heat.range((c.work_id, c.first_day.clone())..=(c.work_id, c.last_day.clone()));
        let values: Vec<f64> = days.map(|(_, v)| *v).collect();
        let first = chrono::NaiveDate::parse_from_str(&c.first_day, "%Y-%m-%d").ok()?;
        let last = chrono::NaiveDate::parse_from_str(&c.last_day, "%Y-%m-%d").ok()?;
        let n = (last - first).num_days() + 1;
        (values.len() as i64 == n).then(|| (values.iter().sum::<f64>() / n as f64 * 100.0).round() as i64)
    }

    /// The day's settlement price: VWAP of the session's last minutes, else the last mid quote, else the
    /// previous settlement.
    fn fut_settlement_price(&self, id: i64, day: &str, session_end: &str) -> (i64, SettleSource) {
        let f = &self.st.futures;
        let from = ts_add_minutes(session_end, -self.futures.settle_window_min);
        let window = f.today.get(&id).filter(|(d, _)| d == day).map(|(_, ticks)| {
            ticks
                .iter()
                .filter(|t| t.ts.as_str() >= from.as_str() && t.ts.as_str() < session_end)
                .fold((0, 0), |(n, q), t| (n + t.price * t.qty, q + t.qty))
        });
        match (window, f.last_mid.get(&id)) {
            (Some((notional, qty)), _) if qty > 0 => (crate::exchange::py_round_div(notional, qty), SettleSource::Vwap),
            (_, Some(&mid)) => (mid, SettleSource::Mid),
            _ => (f.contracts[&id].prev_settle, SettleSource::Prev),
        }
    }

    /// Mark every position on the contract to `price`: pay the variation margins through the clearing
    /// account and reset each position's initial margin at the new price. Returns (cents paid by losers,
    /// cents they could not pay).
    fn fut_mark(&mut self, id: i64, price: i64, day: &str, ts: &str) -> (i64, i64) {
        let c = self.st.futures.contracts[&id].clone();
        let clearing = self.clearing_account();
        let positions: Vec<_> = self.st.futures.positions.values().filter(|p| p.contract_id == id).copied().collect();
        // the margin held is the account's own cash: release it before paying
        for p in &positions {
            self.add_cash(p.account_id, 0, -p.margin);
        }
        let (mut paid, mut short) = (0, 0);
        let losers = positions.iter().filter(|p| p.variation(price) < 0);
        for p in losers {
            let loss = -p.variation(price) * c.tick_value;
            let can =
                loss.min(self.st.accounts[&p.account_id].cash - self.st.accounts[&p.account_id].cash_frozen).max(0);
            self.do_transfer(p.account_id, clearing, can, "futures_mtm", day, ts, Some(c.company_id), Some(id));
            if can < loss {
                self.cover_shortfall(p.account_id, clearing, loss - can, day, ts, id);
                short += loss - can;
            }
            paid += loss;
        }
        for p in positions.iter().filter(|p| p.variation(price) > 0) {
            let gain = p.variation(price) * c.tick_value;
            self.do_transfer(clearing, p.account_id, gain, "futures_mtm", day, ts, Some(c.company_id), Some(id));
        }
        // new basis at the settlement price; initial margin again at the new price, as far as free cash allows
        // (what is missing shows in the maintenance check)
        let mut settled = c.clone();
        settled.prev_settle = price;
        let im_lot = self.fut_im_lot(&settled);
        for p in positions {
            let need = p.qty.abs() * im_lot;
            let margin = need.min(self.available_cash(p.account_id).max(0));
            self.add_cash(p.account_id, 0, margin);
            self.fut_put_position(super::Position { qty: p.qty, basis: p.qty * price, margin, ..p });
        }
        (paid, short)
    }

    /// A loss the account cannot pay: world pays it into clearing so the winners are paid in full, and it
    /// stays on the account as bad debt where it has a credit account (written off on restructuring).
    fn cover_shortfall(&mut self, account_id: i64, clearing: i64, amount: i64, day: &str, ts: &str, contract_id: i64) {
        let world = self.special("world").expect("no world account");
        let company = self.st.futures.contracts[&contract_id].company_id;
        self.do_transfer(world, clearing, amount, "futures_default", day, ts, Some(company), Some(contract_id));
        self.margin_add_bad_debt(account_id, amount);
    }

    /// The clearing account (created on first use): receives variation margin from losers and pays winners;
    /// zero after every close. Not part of the money stock.
    fn clearing_account(&mut self) -> i64 {
        match self.special("clearing") {
            Some(id) => id,
            None => self.do_create_account("clearing", "clearing", None, None, None, "1970-01-01T00:00:00"),
        }
    }

    fn fut_record_day(&mut self, id: i64, day: &str, settle: i64, source: SettleSource) {
        let f = &self.st.futures;
        let ticks = f.today.get(&id).filter(|(d, _)| d == day).map(|(_, t)| t.as_slice()).unwrap_or(&[]);
        let ohlc = (!ticks.is_empty()).then(|| {
            let ps = ticks.iter().map(|t| t.price);
            (ticks[0].price, ps.clone().max().unwrap(), ps.min().unwrap(), ticks[ticks.len() - 1].price)
        });
        let row = FutDay {
            contract_id: id,
            day: day.to_string(),
            ohlc,
            volume: ticks.iter().map(|t| t.qty).sum(),
            settle,
            source,
            open_interest: f.open_interest(id),
        };
        self.fut_out(FuturesChange::Day(row));
    }

    /// A settled contract leaves the state: its (flat) positions and its index days.
    fn fut_retire(&mut self, id: i64) {
        let keys: Vec<_> = self.st.futures.positions.keys().filter(|k| k.1 == id).copied().collect();
        for (a, c) in keys {
            let p = self.st.futures.positions.remove(&(a, c)).unwrap();
            self.add_cash(a, 0, -p.margin);
            self.fut_out(FuturesChange::PositionDelete(a, c));
        }
        let f = &mut self.st.futures;
        let work = f.contracts.remove(&id).map(|c| c.work_id);
        f.books.remove(&id);
        f.today.remove(&id);
        f.last_mid.remove(&id);
        f.sd.remove(&id);
        if let Some(w) = work.filter(|w| !f.contracts.values().any(|c| c.work_id == *w)) {
            f.heat.retain(|(x, _), _| *x != w);
        }
    }
}
