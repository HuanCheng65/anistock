//! The HTTP API: every v1 `/api/*` endpoint with the same paths, methods, status codes and JSON, plus
//! the single-page app from `web/dist`. See `docs/v2-architecture.md` §2 and §5.
//!
//! Routing is done here rather than by axum so that unmatched requests behave like Starlette: routes are
//! tried in v1's declaration order; a path that matches only with another method gives 405
//! `{"detail":"Method Not Allowed"}`; with `web/dist` present, every other GET falls through to the SPA
//! catch-all (`api/...` paths answer 404 `没有这个接口`); without it, Starlette's slash redirect and
//! `{"detail":"Not Found"}`.
//!
//! Read paths: every GET endpoint runs v1's SQL on a pooled read-only SQLite connection inside one read
//! transaction (a consistent snapshot, like v1's single-threaded handlers), on tokio's blocking pool.
//! Writes go through the engine thread (`actor`), which flushes before replying, so reads after a write
//! see it. Using the same SQL as v1 keeps row order and aggregate arithmetic identical; in-memory
//! snapshots (arc-swap) are left for the WebSocket step, when the hot paths move to pushes.

pub mod account;
pub mod json;
pub mod market;
pub mod orders;
pub mod spa;
pub mod v2;
pub mod validate;
pub mod v1;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::{Body as AxBody, Bytes};
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Row};

use crate::actor::Actor;
use crate::config::{Clock, Config, Phase};
use crate::obj;
use json::J;

/// Shared by every handler.
pub struct AppState {
    pub actor: Actor,
    pub cfg: Arc<Config>,
    pub clock: Clock,
    pub secret: Vec<u8>,
    pub reads: ReadPool,
    /// `web/dist`, if it exists at startup (v1 decides at import time)
    pub dist: Option<PathBuf>,
    /// the real-time fan-out on the engine thread (`/api/ws`); None in tools and tests
    pub live: Option<crossbeam_channel::Sender<crate::live::Ctl>>,
    /// API v1 state (rate limits, bot tokens, search names, side writer)
    pub v1: v1::V1State,
}

impl AppState {
    pub fn phase(&self) -> Phase {
        self.clock.phase(&self.cfg.game, self.cfg.always_open)
    }

    /// `time.time()` (follows a frozen game clock, for tests).
    pub fn unix_now(&self) -> f64 {
        self.clock.now().timestamp_micros() as f64 / 1e6
    }

    /// Run `f` on a pooled read connection in one read transaction, off the async workers.
    pub async fn read<R: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Connection) -> Result<R, ApiError> + Send + 'static,
    ) -> Result<R, ApiError> {
        let st = self.clone();
        tokio::task::spawn_blocking(move || st.reads.with(f)).await.map_err(|e| internal(format!("read task: {e}")))?
    }
}

// ---------------------------------------------------------------- read pool

pub struct ReadPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    max_idle: usize,
}

impl ReadPool {
    pub fn new(path: PathBuf, max_idle: usize) -> ReadPool {
        ReadPool { path, idle: Mutex::new(Vec::new()), max_idle }
    }

    fn open(&self) -> rusqlite::Result<Connection> {
        let c = Connection::open(&self.path)?;
        c.busy_timeout(std::time::Duration::from_millis(5000))?;
        c.pragma_update(None, "query_only", "ON")?;
        Ok(c)
    }

    pub fn with<R>(&self, f: impl FnOnce(&Connection) -> Result<R, ApiError>) -> Result<R, ApiError> {
        self.run(f).map_err(sql)?
    }

