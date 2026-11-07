//! A speculator with a view of its own: the forecast times a private bias per work (fixed all season), long
//! when the price is under its view by more than `threshold`, short when over, flat in between; at most
//! `budget` of its net assets per contract, and only at prices that keep the edge.

use super::{normal, Intent, Strategy, View};
use crate::futures::Contract;

pub struct Value {
    /// the trader's seed: its biases
    pub seed: u64,
    /// sd of the log bias
    pub bias_sd: f64,
    pub threshold: f64,
    pub budget: f64,
}

impl Value {
    pub fn view(&self, work: i64, mean: f64) -> f64 {
        mean * (self.bias_sd * normal(self.seed.wrapping_mul(0x1000_0001) ^ work as u64)).exp()
    }
}

impl Strategy for Value {
    fn decide(&mut self, v: &View, account: i64, c: &Contract) -> Option<Intent> {
        let (mean, _) = v.forecast(c);
        let view = self.view(c.work_id, mean);
        let price = c.last_price as f64;
        let size = (self.budget * v.net_assets(account).max(0) as f64 / v.lot_value(c)).floor() as i64;
        let (lo, hi) = (view * (1.0 - self.threshold), view * (1.0 + self.threshold));
        let (lots, limit) = if price < lo {
            (size, lo)
        } else if price > hi {
            (-size, hi)
        } else {
            (0, view)
        };
        Some(Intent { lots, limit: Some(limit.round().max(1.0) as i64) })
    }
}
