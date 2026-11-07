//! `[futures]` of game.toml (docs/v3-futures.md §4; the defaults are the values decided there).

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