    /// Run `f` on a pooled connection inside one read transaction.
    pub fn run<R>(&self, f: impl FnOnce(&Connection) -> R) -> rusqlite::Result<R> {
        let pooled = self.idle.lock().unwrap().pop();
        let conn = match pooled {
            Some(c) => c,
            None => self.open()?,
        };
        conn.execute_batch("BEGIN")?;
        let r = f(&conn);
        let _ = conn.execute_batch("COMMIT");
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < self.max_idle {
            idle.push(conn);
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------- errors and responses

#[derive(Debug)]
pub enum ApiError {
    /// `HTTPException(status, detail)` / `OrderError` (400)
    Http(u16, String),
    /// 422 `{"detail": [...]}`
    Validation(Vec<J>),
    /// an exception v1 does not handle: 500 text/plain
    Internal(String),
}

pub fn http(status: u16, msg: impl Into<String>) -> ApiError {
    ApiError::Http(status, msg.into())
}

pub fn internal(why: impl Into<String>) -> ApiError {
    ApiError::Internal(why.into())
}

/// A 422 for these validation errors, or a 500 where v1 crashes rendering them: the input of a
/// non-JSON body is echoed as text, and FastAPI cannot decode bytes that are not UTF-8.
pub fn invalid(body: &validate::Body, errs: Vec<J>) -> ApiError {
    match body {
        validate::Body::Raw(b) if std::str::from_utf8(b).is_err() => internal("UnicodeDecodeError rendering a 422"),
        _ => ApiError::Validation(errs),
    }
}

pub fn sql(e: rusqlite::Error) -> ApiError {
    internal(format!("sqlite: {e}"))
}

pub type ApiResult = Result<Reply, ApiError>;

/// A successful response.
pub struct Reply {
    pub status: u16,
    pub json: J,
    pub set_cookie: Option<String>,
}

impl Reply {
    pub fn ok(json: J) -> Reply {
        Reply { status: 200, json, set_cookie: None }
    }
}

fn json_response(status: u16, j: &J, cookie: Option<&str>) -> Response<AxBody> {
    let Some(text) = j.dumps() else { return text_500() };
    let mut r = Response::new(AxBody::from(text));
    *r.status_mut() = StatusCode::from_u16(status).unwrap();
    r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
    if let Some(c) = cookie {
        if let Ok(v) = HeaderValue::from_str(c) {
            r.headers_mut().insert("set-cookie", v);
        }
    }
    r
}

fn text_500() -> Response<AxBody> {
    let mut r = Response::new(AxBody::from("Internal Server Error"));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r.headers_mut().insert("content-type", HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

pub fn detail(status: u16, msg: &str) -> Response<AxBody> {
    json_response(status, &obj! {"detail" => msg}, None)
}

impl ApiError {
    fn into_response(self) -> Response<AxBody> {
        match self {
            ApiError::Http(s, m) => detail(s, &m),
            ApiError::Validation(errs) => json_response(422, &obj! {"detail" => J::Arr(errs)}, None),
            ApiError::Internal(why) => {
                eprintln!("500: {why}");
                text_500()
            }
        }
    }
}

// ---------------------------------------------------------------- requests

/// What the handlers see of a request.
pub struct Req {
    pub method: Method,
    /// percent-decoded, like the ASGI `path`
    pub path: String,
    /// `parse_qsl(keep_blank_values=True)`
    pub query: Vec<(String, String)>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Req {
    /// A query parameter (the last occurrence, like Starlette's `QueryParams.get`).
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.headers.get("content-type").and_then(|v| v.to_str().ok())
    }

    /// The `session` cookie (Starlette reads the first Cookie header only).
    pub fn session(&self) -> Option<String> {
        let h = self.headers.get("cookie").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())?;
        crate::auth::cookie_value(&[h.as_str()], "session")
    }

    pub fn body(&self) -> Result<validate::Body, ApiError> {
        validate::read_body(&self.body, self.content_type()).map_err(|e| match e {
            validate::BodyError::Json(err) => ApiError::Validation(vec![err]),
            validate::BodyError::Undecodable => http(400, "There was an error parsing the body"),
        })
    }
}

/// `urllib.parse.unquote` (UTF-8, invalid sequences replaced).
pub fn unquote(s: &str) -> String {
    percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn parse_qsl(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((unquote(&k.replace('+', " ")), unquote(&v.replace('+', " "))))
        })
        .collect()
}

// ---------------------------------------------------------------- routing

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ep {
    Register,
    Login,
    Logout,
    Me,
    Restructure,
    Portfolio,
    Status,
    Market,
    Stock,
    Candles,
    Intraday,
    Place,
    Cancel,
    MyOrders,
    MyTrades,
    MyLedger,
    Ipos,
    Subscribe,
    Unsubscribe,
    Leaderboard,
    News,
    Macro,
}

/// v1's routes in declaration order: (method, pattern, endpoint). `{}` matches one path segment.
const ROUTES: &[(&str, &str, Ep)] = &[
    ("POST", "/api/register", Ep::Register),
    ("POST", "/api/login", Ep::Login),
    ("POST", "/api/logout", Ep::Logout),
    ("GET", "/api/me", Ep::Me),
    ("POST", "/api/me/restructure", Ep::Restructure),
    ("GET", "/api/portfolio", Ep::Portfolio),
    ("GET", "/api/status", Ep::Status),
    ("GET", "/api/market", Ep::Market),
    ("GET", "/api/stocks/{}", Ep::Stock),
    ("GET", "/api/stocks/{}/candles", Ep::Candles),
    ("GET", "/api/stocks/{}/intraday", Ep::Intraday),
    ("POST", "/api/orders", Ep::Place),
    ("DELETE", "/api/orders/{}", Ep::Cancel),
    ("GET", "/api/orders", Ep::MyOrders),
    ("GET", "/api/trades", Ep::MyTrades),
    ("GET", "/api/ledger", Ep::MyLedger),
    ("GET", "/api/ipos", Ep::Ipos),
    ("POST", "/api/ipos/{}/subscribe", Ep::Subscribe),
    ("DELETE", "/api/ipos/{}/subscribe", Ep::Unsubscribe),
    ("GET", "/api/leaderboard", Ep::Leaderboard),
    ("GET", "/api/news", Ep::News),
    ("GET", "/api/macro", Ep::Macro),
];

/// Path parameters if `path` matches `pattern` (Starlette `{name}` = `[^/]+`).
fn match_path(pattern: &str, path: &str) -> Option<Vec<String>> {
    let ps: Vec<&str> = pattern.split('/').collect();
    let xs: Vec<&str> = path.split('/').collect();
    if ps.len() != xs.len() {
        return None;
    }
    let mut params = Vec::new();
    for (p, x) in ps.iter().zip(&xs) {
        if *p == "{}" {
            if x.is_empty() {
                return None;
            }
            params.push((*x).to_string());
        } else if p != x {
            return None;
        }
    }
    Some(params)
}

/// FastAPI's `APIRoute` allows exactly the declared methods (no implicit HEAD, unlike Starlette's Route).
fn method_ok(route: &str, m: &Method) -> bool {
    m.as_str() == route
}

/// The axum entry point: every request comes here.
pub async fn dispatch(st: Arc<AppState>, req: Request<AxBody>) -> Response<AxBody> {
    let (parts, body) = req.into_parts();
    let body = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return detail(400, "There was an error parsing the body"),
    };
    let raw_path = parts.uri.path().to_string();
    let r = Req {
        method: parts.method.clone(),
        path: unquote(&raw_path),
        query: parse_qsl(parts.uri.query().unwrap_or("")),
        headers: parts.headers,
        body,
    };
    // for HEAD, hyper sends the headers (content-length included) and drops the body, like uvicorn
    route(&st, &r, parts.uri.query()).await
}

async fn route(st: &Arc<AppState>, r: &Req, raw_query: Option<&str>) -> Response<AxBody> {
    // read-only endpoints for the redesigned frontend (outside v1's API)
    if let Some(resp) = v2::route(st, r).await {
        return resp;
    }
    // Starlette: the first full match wins; else the first route matching the path only gives 405
    let mut partial: Option<&str> = None;
    for (m, pat, ep) in ROUTES {
        if let Some(params) = match_path(pat, &r.path) {
            if method_ok(m, &r.method) {
                return match endpoint(st, r, *ep, &params).await {
                    Ok(reply) => json_response(reply.status, &reply.json, reply.set_cookie.as_deref()),
                    Err(e) => e.into_response(),
                };
            }
            partial.get_or_insert(m);
        }
    }
    if let Some(dist) = &st.dist {
        if let Some(rest) = r.path.strip_prefix("/assets/") {
            return spa::asset(dist, rest, r).await;
        }
        if r.method == Method::GET {
            return spa::catch_all(dist, &r.path).await;
        }
        partial.get_or_insert("GET");
    }
    if let Some(m) = partial {
        let mut resp = detail(405, "Method Not Allowed");
        resp.headers_mut().insert("allow", HeaderValue::from_static(m));
        return resp;
    }
    // Starlette redirect_slashes: try the path with the trailing slash removed / added
    if r.path != "/" {
        let other = match r.path.strip_suffix('/') {
            Some(p) => p.to_string(),
            None => format!("{}/", r.path),
        };
        if ROUTES.iter().any(|(_, pat, _)| match_path(pat, &other).is_some()) {
            let scheme = r.headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
            let host = r.headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("localhost");
            let enc = percent_encoding::utf8_percent_encode(&other, PATH_SET).to_string();
            let q = raw_query.map(|q| format!("?{q}")).unwrap_or_default();
            let mut resp = Response::new(AxBody::empty());
            *resp.status_mut() = StatusCode::TEMPORARY_REDIRECT;
            if let Ok(v) = HeaderValue::from_str(&format!("{scheme}://{host}{enc}{q}")) {
                resp.headers_mut().insert("location", v);
            }
            return resp;
        }
    }
    detail(404, "Not Found")
}

const PATH_SET: &percent_encoding::AsciiSet =
    &percent_encoding::CONTROLS.add(b' ').add(b'"').add(b'#').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}');

async fn endpoint(st: &Arc<AppState>, r: &Req, ep: Ep, params: &[String]) -> ApiResult {
    let p = |i: usize| params[i].as_str();
    match ep {
        Ep::Register => account::register(st, r).await,
        Ep::Login => account::login(st, r).await,
        Ep::Logout => account::logout(),
        Ep::Me => account::me(st, r).await,
        Ep::Restructure => account::restructure(st, r).await,
        Ep::Portfolio => account::portfolio(st, r).await,
        Ep::Status => market::status(st).await,
        Ep::Market => market::market(st).await,
        Ep::Stock => market::stock(st, r, p(0)).await,
        Ep::Candles => market::candles(st, p(0)).await,
        Ep::Intraday => market::intraday(st, p(0)).await,
        Ep::Place => orders::place(st, r).await,
        Ep::Cancel => orders::cancel(st, r, p(0)).await,
        Ep::MyOrders => orders::my_orders(st, r).await,
        Ep::MyTrades => orders::my_trades(st, r).await,
        Ep::MyLedger => orders::my_ledger(st, r).await,
        Ep::Ipos => market::ipos(st, r).await,
        Ep::Subscribe => orders::subscribe(st, r, p(0)).await,
        Ep::Unsubscribe => orders::unsubscribe(st, r, p(0)).await,
        Ep::Leaderboard => market::leaderboard(st).await,
        Ep::News => market::news(st, r).await,
        Ep::Macro => market::macro_days(st).await,
    }
}

// ---------------------------------------------------------------- shared helpers

/// A column as JSON, typed like Python's sqlite3 (INTEGER -> int, REAL -> float, TEXT -> str).
pub fn col(row: &Row, name: &str) -> J {
    match row.get_ref(name) {
        Ok(ValueRef::Null) => J::Null,
        Ok(ValueRef::Integer(i)) => J::Int(i),
        Ok(ValueRef::Real(f)) => J::Float(f),
        Ok(ValueRef::Text(t)) => J::Str(String::from_utf8_lossy(t).into_owned()),
        Ok(ValueRef::Blob(b)) => J::Str(String::from_utf8_lossy(b).into_owned()),
        Err(e) => panic!("column {name}: {e}"),
    }
}

/// An integer column that may be NULL.
pub fn opt_i64(row: &Row, name: &str) -> Option<i64> {
    row.get::<_, Option<i64>>(name).expect("integer column")
}

pub fn get_i64(row: &Row, name: &str) -> i64 {
    row.get::<_, i64>(name).expect("integer column")
}

pub fn get_str(row: &Row, name: &str) -> Option<String> {
    row.get::<_, Option<String>>(name).expect("text column")
}

/// v1 `yuan(cents)`: `None` stays null, else `round(cents / 100, 2)` as a float.
pub fn yuan(v: impl Into<J>) -> J {
    match v.into() {
        J::Int(c) => J::Float(c as f64 / 100.0),
        J::Float(f) => J::Float(crate::pyfmt::round_to(f / 100.0, 2)),
        J::Null => J::Null,
        other => other,
    }
}

/// Python `a / b` for ints.
pub fn div(a: i64, b: i64) -> f64 {
    a as f64 / b as f64
}

/// `gdb.get_state(c, key)`
pub fn state(c: &Connection, key: &str) -> Result<Option<String>, ApiError> {
    use rusqlite::OptionalExtension;
    c.query_row("SELECT value FROM state WHERE key=?", [key], |r| r.get::<_, String>(0)).optional().map_err(sql)
}

/// `float(gdb.get_state(c, "rate"))` (a missing row is a TypeError in v1: 500)
pub fn state_rate(c: &Connection) -> Result<f64, ApiError> {
    let v = state(c, "rate")?.ok_or_else(|| internal("state 'rate' missing"))?;
    crate::pyfmt::parse_float(&v).ok_or_else(|| internal("state 'rate' is not a float"))
}

/// `max(0.0, rate - deposit_spread)`
pub fn deposit_rate(cfg: &Config, rate: f64) -> f64 {
    let spread = match cfg.param("central_bank.deposit_spread") {
        Some(toml::Value::Float(f)) => *f,
        Some(toml::Value::Integer(i)) => *i as f64,
        _ => 0.0,
    };
    let x = rate - spread;
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// A TOML value as v1 would serialize it (ints stay ints).
pub fn toml_json(v: &toml::Value) -> J {
    match v {
        toml::Value::String(s) => J::Str(s.clone()),
        toml::Value::Integer(i) => J::Int(*i),
        toml::Value::Float(f) => J::Float(*f),
        toml::Value::Boolean(b) => J::Bool(*b),
        toml::Value::Array(a) => J::Arr(a.iter().map(toml_json).collect()),
        toml::Value::Table(t) => J::Obj(t.iter().map(|(k, v)| (k.clone(), toml_json(v))).collect()),
        toml::Value::Datetime(d) => J::Str(d.to_string()),
    }
}

/// `cfg.game[key]` as JSON.
pub fn game(cfg: &Config, key: &str) -> J {
    cfg.game_raw.get(key).map(toml_json).unwrap_or(J::Null)
}

/// An int path/query value used in SQL: v1 binds Python ints, and one outside SQLite's 64-bit range
/// raises OverflowError (500).
pub fn sql_int(n: validate::BigInt) -> Result<i64, ApiError> {
    match n {
        validate::BigInt::Small(v) => Ok(v),
        validate::BigInt::Huge(_) => Err(internal("Python int too large to convert to SQLite INTEGER")),
    }
}
