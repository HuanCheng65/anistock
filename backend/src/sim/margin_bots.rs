//! The 融资融券 exploit tests (docs/v2-margin.md §9): a submodule of `bots` so the strategies share the
//! bot's state. Each runs as a player with the starting cash, opens credit on its first day (the bots start
//! in the second week, after the account is old enough) and trades through the same engine.

use serde::Serialize;

use super::{affordable, place, Bot, BotKind};
use crate::engine::{Engine, NewOrder};
use crate::margin::Mode;
use crate::market::{day_of, local, ts_of};

/// Risk figures every bot keeps (sampled every 30 minutes): the worst drawdown of its net assets, the most
/// it owed, and how often it was under a margin call or forced liquidation.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MStats {
    pub peak: i64,
    pub max_drawdown: f64,
    pub max_liabilities: i64,
    pub calls: u32,
    pub liquidations: u32,
    pub defaulted: bool,
    #[serde(skip)]
    last_status: String,
}

impl MStats {
    pub fn sample(&mut self, e: &Engine, acct: i64) {
        let w = e.net_assets(acct);
        self.peak = self.peak.max(w);
        if self.peak > 0 {
            self.max_drawdown = self.max_drawdown.max(1.0 - w as f64 / self.peak as f64);
        }
        self.max_liabilities = self.max_liabilities.max(e.liabilities(acct));
        let status = e.credit(acct).map_or("", |c| c.status.as_str()).to_string();
        if status != self.last_status {
            match status.as_str() {
                crate::margin::CALL => self.calls += 1,
                crate::margin::LIQUIDATING => self.liquidations += 1,
                crate::margin::DEFAULTED => self.defaulted = true,
                _ => {}
            }
            self.last_status = status;
        }
    }
}

fn credit(e: &mut Engine, acct: i64, c: i64, mode: Mode, qty: i64, price: i64, t: i64) -> bool {
    if qty <= 0 || price <= 0 {
        return false;
    }
    let (day, ts) = (day_of(t), ts_of(t));
    if e.state("auction_day") != Some(day.as_str()) {
        e.open_market(&day, &ts);
    }
    let req = NewOrder { account_id: acct, company_id: c, side: mode.side().as_str(), kind: "limit", qty, price: Some(price), day: &day, ts: &ts, match_: true };
    e.place_credit(&req, mode).is_ok()
}

fn eligible(e: &Engine, day: &str) -> Vec<i64> {
    let mut v: Vec<i64> = e.companies().filter(|c| c.status == "listed").map(|c| c.id).filter(|c| e.margin_ineligible(*c, day).is_none()).collect();
    v.sort_unstable();
    v
}

/// Cover every short position at up to 1% over the last price.
fn cover_all(b: &mut Bot, e: &mut Engine, t: i64) {
    let acct = b.account;
    let ts = ts_of(t);
    let owed: Vec<(i64, i64)> = e.shorts_of(acct).filter(|s| s.qty > 0).map(|s| (s.company_id, s.qty)).collect();
    for (c, q) in owed {
        e.cancel_all(acct, c, &ts);
        let p = e.company(c).map_or(0, |x| x.last_price);
        if credit(e, acct, c, Mode::BuyReturn, q, p * 101 / 100 + 1, t) {
            b.trades_placed += 1;
        }
    }
}

/// Short `targets` with an equal share of `frac` of the available margin, at the last price.
fn short_each(b: &mut Bot, e: &mut Engine, targets: &[i64], frac: f64, t: i64) {
    let acct = b.account;
    let day = day_of(t);
    if targets.is_empty() {
        return;
    }
    let budget = e.credit_numbers(acct, &day).available_margin as f64 * frac / targets.len() as f64;
    for &c in targets {
        if e.short_qty(acct, c) > 0 || e.holding(acct, c).is_some_and(|h| h.qty > 0) {
            continue;
        }
        let p = e.company(c).map_or(0, |x| x.last_price);
        if p <= 0 {
            continue;
        }
        let q = ((budget / p as f64) as i64).min(e.lendable(c));
        if credit(e, acct, c, Mode::ShortSell, q, p, t) {
            b.trades_placed += 1;
        }
    }
}

