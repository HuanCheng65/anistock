//! Futures rows (migration V17). Prices are in ticks: hundredths of an index point.

use serde::Serialize;

use crate::exchange::Guard;
use crate::model::{Kind, OrderStatus, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContractStatus {
    Listed,
    /// trading over; waiting for the period's last index values
    Expiring,
    Settled,
}

impl ContractStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ContractStatus::Listed => "listed",
            ContractStatus::Expiring => "expiring",
            ContractStatus::Settled => "settled",
        }
    }
    pub fn parse(s: &str) -> Option<ContractStatus> {
        [ContractStatus::Listed, ContractStatus::Expiring, ContractStatus::Settled]
            .into_iter()
            .find(|c| c.as_str() == s)
    }
}

/// One contract: the average heat index of `work_id` over `first_day..=last_day`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Contract {
    pub id: i64,
    pub code: String,
    pub company_id: i64,
    pub work_id: i64,
    pub period: i64,
    pub first_day: String,
    pub last_day: String,
    pub listed_on: String,
    pub status: ContractStatus,
    /// cents per tick per lot
    pub tick_value: i64,
    /// the model's forecast of the average when listed (ticks)
    pub forecast: i64,
    pub prev_settle: i64,
    pub open_ref: Option<i64>,
    pub vcm_until: Option<String>,
    pub vcm_low: Option<i64>,
    pub vcm_high: Option<i64>,
    pub vcm_count: i64,
    pub last_price: i64,
    pub final_price: Option<i64>,
    pub settled_on: Option<String>,
}

impl Contract {
    pub fn trading(&self) -> bool {
        self.status == ContractStatus::Listed
    }
}

/// The volatility control runs on the previous settlement price, like a stock on its previous close.
impl Guard for Contract {
    fn prev_close(&self) -> i64 {
        self.prev_settle
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FutOrder {
    pub id: i64,
    pub account_id: i64,
    pub contract_id: i64,
    pub side: Side,
    pub kind: Kind,
    pub price: Option<i64>,
    pub qty: i64,
    pub filled: i64,
    /// cents of initial margin frozen for the part that opens a position
    pub frozen: i64,
    pub status: OrderStatus,
    pub created_at: String,
    pub updated_at: String,
}

impl FutOrder {
    pub fn left(&self) -> i64 {
        self.qty - self.filled
    }
}

/// A net position. `basis` is the signed ticks paid since the last settlement, so the variation margin at
/// settlement price `s` is `(qty * s - basis) * tick_value`; summed over every account it is zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Position {
    pub account_id: i64,
    pub contract_id: i64,
    pub qty: i64,
    pub basis: i64,
}

impl Position {
    /// Variation margin at price `s`, in ticks per lot (multiply by the contract's `tick_value`).
    pub fn variation(&self, s: i64) -> i64 {
        self.qty * s - self.basis
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FutTrade {
    pub id: i64,
    pub contract_id: i64,
    pub price: i64,
    pub qty: i64,
    pub buyer_id: i64,
    pub seller_id: i64,
    pub buy_order: i64,
    pub sell_order: i64,
    pub ts: String,
    pub day: String,
}

/// Where a settlement price came from (docs/v3-futures.md §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SettleSource {
    /// volume-weighted average of the last half hour's trades
    Vwap,
    /// the market maker's mid quote
    Mid,
    /// no trades and no quote: the previous settlement
    Prev,
    /// the final settlement on the period's average index
    Final,
}

impl SettleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SettleSource::Vwap => "vwap",
            SettleSource::Mid => "mid",
            SettleSource::Prev => "prev",
            SettleSource::Final => "final",
        }
    }
    pub fn parse(s: &str) -> Option<SettleSource> {
        [SettleSource::Vwap, SettleSource::Mid, SettleSource::Prev, SettleSource::Final]
            .into_iter()
            .find(|x| x.as_str() == s)
    }
}

/// One contract's trading day and its settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FutDay {
    pub contract_id: i64,
    pub day: String,
    /// open, high, low, close of the day's trades (None without trades)
    pub ohlc: Option<(i64, i64, i64, i64)>,
    pub volume: i64,
    pub settle: i64,
    pub source: SettleSource,
    pub open_interest: i64,
}

/// The published heat index of one work and day, as frozen in the game database.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HeatRow {
    pub work_id: i64,
    pub day: String,
    pub idx: f64,
    pub us: Option<f64>,
    pub cn: Option<f64>,
    pub jp: Option<f64>,
    /// JSON object: market -> missing | filled | negative | spike
    pub flags: String,
    pub version: String,
}
