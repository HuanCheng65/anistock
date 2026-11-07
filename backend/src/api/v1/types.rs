//! The serde structures of API v1 (docs/api-v1.md §4). They are the code-side source of the OpenAPI
//! document (`/api/v1/openapi.json`) and of the generated TypeScript types (`web/src/api/schema.d.ts`).
//!
//! Every field is always present (`null` for unknown or not applicable); `openapi.rs` marks every
//! property of a response schema as required so the TS types say `x: T | null`, not `x?: T`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

/// 金额，单位：分（1 星辉 = 100 分）。显示时除以 100。
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(transparent)]
#[schema(value_type = i64, format = Int64)]
pub struct Money(pub i64);

/// 交易日 `YYYY-MM-DD`（04:00 换日）。
#[derive(Serialize, Deserialize, ToSchema, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(transparent)]
#[schema(value_type = String, format = Date)]
pub struct TradingDay(pub String);

/// 时间戳，RFC 3339，带 `+08:00` 偏移。
#[derive(Serialize, Deserialize, ToSchema, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(transparent)]
#[schema(value_type = String, format = DateTime)]
pub struct Timestamp(pub String);

pub fn m(v: i64) -> Money {
    Money(v)
}
pub fn om(v: Option<i64>) -> Option<Money> {
    v.map(Money)
}
pub fn day(s: impl Into<String>) -> TradingDay {
    TradingDay(s.into())
}
pub fn ts(s: impl Into<String>) -> Timestamp {
    Timestamp(s.into())
}

/// Ratios: rounded to 6 decimals (§2.5).
pub fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

// ---------------------------------------------------------------- enums

/// 市场阶段（封闭）
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Auction,
    Continuous,
    Closed,
}

impl From<crate::config::Phase> for Phase {
    fn from(p: crate::config::Phase) -> Phase {
        match p {
            crate::config::Phase::Auction => Phase::Auction,
            crate::config::Phase::Continuous => Phase::Continuous,
            crate::config::Phase::Closed => Phase::Closed,
        }
    }
}

/// 市场情绪档位（封闭）：冷淡 / 平稳 / 火热
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Sentiment {
    Cold,
    Neutral,
    Hot,
}

/// 买卖方向（封闭）
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// 委托类型（开放）：limit、market
pub type OrderType = String;
/// 委托状态（开放）：open、filled、cancelled
pub type OrderStatus = String;

/// 资讯分组（封闭）：由 kind 推出
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NewsCategory {
    Flash,
    Notice,
    Airing,
    Close,
    /// 外部媒体的新闻（docs/v3-news.md）
    Media,
}

impl NewsCategory {
    pub fn of(kind: &str) -> NewsCategory {
        match kind {
            "flash" | "vcm" => NewsCategory::Flash,
            "premiere" | "finale" | "delay" | "hiatus" => NewsCategory::Airing,
            "dividend" | "rate" => NewsCategory::Close,
            "media" => NewsCategory::Media,
            _ => NewsCategory::Notice,
        }
    }
    pub fn parse(s: &str) -> Option<NewsCategory> {
        Some(match s {
            "flash" => NewsCategory::Flash,
            "notice" => NewsCategory::Notice,
            "airing" => NewsCategory::Airing,
            "close" => NewsCategory::Close,
            "media" => NewsCategory::Media,
            _ => return None,
        })
    }
    /// The kinds of a category; `notice` also takes every unknown kind (handled by the caller).
    pub fn kinds(self) -> &'static [&'static str] {
        match self {
            NewsCategory::Flash => &["flash", "vcm"],
            NewsCategory::Notice => &["system", "ipo", "allotment", "delisting", "warning", "sequel", "split", "rights", "buyback"],
            NewsCategory::Airing => &["premiere", "finale", "delay", "hiatus"],
            NewsCategory::Close => &["dividend", "rate"],
            NewsCategory::Media => &["media"],
        }
    }
}

/// K 线来源（封闭）
#[derive(Serialize, Deserialize, ToSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandleSource {
    Live,
    Replay,
}

// ---------------------------------------------------------------- shared objects

/// 股票的简要引用
#[derive(Serialize, ToSchema, Clone, Debug, PartialEq)]
pub struct StockRef {
    /// AniList 作品 id，也是公司 id；路径参数一律用它
    pub id: i64,
    /// 6 位显示代码
    pub code: String,
    pub title: String,
}

#[derive(Serialize, ToSchema, Clone, Debug, PartialEq)]
pub struct Ratios {
    pub pe_ttm: Option<f64>,
    pub pe_dynamic: Option<f64>,
    pub dividend_yield_ttm: Option<f64>,
    pub dividend_yield_dynamic: Option<f64>,
    pub pb: Option<f64>,
    pub book_value_per_share: Option<Money>,
}

