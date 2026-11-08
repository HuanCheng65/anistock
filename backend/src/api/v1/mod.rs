//! Public API v1 (`/api/v1/*`, docs/api-v1.md): the one interface of the web app and the QQ bot.
//!
//! - Routing is axum's own `Router`; each handler is a plain async fn with a `utoipa::path` annotation,
//!   so `openapi.rs` documents exactly what is served.
//! - `layer` runs around every v1 request: request id, caller resolution (session cookie, bot token,
//!   `X-Act-As`, personal API key), rate limits (§2.12), the `AniEx-Version` header, cache headers and ETags (§2.11).
//! - Reads run on the pooled read-only SQLite connections (`AppState::reads`) off the async workers;
//!   writes to game state go through the engine thread (`actor`), which flushes before it replies.
//!   Bindings, bind codes (not game state) are written on a separate small connection (`side`).

pub mod account;
pub mod bind;
pub mod corporate;
pub mod credit;
pub mod error;
pub mod events;
pub mod futures;
pub mod keys;
pub mod media;
pub mod market;
pub mod openapi;
pub mod assess;
pub mod password;
pub mod pow;
pub mod ratelimit;
pub mod search;
pub mod stocks;
pub mod types;
pub mod works;
pub mod ws;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

pub use error::{E, R};

use super::AppState;
use crate::auth::{read_session, Session};

/// The server version (`AniEx-Version`, WebSocket `hello.server_version`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------- shared v1 state

/// What v1 keeps besides the game: rate-limit buckets, bot tokens, the search names, the side writer.
pub struct V1State {
    pub limiter: Mutex<ratelimit::Limiter>,
    pub limits: ratelimit::Limits,
    /// (name, sha256(token))
    pub bots: Vec<(String, [u8; 32])>,
    pub names: std::sync::RwLock<search::Names>,
    /// writes outside the engine: bindings, bind codes, `last_used_at`
    pub side: Mutex<Option<Connection>>,
    /// `Idempotency-Key`: (account, key) -> (when, status, body)
    pub idem: Mutex<HashMap<(i64, String), (std::time::Instant, u16, Vec<u8>)>>,
    pub events: events::EventRules,
    /// register / login proof of work (docs/design/account.md §3.2)
    pub pow: pow::Pow,
    /// password hashes computed at the same time (`limits.hash_concurrency`)
    pub hashing: tokio::sync::Semaphore,
    /// account -> last password change (Unix seconds), loaded on first use (password.rs)
    pub pw_changed: std::sync::RwLock<Option<HashMap<i64, i64>>>,
    /// account -> (trading day, orders placed that day), for `orders_per_day` (kept in memory: a restart
    /// forgets the day's count)
    pub orders_today: Mutex<HashMap<i64, (String, i64)>>,
}

impl V1State {
    pub fn new(cfg: &crate::config::Config) -> V1State {
        V1State {
            limiter: Mutex::new(ratelimit::Limiter::default()),
            limits: ratelimit::Limits::from_config(cfg),
            bots: parse_bot_tokens(std::env::var("ANISTOCK_BOT_TOKENS").ok().as_deref().unwrap_or("")),
            names: std::sync::RwLock::new(search::Names::default()),
            side: Mutex::new(None),
            idem: Mutex::new(HashMap::new()),
            events: events::EventRules::from_config(cfg),
            pow: pow::Pow::new(pow::PowConfig::from_config(cfg)),
            hashing: tokio::sync::Semaphore::new(ratelimit::Limits::from_config(cfg).hash_concurrency),
            pw_changed: std::sync::RwLock::new(None),
            orders_today: Mutex::new(HashMap::new()),
        }
    }
}

/// `ANISTOCK_BOT_TOKENS`: `name:token[,name:token...]`; only hashes are kept.
pub fn parse_bot_tokens(s: &str) -> Vec<(String, [u8; 32])> {
    use sha2::Digest;
    s.split([',', ' ', '\n'])
        .filter_map(|p| {
            let (n, t) = p.trim().split_once(':')?;
            (!n.is_empty() && t.len() >= 16).then(|| (n.to_string(), sha2::Sha256::digest(t.as_bytes()).into()))
        })
        .collect()
}

