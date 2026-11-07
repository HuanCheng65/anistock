//! Futures and the credit account (信用账户, docs/v2-margin.md; docs/v3-futures-impl.md §1):
//!
//! - a player trades futures from an open credit account; while it is under a margin call or liquidating it
//!   may only close positions;
//! - the initial margin is cash. When free cash is short, the credit account finances the rest against its
//!   collateral (stocks at their haircut), like a margin buy: a loan from world, a financing debt in the
//!   contract's company, interest at the financing rate;
//! - a settlement loss the account cannot pay is financed the same way (then the maintenance check decides);
//! - the maintenance ratio counts futures losses and the maintenance margin (`CreditNumbers`), and forced
//!   liquidation closes futures first (`futures_liquidate`);
//! - position caps: one contract's notional and all futures' initial margin, as shares of net assets.

use super::trading::FutOrderReq;
use super::Contract;
use crate::engine::Engine;
use crate::exchange::{pct0_str, quote_band, OrderError};
use crate::margin::{Debt, NORMAL};
use crate::model::{Kind, OrderStatus, Side};

/// An account's futures in cents, at last prices.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FuturesNumbers {
    /// initial margin held: positions and open orders (part of the account's cash)
    pub held: i64,
    /// losses since the last settlement (gains are not counted until they are paid)
    pub loss: i64,
    /// maintenance margin of the positions at the current initial margin
    pub maintenance: i64,
}

impl Engine {
    pub fn futures_numbers(&self, account_id: i64) -> FuturesNumbers {
        let f = &self.st.futures;
        let mut n = FuturesNumbers::default();
        for p in f.positions.range((account_id, i64::MIN)..=(account_id, i64::MAX)).map(|(_, p)| p) {
            let Some(c) = f.contracts.get(&p.contract_id) else { continue };
            n.held += p.margin;
            n.loss += (-p.variation(c.last_price)).max(0) * c.tick_value;
            n.maintenance += (p.qty.abs() as f64 * self.fut_im_lot(c) as f64 * self.futures.maintenance).ceil() as i64;
        }
        n.held += f.by_account.get(&account_id).into_iter().flatten().map(|id| f.orders[id].frozen).sum::<i64>();
        n
    }

    /// Borrow `amount` for a futures payment against the credit account's collateral.
    pub(crate) fn fut_finance(&mut self, account_id: i64, company_id: i64, amount: i64, day: &str, ts: &str) {
        let world = self.special("world").expect("no world account");
        self.do_transfer(world, account_id, amount, "margin_loan", day, ts, Some(company_id), None);
        let d = self.st.margin.debts.entry((account_id, company_id)).or_insert_with(|| Debt {
            account_id,
            company_id,
            principal: 0,
            interest: 0,
            opened_on: day.to_string(),
            qty: 0,
        });
        d.principal += amount;
        self.emit_debt((account_id, company_id));
    }

    /// A player's order: the credit account, the caps, and financing for the part of `frozen` free cash does
    /// not cover. `opening` lots open a position; an order that only closes is always allowed (its commission
    /// is financed if need be: closing must stay possible, forced liquidation included).
    pub(crate) fn fut_player_check(
        &mut self,
        req: &FutOrderReq,
        c: &Contract,
        opening: i64,
        frozen: i64,
    ) -> Result<(), OrderError> {
        let acct = req.account_id;
        let Some(credit) = self.credit(acct) else {
            return Err(OrderError::new("期货交易需要先开通信用账户"));
        };
        let borrow = (frozen - self.available_cash(acct)).max(0);
        if opening > 0 {
            if credit.status != NORMAL {
                return Err(OrderError::new("信用账户在追保或强平中，只能平仓"));
            }
            let net = self.net_assets(acct).max(0) as f64;
            let im = self.fut_im_lot(c);
            let pending: i64 =
                self.st.futures.open_orders(acct, c.id).filter(|o| o.side == req.side).map(|o| o.left()).sum();
            let lots = self.st.futures.position(acct, c.id).abs() + pending + opening;
            let rules = &self.futures;
            if (lots * c.prev_settle * c.tick_value) as f64 > rules.position_cap * net {
                return Err(OrderError(format!("单个合约的持仓不能超过净资产的 {}", pct0_str(rules.position_cap))));
            }
            if (self.futures_numbers(acct).held + opening * im) as f64 > rules.margin_cap * net {
                return Err(OrderError(format!("期货保证金占用不能超过净资产的 {}", pct0_str(rules.margin_cap))));
            }
            // after this order (and its financing) the available margin must stay >= 0
            let avail = self.credit_numbers(acct, req.day).available_margin as f64;
            let after = avail - frozen as f64 - self.margin.margin_ratio * borrow as f64;
            if after < 0.0 {
                return Err(OrderError::new("可用保证金不足"));
            }
        } else if credit.status == crate::margin::DEFAULTED {
            return Err(OrderError::new("信用账户已穿仓"));
        }
        if borrow > 0 {
            self.fut_finance(acct, c.company_id, borrow, req.day, req.ts);
        }
        Ok(())
    }

    /// 强制平仓, futures first: cancel the account's futures orders and close every position with protected
    /// immediate-or-cancel orders (last price ± `protect`, within the cooling-off band), largest first.
    pub(crate) fn futures_liquidate(&mut self, acct: i64, day: &str, ts: &str) {
        let orders: Vec<i64> = self.st.futures.by_account.get(&acct).into_iter().flatten().copied().collect();
        for id in orders {
            self.fut_close(id, OrderStatus::Cancelled, ts);
        }
        let f = &self.st.futures;
        let mut positions: Vec<(i64, i64, i64)> = (f.positions.range((acct, i64::MIN)..=(acct, i64::MAX)))
            .filter(|(_, p)| p.qty != 0)
            .filter_map(|(_, p)| {
                f.contracts
                    .get(&p.contract_id)
                    .filter(|c| c.trading())
                    .map(|c| (p.qty.abs() * c.last_price, p.contract_id, p.qty))
            })
            .collect();
        positions.sort_by(|a, b| b.cmp(a));
        for (_, id, qty) in positions {
            let c = &self.st.futures.contracts[&id];
            let (lo, hi) = quote_band(c, ts, &self.rules);
            let protect = self.margin.protect;
            let (side, price) = if qty > 0 {
                (Side::Sell, ((c.last_price as f64 * (1.0 - protect)).floor() as i64).max(lo).max(1))
            } else {
                (Side::Buy, ((c.last_price as f64 * (1.0 + protect)).ceil() as i64).min(hi))
            };
            let req = FutOrderReq {
                account_id: acct,
                contract_id: id,
                side,
                kind: Kind::Limit,
                qty: qty.abs(),
                price: Some(price),
                day,
                ts,
                match_: true,
            };
            if let Ok(p) = self.do_futures_place(&req) {
                if p.status == OrderStatus::Open {
                    self.fut_close(p.order_id, OrderStatus::Cancelled, ts);
                }
            }
        }
        self.st.futures.reap();
    }
}