/// 行情表的一行
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Quote {
    pub stock: StockRef,
    pub title_ja: Option<String>,
    pub title_en: Option<String>,
    pub price: Money,
    pub prev_close: Money,
    /// 比例：price / prev_close − 1
    pub change: f64,
    pub open: Option<Money>,
    pub high: Option<Money>,
    pub low: Option<Money>,
    pub volume: i64,
    pub turnover: Money,
    pub market_cap: Money,
    pub week_revenue: Money,
    pub week_revenue_change: Option<f64>,
    pub episodes_aired: i64,
    pub episodes: Option<i64>,
    pub is_long_runner: bool,
    #[schema(format = Date)]
    pub first_air: String,
    #[schema(format = Date)]
    pub listed_on: String,
    pub is_new: bool,
    pub ipo_remaining: i64,
    pub ipo_price: Money,
    pub vcm_active: bool,
    /// 最近 20 个交易日收盘价，旧到新
    pub sparkline: Vec<Money>,
    pub ratios: Ratios,
    pub cover_url: String,
}

#[derive(Serialize, ToSchema, Clone, Debug, PartialEq)]
pub struct Vcm {
    pub active: bool,
    pub until: Option<Timestamp>,
    pub low: Option<Money>,
    pub high: Option<Money>,
    pub count_today: i64,
}

#[derive(Serialize, ToSchema, Clone, Debug, PartialEq)]
pub struct BookLevel {
    pub price: Money,
    pub quantity: i64,
    pub orders: i64,
}