// ---------------------------------------------------------------- the caller

/// Who is calling (§3).
#[derive(Clone, Debug)]
pub enum Caller {
    Anon,
    /// a valid session cookie (the account's existence is checked by the handlers that use it)
    Player { id: i64 },
    /// a bot token without `X-Act-As`
    Bot { name: String },
    /// a bot token acting for a bound player
    BotAs { name: String, account: i64, scopes: Vec<String> },
    /// a player's own API key (docs/design/api-access.md)
    ApiKey { account: i64, key: i64, scopes: Vec<String> },
}

/// Who places an order: the account, and the API key when it came through one (`api_orders`).
#[derive(Clone, Copy, Debug)]
pub struct Trader {
    pub account: i64,
    pub key: Option<i64>,
}

impl Caller {
    /// The account whose data a read under `/me` returns: the cookie's, or the bound player's when the
    /// binding grants `read:account`.
    pub fn reader(&self) -> R<i64> {
        match self {
            Caller::Player { id } => Ok(*id),
            Caller::BotAs { account, scopes, .. } if scopes.iter().any(|s| s == "read:account") => Ok(*account),
            Caller::BotAs { .. } => Err(error::forbidden("read:account")),
            Caller::ApiKey { account, scopes, .. } if scopes.iter().any(|s| s == "read:account") => Ok(*account),
            Caller::ApiKey { .. } => Err(error::forbidden("read:account")),
            _ => Err(error::unauthenticated()),
        }
    }

    /// Writes and web-only endpoints: a browser session only (bots get `forbidden`).
    pub fn player(&self) -> R<i64> {
        match self {
            Caller::Player { id } => Ok(*id),
            Caller::BotAs { .. } => Err(error::forbidden("trade")),
            Caller::Bot { .. } => Err(error::forbidden("player")),
            Caller::ApiKey { .. } => Err(error::web_only()),
            Caller::Anon => Err(error::unauthenticated()),
        }
    }

    /// Placing and cancelling orders, subscriptions and repayments: a browser session, or an API key with the
    /// `trade` scope while `[api] trading` is on and the account is verified and has signed the
    /// 程序化交易风险揭示书 (checked on every request, docs/design/api-access.md §4).
    pub async fn trader(&self, st: &Arc<AppState>) -> R<Trader> {
        match self {
            Caller::ApiKey { account, key, scopes } => {
                if !scopes.iter().any(|s| s == "trade") {
                    return Err(error::forbidden("trade"));
                }
                if !st.cfg.api_trading {
                    return Err(error::trade_not_allowed(vec!["api_trading"]));
                }
                let (st2, a) = (st.clone(), *account);
                let missing = read(st, move |c| keys::trade_missing(&st2, c, a)).await?;
                if !missing.is_empty() {
                    return Err(error::trade_not_allowed(missing));
                }
                Ok(Trader { account: *account, key: Some(*key) })
            }
            _ => self.player().map(|account| Trader { account, key: None }),
        }
    }

    /// The account the viewer-specific parts (`viewer`) are for, if any.
    pub fn viewer(&self) -> Option<i64> {
        match self {
            Caller::Player { id } => Some(*id),
            Caller::BotAs { account, scopes, .. } if scopes.iter().any(|s| s == "read:account") => Some(*account),
            Caller::ApiKey { account, scopes, .. } if scopes.iter().any(|s| s == "read:account") => Some(*account),
            _ => None,
        }
    }

    pub fn bot(&self) -> R<&str> {
        match self {
            Caller::Bot { name } | Caller::BotAs { name, .. } => Ok(name),
            _ => Err(error::forbidden("bot")),
        }
    }
}

/// The session cookie's account, if the cookie is valid.
pub fn session_account(st: &AppState, headers: &HeaderMap) -> Option<i64> {
    let token = headers
        .get_all("cookie")
        .iter()
        .find_map(|v| crate::auth::cookie_value(&[&String::from_utf8_lossy(v.as_bytes())], "session"));
    match read_session(&st.secret, token.as_deref(), st.unix_now()) {
        // a cookie from before the last password change no longer counts
        Session::Player(a) if a > 0 && !password::issued_before_change(st, a, token.as_deref().unwrap_or("")) => Some(a),
        _ => None,
    }
}

