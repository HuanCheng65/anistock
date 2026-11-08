//! 指数基金 exploit tests (docs/design/funds.md §6.1), a submodule of `bots`. Step 1 has two:
//!
//! - `fund_hold`: subscribes all its cash to the 综合指数基金 on its first day and holds the units (against
//!   `buy_hold_cap`: what the fund costs and how closely it tracks);
//! - `ipo_front_run`: buys each new listing on its first session (a fifth of its cash each, limit 0.5% over the
//!   last price, every 30 minutes until filled) and sells it all on the next session, the first one in which the
//!   fund buys the stock: into the opening auction (`exec = "auction"`, limit 3% under the close: sure to fill) or
//!   at 12:00, after the fund's first batches (limit 1% under the last price).

use super::{place, Bot, BotKind};
use crate::engine::{Engine, NewOrder};
use crate::market::{day_of, local, ts_of};

pub fn is_funds(k: BotKind) -> bool {
    matches!(k, BotKind::FundHold | BotKind::IpoFrontRun)
}

/// The fund the bots use.
pub const KEY: &str = "all";

impl Bot {
    /// Continuous trading, every minute.
    pub(super) fn funds_minute(&mut self, e: &mut Engine, t: i64) {
        let (_, s) = local(t);
        let (day, ts) = (day_of(t), ts_of(t));
        match self.kind {
            BotKind::FundHold => {
                if !self.bought_initial {
                    let cash = e.available_cash(self.account);
                    if e.fund_subscribe(self.account, KEY, cash, &day, &ts).is_ok() {
                        self.bought_initial = true;
                        self.trades_placed += 1;
                    }
                }
            }
            BotKind::IpoFrontRun => {
                if s == 12 * 3600 && e.funds.exec != "auction" {
                    let held: Vec<(i64, i64, i64)> = e
                        .holdings_of(self.account)
                        .filter(|h| h.qty > h.qty_frozen)
                        .filter_map(|h| {
                            e.company(h.company_id).filter(|c| c.status == "listed" && c.listed_on < day).map(|c| (c.id, h.qty - h.qty_frozen, c.last_price))
                        })
                        .collect();
                    for (c, q, p) in held {
                        e.cancel_all(self.account, c, &ts);
                        if place(e, self.account, c, "sell", "limit", q, Some((p * 99 / 100).max(1)), t) {
                            self.trades_placed += 1;
                        }
                    }
                }
                if s % 1800 != 0 {
                    return;
                }
                let new: Vec<(i64, i64)> = e
                    .companies()
                    .filter(|c| c.status == "listed" && c.listed_on == day && c.last_price > 0)
                    .map(|c| (c.id, c.last_price))
                    .collect();
                let budget = self.start_cash / 5;
                for (c, p) in new {
                    let have = e.holding(self.account, c).map_or(0, |h| h.qty) * p;
                    if have >= budget * 8 / 10 {
                        continue;
                    }
                    e.cancel_all(self.account, c, &ts);
                    let lim = p * 1005 / 1000 + 1;
                    let q = (budget - have).min(e.available_cash(self.account)) / e.fees.buy_reserve(lim, 1).max(1);
                    if place(e, self.account, c, "buy", "limit", q, Some(lim), t) {
                        self.trades_placed += 1;
                    }
                }
            }
            _ => {}
        }
    }

    /// The opening auction (08:55): `ipo_front_run` sells what it bought the session before.
    pub fn funds_auction(&mut self, e: &mut Engine, t: i64) {
        if self.kind != BotKind::IpoFrontRun || e.funds.exec != "auction" {
            return;
        }
        let (day, ts) = (day_of(t), ts_of(t));
        let held: Vec<(i64, i64, i64)> = e
            .holdings_of(self.account)
            .filter(|h| h.qty > h.qty_frozen)
            .filter_map(|h| e.company(h.company_id).filter(|c| c.status == "listed").map(|c| (c.id, h.qty - h.qty_frozen, c.prev_close)))
            .collect();
        for (c, q, pc) in held {
            e.cancel_all(self.account, c, &ts);
            let lim = (pc * 97 / 100).max(1);
            let ok = e
                .place_order(&NewOrder { account_id: self.account, company_id: c, side: "sell", kind: "limit", qty: q, price: Some(lim), day: &day, ts: &ts, match_: false })
                .is_ok();
            if ok {
                self.trades_placed += 1;
            }
        }
    }
}
