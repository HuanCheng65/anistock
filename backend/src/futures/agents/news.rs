//! A news trader: after a 热度快讯 it takes the flash's direction in the show's contracts for `hold_s` seconds
//! (a market order, protected `slippage` from the last price), then closes.

use super::{Intent, Strategy, View};
use crate::futures::Contract;

pub struct News {
    pub hold_s: i64,
    /// share of net assets per contract
    pub budget: f64,
    pub slippage: f64,
}

impl Strategy for News {
    fn decide(&mut self, v: &View, account: i64, c: &Contract) -> Option<Intent> {
        let recent = v.flashes.iter().rev().find(|f| f.company == c.company_id && v.t - f.t < self.hold_s);
        let size = (self.budget * v.net_assets(account).max(0) as f64 / v.lot_value(c)).floor() as i64;
        let (lots, worse) = match recent {
            Some(f) if f.up => (size, 1.0 + self.slippage),
            Some(_) => (-size, 1.0 - self.slippage),
            None if v.position(account, c) == 0 => return None,
            None => (0, if v.position(account, c) > 0 { 1.0 - self.slippage } else { 1.0 + self.slippage }),
        };
        Some(Intent { lots, limit: Some((c.last_price as f64 * worse).round().max(1.0) as i64) })
    }
}