/// The personal API key in `Authorization: Bearer anx_…`, if that is what the header carries.
pub fn api_key_token(headers: &HeaderMap) -> Option<String> {
    let h = headers.get("authorization")?.to_str().ok()?;
    let t = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer "))?.trim();
    t.starts_with(keys::PREFIX).then(|| t.to_string())
}

/// The bot behind `Authorization: Bearer`, if any: Ok(None) without the header, Err for a bad token.
pub fn bearer(st: &AppState, headers: &HeaderMap) -> R<Option<String>> {
    use sha2::Digest;
    use subtle::ConstantTimeEq;
    let Some(h) = headers.get("authorization") else { return Ok(None) };
    let h = h.to_str().unwrap_or("");
    let Some(token) = h.strip_prefix("Bearer ").or_else(|| h.strip_prefix("bearer ")) else {
        return Err(error::bad_token());
    };
    let hash: [u8; 32] = sha2::Sha256::digest(token.trim().as_bytes()).into();
    let mut found = None;
    for (name, h) in &st.v1.bots {
        if bool::from(h.ct_eq(&hash)) {
            found = Some(name.clone());
        }
    }
    found.map(Some).ok_or_else(error::bad_token)
}

async fn resolve(st: &Arc<AppState>, headers: &HeaderMap, ip: &str) -> R<Caller> {
    if let Some(token) = api_key_token(headers) {
        let k = keys::authenticate(st, &token, ip).await?;
        return Ok(Caller::ApiKey { account: k.account, key: k.id, scopes: k.scopes });
    }
    if let Some(name) = bearer(st, headers)? {
        let Some(act) = headers.get("x-act-as") else { return Ok(Caller::Bot { name }) };
        let act = act.to_str().unwrap_or("").trim().to_string();
        let Some((platform, external)) = act.split_once(':') else {
            return Err(error::invalid(vec![error::bad_format("x-act-as")]));
        };
        let (platform, external) = (platform.to_string(), external.to_string());
        let found = read(st, move |c| {
            c.query_row(
                "SELECT id, account_id, scopes FROM bindings WHERE platform=? AND external_id=?",
                [&platform, &external],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(error::sql)
        })
        .await?;
        let Some((bid, account, scopes)) = found else { return Err(error::not_bound()) };
        let now = st.clock.ts();
        let st2 = st.clone();
        tokio::task::spawn_blocking(move || {
            let _ = bind::side_exec(&st2, "UPDATE bindings SET last_used_at=? WHERE id=?", rusqlite::params![now, bid]);
        });
        let scopes = scopes.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
        return Ok(Caller::BotAs { name, account, scopes });
    }
    Ok(match session_account(st, headers) {
        Some(id) => Caller::Player { id },
        None => Caller::Anon,
    })
}

/// The client address: the peer, or (only when the peer is the local reverse proxy) `X-Real-IP`, else
/// the last `X-Forwarded-For` entry.
pub fn client_ip(peer: Option<SocketAddr>, headers: &HeaderMap) -> String {
    let peer_ip = peer.map(|p| p.ip());
    if peer_ip.is_none_or(|ip| ip.is_loopback()) {
        if let Some(v) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            return v.trim().to_string();
        }
        // the last entry is the one the local proxy appended; earlier ones come from the client
        if let Some(v) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(last) = v.rsplit(',').next().map(str::trim).filter(|s| !s.is_empty()) {
                return last.to_string();
            }
        }
    }
    peer_ip.map(|i| i.to_string()).unwrap_or_else(|| "local".into())
}

// ---------------------------------------------------------------- the layer around every v1 request

fn request_id() -> String {
    let b: [u8; 8] = rand::random();
    hex::encode(b)
}

fn set(h: &mut HeaderMap, k: &'static str, v: &str) {
    if let Ok(v) = HeaderValue::from_str(v) {
        h.insert(k, v);
    }
}

