//! v1 NPC traders (port of `server/npc.py`): one market maker per stock quoting around its private
//! value with an inventory lean, rights holders (制作委员会) trimming when the price runs far above value,
//! and noise traders placing small market orders. They trade through the ordinary book.
//!
//! Randomness goes through `PyRandom`, a bit-exact port of Python's `random.Random`, so a tick with the
//! same generator state does exactly what v1 does (the market maker's daily noise is
//! `Random(int(sha256("{id}:{day}")[:12], 16)).gauss(0, sigma)`, also reproduced).

use crate::config::Config;
use crate::engine::{Engine, NewOrder};
use crate::exchange::quote_band;
use crate::model::OrderStatus;
use crate::pyrandom::{sha256_prefix48, PyRandom};

/// Same draw all day for a given stock (v1 `_daily_noise`).
pub fn daily_noise(company_id: i64, day: &str, sigma: f64) -> f64 {
    PyRandom::new(sha256_prefix48(&format!("{company_id}:{day}"))).gauss(0.0, sigma)
}

/// Python `float // float` (CPython `float_floordiv`, which is not always `floor(a / b)`).
pub fn py_float_floordiv(vx: f64, wx: f64) -> f64 {
    let mut m = vx % wx; // C fmod
    let mut div = (vx - m) / wx;
    if m != 0.0 {
        if (wx < 0.0) != (m < 0.0) {
            m += wx;
            div -= 1.0;
        }
    }
    let _ = m;
    if div != 0.0 {
        let mut f = div.floor();
        if div - f > 0.5 {
            f += 1.0;
        }
        f
    } else {
        0.0f64.copysign(vx / wx)
    }
}

fn pmax(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}
fn pmin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

impl Engine {
    fn has_open_company_order(&self, company_id: i64) -> bool {
        self.st.orders.values().any(|o| {
            o.company_id == company_id
                && o.status == OrderStatus::Open
                && self.st.accounts.get(&o.account_id).is_some_and(|a| a.kind == "company")
        })
    }

    /// v1 `npc._place`: place, ignoring rejections.
    #[allow(clippy::too_many_arguments)]
    fn npc_place(
        &mut self,
        account_id: i64,
        company_id: i64,
        side: &str,
        kind: &str,
        qty: i64,
        price: Option<i64>,
        day: &str,
        ts: &str,
        match_: bool,
    ) {
        let _ = self.place_order(&NewOrder { account_id, company_id, side, kind, qty, price, day, ts, match_ });
    }

    /// One round of NPC activity (v1 `npc.tick`). During the opening auction they only rest limit orders.
    pub fn npc_tick(&mut self, cfg: &Config, day: &str, ts: &str, rng: &mut PyRandom, auction: bool) {
        let npc = &cfg.npc;
        let match_ = !auction;
        let float_frac = match cfg.param("company.ipo_float") {
            Some(toml::Value::Float(f)) => *f,
            Some(toml::Value::Integer(i)) => *i as f64,
            _ => panic!("company.ipo_float missing"),
        };
        // a snapshot of the listed companies, as v1 fetches them at the start of the tick
        let companies: Vec<_> = self.companies().filter(|c| c.status == "listed").cloned().collect();
        for c in &companies {
            let (lo, hi) = quote_band(c, ts, &self.rules);
            let Some(mm) = self.account_of("mm", c.id) else { continue };
            if c.fair_value <= 0 {
                continue;
            }
            self.cancel_all(mm, c.id, ts);
            let ipo_open = self.has_open_company_order(c.id);

            let fair = c.fair_value as f64 * (1.0 + daily_noise(c.id, day, npc.mm_value_noise));
            let a = npc.mm_anchor_to_price;
            let r = (1.0 - a) * fair + a * c.last_price as f64;
            let inv = self.available_shares(mm, c.id);
            let target = c.shares as f64 * float_frac;
            let skew = pmax(-0.5, pmin(0.5, npc.mm_inventory_skew * (inv as f64 - target) / pmax(1.0, target)));
            let half = pmax(npc.mm_spread / 2.0 * r, 1.0);
            let ask_px = if skew >= 0.0 { r + half } else { r * (1.0 - skew) + half };
            let bid_px = if skew <= 0.0 { r - half } else { r * (1.0 - skew) - half };
            let ask = (lo + 1).max(hi.min(ask_px.ceil() as i64));
            let bid = lo.max((ask - 1).min(bid_px.floor() as i64));
            let size = 1i64.max((c.shares as f64 * npc.mm_quote_frac).round_ties_even() as i64);

            if !ipo_open {
                let cash = self.available_cash(mm);
                let q = size.min(cash.div_euclid(1i64.max(self.fees.buy_reserve(bid, 1))));
                if q > 0 && bid >= lo {
                    self.npc_place(mm, c.id, "buy", "limit", q, Some(bid), day, ts, match_);
                }
            }
            let q = size.min(self.available_shares(mm, c.id));
            if q > 0 && ask <= hi {
                self.npc_place(mm, c.id, "sell", "limit", q, Some(ask), day, ts, match_);
            }

            // rights holders trim when the price runs far above value, faster the further it runs
            if let Some(com) = self.account_of("committee", c.id) {
                self.cancel_all(com, c.id, ts);
                let last = self.company(c.id).map_or(0, |x| x.last_price);
                let ratio = last as f64 / c.fair_value as f64;
                if ratio > npc.committee_sell_above {
                    let pace = if ratio < 1.6 {
                        1.0
                    } else if ratio < 2.0 {
                        3.0
                    } else {
                        10.0
                    };
                    let q = self
                        .available_shares(com, c.id)
                        .min(1i64.max((c.shares as f64 * npc.committee_sell_frac * pace).round_ties_even() as i64));
                    let price = lo.max(hi.min((c.fair_value as f64 * npc.committee_sell_above).ceil() as i64));
                    if q > 0 {
                        self.npc_place(com, c.id, "sell", "limit", q, Some(price), day, ts, match_);
                    }
                }
            }
        }

        if auction {
            return;
        }
        // noise traders
        let listed: Vec<i64> = companies.iter().map(|c| c.id).filter(|id| !self.has_open_company_order(*id)).collect();
        if listed.is_empty() {
            return;
        }
        let noise: Vec<i64> = self.accounts().filter(|a| a.kind == "noise").map(|a| a.id).collect();
        for n in noise {
            if rng.random() > npc.noise_trade_prob {
                continue;
            }
            let cid = listed[rng.choice_index(listed.len())];
            let held = self.available_shares(n, cid);
            let side = if held > 0 && rng.random() < 0.5 { "sell" } else { "buy" };
            let last = self.company(cid).map_or(0, |x| x.last_price);
            let qty = if side == "buy" {
                let budget = self.available_cash(n) as f64 * rng.uniform(0.01, 0.05);
                py_float_floordiv(budget, 1i64.max(last) as f64).trunc() as i64
            } else {
                1i64.max((held as f64 * rng.uniform(0.2, 1.0)).trunc() as i64)
            };
            if qty <= 0 {
                continue;
            }
            self.npc_place(n, cid, side, "market", qty, None, day, ts, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_floordiv_like_python() {
        assert_eq!(py_float_floordiv(7.0, 2.0), 3.0);
        assert_eq!(py_float_floordiv(-7.0, 2.0), -4.0);
        assert_eq!(py_float_floordiv(0.0, 5.0), 0.0);
        // python: 1 // 0.1 == 9.0 (floor(1/0.1) would be 10)
        assert_eq!(py_float_floordiv(1.0, 0.1), 9.0);
    }
}
