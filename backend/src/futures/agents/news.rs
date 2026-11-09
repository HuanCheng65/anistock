//! A news trader: after a 热度快讯 it takes the flash's side in the show's contracts for `hold_s` seconds, at most up
//! to the price the flash justifies, then closes.
//!
//! The price it pays: the contract's price when it first saw the flash × (1 + surprise)^`beta`, where `beta` is how
//! much an episode's comment surprise has moved the next four weeks' heat index (`NpcRules::news_beta`). Measured
//! 2026-11-09 (sim/research/cn_views/flashidx.py, 490 episodes of 60 shows, final comment counts): slope 0.055,
//! corr 0.06, so 35% more comments is worth about 1.7% on the settlement. It used to buy at market with 2% slippage,
//! whatever the surprise: a move the flash did not justify, for players to fade.

use std::collections::HashMap;

use super::{Intent, Strategy, View};
use crate::futures::Contract;

pub struct News {
    pub hold_s: i64,
    /// share of net assets per contract
    pub budget: f64,
    pub slippage: f64,
    /// log price change per log comment surprise
    pub beta: f64,
    /// (contract, flash time) -> the contract's price when the flash was first seen
    pub anchors: HashMap<(i64, i64), i64>,
}

impl News {
    pub fn new(hold_s: i64, budget: f64, slippage: f64, beta: f64) -> News {
        News { hold_s, budget, slippage, beta, anchors: HashMap::new() }
    }
}

impl Strategy for News {
    fn decide(&mut self, v: &View, account: i64, c: &Contract) -> Option<Intent> {
        let hold = self.hold_s;
        self.anchors.retain(|&(_, t), _| v.t - t < hold);
        let recent = v.flashes.iter().rev().find(|f| f.company == c.company_id && v.t - f.t < self.hold_s);
        let size = (self.budget * v.net_assets(account).max(0) as f64 / v.lot_value(c)).floor() as i64;
        match recent {
            Some(f) => {
                let anchor = *self.anchors.entry((c.id, f.t)).or_insert(c.last_price);
                let target = (anchor as f64 * (1.0 + f.surprise).max(0.01).powf(self.beta)).round().max(1.0) as i64;
                let lots = if f.surprise > 0.0 { size } else if f.surprise < 0.0 { -size } else { 0 };
                Some(Intent { lots, limit: Some(target) })
            }
            None if v.position(account, c) == 0 => None,
            None => {
                let worse = if v.position(account, c) > 0 { 1.0 - self.slippage } else { 1.0 + self.slippage };
                Some(Intent { lots: 0, limit: Some((c.last_price as f64 * worse).round().max(1.0) as i64) })
            }
        }
    }
}