/// Cache class of a GET path (§2.11).
fn cache_class(path: &str) -> &'static str {
    let p = path.strip_prefix("/api/v1").unwrap_or(path);
    if p.starts_with("/me") || p.starts_with("/bot") || p.starts_with("/auth") {
        "private"
    } else if p == "/rules" || p == "/market/macro" || p.ends_with("/candles") || p == "/market/indices" {
        "daily"
    } else if p == "/openapi.json" {
        "openapi"
    } else if p.ends_with("/cover") {
        "cover"
    } else {
        "live"
    }
}

pub async fn layer(State(st): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let rid = request_id();
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = client_ip(peer, req.headers());
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let if_none = req.headers().get("if-none-match").and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut resp = match resolve(&st, req.headers(), &ip).await {
        Err(e) => e.into_response(),
        Ok(caller) => {
            let ip_key = ratelimit::ip_key(&ip);
            let key = match &caller {
                // a key shares its account's bucket with the browser (docs/design/api-access.md §5)
                Caller::Player { id } | Caller::ApiKey { account: id, .. } => ratelimit::Key::Player(*id),
                Caller::Bot { name } | Caller::BotAs { name, .. } => ratelimit::Key::Bot(name.clone()),
                Caller::Anon => ratelimit::Key::Anon(ip_key.clone()),
            };
            let register = method == Method::POST && path.ends_with("/auth/register");
            let auth_path = register
                || (method == Method::POST && (path.ends_with("/auth/login") || path.ends_with("/auth/password-reset")));
            if auth_path {
                st.v1.pow.note_attempt();
            }
            let verdict = {
                let mut l = st.v1.limiter.lock().unwrap();
                let mut v = l.take(&st.v1.limits, key);
                if v.allowed && auth_path {
                    v = l.take(&st.v1.limits, ratelimit::Key::Auth(ip_key.clone()));
                }
                // the daily registration quota is spent only by a registration that succeeds (below):
                // failed attempts from a shared campus address must not lock everyone else out
                if v.allowed && register {
                    v = l.peek(&st.v1.limits, ratelimit::Key::Register(ip_key.clone()));
                }
                v
            };
            if !verdict.allowed {
                let mut r = error::rate_limited(verdict.reset.max(1)).into_response();
                verdict.headers(r.headers_mut());
                r
            } else {
                req.extensions_mut().insert(caller);
                req.extensions_mut().insert(ClientIp(ip.clone()));
                let mut r = next.run(req).await;
                if register && r.status() == StatusCode::CREATED {
                    st.v1.limiter.lock().unwrap().take(&st.v1.limits, ratelimit::Key::Register(ip_key.clone()));
                }
                verdict.headers(r.headers_mut());
                r
            }
        }
    };
    let h = resp.headers_mut();
    set(h, "aniex-version", VERSION);
    set(h, "aniex-request-id", &rid);
    if resp.status().is_server_error() {
        eprintln!("v1 {rid} {method} {path} -> {}", resp.status());
    }
    if method != Method::GET || resp.status() != StatusCode::OK {
        if !resp.headers().contains_key("cache-control") {
            set(resp.headers_mut(), "cache-control", "no-store");
        }
        return resp;
    }
    match cache_class(&path) {
        "private" => {
            set(resp.headers_mut(), "cache-control", "no-store");
            resp
        }
        "cover" => resp,
        class => {
            let (parts, body) = resp.into_parts();
            let Ok(bytes) = axum::body::to_bytes(body, 64 * 1024 * 1024).await else {
                return error::internal("response body").into_response();
            };
            let tag = if class == "openapi" {
                format!("\"v{VERSION}\"")
            } else {
                use md5::Digest;
                format!("\"{}\"", hex::encode(md5::Md5::digest(&bytes)))
            };
            let cache = match class {
                "daily" => "public, max-age=60",
                "openapi" => "public, max-age=300",
                _ => "no-cache",
            };
            let mut resp = Response::from_parts(parts, Body::from(bytes));
            set(resp.headers_mut(), "cache-control", cache);
            set(resp.headers_mut(), "etag", &tag);
            if if_none.as_deref().is_some_and(|v| v.split(',').any(|t| t.trim() == tag || t.trim() == "*")) {
                let mut nm = Response::new(Body::empty());
                *nm.status_mut() = StatusCode::NOT_MODIFIED;
                for k in ["cache-control", "etag", "aniex-version", "aniex-request-id"] {
                    if let Some(v) = resp.headers().get(k).cloned() {
                        nm.headers_mut().insert(k, v);
                    }
                }
                return nm;
            }
            resp
        }
    }
}