pub fn on_minute(b: &mut Bot, e: &mut Engine, t: i64) {
    let (d, s) = local(t);
    let acct = b.account;
    let day = day_of(t);
    let ts = ts_of(t);
    if e.credit(acct).is_none() {
        if e.open_credit(acct, b.start_cash, &day, &ts).is_err() {
            return;
        }
    }
    let sunday = (d + 4).rem_euclid(7) == 0;
    let monday = (d + 4).rem_euclid(7) == 1;
    match b.kind {
        BotKind::LeverHold => {
            if s % 1800 != 0 || !(9 * 3600 + 1800..=22 * 3600).contains(&s) {
                return;
            }
            let today = crate::vclose::day_no(&day).unwrap_or(0);
            if b.since == 0 {
                b.since = today;
            }
            if !b.bought_initial {
                // like buy_hold: own cash into every listed stock equally over five days
                let step = (today - b.since + 1).min(5);
                let listed: Vec<i64> = e.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
                let per = b.start_cash / listed.len().max(1) as i64 * step / 5;
                let mut pending = false;
                for c in listed {
                    let p = e.company(c).unwrap().last_price;
                    let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
                    if have < per * 8 / 10 {
                        e.cancel_all(acct, c, &ts);
                        let lim = p * 1005 / 1000 + 1;
                        let q = affordable(e, (per - have).min(e.available_cash(acct)), lim);
                        if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                            b.trades_placed += 1;
                        }
                        pending = true;
                    }
                }
                if (step == 5 && !pending) || today - b.since >= 7 {
                    b.bought_initial = true;
                }
                return;
            }
            // then, once a day at 10:00, all the available margin into the marginable stocks equally
            if s != 10 * 3600 || e.credit(acct).is_some_and(|c| c.status != crate::margin::NORMAL) {
                return;
            }
            let names = eligible(e, &day);
            let n = e.credit_numbers(acct, &day);
            let r = &e.margin;
            // each 币 financed takes 保证金比例 of margin (the new shares' value against the loan nets to ~0)
            let budget = n.available_margin as f64 / r.margin_ratio * 0.95;
            if budget <= 0.0 || names.is_empty() {
                return;
            }
            let per = budget / names.len() as f64;
            for c in names {
                if e.short_qty(acct, c) > 0 {
                    continue;
                }
                let p = e.company(c).unwrap().last_price;
                let lim = p * 1005 / 1000 + 1;
                let q = (per / e.fees.buy_reserve(lim, 1) as f64) as i64;
                let q = q.min(e.financing_room(c) / e.fees.buy_reserve(lim, 1).max(1));
                if credit(e, acct, c, Mode::MarginBuy, q, lim, t) {
                    b.trades_placed += 1;
                }
            }
        }
        BotKind::ShortRich => {
            // 10:00 and 20:00: short the five stocks priced furthest over their public value; cover what is
            // back at or under its value
            if s != 10 * 3600 && s != 20 * 3600 {
                return;
            }
            let owed: Vec<i64> = e.shorts_of(acct).filter(|x| x.qty > 0).map(|x| x.company_id).collect();
            for c in owed {
                let Some(co) = e.company(c).cloned() else { continue };
                if co.status == "listed" && co.fair_value > 0 && co.last_price <= co.fair_value {
                    e.cancel_all(acct, c, &ts);
                    let q = e.short_qty(acct, c);
                    if credit(e, acct, c, Mode::BuyReturn, q, co.last_price * 101 / 100 + 1, t) {
                        b.trades_placed += 1;
                    }
                }
            }
            let mut rich: Vec<(f64, i64)> = eligible(e, &day)
                .into_iter()
                .filter_map(|c| {
                    let co = e.company(c)?;
                    (co.fair_value > 0).then(|| (co.last_price as f64 / co.fair_value as f64, c))
                })
                .filter(|x| x.0 > 1.05)
                .collect();
            rich.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
            let targets: Vec<i64> = rich.into_iter().take(5).map(|x| x.1).collect();
            short_each(b, e, &targets, 0.9, t);
        }
        BotKind::ShortDividend => {
            // Sunday 22:30: short the five highest expected dividend yields; Monday 09:30: cover
            if sunday && s == 22 * 3600 + 1800 {
                let mut yields: Vec<(f64, i64)> = eligible(e, &day)
                    .into_iter()
                    .filter_map(|c| {
                        let co = e.company(c)?;
                        (co.profit_week > 0 && co.last_price > 0).then(|| (co.profit_week as f64 / (co.shares * co.last_price) as f64, c))
                    })
                    .collect();
                yields.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
                let targets: Vec<i64> = yields.into_iter().take(5).map(|x| x.1).collect();
                short_each(b, e, &targets, 0.9, t);
            } else if monday && s == 9 * 3600 + 1800 {
                cover_all(b, e, t);
            } else if s == 22 * 3600 + 3000 && !sunday {
                // whatever is still owed (a cover that did not fill) is retried in the evening
                cover_all(b, e, t);
            }
        }
        _ => {}
    }
}
