//! Corporate action exploit tests (docs/v2-corporate.md §6): a submodule of `bots` so the strategies share the
//! bot's state. Each holds the market by market value with most of its money (so its excess over the
//! cap-weighted benchmark is what the event trades add) and trades the company notices it is given.

use super::{affordable, place, Bot, BotCtx, BotKind};
use crate::engine::Engine;
use crate::market::{at_local, day_of, local, ts_of};

/// Share of the starting cash in the market portfolio; the rest is for the event trades.
fn base_frac(k: BotKind) -> f64 {
    match k {
        BotKind::RightsSell => 1.0,
        _ => 0.8,
    }
}

fn day_after(t: i64, days: i64, h: i64) -> i64 {
    let d = chrono::NaiveDate::parse_from_str(&day_of(t), "%Y-%m-%d").unwrap() + chrono::Duration::days(days);
    at_local(&d.format("%Y-%m-%d").to_string(), h, 0)
}

/// An event position: the stock, when to close it, the market-portfolio shares to keep (in the units of the
/// trading day it was opened on, rescaled by any split since).
#[derive(Debug, Clone)]
pub struct CorpExit {
    pub company: i64,
    pub at: i64,
    pub keep: i64,
    pub opened: String,
}

impl Bot {
    /// Build the market portfolio over the first five days (as `buy_hold_cap`), `frac` of the starting cash.
    fn corp_base(&mut self, e: &mut Engine, t: i64, frac: f64) {
        let (_, s) = local(t);
        if self.bought_initial || s % 1800 != 0 {
            return;
        }
        let acct = self.account;
        let ts = ts_of(t);
        let today = crate::vclose::day_no(&day_of(t)).unwrap_or(0);
        if self.since == 0 {
            self.since = today;
        }
        let step = (today - self.since + 1).min(5);
        let listed: Vec<(i64, i64)> = e.companies().filter(|c| c.status == "listed").map(|c| (c.id, c.shares * c.last_price)).collect();
        let cap_total: i64 = listed.iter().map(|x| x.1).sum::<i64>().max(1);
        let budget = (self.start_cash as f64 * frac) as i64;
        let mut pending = false;
        for (c, cap) in listed {
            let per = (budget as i128 * cap as i128 / cap_total as i128) as i64 * step / 5;
            let p = e.company(c).unwrap().last_price;
            let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
            if have < per * 8 / 10 {
                e.cancel_all(acct, c, &ts);
                let lim = p * 1005 / 1000 + 1;
                let q = affordable(e, (per - have).min(e.available_cash(acct)), lim);
                if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                    self.trades_placed += 1;
                }
                pending = true;
            }
        }
        if (step == 5 && !pending) || today - self.since >= 7 {
            self.bought_initial = true;
        }
    }

    /// Sell the shares of `c` over `keep` (limit 1% under the last price).
    fn corp_sell_over(&mut self, e: &mut Engine, c: i64, keep: i64, t: i64) {
        let acct = self.account;
        e.cancel_all(acct, c, &ts_of(t));
        let q = (e.available_shares(acct, c) - keep).max(0);
        let Some(p) = e.company(c).filter(|x| x.status == "listed").map(|x| x.last_price) else { return };
        if place(e, acct, c, "sell", "limit", q, Some((p * 99 / 100).max(1)), t) {
            self.trades_placed += 1;
        }
    }

    fn corp_open(&mut self, e: &mut Engine, c: i64, budget: i64, at: i64, t: i64) {
        let keep = e.holding(self.account, c).map_or(0, |h| h.qty);
        self.buy(e, c, budget, t);
        self.corp_exits.push(CorpExit { company: c, at, keep, opened: day_of(t) });
    }

    pub(super) fn corp_minute(&mut self, e: &mut Engine, t: i64, ctx: &BotCtx) {
        self.corp_base(e, t, base_frac(self.kind));
        let (_, s) = local(t);
        let acct = self.account;
        let wealth = self.wealth(e);
        // close the event positions whose time has come (retried every 10 minutes until done)
        let mut left = Vec::new();
        for x in std::mem::take(&mut self.corp_exits) {
            let keep = (x.keep as f64 * e.adjust_factor(x.company, &x.opened).1).round() as i64;
            let held = e.holding(acct, x.company).map_or(0, |h| h.qty);
            if t < x.at || (held > keep && e.company(x.company).is_some_and(|c| c.status == "listed")) {
                if t >= x.at && (t / 60) % 10 == 0 {
                    self.corp_sell_over(e, x.company, keep, t);
                }
                left.push(x);
            }
        }
        self.corp_exits = left;
        // rights_sell: buy back after the issue
        let due: Vec<(i64, i64, i64)> = self.corp_rebuy.iter().filter(|x| t >= x.1).copied().collect();
        self.corp_rebuy.retain(|x| t < x.1);
        for (c, _, budget) in due {
            self.buy(e, c, budget.min(e.available_cash(acct)), t);
        }
        while self.notices_seen < ctx.notices.len() {
            let (c, _, kind, title) = ctx.notices[self.notices_seen].clone();
            self.notices_seen += 1;
            if e.company(c).is_none_or(|x| x.status != "listed") {
                continue;
            }
            match (self.kind, kind.as_str()) {
                // a buyback programme starts: buy a fifth of wealth, hold four weeks
                (BotKind::BuybackFollow, "buyback") if title.contains("回购：") => {
                    self.corp_open(e, c, wealth / 5, day_after(t, 28, 10), t);
                }
                // a rights issue: sell the stock now (no subscription: the lapsed rights are compensated), buy the
                // same value back the morning after the issue
                (BotKind::RightsSell, "rights") if title.contains("配股：") => {
                    let held = e.holding(acct, c).map_or(0, |h| h.qty);
                    if held > 0 {
                        let value = held * e.company(c).map_or(0, |x| x.last_price);
                        self.corp_sell_over(e, c, 0, t);
                        let n = e.open_action(c, "rights").map_or(3, |a| crate::vclose::day_no(&a.effective).unwrap_or(0) - crate::vclose::day_no(&day_of(t)).unwrap_or(0));
                        self.corp_rebuy.push((c, day_after(t, n.max(0) + 1, 10), value));
                    }
                }
                // a delay: bet that the market over-reacts, buy and sell a day later
                (BotKind::DelayFade, "delay") => {
                    self.corp_open(e, c, wealth / 5, t + 86_400, t);
                }
                _ => {}
            }
        }
        // split_buy: at 10:00, a stock above the split threshold at the last three opens is bought ahead of the
        // announcement (after five closes above) and held to five days after the split takes effect
        if self.kind == BotKind::SplitBuy && s == 10 * 3600 {
            let above = e.corporate.split_above * 100.0;
            let cands: Vec<i64> = self
                .opens
                .iter()
                .filter(|(_, v)| v.len() >= 3 && v[v.len() - 3..].iter().all(|p| *p as f64 > above))
                .map(|(c, _)| *c)
                .collect();
            for c in cands {
                if self.corp_done.contains(&c) || e.company(c).is_none_or(|x| x.status != "listed") {
                    continue;
                }
                self.corp_done.insert(c);
                let at = day_after(t, 3 + e.corporate.split_lead_days + 5, 10);
                self.corp_open(e, c, wealth / 5, at, t);
            }
        }
    }
}
