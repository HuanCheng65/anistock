//! Exchange rules shared by the engine: fees, volatility control parameters, price bands, and the
//! player-facing error type. Port of the pure parts of `server/exchange.py`; the stateful parts
//! (orders, matching, settlement) live in `engine.rs`.
//!
//! Float formulas are copied operation for operation from Python so results agree to the cent:
//! Python `math.ceil(int * float)` is `(i as f64 * f).ceil()`, `round()` is `round_ties_even`.

use crate::model::Company;

/// A rejected request. The message is shown to the player as-is (Chinese), same text as v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderError(pub String);

impl OrderError {
    pub fn new(msg: impl Into<String>) -> OrderError {
        OrderError(msg.into())
    }
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for OrderError {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fees {
    /// each side, per fill
    pub commission: f64,
    /// seller only
    pub stamp_duty: f64,
}

impl Default for Fees {
    fn default() -> Fees {
        Fees { commission: 0.00025, stamp_duty: 0.0005 }
    }
}

impl Fees {
    pub fn commission_on(&self, notional: i64) -> i64 {
        if notional > 0 {
            (notional as f64 * self.commission).ceil() as i64
        } else {
            0
        }
    }

    pub fn stamp_on(&self, notional: i64) -> i64 {
        if notional > 0 {
            (notional as f64 * self.stamp_duty).ceil() as i64
        } else {
            0
        }
    }

    /// Cash to freeze for a buy: notional + commission, plus one cent per share for per-fill rounding.
    pub fn buy_reserve(&self, price: i64, qty: i64) -> i64 {
        let notional = price * qty;
        notional + self.commission_on(notional) + qty
    }
}

/// Hong Kong-style volatility control (市场波动调节机制) instead of daily price limits. See v1 `Rules`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    pub vcm_threshold: f64,
    pub vcm_window_min: i64,
    pub vcm_cool_min: i64,
    /// a limit price may not exceed this multiple of the previous close
    pub sanity_mult: i64,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { vcm_threshold: 0.10, vcm_window_min: 5, vcm_cool_min: 5, sanity_mult: 20 }
    }
}

/// Python truthiness of `company["vcm_until"]` / `company["open_ref"]`.
fn truthy_str(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// The cooling-off band if the stock is cooling off at `ts` (string comparison, like v1).
pub fn cooling(c: &Company, ts: &str) -> Option<(i64, i64)> {
    match truthy_str(&c.vcm_until) {
        Some(until) if ts < until => Some((c.vcm_low.unwrap_or(0), c.vcm_high.unwrap_or(0))),
        _ => None,
    }
}

pub fn sanity_band(c: &Company, rules: &Rules) -> (i64, i64) {
    (1, (c.prev_close * rules.sanity_mult).max(100))
}

/// Prices that can trade right now: the cooling band, else the sanity band.
pub fn quote_band(c: &Company, ts: &str, rules: &Rules) -> (i64, i64) {
    cooling(c, ts).unwrap_or_else(|| sanity_band(c, rules))
}

/// `company["open_ref"] or company["prev_close"]`
pub fn vcm_anchor(c: &Company) -> i64 {
    match c.open_ref {
        Some(r) if r != 0 => r,
        _ => c.prev_close,
    }
}

/// Python `f"{cents / 100:.2f}"`. For integer cents this is exact decimal formatting.
pub fn yuan_str(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let a = cents.unsigned_abs();
    format!("{sign}{}.{:02}", a / 100, a % 100)
}

/// Python `f"{x:.0%}"`: multiply by 100 (in floating point), round half to even, append `%`.
pub fn pct0_str(x: f64) -> String {
    let v = (x * 100.0).round_ties_even();
    let v = if v == 0.0 { 0.0 } else { v }; // Python prints -0 as "-0%"; not reachable for thresholds
    format!("{}%", v as i64)
}

/// Python `round(a / b)` for ints (true division, then round half to even).
pub fn py_round_div(a: i64, b: i64) -> i64 {
    (a as f64 / b as f64).round_ties_even() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(yuan_str(505), "5.05");
        assert_eq!(yuan_str(10000), "100.00");
        assert_eq!(yuan_str(7), "0.07");
        assert_eq!(pct0_str(0.1), "10%");
        assert_eq!(pct0_str(0.1 * 3.0), "30%");
        assert_eq!(pct0_str(0.125), "12%");
        assert_eq!(py_round_div(5, 2), 2);
        assert_eq!(py_round_div(7, 2), 4);
    }

    #[test]
    fn fees() {
        let f = Fees::default();
        assert_eq!(f.commission_on(20800), 6);
        assert_eq!(f.commission_on(0), 0);
        assert_eq!(f.stamp_on(20800), 11);
        assert_eq!(f.buy_reserve(500, 10), 5000 + 2 + 10);
    }
}