/// The client address of this request (for per-IP limits inside handlers).
#[derive(Clone, Debug)]
pub struct ClientIp(pub String);

// ---------------------------------------------------------------- reads

/// Run `f` on a pooled read connection in one read transaction, off the async workers.
pub async fn read<T: Send + 'static>(
    st: &Arc<AppState>,
    f: impl FnOnce(&Connection) -> R<T> + Send + 'static,
) -> R<T> {
    let st = st.clone();
    tokio::task::spawn_blocking(move || st.reads.run(f).map_err(error::sql)?)
        .await
        .map_err(|e| error::internal(format!("read task: {e}")))?
}

/// Run `f` on the engine thread.
pub async fn engine<T: Send + 'static>(
    st: &Arc<AppState>,
    f: impl FnOnce(&mut crate::actor::Ctx) -> T + Send + 'static,
) -> R<T> {
    st.actor.call(f).await.map_err(|_| error::unavailable())
}

// ---------------------------------------------------------------- query strings

/// Parsed query parameters with typed getters that produce `invalid_request` field errors.
pub struct Q(Vec<(String, String)>);

impl Q {
    pub fn parse(raw: Option<&str>) -> Q {
        let dec = |s: &str| percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned();
        Q(raw
            .unwrap_or("")
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('=') {
                Some((k, v)) => (dec(k), dec(v)),
                None => (dec(p), String::new()),
            })
            .collect())
    }

    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().rev().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
    }

    pub fn int(&self, k: &str, min: i64, max: i64) -> R<Option<i64>> {
        match self.get(k) {
            None => Ok(None),
            Some(v) => {
                let n: i64 = v.trim().parse().map_err(|_| error::invalid(vec![error::wrong_type(k)]))?;
                if n < min {
                    return Err(error::invalid(vec![error::too_small(k, min)]));
                }
                if n > max {
                    return Err(error::invalid(vec![error::too_large(k, max)]));
                }
                Ok(Some(n))
            }
        }
    }

    pub fn day(&self, k: &str) -> R<Option<String>> {
        match self.get(k) {
            None => Ok(None),
            Some(v) => {
                let v = v.trim();
                chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| error::invalid(vec![error::bad_format(k)]))?;
                Ok(Some(v.to_string()))
            }
        }
    }

    /// A comma-separated list (empty items dropped).
    pub fn list(&self, k: &str) -> Option<Vec<String>> {
        self.get(k).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
    }

    pub fn bool(&self, k: &str, default: bool) -> R<bool> {
        match self.get(k) {
            None => Ok(default),
            Some("true" | "1") => Ok(true),
            Some("false" | "0") => Ok(false),
            Some(_) => Err(error::invalid(vec![error::wrong_type(k)])),
        }
    }
}

/// A path id (`/stocks/{id}`): an integer, else `invalid_request`.
pub fn path_id(raw: &str, field: &str) -> R<i64> {
    raw.trim().parse::<i64>().map_err(|_| error::invalid(vec![error::wrong_type(field)]))
}

// ---------------------------------------------------------------- cursor pagination (§2.10)

#[derive(Clone, Copy, Debug)]
pub struct Page {
    pub limit: i64,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

fn cursor(q: &Q, k: &str) -> R<Option<i64>> {
    match q.get(k) {
        None => Ok(None),
        Some(v) => v
            .trim()
            .strip_prefix("c_")
            .and_then(|n| n.parse::<i64>().ok())
            .map(Some)
            .ok_or_else(|| error::invalid(vec![error::bad_format(k)])),
    }
}

impl Page {
    pub fn parse(q: &Q) -> R<Page> {
        let limit = q.int("limit", 1, 200)?.unwrap_or(50);
        let before = cursor(q, "before")?;
        let after = cursor(q, "after")?;
        if before.is_some() && after.is_some() {
            return Err(error::invalid(vec![error::field("after", "not_allowed", "before 和 after 只能用一个")]));
        }
        Ok(Page { limit, before, after })
    }

