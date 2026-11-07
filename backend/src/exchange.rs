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
    /// the VCM band is always at least one cent each side of the reference, and a one-cent move never
    /// triggers it (v1: at a few cents the ±10% band rounded to a single price and froze the stock)
    pub vcm_min_tick: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { vcm_threshold: 0.10, vcm_window_min: 5, vcm_cool_min: 5, sanity_mult: 20, vcm_min_tick: true }
    }
}

impl Rules {
    /// v1's rules exactly (for the difftests against the v1 Python exchange).
    pub fn v1() -> Rules {
        Rules { vcm_min_tick: false, ..Rules::default() }
    }
}

/// The price state the volatility control works on, for any instrument: the previous close (stocks) or
/// settlement price (futures), the day's anchor, and the cooling-off period.
pub trait Guard {
    /// previous close / settlement price
    fn prev_close(&self) -> i64;
    /// the day's VCM anchor (the opening auction price), if set
    fn open_ref(&self) -> Option<i64>;
    /// cooling-off period: end time and band
    fn vcm_until(&self) -> &Option<String>;
    fn vcm_band(&self) -> (Option<i64>, Option<i64>);
    /// cooling-off periods started today
    fn vcm_count(&self) -> i64;
    /// Start a cooling-off period until `until`, trading only within `low..=high`.
    fn start_cooling(&mut self, until: String, low: i64, high: i64);
}

impl Guard for Company {
    fn prev_close(&self) -> i64 {
        self.prev_close
    }
    fn open_ref(&self) -> Option<i64> {
        self.open_ref
    }
    fn vcm_until(&self) -> &Option<String> {
        &self.vcm_until
    }
    fn vcm_band(&self) -> (Option<i64>, Option<i64>) {
        (self.vcm_low, self.vcm_high)
    }
    fn vcm_count(&self) -> i64 {
        self.vcm_count
    }
    fn start_cooling(&mut self, until: String, low: i64, high: i64) {
        self.vcm_until = Some(until);
        self.vcm_low = Some(low);
        self.vcm_high = Some(high);
        self.vcm_count += 1;
    }
}

/// Python truthiness of `company["vcm_until"]` / `company["open_ref"]`.
fn truthy_str(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

/// The cooling-off band if the instrument is cooling off at `ts` (string comparison, like v1).
pub fn cooling(c: &impl Guard, ts: &str) -> Option<(i64, i64)> {
    match truthy_str(c.vcm_until()) {
        Some(until) if ts < until => {
            let (lo, hi) = c.vcm_band();
            Some((lo.unwrap_or(0), hi.unwrap_or(0)))
        }
        _ => None,
    }
}

pub fn sanity_band(c: &impl Guard, rules: &Rules) -> (i64, i64) {
    (1, (c.prev_close() * rules.sanity_mult).max(100))
}

/// Prices that can trade right now: the cooling band, else the sanity band.
pub fn quote_band(c: &impl Guard, ts: &str, rules: &Rules) -> (i64, i64) {
    cooling(c, ts).unwrap_or_else(|| sanity_band(c, rules))
}

/// `company["open_ref"] or company["prev_close"]`
pub fn vcm_anchor(c: &impl Guard) -> i64 {
    match c.open_ref() {
        Some(r) if r != 0 => r,
        _ => c.prev_close(),
    }
}

/// The volatility control's verdict on a trade price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vcm {
    Allow,
    /// outside the current cooling-off band
    Block,
    /// too far from `reference`: start a cooling-off period with this band
    Trigger {
        reference: i64,
        threshold: f64,
        low: i64,
        high: i64,
    },
}

/// May a trade print at `tp` at time `ts`? `lookback` is the last trade price at least the VCM window
/// before `ts` today (asked for only when no cooling-off period has started today).
pub fn vcm_check(c: &impl Guard, ts: &str, tp: i64, rules: &Rules, lookback: impl FnOnce() -> Option<i64>) -> Vcm {
    if let Some((lo, hi)) = cooling(c, ts) {
        return if lo <= tp && tp <= hi { Vcm::Allow } else { Vcm::Block };
    }
    let anchor = vcm_anchor(c);
    let (r, thr) = match c.vcm_count() {
        0 => (lookback().unwrap_or(anchor), rules.vcm_threshold),
        n => (anchor, rules.vcm_threshold * (n + 1) as f64),
    };
    if r <= 0 || ((tp as f64) / (r as f64) - 1.0).abs() <= thr + 1e-12 {
        return Vcm::Allow;
    }
    if rules.vcm_min_tick && (tp - r).abs() <= 1 {
        return Vcm::Allow;
    }
    let rf = r as f64;
    let mut lo = ((rf * (1.0 - thr) - 1e-9).ceil() as i64).max(1);
    let mut hi = (rf * (1.0 + thr) + 1e-9).floor() as i64;
    if rules.vcm_min_tick {
        lo = lo.min((r - 1).max(1));
        hi = hi.max(r + 1);
    }
    Vcm::Trigger { reference: r, threshold: thr, low: lo, high: hi }
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
