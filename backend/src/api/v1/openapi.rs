//! `GET /api/v1/openapi.json`: the OpenAPI 3.1 document of API v1, generated from the handlers and the
//! serde structures (utoipa). `cargo run --bin openapi` prints the same document (web: `npm run gen:api`).
//!
//! One adjustment after generation: every property of a response schema is marked required, because v1
//! always sends every key (`null` when unknown, §2.8); request schemas keep their optional fields.

use std::sync::OnceLock;

use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use utoipa::OpenApi;

use super::types::*;
use super::{account, bind, credit, market, search, stocks};

#[derive(OpenApi)]
#[openapi(
    info(title = "安易 AniEx API", version = "1", description = "网页前端和 QQ 机器人共用的公开接口（docs/api-v1.md）。金额一律是整数分；比例是小数；时间戳带 +08:00；交易日 04:00 换日。"),
    paths(
        market::status, market::rules, market::quotes, market::indices, market::airings, market::macro_days,
        search::search, stocks::detail, stocks::book, stocks::trades, stocks::candles, stocks::intraday, stocks::cover,
        market::news, market::news_one,
        stocks::ipos, stocks::ipo_one, account::subscribe, account::unsubscribe,
        account::me, account::portfolio, account::asset_history, account::orders, account::order_one, account::place,
        account::cancel, account::my_trades, account::ledger, account::restructure,
        account::leaderboard, credit::credit, credit::open, credit::repay, credit::stock_margin,
        account::register, account::login, account::logout,
        bind::create_code, bind::list, bind::remove, bind::redeem, bind::bot_remove,
    ),
    components(schemas(
        super::error::ErrorBody, super::error::FieldError, Money, TradingDay, Timestamp,
        super::ws::WsServerMessage, super::ws::WsClientMessage, super::events::Event,
    )),
    tags(
        (name = "market", description = "市场"), (name = "stocks", description = "股票"), (name = "news", description = "资讯"),
        (name = "ipos", description = "新股"), (name = "account", description = "账户"), (name = "leaderboard", description = "排行"),
        (name = "auth", description = "认证"), (name = "bindings", description = "绑定"), (name = "bot", description = "机器人"),
    )
)]
pub struct ApiDoc;

/// Request bodies: optional fields stay optional.
const REQUESTS: &[&str] = &[
    "PlaceOrderRequest", "SubscriptionRequest", "RestructureRequest", "RegisterRequest", "LoginRequest",
    "BindCodeRequest", "RedeemRequest", "WsClientMessage", "RepayRequest",
];

fn require_all(v: &mut Value) {
    match v {
        Value::Object(o) => {
            // a free-form object (`details`, event `data`): any keys, not "no keys"
            if o.get("type") == Some(&Value::String("object".into()))
                && !o.contains_key("properties")
                && !o.contains_key("additionalProperties")
            {
                o.insert("additionalProperties".into(), Value::Bool(true));
            }
            if let Some(Value::Object(props)) = o.get("properties") {
                let keys: Vec<Value> = props.keys().map(|k| Value::String(k.clone())).collect();
                o.insert("required".into(), Value::Array(keys));
            }
            for (_, x) in o.iter_mut() {
                require_all(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(require_all),
        _ => {}
    }
}

/// The document as JSON.
pub fn document() -> Value {
    let mut doc = serde_json::to_value(ApiDoc::openapi()).expect("openapi serializes");
    if let Some(Value::Object(schemas)) = doc.pointer_mut("/components/schemas") {
        for (name, s) in schemas.iter_mut() {
            if !REQUESTS.contains(&name.as_str()) {
                require_all(s);
            }
        }
    }
    // bot token security scheme
    if let Some(Value::Object(c)) = doc.get_mut("components") {
        c.insert(
            "securitySchemes".into(),
            serde_json::json!({"bot": {"type": "http", "scheme": "bearer", "description": "机器人令牌（ANISTOCK_BOT_TOKENS）；加 X-Act-As: qq:<QQ号> 代表已绑定的玩家"}}),
        );
    }
    doc
}

fn cached() -> &'static str {
    static DOC: OnceLock<String> = OnceLock::new();
    DOC.get_or_init(|| serde_json::to_string_pretty(&document()).expect("json"))
}

pub async fn serve() -> Response {
    let mut r = cached().to_string().into_response();
    r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
    r
}