    /// SQL tail on `col`: the id condition (to AND into the WHERE) and the ORDER BY / LIMIT clause.
    pub fn sql(&self, col: &str) -> (String, String) {
        match (self.before, self.after) {
            (_, Some(a)) => (format!("{col} > {a}"), format!("ORDER BY {col} ASC LIMIT {}", self.limit + 1)),
            (Some(b), _) => (format!("{col} < {b}"), format!("ORDER BY {col} DESC LIMIT {}", self.limit + 1)),
            _ => ("1=1".into(), format!("ORDER BY {col} DESC LIMIT {}", self.limit + 1)),
        }
    }

    /// Trim the extra row; returns (items, next_cursor, has_more).
    pub fn finish<T>(&self, mut rows: Vec<T>, id: impl Fn(&T) -> i64) -> (Vec<T>, Option<String>, bool) {
        let more = rows.len() as i64 > self.limit;
        rows.truncate(self.limit as usize);
        let next = if more { rows.last().map(|r| format!("c_{}", id(r))) } else { None };
        (rows, next, more)
    }
}

// ---------------------------------------------------------------- request bodies

/// A JSON object body (writes require `Content-Type: application/json`, §2.13).
pub fn json_body(headers: &HeaderMap, body: &[u8]) -> R<serde_json::Map<String, Value>> {
    let ct = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !ct.trim_start().to_ascii_lowercase().starts_with("application/json") {
        return Err(error::invalid_json());
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(error::invalid_json()),
    }
}

/// An optional JSON body: empty is `{}`.
pub fn json_body_opt(headers: &HeaderMap, body: &[u8]) -> R<serde_json::Map<String, Value>> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Default::default());
    }
    json_body(headers, body)
}

pub fn body_str(b: &serde_json::Map<String, Value>, k: &str) -> R<Option<String>> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(error::invalid(vec![error::wrong_type(k)])),
    }
}

pub fn body_int(b: &serde_json::Map<String, Value>, k: &str) -> R<Option<i64>> {
    match b.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_i64().map(Some).ok_or_else(|| error::invalid(vec![error::wrong_type(k)])),
        Some(_) => Err(error::invalid(vec![error::wrong_type(k)])),
    }
}

pub fn need<T>(v: Option<T>, k: &str) -> R<T> {
    v.ok_or_else(|| error::invalid(vec![error::missing(k)]))
}

// ---------------------------------------------------------------- the router

