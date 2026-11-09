//! `[futures]` of game.toml (docs/design/futures.md §4; the defaults are the values decided there).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct FuturesRules {
    /// list contracts, trade and settle them. Off: nothing about futures runs.
    pub enabled: bool,
    /// a contract averages this many weeks; period k covers weeks `weeks*(k-1)+1 ..= weeks*k` after the premiere
    pub weeks: i64,
    /// list a period only if the model's forecast of its average index is at least this
    pub listing_floor: f64,
    /// cents per tick (0.01 index point) per lot: 1 point = 10 星辉
    pub tick_value: i64,
    /// initial margin: this share of the notional ...
    pub im_rate: f64,
    /// ... and at least this many standard deviations of the forecast of the average
    pub im_sds: f64,
    /// maintenance margin as a share of the initial margin
    pub maintenance: f64,
    /// one contract's notional at most this share of net assets
    pub position_cap: f64,
    /// all futures' initial margin at most this share of net assets
    pub margin_cap: f64,
    /// the settlement price is the volume-weighted average of the trades in the session's last minutes
    pub settle_window_min: i64,
    /// the index in the valuation filter's units, until the filter observes the index itself (docs/design/futures.md
    /// §3.5, §3.7): index = scale × Σ weight × link × attention / platform, per market [us, cn, jp]
    pub index_weights: [f64; 3],
    /// index units per unit of the inputs' att, per market [us, cn, jp] and four-week block since the premiere
    /// (weeks 0-3, 4-7, 8-11, 12 and later): Bangumi's marks crowd into the first weeks and fade faster than
    /// the comments the cn series is built from
    pub index_link: [[f64; 4]; 3],
    pub index_platform: [f64; 3],
    pub index_scale: f64,
    /// the index method's version these constants belong to (engine/heat_index.py VERSION)
    pub index_version: String,
    /// the realised average's spread around the model's expectation beyond the level's own uncertainty (the
    /// link, the index's daily noise), as a share of the expectation of the days still to come
    pub model_cv: f64,
    /// the expectation of a day `h` days ahead is scaled by exp(-horizon_drift x h): the filter's paths run
    /// about 6.5% high five weeks out and are unbiased within the period (2026-11-07 replays)
    pub horizon_drift: f64,
    /// the market maker (one account, `fmm`): capital from world per contract listed (returned when it
    /// settles), position limit and quote size in lots, quote levels
    pub mm_capital: i64,
    pub mm_limit: i64,
    pub mm_size: i64,
    pub mm_levels: i64,
    /// half-spread: this many of a day's share of the forecast's uncertainty (sd / sqrt(days left)), at least
    /// `mm_min_half` ticks
    pub mm_spread_sd: f64,
    pub mm_min_half: i64,
    /// the quote's centre leans this far from the forecast toward the last price
    pub mm_last_weight: f64,
    /// at its position limit the centre moves this many half-spreads against its inventory
    pub mm_skew: f64,
    /// the other traders (crate::futures::agents)
    pub npc: NpcRules,
}

/// `[futures.npc]`: committees hedging, speculators, news traders.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct NpcRules {
    /// actions per trader per hour of trading (Poisson)
    pub rate: f64,
    /// committees: share of a period's revenue sold, lean toward the price, lowest price in forecast sds
    pub hedge_ratio: f64,
    pub hedge_lean: f64,
    pub hedge_floor_sds: f64,
    /// speculators: how many, their capital each (cents, from world), the sd of their private bias, the edge
    /// they want, the share of net assets per contract
    pub value_n: usize,
    pub value_capital: i64,
    pub value_bias_sd: f64,
    pub value_threshold: f64,
    pub value_budget: f64,
    /// news traders: how many, capital each, how long they hold, share per contract, slippage accepted
    pub news_n: usize,
    pub news_capital: i64,
    pub news_hold_h: f64,
    pub news_budget: f64,
    pub news_slippage: f64,
    /// how far a flash moves their price: (1 + surprise)^news_beta (futures/agents/news.rs)
    pub news_beta: f64,
}

impl Default for NpcRules {
    fn default() -> NpcRules {
        NpcRules {
            rate: 2.0,
            hedge_ratio: 0.4,
            hedge_lean: 1.0,
            hedge_floor_sds: 1.0,
            value_n: 12,
            value_capital: 20_000_000,
            value_bias_sd: 0.15,
            value_threshold: 0.04,
            value_budget: 0.05,
            news_n: 4,
            news_capital: 10_000_000,
            news_hold_h: 24.0,
            news_budget: 0.05,
            news_slippage: 0.02,
            news_beta: 0.055,
        }
    }
}

impl Default for FuturesRules {
    fn default() -> FuturesRules {
        FuturesRules {
            enabled: false,
            weeks: 4,
            listing_floor: 75.0,
            tick_value: 10,
            im_rate: 0.15,
            im_sds: 2.0,
            maintenance: 0.75,
            position_cap: 0.5,
            margin_cap: 0.8,
            settle_window_min: 30,
            index_weights: [0.3, 0.5, 0.2],
            // index / the inputs' att, median per show-day of each block (2026-07..11 backfill, sim/heat_inputs.py)
            index_link: [[1.0; 4], [5.6, 2.25, 2.3, 2.3], [0.66, 0.76, 0.85, 1.1]],
            // the index method v1 (engine/heat_index.py PLATFORM, SCALE)
            index_platform: [116352.0, 1772.0, 1649.0],
            index_scale: 14437.0,
            index_version: "v1".into(),
            model_cv: 0.15,
            horizon_drift: 0.002,
            mm_capital: 20_000_000,
            mm_limit: 200,
            mm_size: 10,
            mm_levels: 3,
            // the narrowest spread the gate passes (docs/design/futures.md §4.5, 2026-11-08): at 0.3 the momentum bot
            // beats buy-and-hold by 8 points; 0.6 left the maker 11-14k a season
            mm_spread_sd: 0.4,
            mm_min_half: 10,
            mm_last_weight: 0.2,
            mm_skew: 1.0,
            npc: NpcRules::default(),
        }
    }
}

impl FuturesRules {
    pub fn from_table(t: Option<&toml::Value>) -> Result<FuturesRules, String> {
        match t {
            None => Ok(FuturesRules::default()),
            Some(v) => v.clone().try_into().map_err(|e| format!("[futures]: {e}")),
        }
    }
}

impl FuturesRules {
    /// Index points per unit of the filter's attention in market `m`, `weeks` weeks after the premiere.
    pub fn index_per_att(&self, m: usize, weeks: i64) -> f64 {
        let block = (weeks.max(0) / 4).min(3) as usize;
        self.index_scale * self.index_weights[m] * self.index_link[m][block] / self.index_platform[m]
    }
}
