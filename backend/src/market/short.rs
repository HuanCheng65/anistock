//! Value agents who sell short (docs/v2-margin.md §8). A fixed share of the value agents (`npc_short_frac`
//! of `[margin]`) open credit and, when a stock trades well above what they think it is worth, borrow it from
//! the rights holders and sell it at or above the last price; they buy it back once the price is under their
//! value. Same lending pool, prices, fees and forced liquidation as players.

use super::{AgentKind, Market};
use crate::engine::{Engine, NewOrder};
use crate::margin::Mode;
use crate::market::{day_of, ts_of};

impl Market {
    /// Is agent `i` one of the short sellers? (Fixed per account.)
    fn short_seller(&self, engine: &Engine, i: usize) -> bool {
        let a = &self.agents[i];
        let frac = engine.margin.npc_short_frac;
        if a.kind != AgentKind::Value || frac <= 0.0 || !engine.margin.enabled {
            return false;
        }
        let h = (a.account as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40;
        (h % 10_000) as f64 / 10_000.0 < frac
    }

    /// A credit order (continuous trading only), counted under `label`.
    pub(super) fn credit_order(&mut self, engine: &mut Engine, label: &str, acct: i64, c: i64, mode: Mode, qty: i64, price: i64, t: i64) {
        if qty <= 0 {
            return;
        }
        let (day, ts) = (day_of(t), ts_of(t));
        if engine.state("auction_day") != Some(day.as_str()) {
            engine.open_market(&day, &ts);
        }
        let req = NewOrder { account_id: acct, company_id: c, side: mode.side().as_str(), kind: "limit", qty, price: Some(price), day: &day, ts: &ts, match_: true };
        let r = engine.place_credit(&req, mode);
        *self.stats.orders.entry(label.to_string()).or_default() += 1;
        if r.is_err() {
            self.stats.rejected += 1;
        }
        self.sample(engine, c, t);
    }

    /// Called by a value agent after it cancelled its orders in `c`, with `m` = ln(belief / price). Returns
    /// true when the stock was handled here (a short is open, or one was opened).
    pub(super) fn value_short(&mut self, engine: &mut Engine, i: usize, c: i64, t: i64, m: f64, belief: f64) -> bool {
        if Self::phase_auction(t) {
            return false;
        }
        let a = self.agents[i].clone();
        let owed = engine.short_qty(a.account, c);
        let p = engine.company(c).map_or(0, |x| x.last_price);
        if owed > 0 {
            if m > 0.0 && p > 0 {
                // under its value now: buy it back, paying up to the value, as far as its cash goes
                let limit = Self::clamp_price(engine, &engine.rules.clone(), c, (p as f64 * (a.aggression * m).exp()).min(belief), t);
                let cash = engine.available_cash(a.account) + engine.short_of(a.account, c).map_or(0, |s| s.proceeds);
                let q = owed.min(cash / engine.fees.buy_reserve(limit, 1).max(1));
                self.credit_order(engine, "value_short", a.account, c, Mode::BuyReturn, q, limit, t);
            }
            return true;
        }
        let extra = engine.margin.npc_short_extra;
        if m >= -(a.threshold + extra) || p <= 0 || !self.short_seller(engine, i) {
            return false;
        }
        let day = day_of(t);
        if engine.holding(a.account, c).is_some_and(|h| h.qty > 0) || engine.margin_ineligible(c, &day).is_some() {
            return false;
        }
        if engine.credit(a.account).is_none() {
            engine.open_credit_unchecked(a.account, &ts_of(t));
        }
        if engine.credit(a.account).is_some_and(|x| x.status != crate::margin::NORMAL) {
            return false;
        }
        // the same limits a player meets: initial margin, concentration, the lending pool
        let n = engine.credit_numbers(a.account, &day);
        let r = &engine.margin;
        let by_margin = (n.available_margin as f64 / (r.margin_ratio * p as f64)).floor() as i64;
        let by_conc = (r.concentration * n.net() as f64 / p as f64).floor() as i64;
        let conviction = ((-m - a.threshold - extra) / self.cfg.value_full).clamp(0.0, 1.0);
        let wealth = self.wealth(engine, a.account).max(0) as f64;
        let budget = wealth * r.npc_short_size * conviction;
        let q = ((budget / p as f64) as i64).min(engine.lendable(c)).min(by_margin).min(by_conc);
        // at or above the last trade (the short-sale price rule): it rests and waits for buyers
        self.credit_order(engine, "value_short", a.account, c, Mode::ShortSell, q, p, t);
        true
    }
}