pub fn router(st: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/market/status", get(market::status))
        .route("/rules", get(market::rules))
        .route("/market/quotes", get(market::quotes))
        .route("/market/indices", get(market::indices))
        .route("/market/airings", get(market::airings))
        .route("/market/macro", get(market::macro_days))
        .route("/stocks/search", get(search::search))
        .route("/stocks/{id}", get(stocks::detail))
        .route("/stocks/{id}/book", get(stocks::book))
        .route("/stocks/{id}/trades", get(stocks::trades))
        .route("/stocks/{id}/candles", get(stocks::candles))
        .route("/stocks/{id}/intraday", get(stocks::intraday))
        .route("/stocks/{id}/cover", get(stocks::cover))
        .route("/stocks/{id}/works", get(works::works))
        .route("/stocks/{id}/corporate-actions", get(corporate::actions))
        .route("/stocks/{id}/rights", get(corporate::rights))
        .route("/stocks/{id}/rights/subscription", put(corporate::subscribe).delete(corporate::unsubscribe))
        .route("/news", get(market::news))
        .route("/news/{id}", get(market::news_one))
        .route("/ipos", get(stocks::ipos))
        .route("/ipos/{id}", get(stocks::ipo_one))
        .route("/ipos/{id}/subscription", put(account::subscribe).delete(account::unsubscribe))
        .route("/me", get(account::me).delete(account::close))
        .route("/assessments/{kind}", get(assess::questions))
        .route("/me/assessments/{kind}", post(assess::submit))
        .route("/me/portfolio", get(account::portfolio))
        .route("/me/asset-history", get(account::asset_history))
        .route("/me/orders", get(account::orders).post(account::place))
        .route("/me/orders/{id}", get(account::order_one).delete(account::cancel))
        .route("/me/trades", get(account::my_trades))
        .route("/me/ledger", get(account::ledger))
        .route("/me/restructure", post(account::restructure))
        .route("/me/agreements", post(account::sign))
        .route("/me/announcements/{id}/read", post(account::read_announcement))
        .route("/me/credit", get(credit::credit).post(credit::open))
        .route("/me/credit/repay", post(credit::repay))
        .route("/stocks/{id}/margin", get(credit::stock_margin))
        .route("/stocks/{id}/heat", get(futures::heat))
        .route("/futures", get(futures::list))
        .route("/futures/{id}", get(futures::detail))
        .route("/me/futures", get(futures::mine))
        .route("/me/futures/orders", post(futures::place))
        .route("/me/futures/orders/{id}", delete(futures::cancel))
        .route("/leaderboard", get(account::leaderboard))
        .route("/auth/challenge", get(account::challenge))
        .route("/auth/register", post(account::register))
        .route("/auth/login", post(account::login))
        .route("/auth/logout", post(account::logout))
        .route("/auth/password-reset", post(password::reset))
        .route("/me/password", post(password::change))
        .route("/bot/password-resets", post(password::bot_issue))
        .route("/me/bind-codes", post(bind::create_code))
        .route("/me/bindings", get(bind::list))
        .route("/me/bindings/{id}", delete(bind::remove))
        .route("/me/api-keys", get(keys::list).post(keys::create))
        .route("/me/api-keys/{id}", delete(keys::revoke))
        .route("/bot/bindings", post(bind::redeem))
        .route("/bot/news", post(media::push))
        .route("/bot/bindings/{platform}/{external_id}", delete(bind::bot_remove))
        .route("/openapi.json", get(openapi::serve))
        .fallback(|| async { error::not_found() })
        .method_not_allowed_fallback(|req: Request| async move { method_fallback(req.uri().path()) })
        .layer(axum::middleware::from_fn_with_state(st.clone(), layer))
        .with_state(st.clone());
    Router::new()
        .nest("/api/v1", api)
        .route("/api/v1/ws", get(ws::handler).with_state(st.clone()))
        .route("/api/internal/health", get(health).with_state(st))
}

/// 405 with the methods the path does take.
fn method_fallback(path: &str) -> Response {
    let p = path.strip_prefix("/api/v1").unwrap_or(path);
    let segs: Vec<&str> = p.split('/').collect();
    let allow = match segs.as_slice() {
        ["", "me", "orders"] => "GET, POST",
        ["", "me", "orders", _] => "GET, DELETE",
        ["", "ipos", _, "subscription"] | ["", "stocks", _, "rights", "subscription"] => "PUT, DELETE",
        ["", "auth", "challenge"] => "GET",
        ["", "me"] => "GET, DELETE",
        ["", "me", "assessments", _] | ["", "me", "password"] | ["", "bot", "password-resets"] => "POST",
        ["", "auth", _] | ["", "me", "restructure"] | ["", "me", "agreements"] | ["", "me", "bind-codes"] | ["", "bot", "bindings"] => "POST",
        ["", "me", "api-keys"] => "GET, POST",
        ["", "me", "bindings", _] | ["", "me", "api-keys", _] | ["", "bot", "bindings", _, _] => "DELETE",
        ["", "me", "announcements", _, "read"] => "POST",
        _ => "GET",
    };
    error::method_not_allowed(allow).into_response()
}

/// `GET /api/internal/health` (not in the OpenAPI document).
async fn health(State(st): State<Arc<AppState>>) -> Response {
    let t0 = std::time::Instant::now();
    let ok = st.actor.call(|_| ()).await.is_ok();
    let lag = t0.elapsed().as_millis() as i64;
    let mut r = axum::Json(types::Health { ok, version: VERSION.into(), engine_lag_ms: lag }).into_response();
    if !ok {
        *r.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    }
    set(r.headers_mut(), "aniex-version", VERSION);
    set(r.headers_mut(), "cache-control", "no-store");
    r
}

/// 201 with a JSON body.
pub fn created<T: serde::Serialize>(v: T) -> Response {
    (StatusCode::CREATED, axum::Json(v)).into_response()
}

pub fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}
