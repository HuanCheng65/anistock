//! Rows of the game database as kept in memory. Money and prices are integer cents.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    Open,
    Filled,
    Cancelled,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Buy => "buy",
            Side::Sell => "sell",
        }
    }
    pub fn parse(s: &str) -> Option<Side> {
        match s {
            "buy" => Some(Side::Buy),
            "sell" => Some(Side::Sell),
            _ => None,
        }
    }
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Limit => "limit",
            Kind::Market => "market",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "limit" => Some(Kind::Limit),
            "market" => Some(Kind::Market),
            _ => None,
        }
    }
}

impl OrderStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OrderStatus::Open => "open",
            OrderStatus::Filled => "filled",
            OrderStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<OrderStatus> {
        match s {
            "open" => Some(OrderStatus::Open),
            "filled" => Some(OrderStatus::Filled),
            "cancelled" => Some(OrderStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Account {
    pub id: i64,
    /// player | world | cb | gov | company | committee | mm | noise
    pub kind: String,
    pub name: String,
    pub username: Option<String>,
    pub pw_hash: Option<String>,
    pub company_id: Option<i64>,
    pub cash: i64,
    pub cash_frozen: i64,
    pub created_at: String,
}

/// Every column of `companies`. The exchange only changes the market fields
/// (`last_price`, `open_ref`, `vcm_*`); see `Change::CompanyMarket`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Company {
    pub id: i64,
    pub code: String,
    pub title: String,
    pub title_ja: Option<String>,
    pub title_en: Option<String>,
    pub studio: Option<String>,
    pub episodes: Option<i64>,
    pub long_runner: i64,
    pub first_air: String,
    pub listed_on: String,
    /// subscribing | listed | delisted
    pub status: String,
    pub sub_start: Option<String>,
    pub sub_end: Option<String>,
    pub shares: i64,
    pub ipo_price: i64,
    pub prev_close: i64,
    pub open_ref: Option<i64>,
    pub vcm_until: Option<String>,
    pub vcm_low: Option<i64>,
    pub vcm_high: Option<i64>,
    pub vcm_count: i64,
    pub last_price: i64,
    pub fair_value: i64,
    pub profit_week: i64,
    pub low_weeks: i64,
    pub delisted_on: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Holding {
    pub account_id: i64,
    pub company_id: i64,
    pub qty: i64,
    pub qty_frozen: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lot {
    pub id: i64,
    pub account_id: i64,
    pub company_id: i64,
    pub qty: i64,
    /// price per share, cents
    pub cost: i64,
    pub acquired_on: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Order {
    pub id: i64,
    pub account_id: i64,
    pub company_id: i64,
    pub side: Side,
    pub kind: Kind,
    /// limit price; None for market orders
    pub price: Option<i64>,
    pub qty: i64,
    pub filled: i64,
    /// cash (buy) still reserved
    pub frozen: i64,
    pub status: OrderStatus,
    pub created_at: String,
    pub updated_at: String,
}

impl Order {
    pub fn left(&self) -> i64 {
        self.qty - self.filled
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Trade {
    pub id: i64,
    pub company_id: i64,
    pub price: i64,
    pub qty: i64,
    pub buyer_id: i64,
    pub seller_id: i64,
    pub buy_order: Option<i64>,
    pub sell_order: Option<i64>,
    pub ts: String,
    pub day: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerEntry {
    pub id: i64,
    pub ts: String,
    pub day: String,
    pub from_id: i64,
    pub to_id: i64,
    pub amount: i64,
    pub reason: String,
    pub company_id: Option<i64>,
    pub r#ref: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct News {
    pub id: i64,
    pub ts: String,
    pub day: String,
    pub kind: String,
    pub company_id: Option<i64>,
    pub title: String,
    pub body: Option<String>,
}

/// `ipo_applications`: one subscription per player per company; cash frozen until allotment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IpoApplication {
    pub company_id: i64,
    pub account_id: i64,
    pub qty: i64,
    pub frozen: i64,
    /// None until allotment
    pub allotted: Option<i64>,
    pub created_at: String,
}

/// `company_days`: what a business did on one day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompanyDay {
    pub company_id: i64,
    pub day: String,
    pub revenue: i64,
    pub rev_us: i64,
    pub rev_cn: i64,
    pub rev_jp: i64,
    pub aired: i64,
    pub dividend_ps: i64,
}

/// `macro_days`: the economy on one day.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MacroDay {
    pub day: String,
    pub rate: f64,
    pub m2: i64,
    pub m2_growth: Option<f64>,
    pub injected: i64,
    pub exported: i64,
    pub taxes: i64,
    pub interest: i64,
}

/// `candles`: one stock's daily bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candle {
    pub company_id: i64,
    pub day: String,
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
    /// live | replay
    pub source: String,
}

/// `player_returns`: the chain-linked (time-weighted) return of a player who has restructured. A player
/// without a row has factor 1, base = the starting cash, no restructurings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerReturn {
    pub account_id: i64,
    /// product of the closed segments' end assets / start assets
    pub return_factor: f64,
    /// the current segment's starting assets, cents
    pub segment_base: i64,
    pub restructures: i64,
    /// trading day of the latest restructuring
    pub last_day: String,
}

/// A `restructures` row (the database assigns the id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Restructure {
    pub account_id: i64,
    pub day: String,
    pub ts: String,
    pub assets_before: i64,
    pub sold: i64,
    pub injected: i64,
}

/// A `player_days` row: a player's assets after one daily close.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayerDay {
    pub account_id: i64,
    pub day: String,
    pub cash: i64,
    pub stock_value: i64,
    /// cents owed for margin trading (V7)
    pub liabilities: i64,
    pub return_index: f64,
    pub restructured: bool,
}
