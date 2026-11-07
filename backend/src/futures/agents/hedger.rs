//! A production committee hedging its revenue (the farmer selling wheat futures): revenue moves with the
//! show's heat, so it sells futures worth `ratio` of the revenue a period brings, a little more when the price
//! is above the forecast and a little less when below (`lean`). It never sells below the forecast's lower edge.

use super::{Intent, Strategy, View};
use crate::futures::Contract;

pub struct Hedger {
    pub company: i64,
    /// share of a period's expected revenue to sell
    pub ratio: f64,
    /// how much the price's distance from the forecast moves the hedge (1: +10% price, +10% hedge)
    pub lean: f64,
    /// lowest price to sell at, in forecast standard deviations under the forecast
    pub floor_sds: f64,
}

impl Strategy for Hedger {
    fn decide(&mut self, v: &View, _account: i64, c: &Contract) -> Option<Intent> {
        if c.company_id != self.company {
            return None;
        }
        let (mean, sd) = v.forecast(c);
        // a period's revenue: the last four weeks' pace
        let revenue = v.revenue(self.company, 28) as f64;
        if revenue <= 0.0 || mean <= 0.0 {
            return Some(Intent { lots: 0, limit: None });
        }
        let lean = (1.0 + self.lean * (c.last_price as f64 / mean - 1.0)).clamp(0.5, 1.5);
        let lots = (self.ratio * lean * revenue / v.lot_value(c)).round() as i64;
        Some(Intent { lots: -lots, limit: Some(((mean - self.floor_sds * sd).max(1.0)).round() as i64) })
    }
}