#[derive(Serialize, ToSchema, Clone, Debug, PartialEq)]
pub struct AuctionIndication {
    /// 参考成交价；无法撮合时为 null
    pub price: Option<Money>,
    pub quantity: i64,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct PublicTrade {
    pub id: i64,
    pub price: Money,
    pub quantity: i64,
    pub at: Timestamp,
    pub trading_day: TradingDay,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct NewsItem {
    pub id: i64,
    pub at: Timestamp,
    pub trading_day: TradingDay,
    /// 开放枚举：system、ipo、allotment、premiere、finale、dividend、rate、delisting、warning、vcm、flash、sequel（续作公布、新一季并入系列公司）
    pub kind: String,
    pub category: NewsCategory,
    pub title: String,
    pub body: Option<String>,
    pub stock: Option<StockRef>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Order {
    pub id: i64,
    pub stock: StockRef,
    pub side: OrderSide,
    /// 开放枚举：limit、market
    #[serde(rename = "type")]
    pub r#type: String,
    pub price: Option<Money>,
    pub quantity: i64,
    pub filled_quantity: i64,
    pub average_fill_price: Option<Money>,
    pub frozen_cash: Money,
    /// 开放枚举：open、filled、cancelled
    pub status: String,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub trading_day: TradingDay,
    /// 信用委托的类型，开放枚举：margin_buy（融资买入）、short_sell（融券卖出）、sell_repay（卖券还款）、buy_return（买券还券）；普通委托为 null
    pub credit: Option<String>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct IpoResult {
    pub oversubscribed: f64,
    pub public_quantity: i64,
    pub allotment_ratio: Option<f64>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct IpoFirstDay {
    pub close: Money,
    pub change: Option<f64>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct IpoViewer {
    pub quantity: i64,
    pub frozen: Money,
    pub allotted: Option<i64>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Ipo {
    pub stock: StockRef,
    pub title_ja: Option<String>,
    /// 开放枚举：upcoming、open、allotted、listed、cancelled
    pub status: String,
    pub price: Money,
    #[schema(format = Date)]
    pub sub_start: String,
    #[schema(format = Date)]
    pub sub_end: String,
    #[schema(format = Date)]
    pub listed_on: String,
    #[schema(format = Date)]
    pub first_air: String,
    pub float_quantity: i64,
    pub base_public_quantity: i64,
    pub applied_quantity: i64,
    pub applicants: i64,
    pub result: Option<IpoResult>,
    pub first_day: Option<IpoFirstDay>,
    pub viewer: Option<IpoViewer>,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Position {
    pub stock: StockRef,
    pub quantity: i64,
    pub available_quantity: i64,
    pub average_cost: Money,
    pub price: Money,
    pub value: Money,
    pub pnl: Money,
    pub pnl_ratio: Option<f64>,
    pub day_change: f64,
    pub dividends: Money,
    #[schema(format = Date)]
    pub oldest_lot_on: Option<String>,
}

// ---------------------------------------------------------------- market

#[derive(Serialize, ToSchema)]
pub struct MarketStatus {
    pub now: Timestamp,
    pub trading_day: TradingDay,
    pub phase: Phase,
    pub phase_ends_at: Option<Timestamp>,
    pub next_phase: Option<Phase>,
    pub next_open_at: Option<Timestamp>,
    pub last_closed_day: Option<TradingDay>,
    pub launched_on: Option<TradingDay>,
    pub policy_rate: f64,
    pub deposit_rate: f64,
    /// 当天的市场情绪档位；NPC 市场或情绪层未开启时为 null
    pub sentiment: Option<Sentiment>,
}

#[derive(Serialize, ToSchema)]
pub struct RulesSession {
    pub auction_start: String,
    pub open: String,
    pub close: String,
    pub close_is_next_day: bool,
    pub day_rollover: String,
    pub daily_close: String,
}

#[derive(Serialize, ToSchema)]
pub struct RulesFees {
    pub commission: f64,
    pub stamp_duty: f64,
    /// 印花税由哪一方缴纳
    pub stamp_duty_on: OrderSide,
}

#[derive(Serialize, ToSchema)]
pub struct DividendTaxBand {
    /// 持有天数上限（含）；null 为无上限
    pub max_holding_days: Option<i64>,
    pub rate: f64,
}

#[derive(Serialize, ToSchema)]
pub struct RulesVcm {
    pub threshold: f64,
    pub window_minutes: i64,
    pub cool_minutes: i64,
    pub later_bands: Vec<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct RulesRestructure {
    pub threshold_ratio: f64,
    pub per_season: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ClawbackBand {
    pub oversubscribed: f64,
    pub public_share: f64,
}

#[derive(Serialize, ToSchema)]
pub struct RulesIpo {
    pub subscription_days: i64,
    pub public_base: f64,
    pub clawback: Vec<ClawbackBand>,
}

#[derive(Serialize, ToSchema)]
pub struct Rules {
    pub timezone: String,
    pub session: RulesSession,
    pub fees: RulesFees,
    pub dividend_tax: Vec<DividendTaxBand>,
    pub vcm: RulesVcm,
    pub price_limit_multiple: i64,
    pub initial_cash: Money,
    pub restructure: RulesRestructure,
    pub ipo: RulesIpo,
    pub order_quantity_max: i64,
    pub contact: RulesContact,
}

/// 官方 QQ 机器人（加好友链接）和运营者邮箱；未配置时为 null
#[derive(Serialize, ToSchema)]
pub struct RulesContact {
    pub qq_bot: Option<String>,
    pub email: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct QuotesResponse {
    pub trading_day: TradingDay,
    pub as_of: Timestamp,
    pub items: Vec<Quote>,
}

#[derive(Serialize, ToSchema)]
pub struct Breadth {
    pub up: i64,
    pub flat: i64,
    pub down: i64,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct IndexPoint {
    pub trading_day: TradingDay,
    pub value: f64,
}

#[derive(Serialize, ToSchema)]
pub struct MarketIndex {
    /// 开放枚举：all、season、long
    pub key: String,
    pub name: String,
    pub members: i64,
    pub base_day: TradingDay,
    pub value: f64,
    pub prev_close: f64,
    pub change: f64,
    pub series: Vec<IndexPoint>,
}

#[derive(Serialize, ToSchema)]
pub struct IndicesResponse {
    pub trading_day: TradingDay,
    #[schema(format = Date)]
    pub season_start: String,
    /// total_return（全收益：分红在当天加回）或 price（价格指数）
    pub basis: String,
    pub breadth: Breadth,
    pub items: Vec<MarketIndex>,
}

#[derive(Serialize, ToSchema)]
pub struct Airing {
    pub stock: StockRef,
    pub episode: i64,
    pub airs_at: Timestamp,
    pub has_aired: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AiringsResponse {
    pub trading_day: TradingDay,
    pub from: Timestamp,
    pub to: Timestamp,
    pub items: Vec<Airing>,
}

#[derive(Serialize, ToSchema)]
pub struct MacroRow {
    pub trading_day: TradingDay,
    pub policy_rate: Option<f64>,
    pub m2: Option<Money>,
    pub m2_growth: Option<f64>,
    pub injected: Option<Money>,
    pub exported: Option<Money>,
    pub taxes: Option<Money>,
    pub interest: Option<Money>,
    pub revenue_7d: Option<Money>,
    pub demand_index: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct MacroResponse {
    pub items: Vec<MacroRow>,
}

// ---------------------------------------------------------------- stocks

#[derive(Serialize, ToSchema)]
pub struct SearchMatch {
    /// code、title、title_ja、title_romaji、title_en、alias
    pub field: String,
    pub text: String,
}

#[derive(Serialize, ToSchema)]
pub struct SearchHit {
    pub stock: StockRef,
    /// 开放枚举：subscribing、listed、delisted
    pub status: String,
    pub score: f64,
    pub matched: SearchMatch,
    pub price: Money,
    pub change: f64,
}

#[derive(Serialize, ToSchema)]
pub struct SearchResponse {
    pub query: String,
    pub items: Vec<SearchHit>,
}

#[derive(Serialize, ToSchema)]
pub struct StockQuote {
    pub price: Money,
    pub prev_close: Money,
    pub change: f64,
    pub open: Option<Money>,
    pub high: Option<Money>,
    pub low: Option<Money>,
    pub open_reference: Option<Money>,
    pub volume: i64,
    pub turnover: Money,
    pub market_cap: Money,
    pub ipo_remaining: i64,
}

#[derive(Serialize, ToSchema)]
pub struct ByMarket {
    pub us: Money,
    pub cn: Money,
    pub jp: Money,
}

#[derive(Serialize, ToSchema)]
pub struct RevenueWeek {
    #[schema(format = Date)]
    pub week_start: String,
    pub revenue: Money,
    pub by_market: ByMarket,
    pub dividend_per_share: Money,
    /// 每股收益 = 营收 ×（1 − 运营成本率）×（1 − 企业所得税）/ 股本（与 `Ratios.pe_ttm` 同一口径，四舍五入到分）
    pub earnings_per_share: Money,
    pub episodes_aired: i64,
}

/// 近 4 周（28 天，与 `Ratios` 的窗口同一切法）的实际合计，不年化
#[derive(Serialize, ToSchema)]
pub struct Trailing4w {
    /// 每股收益合计
    pub earnings_per_share: Money,
    /// 每股分红合计
    pub dividends_per_share: Money,
}

#[derive(Serialize, ToSchema)]
pub struct EpisodeDay {
    pub episode: i64,
    pub trading_day: TradingDay,
}

#[derive(Serialize, ToSchema)]
pub struct NextEpisode {
    pub episode: i64,
    pub airs_at: Timestamp,
}

#[derive(Serialize, ToSchema)]
pub struct DividendPaid {
    pub trading_day: TradingDay,
    pub per_share: Money,
}

#[derive(Serialize, ToSchema)]
pub struct NpcHolders {
    pub total: i64,
    /// 按主体类型（value、trend、news、retail）；NPC 市场能给出之前为 null
    pub by_style: Option<std::collections::BTreeMap<String, i64>>,
}

#[derive(Serialize, ToSchema)]
pub struct Holders {
    pub player: i64,
    pub committee: i64,
    pub unissued: i64,
    pub market_maker: i64,
    pub other: i64,
    pub npc: NpcHolders,
}

#[derive(Serialize, ToSchema)]
pub struct AnilistAudience {
    pub popularity: Option<i64>,
    pub score: Option<f64>,
    pub genres: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct BangumiCollection {
    pub wish: i64,
    pub doing: i64,
    pub collect: i64,
    pub on_hold: i64,
    pub dropped: i64,
}

#[derive(Serialize, ToSchema)]
pub struct BangumiAudience {
    pub id: Option<i64>,
    pub collection: Option<BangumiCollection>,
    pub score: Option<f64>,
}

#[derive(Serialize, ToSchema)]
pub struct AnnictAudience {
    pub id: Option<i64>,
    pub watchers: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct Audiences {
    pub anilist: Option<AnilistAudience>,
    pub bangumi: Option<BangumiAudience>,
    pub annict: Option<AnnictAudience>,
}

#[derive(Serialize, ToSchema)]
pub struct StockDetail {
    pub stock: StockRef,
    pub title_ja: Option<String>,
    pub title_en: Option<String>,
    pub studio: Option<String>,
    /// 开放枚举：subscribing、listed、delisted
    pub status: String,
    pub trading_day: TradingDay,
    pub shares: i64,
    #[schema(format = Date)]
    pub first_air: String,
    #[schema(format = Date)]
    pub listed_on: String,
    #[schema(format = Date)]
    pub delisted_on: Option<String>,
    pub episodes: Option<i64>,
    pub episodes_aired: i64,
    pub is_long_runner: bool,
    pub ipo_price: Money,
    pub quote: StockQuote,
    pub vcm: Vcm,
    pub ratios: Ratios,
    pub revenue_weeks: Vec<RevenueWeek>,
    pub trailing_4w: Trailing4w,
    pub episode_days: Vec<EpisodeDay>,
    pub next_episode: Option<NextEpisode>,
    pub dividends: Vec<DividendPaid>,
    pub holders: Holders,
    pub audiences: Audiences,
    pub ipo: Option<Ipo>,
    pub cover_url: String,
}

#[derive(Serialize, ToSchema)]
pub struct BookResponse {
    pub trading_day: TradingDay,
    /// 带毫秒
    pub as_of: Timestamp,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub auction: Option<AuctionIndication>,
}

#[derive(Serialize, ToSchema)]
pub struct Candle {
    pub trading_day: TradingDay,
    pub open: Money,
    pub high: Money,
    pub low: Money,
    pub close: Money,
    pub volume: i64,
    pub source: CandleSource,
    pub is_partial: bool,
}

#[derive(Serialize, ToSchema)]
pub struct CandlesResponse {
    pub items: Vec<Candle>,
    /// 复权方式：`forward`（前复权，默认：拆股、配股之前的价格和成交量按之后的复权因子换算）或 `none`
    pub adjust: String,
    /// 区间内的除权日（拆股、配股生效的第一个交易日）
    pub ex_days: Vec<ExDay>,
}

/// 一次除权：这一天起按新的股数交易
#[derive(Serialize, ToSchema)]
pub struct ExDay {
    pub trading_day: TradingDay,
    /// split | rights
    pub kind: String,
    /// 这一天之前的价格 × 它（前复权）
    pub price_factor: f64,
}

// ---------------------------------------------------------------- corporate actions (docs/v2-corporate.md)

/// 公司资本政策的一次行动：拆股、配股或回购
#[derive(Serialize, ToSchema)]
pub struct CorporateAction {
    pub id: i64,
    /// split | rights | buyback
    pub kind: String,
    /// pending（已公告、未生效）| active（回购进行中）| done | cancelled
    pub status: String,
    /// 公告的交易日
    pub announced: TradingDay,
    /// 拆股：按新股数交易的第一天；配股：登记、缴款、发股的交易日（认购截止）；回购：最后一天
    pub effective: TradingDay,
    /// 拆股：每 1 股拆为几股
    pub split_ratio: Option<i64>,
    /// 配股：每股配几股（每 10 股配 x 股的 x ÷ 10）
    pub rights_ratio: Option<f64>,
    pub rights_price: Option<Money>,
    /// 配股：要筹集的续作制作费
    pub rights_budget: Option<Money>,
    /// 配股完成后：除权参考价、发行股数、玩家认购股数
    pub rights_terp: Option<Money>,
    pub rights_issued: Option<i64>,
    pub rights_subscribed: Option<i64>,
    /// 回购：资金上限、价格上限、已花费、已回购股数
    pub buyback_budget: Option<Money>,
    pub buyback_cap: Option<Money>,
    pub buyback_spent: Option<Money>,
    pub buyback_bought: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct CorporateActionsResponse {
    pub items: Vec<CorporateAction>,
}

/// 正在认购期的配股，以及调用者的配股权和认购
#[derive(Serialize, ToSchema)]
pub struct RightsOffer {
    pub action: Option<CorporateAction>,
    /// 按当前持仓可以认购的股数（登记日按登记时的持仓重新计算）；未登录为 0
    pub entitlement: i64,
    /// 已认购的股数和冻结的资金
    pub subscribed: i64,
    pub frozen: Money,
}

/// 认购配股（设定数量，幂等；0 等于撤回）
#[derive(Deserialize, ToSchema)]
pub struct RightsRequest {
    pub quantity: i64,
}

#[derive(Serialize, ToSchema)]
pub struct IntradayPoint {
    pub minute: Timestamp,
    pub price: Money,
    pub volume: i64,
}

#[derive(Serialize, ToSchema)]
pub struct IntradayResponse {
    pub trading_day: TradingDay,
    pub prev_close: Money,
    pub items: Vec<IntradayPoint>,
}

// ---------------------------------------------------------------- pages

macro_rules! page {
    ($name:ident, $item:ty) => {
        #[derive(Serialize, ToSchema)]
        pub struct $name {
            pub items: Vec<$item>,
            /// 继续翻页用的游标；没有更多时为 null
            pub next_cursor: Option<String>,
            pub has_more: bool,
        }
    };
}
page!(NewsPage, NewsItem);
page!(PublicTradePage, PublicTrade);
page!(OrderPage, Order);
page!(MyTradePage, MyTrade);
page!(LedgerPage, LedgerEntry);

// ---------------------------------------------------------------- ipos

#[derive(Serialize, ToSchema)]
pub struct IposResponse {
    pub trading_day: TradingDay,
    pub items: Vec<Ipo>,
}

#[derive(Deserialize, ToSchema)]
pub struct SubscriptionRequest {
    pub quantity: i64,
}

#[derive(Serialize, ToSchema)]
pub struct SubscriptionResponse {
    pub ipo: Ipo,
    pub frozen: Money,
}

#[derive(Serialize, ToSchema)]
pub struct IpoEnvelope {
    pub ipo: Ipo,
}

// ---------------------------------------------------------------- account

#[derive(Serialize, ToSchema, Clone)]
pub struct RestructureState {
    pub count: i64,
    pub eligible: bool,
    /// not_poor、this_season、market_closed、margin_debt；可以申请时为 null
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Me {
    pub id: i64,
    pub username: String,
    pub trading_day: TradingDay,
    pub cash: Money,
    pub frozen_cash: Money,
    pub available_cash: Money,
    pub stock_value: Money,
    /// 融资融券负债
    pub liabilities: Money,
    /// 净资产：现金 + 持仓市值 − 负债
    pub total_assets: Money,
    pub initial_cash: Money,
    /// 链接收益率（时间加权）
    pub total_return: f64,
    pub dividends: Money,
    pub deposit_rate: f64,
    pub restructure: RestructureState,
    /// 信用账户状态，开放枚举：normal、call、liquidating、defaulted；未开通时为 null
    pub credit_status: Option<String>,
    /// 开户时间
    pub created_at: Timestamp,
    /// 还没有同意或签署的文件（当前版本）；不为空时不能委托、认购或开通信用账户
    pub pending_documents: Vec<PendingDocument>,
    /// 身份验证
    pub verification: Verification,
    /// 财政补贴、新股认购、排行榜的资格
    pub eligibility: EligibilityState,
    /// 最近一次风险测评；没有测评过时为 null
    pub risk: Option<RiskResult>,
    /// 开通信用账户还需要的步骤
    pub credit_steps: CreditSteps,
    /// 用 API 密钥交易（程序化交易）的条件
    pub api_trading: ApiTradingSteps,
    /// 登录后要弹出的公告：开户之后发布、还没看过的最新一条；没有时为 null
    pub announcement: Option<Announcement>,
}

/// 运营公告
#[derive(Serialize, ToSchema, Clone)]
pub struct Announcement {
    pub id: i64,
    pub title: String,
    /// 纯文本，空行分段
    pub body: String,
    pub published_at: Timestamp,
}

/// 风险测评结果
#[derive(Serialize, ToSchema, Clone)]
pub struct RiskResult {
    /// `C1`..`C5`
    pub level: String,
    /// 保守型、稳健型、平衡型、积极型、激进型
    pub name: String,
    pub description: String,
    /// 结果卡上的一句话
    pub motto: String,
    pub score: i64,
    pub taken_at: Timestamp,
    pub valid_until: TradingDay,
    /// 是否仍在有效期内
    pub valid: bool,
}

/// 用 API 密钥交易的条件（每次请求都检查）
#[derive(Serialize, ToSchema, Clone)]
pub struct ApiTradingSteps {
    /// 本站开放了 API 交易；为 false 时只能生成只读密钥
    pub enabled: bool,
    /// 完成了身份验证（服务器不要求身份验证时恒为 true）
    pub verified: bool,
    /// 签署了当前版本的《程序化交易风险揭示书》
    pub disclosure_signed: bool,
    pub disclosure_version: i64,
}

/// 开通信用账户的前置步骤（注册天数、净资产在开通时检查）
#[derive(Serialize, ToSchema, Clone)]
pub struct CreditSteps {
    /// 完成了身份验证（服务器不要求身份验证时恒为 true）
    pub verified: bool,
    /// 风险测评有效且不低于 `min_level`
    pub risk_ok: bool,
    pub min_level: String,
    /// 通过了融资融券知识测试
    pub quiz_passed: bool,
    /// 签署了当前版本的《融资融券风险揭示书》
    pub disclosure_signed: bool,
    pub disclosure_version: i64,
}

/// 一份测评的题目
#[derive(Serialize, ToSchema)]
pub struct Assessment {
    /// `risk` 或 `margin-quiz`
    pub kind: String,
    pub version: i64,
    /// 知识测试的及格题数；风险测评为 null
    pub pass_score: Option<i64>,
    pub questions: Vec<AssessmentQuestion>,
}

#[derive(Serialize, ToSchema)]
pub struct AssessmentQuestion {
    pub text: String,
    pub options: Vec<String>,
}

/// 每题所选选项的序号（从 0 开始）
#[derive(Deserialize, ToSchema)]
pub struct AnswersRequest {
    pub answers: Vec<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct AssessmentResult {
    /// 风险测评的结果
    pub risk: Option<RiskResult>,
    /// 知识测试的结果
    pub quiz: Option<QuizResult>,
}

#[derive(Serialize, ToSchema)]
pub struct QuizResult {
    pub score: i64,
    pub total: i64,
    pub pass_score: i64,
    pub passed: bool,
    pub review: Vec<QuizReview>,
}

/// 一题的批改
#[derive(Serialize, ToSchema)]
pub struct QuizReview {
    pub correct: bool,
    /// 正确选项的序号
    pub answer: i64,
    pub explanation: String,
}

/// 修改密码
#[derive(Deserialize, ToSchema)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

/// 用验证码重设密码
#[derive(Deserialize, ToSchema)]
pub struct ResetPasswordRequest {
    pub username: String,
    /// 机器人发送的 8 位验证码，大小写和连字符不限
    pub code: String,
    pub new_password: String,
}

/// 机器人为私聊它的 QQ 申请找回密码的验证码
#[derive(Deserialize, ToSchema)]
pub struct BotResetRequest {
    /// `qq`
    pub platform: String,
    /// 私聊消息里发送者的 openid
    pub external_id: String,
}

/// 找回密码的验证码（10 分钟有效，只能用一次）
#[derive(Serialize, ToSchema)]
pub struct ResetCode {
    /// `K7Q3-9MZD`
    pub code: String,
    pub expires_at: Timestamp,
    /// 绑定的账号
    pub account: UserRef,
}

/// 注销账号
#[derive(Deserialize, ToSchema)]
pub struct CloseAccountRequest {
    pub password: String,
    /// 必须为 true
    pub confirm: bool,
}

/// 身份验证（docs/v2-account.md §2）
#[derive(Serialize, ToSchema, Clone)]
pub struct Verification {
    pub verified: bool,
    /// 验证方式：`qq`（绑定 QQ）；未验证时为 null
    pub method: Option<String>,
}

/// 一项资格
#[derive(Serialize, ToSchema, Clone)]
pub struct Eligibility {
    pub eligible: bool,
    /// 不符合时的原因，可以直接展示
    pub reason: Option<String>,
}

/// 新股认购资格
#[derive(Serialize, ToSchema, Clone)]
pub struct IpoEligibility {
    pub eligible: bool,
    pub reason: Option<String>,
    /// 认购额度：前几个交易日的日均持仓市值；每支新股的认购金额（数量 × 发行价）不能超过它
    pub quota: Money,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct EligibilityState {
    /// 财政补贴
    pub transfer: Eligibility,
    pub ipo: IpoEligibility,
    /// 排行榜
    pub leaderboard: Eligibility,
}

/// 一份待同意或签署的文件
#[derive(Serialize, ToSchema, Clone)]
pub struct PendingDocument {
    /// `terms` 用户协议、`privacy` 隐私政策、`disclosure` 风险揭示书
    pub doc: String,
    pub version: i64,
}

/// 同意或签署一份文件
#[derive(Deserialize, ToSchema)]
pub struct AgreementRequest {
    /// `terms`、`privacy` 或 `disclosure`
    pub doc: String,
    /// 必须是当前版本
    pub version: i64,
    /// 风险揭示书：输入自己的用户名作为签名；其他文件不需要
    pub signature: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct PortfolioSummary {
    pub cash: Money,
    pub frozen_cash: Money,
    pub available_cash: Money,
    pub stock_value: Money,
    pub liabilities: Money,
    pub total_assets: Money,
}

#[derive(Serialize, ToSchema)]
pub struct Portfolio {
    pub trading_day: TradingDay,
    pub summary: PortfolioSummary,
    pub items: Vec<Position>,
}

#[derive(Serialize, ToSchema)]
pub struct AssetDay {
    pub trading_day: TradingDay,
    pub cash: Money,
    pub stock_value: Money,
    pub liabilities: Money,
    pub total_assets: Money,
    pub return_index: f64,
    pub total_return: f64,
    pub restructured: bool,
    pub is_partial: bool,
}

#[derive(Serialize, ToSchema)]
pub struct AssetHistory {
    pub items: Vec<AssetDay>,
}

#[derive(Deserialize, ToSchema)]
pub struct PlaceOrderRequest {
    pub stock_id: i64,
    pub side: OrderSide,
    /// limit 或 market
    #[serde(rename = "type")]
    pub r#type: String,
    /// 限价，整数分；市价委托省略或为 null
    pub price: Option<Money>,
    pub quantity: i64,
    /// 信用委托：margin_buy、short_sell（side 为 sell）、sell_repay（sell）、buy_return（buy）；只接受限价，只在连续交易时段
    pub credit: Option<String>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Fill {
    pub trade_id: i64,
    pub price: Money,
    pub quantity: i64,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct PlaceOrderResponse {
    pub order: Order,
    pub fills: Vec<Fill>,
}

#[derive(Serialize, ToSchema)]
pub struct OrderEnvelope {
    pub order: Order,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct MyTrade {
    pub id: i64,
    /// 我这一方的委托
    pub order_id: Option<i64>,
    pub stock: StockRef,
    pub side: OrderSide,
    pub price: Money,
    pub quantity: i64,
    pub amount: Money,
    pub commission: Money,
    pub stamp_duty: Money,
    pub at: Timestamp,
    pub trading_day: TradingDay,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct LedgerEntry {
    pub id: i64,
    pub at: Timestamp,
    pub trading_day: TradingDay,
    /// 开放枚举，见 reason_label
    pub reason: String,
    /// 显示用的中文标签
    pub reason_label: String,
    /// 收入为正，支出为负
    pub amount: Money,
    pub stock: Option<StockRef>,
    #[serde(rename = "ref")]
    pub r#ref: Option<i64>,
}

#[derive(Deserialize, ToSchema)]
pub struct RestructureRequest {
    /// 必须为 true
    pub confirm: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MeEnvelope {
    pub me: Me,
}

// ---------------------------------------------------------------- leaderboard

#[derive(Serialize, ToSchema, Clone)]
pub struct UserRef {
    pub id: i64,
    pub username: String,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct LeaderRow {
    pub rank: i64,
    pub user: UserRef,
    pub total_assets: Money,
    /// 比例
    #[serde(rename = "return")]
    pub r#return: f64,
    pub restructures: i64,
    /// 程序化：这个周期内（总榜为开户以来）用 API 密钥下过单
    pub program: bool,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct LeaderViewer {
    pub rank: i64,
    #[serde(rename = "return")]
    pub r#return: f64,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Leaderboard {
    /// all、week、season
    pub period: String,
    pub from_day: Option<TradingDay>,
    pub to_day: TradingDay,
    pub as_of: Timestamp,
    pub items: Vec<LeaderRow>,
    pub viewer: Option<LeaderViewer>,
}

// ---------------------------------------------------------------- auth and bindings

#[derive(Deserialize, ToSchema)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    /// 只在邀请注册模式下需要
    pub invite: Option<String>,
    /// `GET /auth/challenge?purpose=register` 返回题目时必填
    pub pow: Option<super::pow::PowSolution>,
    /// 同意《用户协议》和《隐私政策》，必须为 true
    pub agree: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// 登录请求过多时需要（`GET /auth/challenge?purpose=login` 返回题目时）
    pub pow: Option<super::pow::PowSolution>,
}

/// 注册模式和验证题目
#[derive(Serialize, ToSchema)]
pub struct AuthChallenge {
    /// `open`：开放注册；`invite`：需要邀请码；`closed`：暂停注册
    pub registration: String,
    /// 不需要验证时为 null
    pub challenge: Option<super::pow::PowChallenge>,
}

#[derive(Deserialize, ToSchema)]
pub struct BindCodeRequest {
    /// 默认 ["read:account"]，v1 只接受这一个值
    pub scopes: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct BindCode {
    /// 显示用，带连字符
    pub code: String,
    pub expires_at: Timestamp,
    pub scopes: Vec<String>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct Binding {
    pub id: i64,
    pub platform: String,
    pub external_id: String,
    /// 打码后的 openid（显示用）
    pub display_name: String,
    pub scopes: Vec<String>,
    pub created_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

#[derive(Serialize, ToSchema)]
pub struct BindingsResponse {
    pub items: Vec<Binding>,
}

/// 外部新闻：news-feed 股市扩展的一次判断（docs/v3-news.md）。只有 `event_id` 和 `merged_into` 时表示这个事件被合并
#[derive(Deserialize, ToSchema)]
pub struct MediaNewsRequest {
    /// news-feed 的事件 id；同一个 id 再推是更新判断
    pub event_id: String,
    /// 这个事件被合并进的事件 id
    pub merged_into: Option<String>,
    /// 信源，如 animeanime、ann
    pub source: Option<String>,
    pub url: Option<String>,
    /// 中文标题（最多 60 字）
    pub title: Option<String>,
    pub original_title: Option<String>,
    /// 信源给出的发布时间
    pub published_at: Option<String>,
    /// news-feed 抓到的时间
    pub fetched_at: Option<String>,
    pub stock_id: Option<i64>,
    /// sequel、movie、delay、hiatus、sales、award、collab、controversy、staff、rumor、denial、other
    pub event: Option<String>,
    /// positive、negative、neutral
    pub direction: Option<String>,
    /// 0–10
    pub magnitude: Option<f64>,
    /// official、media、rumor
    pub credibility: Option<String>,
    /// 判断用的模型，如 jev-1.14.0
    pub model: Option<String>,
    /// 每个问题的完整概率分布
    #[schema(value_type = Object)]
    pub probabilities: Option<serde_json::Value>,
}

#[derive(Serialize, ToSchema)]
pub struct MediaNewsResponse {
    pub id: i64,
    /// stored、published、merged
    pub status: String,
    /// 发布成游戏资讯时的资讯 id
    pub news_id: Option<i64>,
    /// 服务器当前的模式：shadow、publish
    pub mode: String,
}

#[derive(Deserialize, ToSchema)]
pub struct RedeemRequest {
    pub code: String,
    pub platform: String,
    pub external_id: String,
}

#[derive(Serialize, ToSchema)]
pub struct RedeemResponse {
    pub binding: Binding,
    pub account: UserRef,
}

/// API 密钥（docs/v2-api-access.md）。明文只在生成时返回一次
#[derive(Serialize, ToSchema, Clone)]
pub struct ApiKey {
    pub id: i64,
    pub name: String,
    /// 明文的前 8 位（`anx_` 加 4 位），用来区分密钥
    pub prefix: String,
    pub scopes: Vec<String>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub expired: bool,
    pub last_used_at: Option<Timestamp>,
    pub last_used_ip: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeysResponse {
    /// 没有吊销的密钥（含已过期的），新的在前
    pub items: Vec<ApiKey>,
    /// 同时有效的密钥上限
    pub max: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct ApiKeyRequest {
    /// 备注名，1–20 字
    pub name: String,
    /// 目前只能是 `["read:account"]`（默认）
    pub scopes: Option<Vec<String>>,
    /// 30、90（默认）或 365
    pub expires_in_days: Option<i64>,
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeyCreated {
    pub key: ApiKey,
    /// 密钥明文，只返回这一次
    pub token: String,
}

#[derive(Serialize, ToSchema)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub engine_lag_ms: i64,
}

/// Arbitrary JSON (event payloads).
pub type Json = Value;

// ---------------------------------------------------------------- works (§4.2.10)

/// 一集的播出：已播的日期是实际播出的交易日；未播的日期来自公开排期（Bangumi、AniList 下一集），没有公开日期时为 null
#[derive(Serialize, ToSchema)]
pub struct ScheduledEpisode {
    pub episode: i64,
    #[schema(format = Date)]
    pub date: Option<TradingDay>,
    pub aired: bool,
    /// 晚于 Bangumi 原定日期，或有这一集的延期公告
    pub delayed: bool,
}

/// Bangumi 单集吐槽数（采集器最新快照）
#[derive(Serialize, ToSchema)]
pub struct EpisodeComments {
    pub episode: i64,
    pub comments: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Work {
    /// AniList 作品 id（第一部就是股票 id）
    pub id: i64,
    pub title: String,
    #[schema(format = Date)]
    pub first_air: Option<TradingDay>,
    /// 总集数，未知或长篇为 null
    pub episodes: Option<i64>,
    pub episodes_aired: i64,
    pub is_long_runner: bool,
    /// 开放枚举：upcoming（未开播）、airing、finished、halted（暂停播出）
    pub status: String,
    /// 一季的全部集数（长篇只有最近 8 集和下一集）
    pub schedule: Vec<ScheduledEpisode>,
    /// 已播各集的讨论（最多最近 26 集）
    pub discussion: Vec<EpisodeComments>,
}

/// 续作
#[derive(Serialize, ToSchema)]
pub struct Sequel {
    /// 开放枚举：unknown（还没有作品关系数据）、none（未公布）、announced（已公布，未定档）、dated（定档）
    pub status: String,
    pub title: Option<String>,
    /// 公布的开播时间，精度按公布的：`2027-01-10`、`2027-01` 或 `2027`；dated 时一定是完整日期
    pub premiere: Option<String>,
    #[schema(format = Date)]
    pub announced: Option<TradingDay>,
}

#[derive(Serialize, ToSchema)]
pub struct WorksResponse {
    pub works: Vec<Work>,
    pub sequel: Sequel,
}
