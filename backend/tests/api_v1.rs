//! Contract tests of API v1 (docs/api-v1.md §7.3): the router in-process on a small seeded database with
//! a frozen clock. Every 2xx JSON body is validated against the generated OpenAPI document (fields
//! present, types, nullability, enums), and each error code the fixture can reach is checked for status,
//! `code` and message.

mod common;

use std::sync::Arc;

use anistock::actor::{Actor, Ctx, EngineOps};
use anistock::api::v1::{self, openapi, V1State};
use anistock::api::{AppState, ReadPool};
use anistock::config::{Clock, Config};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Fx, DAY, TS};
use serde_json::{json, Value};
use tower::ServiceExt;

const BOT: &str = "contract-bot-token-0123456789";

struct App {
    _dir: tempfile::TempDir,
    st: Arc<AppState>,
    doc: Value,
}

fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn app(at: &str, always_open: bool) -> App {
    app_env(at, always_open, &[])
}

/// `app` with extra environment variables for `Config::load_from`.
fn app_env(at: &str, always_open: bool, extra: &[(&'static str, &'static str)]) -> App {
    let funds_on = extra.iter().any(|(k, _)| *k == "TEST_FUNDS");
    let extra: Vec<(&'static str, &'static str)> = extra.to_vec();
    let mut fx = Fx::new();
    let pw = anistock::auth::hash_pw_with_salt("secret1", &[7u8; 16]);
    let alice = fx.e.create_account("player", "alice", Some("alice"), Some(&pw), None, TS);
    let world = fx.e.special("world").unwrap();
    fx.e.transfer(world, alice, 10_000_000, "grant", DAY, TS, None, None);
    fx.e.add_shares(alice, 1, 100, 500, "2026-10-01");
    fx.seed_rest();
    fx.e.set_state("last_closed", "2026-11-04");
    fx.e.set_state("launched_on", "2026-11-01");
    fx.e.set_state("rate", "0.03");
    fx.e.post_news(TS, DAY, "flash", Some(1), "热度快讯：《テスト》第 3 集讨论量高于预期 35%".into(), "正文".into());
    fx.e.flush().unwrap();
    let db = fx.dir.path().join("game.db");
    let (db2, coll) = (db.clone(), fx.dir.path().join("collector.db"));
    let mut cfg = Config::load_from(&root(), move |k| match k {
        "ANISTOCK_GAME_DB" => Some(db2.display().to_string()),
        "ANISTOCK_DB" => Some(coll.display().to_string()),
        "ANISTOCK_INVITE" => Some("club".into()),
        // API trading off unless a test turns it on (server/game.toml may have it on)
        "ANISTOCK_NEWS_MODE" => Some(extra.iter().find(|(n, _)| *n == "ANISTOCK_NEWS_MODE").map_or("shadow", |(_, v)| *v).to_string()),
        "ANISTOCK_API_TRADING" => Some(extra.iter().find(|(n, _)| *n == "ANISTOCK_API_TRADING").map_or("0", |(_, v)| *v).to_string()),
        k => extra.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()),
    })
    .unwrap();
    cfg.always_open = always_open;
    // 指数基金 only where a test asks for them (whatever server/game.toml says)
    cfg.funds.enabled = funds_on;
    // alice has accepted every current document
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        for (doc, v) in &cfg.documents {
            c.execute("INSERT INTO account_agreements VALUES (?,?,?,?,?)", rusqlite::params![alice, doc, v, TS, "alice"]).unwrap();
        }
        // and may open credit: C5, the knowledge test, the margin disclosure
        c.execute("INSERT INTO account_agreements VALUES (?,'margin_disclosure',?,?,?)", rusqlite::params![alice, cfg.margin_disclosure, TS, "alice"]).unwrap();
        c.execute("INSERT INTO risk_assessments(account_id, version, answers, score, level, taken_at, day) VALUES (?,1,'[]',32,5,?,?)", rusqlite::params![alice, TS, "2026-11-05"]).unwrap();
        c.execute("INSERT INTO quiz_results(account_id, quiz, version, score, passed, taken_at, day) VALUES (?,'margin',1,10,1,?,?)", rusqlite::params![alice, TS, "2026-11-05"]).unwrap();
    }
    let cfg = Arc::new(cfg);
    let clock = Clock::fixed(cfg.tz, at).with_rollover(&cfg.game.day_rollover);
    let Fx { dir, mut e } = fx;
    e.funds = cfg.funds.clone();
    e.funds_setup(TS);
    e.flush().unwrap();
    let (actor, _h) = Actor::start(Ctx { engine: e, cfg: cfg.clone(), ops: Box::new(EngineOps::default()) });
    let mut v1s = V1State::new(&cfg);
    v1s.bots = v1::parse_bot_tokens(&format!("koishi:{BOT}"));
    v1s.limits.anon = (1e9, 1e9);
    v1s.limits.player = (1e9, 1e9);
    v1s.limits.auth = (1e9, 1e9);
    v1s.limits.register = (1e9, 1e9);
    v1s.limits.order = (1e9, 1e9);
    // a cheap proof of work: the tests run unoptimised
    v1s.pow.cfg.cost = 2;
    v1s.pow.cfg.register_max = 20;
    let st = Arc::new(AppState {
        actor,
        cfg: cfg.clone(),
        clock,
        secret: b"contract-secret".to_vec(),
        reads: ReadPool::new(db, 4),
        dist: None,
        live: None,
        v1: v1s,
    });
    App { _dir: dir, st, doc: openapi::document() }
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    json: Value,
}

impl App {
    async fn req(&self, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Resp {
        let mut b = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let body = match body {
            Some(v) => {
                b = b.header("content-type", "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let router = v1::router(self.st.clone()).fallback({
            let st = self.st.clone();
            move |r| anistock::api::fallback(st.clone(), r)
        });
        let r = router.oneshot(b.body(body).unwrap()).await.unwrap();
        let (parts, body) = r.into_parts();
        let bytes = axum::body::to_bytes(body, 1 << 24).await.unwrap();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        // the static cover files are outside the API (no API headers)
        if !path.starts_with("/covers/") {
            assert_eq!(parts.headers.get("aniex-version").unwrap(), v1::VERSION, "{path}");
        }
        Resp { status: parts.status, headers: parts.headers, json }
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Resp {
        let h: Vec<(&str, &str)> = cookie.map(|c| vec![("cookie", c)]).unwrap_or_default();
        self.req("GET", path, None, &h).await
    }

    async fn login(&self) -> String {
        let r = self.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "secret1"})), &[]).await;
        assert_eq!(r.status, 200, "{}", r.json);
        r.headers.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
    }

    /// The response schema of `method path` (status 200/201) from the OpenAPI document.
    fn schema(&self, method: &str, template: &str, status: u16) -> Value {
        let op = &self.doc["paths"][template][method.to_ascii_lowercase()];
        op["responses"][status.to_string()]["content"]["application/json"]["schema"].clone()
    }

    fn validate(&self, schema: &Value, v: &Value, at: &str) {
        let s = self.resolve(schema);
        if let Some(alts) = s.get("oneOf").and_then(Value::as_array) {
            let ok = alts.iter().any(|a| {
                let r = self.resolve(a);
                if r.get("type") == Some(&json!("null")) {
                    v.is_null()
                } else {
                    !v.is_null()
                        && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.validate(a, v, at))).is_ok()
                }
            });
            assert!(ok, "{at}: {v} matches no alternative");
            return;
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array) {
            assert!(e.contains(v) || (v.is_null() && type_allows(&s, "null")), "{at}: {v} not in {e:?}");
        }
        if v.is_null() {
            assert!(type_allows(&s, "null"), "{at}: null not allowed by {s}");
            return;
        }
        if type_allows(&s, "object") && v.is_object() {
            let o = v.as_object().unwrap();
            if let Some(req) = s.get("required").and_then(Value::as_array) {
                for k in req {
                    assert!(o.contains_key(k.as_str().unwrap()), "{at}: missing {k}");
                }
            }
            if let Some(props) = s.get("properties").and_then(Value::as_object) {
                for (k, x) in o {
                    let p = props.get(k).unwrap_or_else(|| panic!("{at}: unexpected field {k}"));
                    self.validate(p, x, &format!("{at}.{k}"));
                }
            }
            return;
        }
        if type_allows(&s, "array") {
            let a = v.as_array().unwrap_or_else(|| panic!("{at}: not an array: {v}"));
            if let Some(items) = s.get("items") {
                for (i, x) in a.iter().enumerate() {
                    self.validate(items, x, &format!("{at}[{i}]"));
                }
            }
            return;
        }
        let ok = (type_allows(&s, "integer") && v.is_i64())
            || (type_allows(&s, "number") && v.is_number())
            || (type_allows(&s, "string") && v.is_string())
            || (type_allows(&s, "boolean") && v.is_boolean())
            || s.get("type").is_none();
        assert!(ok, "{at}: {v} is not {s}");
    }

    fn resolve(&self, s: &Value) -> Value {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => self.resolve(self.doc.pointer(&r[1..]).unwrap_or_else(|| panic!("ref {r}"))),
            None => s.clone(),
        }
    }

    async fn checked(&self, template: &str, path: &str, cookie: Option<&str>) -> Value {
        let r = self.get(path, cookie).await;
        assert_eq!(r.status, 200, "{path}: {}", r.json);
        let schema = self.schema("GET", template, 200);
        assert!(!schema.is_null(), "{template} has no 200 schema");
        self.validate(&schema, &r.json, path);
        r.json
    }
}

fn type_allows(s: &Value, t: &str) -> bool {
    match s.get("type") {
        Some(Value::String(x)) => x == t || (t == "number" && x == "integer"),
        Some(Value::Array(a)) => a.iter().any(|x| x == t || (t == "number" && x == "integer")),
        None => s.get("properties").is_some() && t == "object",
        _ => false,
    }
}

fn err(r: &Resp, status: u16, code: &str) {
    assert_eq!(r.status.as_u16(), status, "{}", r.json);
    assert_eq!(r.json["code"], code, "{}", r.json);
    let msg = r.json["message"].as_str().unwrap();
    assert!(!msg.is_empty() && !msg.contains("请勿") && !msg.contains('您') && !msg.contains('！') && !msg.contains("失败"), "{msg}");
    assert!(r.json["details"].is_object());
}

#[tokio::test]
async fn reads_match_the_openapi_document() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let c = Some(ck.as_str());
    for (t, p) in [
        ("/api/v1/market/status", "/api/v1/market/status"),
        ("/api/v1/rules", "/api/v1/rules"),
        ("/api/v1/market/quotes", "/api/v1/market/quotes"),
        ("/api/v1/market/indices", "/api/v1/market/indices"),
        ("/api/v1/market/airings", "/api/v1/market/airings"),
        ("/api/v1/market/macro", "/api/v1/market/macro"),
        ("/api/v1/stocks/search", "/api/v1/stocks/search?q=100001"),
        ("/api/v1/stocks/{id}", "/api/v1/stocks/1"),
        ("/api/v1/stocks/{id}/book", "/api/v1/stocks/1/book"),
        ("/api/v1/stocks/{id}/trades", "/api/v1/stocks/1/trades"),
        ("/api/v1/stocks/{id}/candles", "/api/v1/stocks/1/candles"),
        ("/api/v1/stocks/{id}/intraday", "/api/v1/stocks/1/intraday"),
        ("/api/v1/news", "/api/v1/news"),
        ("/api/v1/ipos", "/api/v1/ipos?status=upcoming,open,allotted,listed"),
        ("/api/v1/leaderboard", "/api/v1/leaderboard?period=week"),
        ("/api/v1/futures", "/api/v1/futures"),
        ("/api/v1/stocks/{id}/heat", "/api/v1/stocks/1/heat"),
    ] {
        a.checked(t, p, None).await;
        a.checked(t, p, c).await;
    }
    for (t, p) in [
        ("/api/v1/me", "/api/v1/me"),
        ("/api/v1/me/portfolio", "/api/v1/me/portfolio"),
        ("/api/v1/me/asset-history", "/api/v1/me/asset-history"),
        ("/api/v1/me/orders", "/api/v1/me/orders?status=all"),
        ("/api/v1/me/trades", "/api/v1/me/trades"),
        ("/api/v1/me/ledger", "/api/v1/me/ledger"),
        ("/api/v1/me/bindings", "/api/v1/me/bindings"),
        ("/api/v1/me/futures", "/api/v1/me/futures"),
    ] {
        a.checked(t, p, c).await;
    }
    let news = a.checked("/api/v1/news", "/api/v1/news?category=flash", None).await;
    assert_eq!(news["items"][0]["category"], "flash");
    let q = a.checked("/api/v1/market/quotes", "/api/v1/market/quotes", None).await;
    assert_eq!(q["items"][0]["price"], 500, "integer cents");
    let me = a.checked("/api/v1/me", "/api/v1/me", c).await;
    assert_eq!(me["cash"], 10_000_000);
    // ETag: an unchanged quotes snapshot answers 304
    let r = a.get("/api/v1/market/quotes", None).await;
    let tag = r.headers.get("etag").unwrap().to_str().unwrap().to_string();
    let r = a.req("GET", "/api/v1/market/quotes", None, &[("if-none-match", &tag)]).await;
    assert_eq!(r.status, 304);
}

#[tokio::test]
async fn orders_and_their_errors() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let place = |b: Value| a.req("POST", "/api/v1/me/orders", Some(b), &h);
    // a resting sell, then a crossing buy from nobody: the sell rests
    let r = place(json!({"stock_id": 1, "side": "sell", "type": "limit", "price": 600, "quantity": 10})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/orders", 201), &r.json, "place");
    let id = r.json["order"]["id"].as_i64().unwrap();
    // the same Idempotency-Key, twice at once and once after: one order
    let keyed = [("cookie", ck.as_str()), ("idempotency-key", "k-1")];
    let body = json!({"stock_id": 1, "side": "sell", "type": "limit", "price": 650, "quantity": 1});
    let (r1, r2) = tokio::join!(
        a.req("POST", "/api/v1/me/orders", Some(body.clone()), &keyed),
        a.req("POST", "/api/v1/me/orders", Some(body.clone()), &keyed)
    );
    let r3 = a.req("POST", "/api/v1/me/orders", Some(body), &keyed).await;
    assert_eq!(r1.status, 201, "{}", r1.json);
    assert_eq!(r1.json["order"]["id"], r2.json["order"]["id"]);
    assert_eq!(r1.json["order"]["id"], r3.json["order"]["id"]);
    let open = a.req("GET", "/api/v1/me/orders?stock_id=1", None, &h).await;
    assert_eq!(open.json["items"].as_array().unwrap().len(), 2, "{}", open.json);
    a.req("DELETE", &format!("/api/v1/me/orders/{}", r1.json["order"]["id"]), None, &h).await;
    let r = a.req("GET", &format!("/api/v1/me/orders/{id}"), None, &h).await;
    assert_eq!(r.json["status"], "open");
    let r = a.req("DELETE", &format!("/api/v1/me/orders/{id}"), None, &h).await;
    assert_eq!(r.json["order"]["status"], "cancelled");
    err(&a.req("DELETE", &format!("/api/v1/me/orders/{id}"), None, &h).await, 409, "order_finished");
    err(&a.req("DELETE", "/api/v1/me/orders/999999", None, &h).await, 404, "order_not_found");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "limit", "quantity": 1})).await, 422, "price_required");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 999_999, "quantity": 1})).await, 422, "price_out_of_range");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 9_000_000})).await, 422, "insufficient_cash");
    err(&place(json!({"stock_id": 1, "side": "sell", "type": "limit", "price": 500, "quantity": 9_999})).await, 422, "insufficient_shares");
    err(&place(json!({"stock_id": 77, "side": "buy", "type": "limit", "price": 500, "quantity": 1})).await, 404, "stock_not_found");
    let r = place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 0})).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["message"], "数量需大于 0");
    assert_eq!(r.json["details"]["fields"][0]["field"], "quantity");
    let r = a.req("POST", "/api/v1/me/orders", None, &h).await;
    err(&r, 400, "invalid_json");
    err(&a.req("POST", "/api/v1/me/restructure", Some(json!({"confirm": true})), &h).await, 422, "restructure_not_eligible");
}

#[tokio::test]
async fn futures_need_their_steps() {
    // server/game.toml turns futures on; a player without the steps (credit, the test, the disclosure) is refused
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let order = json!({"contract_id": 1, "side": "buy", "type": "limit", "price": 9000, "quantity": 1});
    let r = a.req("POST", "/api/v1/me/futures/orders", Some(order), &h).await;
    err(&r, 422, "futures_not_eligible");
    let me = a.req("GET", "/api/v1/me", None, &h).await;
    assert_eq!(me.json["futures_steps"]["credit_open"], json!(false));
    assert_eq!(me.json["futures_steps"]["quiz_passed"], json!(false));
    err(&a.req("GET", "/api/v1/futures/1", None, &h).await, 404, "contract_not_found");
    err(&a.req("DELETE", "/api/v1/me/futures/orders/1", None, &h).await, 404, "order_not_found");
    let bad = json!({"contract_id": 1, "side": "buy", "type": "market", "price": 9000, "quantity": 1});
    err(&a.req("POST", "/api/v1/me/futures/orders", Some(bad), &h).await, 400, "invalid_request");
}

#[tokio::test]
async fn funds_take_requests_until_they_execute() {
    let off = app("2026-11-05T10:00:00+08:00", true);
    err(&off.get("/api/v1/funds", None).await, 404, "funds_closed");
    let a = app_env("2026-11-05T10:00:00+08:00", true, &[("TEST_FUNDS", "1")]);
    let list = a.checked("/api/v1/funds", "/api/v1/funds", None).await;
    let keys: Vec<&str> = list["funds"].as_array().unwrap().iter().map(|f| f["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["all", "top20", "long", "manga", "light_novel", "isekai", "romance", "action"]);
    assert_eq!(list["funds"][0]["name"], "综合");
    assert_eq!(list["funds"][0]["nav"], 1.0);
    a.checked("/api/v1/funds/{key}", "/api/v1/funds/manga", None).await;
    err(&a.get("/api/v1/funds/nope", None).await, 404, "fund_not_found");
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let cash0 = a.req("GET", "/api/v1/me", None, &h).await.json["cash"].as_i64().unwrap();
    let post = |b: Value| a.req("POST", "/api/v1/me/fund-requests", Some(b), &h);
    let r = post(json!({"fund": "all", "kind": "subscribe", "amount": 50_000})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/fund-requests", 201), &r.json, "subscribe");
    // before the session's requests start executing (08:50): counted on the day before, executed today
    assert_eq!((r.json["request"]["status"].as_str(), r.json["request"]["day"].as_str()), (Some("pending"), Some("2026-11-04")));
    let id = r.json["request"]["id"].as_i64().unwrap();
    let me = a.req("GET", "/api/v1/me", None, &h).await.json;
    assert_eq!((me["cash"].as_i64().unwrap(), me["fund_value"].as_i64().unwrap()), (cash0 - 50_000, 50_000));
    let r = post(json!({"fund": "all", "kind": "subscribe", "amount": 100})).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["message"], "申购金额不能低于 100 星辉");
    err(&post(json!({"fund": "all", "kind": "redeem", "units": 100})).await, 422, "insufficient_units");
    err(&post(json!({"fund": "all", "kind": "subscribe", "amount": 999_999_999})).await, 422, "insufficient_cash");
    err(&post(json!({"fund": "nope", "kind": "subscribe", "amount": 50_000})).await, 404, "fund_not_found");
    err(&post(json!({"fund": "all", "kind": "swap", "amount": 50_000})).await, 400, "invalid_request");
    let mine = a.checked("/api/v1/me/funds", "/api/v1/me/funds", Some(&ck)).await;
    assert_eq!(mine["fund_value"], 50_000);
    let r = a.req("DELETE", &format!("/api/v1/me/fund-requests/{id}"), None, &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("DELETE", "/api/v1/me/fund-requests/{id}", 200), &r.json, "cancel");
    assert_eq!(r.json["requests"][0]["status"], "cancelled");
    assert_eq!(a.req("GET", "/api/v1/me", None, &h).await.json["cash"].as_i64().unwrap(), cash0);
    err(&a.req("DELETE", &format!("/api/v1/me/fund-requests/{id}"), None, &h).await, 404, "request_not_cancellable");
}

/// A fresh register challenge with its answer, as the request's `pow`.
async fn solved(a: &App) -> Value {
    solved_for(a, "register").await
}

async fn solved_for(a: &App, purpose: &str) -> Value {
    let r = a.get(&format!("/api/v1/auth/challenge?purpose={purpose}"), None).await;
    let c: anistock::api::v1::pow::PowChallenge = serde_json::from_value(r.json["challenge"].clone()).unwrap();
    let n = anistock::api::v1::pow::solve(&c).unwrap();
    json!({"challenge": c, "number": n})
}

#[tokio::test]
async fn registration_modes() {
    let a = app_env("2026-11-05T10:00:00+08:00", true, &[("ANISTOCK_REGISTRATION", "open")]);
    let r = a.get("/api/v1/auth/challenge?purpose=register", None).await;
    assert_eq!(r.json["registration"], "open");
    let r = a.req("POST", "/api/v1/auth/register", Some(json!({"username": "carol", "password": "secret1", "agree": true, "pow": solved(&a).await})), &[]).await;
    assert_eq!(r.status, 201, "{}", r.json);

    let a = app_env("2026-11-05T10:00:00+08:00", true, &[("ANISTOCK_REGISTRATION", "closed")]);
    let r = a.get("/api/v1/auth/challenge?purpose=register", None).await;
    assert_eq!(r.json["registration"], "closed");
    assert_eq!(r.json["challenge"], Value::Null);
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "carol", "password": "secret1", "invite": "club"})), &[]).await, 403, "registration_closed");
    // login is unaffected
    assert!(!a.login().await.is_empty());
}

#[tokio::test]
async fn login_needs_pow_during_a_surge() {
    let mut a = app("2026-11-05T10:00:00+08:00", true);
    let v1 = &mut Arc::get_mut(&mut a.st).unwrap().v1;
    v1.pow.cfg.surge_per_min = 0; // any attempt starts a surge
    v1.pow.cfg.login_max = 20;
    let body = |pow: Value| Some(json!({"username": "alice", "password": "secret1", "pow": pow}));
    err(&a.req("POST", "/api/v1/auth/login", body(Value::Null), &[]).await, 422, "pow_required");
    let r = a.req("POST", "/api/v1/auth/login", body(solved_for(&a, "login").await), &[]).await;
    assert_eq!(r.status, 200, "{}", r.json);
    // a register challenge is not a login challenge
    err(&a.req("POST", "/api/v1/auth/login", body(solved_for(&a, "register").await), &[]).await, 422, "pow_invalid");
}

#[tokio::test]
async fn only_successful_registrations_use_the_daily_quota() {
    let mut a = app("2026-11-05T10:00:00+08:00", true);
    Arc::get_mut(&mut a.st).unwrap().v1.limits.register = (1.0 / 86400.0, 1.0);
    let reg = |name: &str, pow: Value| Some(json!({"username": name, "password": "secret1", "invite": "club", "agree": true, "pow": pow}));
    for _ in 0..3 {
        err(&a.req("POST", "/api/v1/auth/register", reg("alice", solved(&a).await), &[]).await, 409, "username_taken");
    }
    let r = a.req("POST", "/api/v1/auth/register", reg("bob", solved(&a).await), &[]).await;
    assert_eq!(r.status, 201, "{}", r.json);
    err(&a.req("POST", "/api/v1/auth/register", reg("carol", solved(&a).await), &[]).await, 429, "rate_limited");
}

#[tokio::test]
async fn agreements_gate_trading() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let body = |agree: Value| Some(json!({"username": "dan", "password": "secret1", "invite": "club", "agree": agree, "pow": Value::Null}));
    let r = a.req("POST", "/api/v1/auth/register", body(Value::Null), &[]).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["details"]["fields"][0]["field"], "agree");
    let r = a
        .req("POST", "/api/v1/auth/register", Some(json!({"username": "dan", "password": "secret1", "invite": "club", "agree": true, "pow": solved(&a).await})), &[])
        .await;
    assert_eq!(r.status, 201, "{}", r.json);
    // terms and privacy accepted at registration; the disclosure is still to sign
    assert_eq!(r.json["pending_documents"], json!([{"doc": "disclosure", "version": 1}]));
    let ck = r.headers.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let h = [("cookie", ck.as_str())];
    let order = json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 1});
    err(&a.req("POST", "/api/v1/me/orders", Some(order.clone()), &h).await, 403, "agreement_required");
    err(&a.req("POST", "/api/v1/me/credit", None, &h).await, 403, "agreement_required");
    let sign = |sig: &str, v: i64| Some(json!({"doc": "disclosure", "version": v, "signature": sig}));
    let r = a.req("POST", "/api/v1/me/agreements", sign("alice", 1), &h).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["message"], "签名需与用户名一致");
    err(&a.req("POST", "/api/v1/me/agreements", sign("dan", 2), &h).await, 409, "document_outdated");
    err(&a.req("POST", "/api/v1/me/agreements", Some(json!({"doc": "nope", "version": 1})), &h).await, 400, "invalid_request");
    let r = a.req("POST", "/api/v1/me/agreements", sign(" dan ", 1), &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/agreements", 200), &r.json, "sign");
    assert_eq!(r.json["pending_documents"], json!([]));
    let r = a.req("POST", "/api/v1/me/orders", Some(order), &h).await;
    assert_eq!(r.status, 201, "{}", r.json);
}

#[tokio::test]
async fn verification_gates_transfers_ipo_and_leaderboard() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let me = a.get("/api/v1/me", Some(&ck)).await.json;
    assert_eq!(me["verification"], json!({"verified": false, "method": null}));
    assert_eq!(me["eligibility"]["transfer"]["reason"], "完成身份验证后才能领取财政补贴");
    assert_eq!(me["eligibility"]["ipo"]["reason"], "完成身份验证后才能认购新股");
    assert_eq!(me["eligibility"]["leaderboard"]["reason"], "完成身份验证后才能进入排行");
    let lb = a.get("/api/v1/leaderboard", Some(&ck)).await.json;
    assert_eq!(lb["items"], json!([]));
    err(&a.req("PUT", "/api/v1/ipos/2/subscription", Some(json!({"quantity": 10})), &h).await, 403, "verification_required");

    // 身份验证 = a QQ binding today
    let id = me["id"].as_i64().unwrap();
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    c.execute(
        "INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', 'OPENID1', 'read:account', ?)",
        rusqlite::params![id, TS],
    )
    .unwrap();
    let me = a.get("/api/v1/me", Some(&ck)).await.json;
    assert_eq!(me["verification"], json!({"verified": true, "method": "qq"}));
    // registered today: too new for 财政补贴; 排行榜 waits for trades, not days
    assert_eq!(me["eligibility"]["transfer"]["reason"], "注册满 7 天后才能领取财政补贴");
    assert_eq!(me["eligibility"]["leaderboard"]["reason"], "完成第一笔成交后才能进入排行");
    assert_eq!(me["eligibility"]["ipo"]["quota"], 0);
    assert!(me["eligibility"]["ipo"]["reason"].as_str().unwrap().starts_with("认购额度按前 5 个交易日的日均持仓市值计算"));
    a.validate(&a.schema("GET", "/api/v1/me", 200), &me, "me");
}

#[tokio::test]
async fn assessments_and_opening_credit() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let r = a.get("/api/v1/assessments/risk", None).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("GET", "/api/v1/assessments/{kind}", 200), &r.json, "risk");
    assert_eq!(r.json["questions"].as_array().unwrap().len(), 8);
    let quiz = a.get("/api/v1/assessments/margin-quiz", None).await.json;
    assert_eq!(quiz["pass_score"], 8);
    err(&a.get("/api/v1/assessments/nope", None).await, 404, "not_found");

    // a new player: everything to do before opening credit
    let r = a
        .req("POST", "/api/v1/auth/register", Some(json!({"username": "erin", "password": "secret1", "invite": "club", "agree": true, "pow": solved(&a).await})), &[])
        .await;
    assert_eq!(r.json["risk"], Value::Null);
    assert_eq!(r.json["credit_steps"]["risk_ok"], false);
    let ck = r.headers.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    let h = [("cookie", ck.as_str())];
    a.req("POST", "/api/v1/me/agreements", Some(json!({"doc": "disclosure", "version": 1, "signature": "erin"})), &h).await;

    let r = a.req("POST", "/api/v1/me/assessments/risk", Some(json!({"answers": [1, 1, 1, 1, 1, 1, 1, 1]})), &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["risk"]["level"], "C2");
    assert_eq!(r.json["risk"]["name"], "稳健型");
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    err(&r, 422, "credit_not_eligible");
    assert_eq!(r.json["message"], "完成身份验证后才能开通");
    let id = a.get("/api/v1/me", Some(&ck)).await.json["id"].as_i64().unwrap();
    rusqlite::Connection::open(&a.st.cfg.game_db)
        .unwrap()
        .execute("INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', 'OPENID2', 'read:account', ?)", rusqlite::params![id, TS])
        .unwrap();
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    assert_eq!(r.json["message"], "风险测评为 C4 及以上才能开通");
    a.req("POST", "/api/v1/me/assessments/risk", Some(json!({"answers": [3, 3, 3, 3, 3, 3, 3, 3]})), &h).await;
    let r = a.req("POST", "/api/v1/me/assessments/risk", Some(json!({"answers": [3, 3, 3, 3, 3, 3, 3, 3]})), &h).await;
    assert_eq!(r.json["risk"]["level"], "C5");
    err(&a.req("POST", "/api/v1/me/assessments/risk", Some(json!({"answers": [3, 3, 3, 3, 3, 3, 3, 3]})), &h).await, 422, "assessment_limit");
    err(&a.req("POST", "/api/v1/me/assessments/margin-quiz", Some(json!({"answers": [0, 0]})), &h).await, 400, "invalid_request");
    let r = a.req("POST", "/api/v1/me/assessments/margin-quiz", Some(json!({"answers": vec![0; 10]})), &h).await;
    assert_eq!(r.json["quiz"]["passed"], false);
    assert_eq!(r.json["quiz"]["review"].as_array().unwrap().len(), 10);
    let right = json!([1, 0, 1, 2, 1, 0, 2, 1, 1, 1]);
    let r = a.req("POST", "/api/v1/me/assessments/margin-quiz", Some(json!({"answers": right})), &h).await;
    assert_eq!(r.json["quiz"]["score"], 10);
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    assert_eq!(r.json["message"], "签署《融资融券风险揭示书》后才能开通");
    a.req("POST", "/api/v1/me/agreements", Some(json!({"doc": "margin_disclosure", "version": 1, "signature": "erin"})), &h).await;
    let me = a.get("/api/v1/me", Some(&ck)).await.json;
    assert_eq!(me["credit_steps"], json!({"verified": true, "risk_ok": true, "min_level": "C4", "quiz_passed": true, "disclosure_signed": true, "disclosure_version": 1}));
    // registered today and verified: no waiting period
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["credit"]["opened"], true);
}

#[tokio::test]
async fn closing_an_account() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let me = a.get("/api/v1/me", Some(&ck)).await.json;
    let id = me["id"].as_i64().unwrap();
    err(&a.req("DELETE", "/api/v1/me", Some(json!({"password": "secret1"})), &h).await, 400, "invalid_request");
    let r = a.req("DELETE", "/api/v1/me", Some(json!({"password": "wrong!!", "confirm": true})), &h).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["message"], "密码不正确");
    let r = a.req("POST", "/api/v1/me/api-keys", Some(json!({"name": "脚本"})), &h).await;
    assert_eq!(r.status, 201, "{}", r.json);
    let key = format!("Bearer {}", r.json["token"].as_str().unwrap());
    let r = a.req("DELETE", "/api/v1/me", Some(json!({"password": "secret1", "confirm": true})), &h).await;
    assert_eq!(r.status.as_u16(), 204, "{}", r.json);
    err(&a.req("GET", "/api/v1/me", None, &[("authorization", key.as_str())]).await, 401, "unauthenticated");
    // the cookie no longer works, the name and password are gone, the books still balance
    err(&a.get("/api/v1/me", Some(&ck)).await, 401, "unauthenticated");
    err(&a.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "secret1"})), &[]).await, 401, "bad_credentials");
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    let (kind, name, user, cash): (String, String, Option<String>, i64) =
        c.query_row("SELECT kind, name, username, cash FROM accounts WHERE id=?", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    assert_eq!((kind.as_str(), user, cash), ("closed", None, 0));
    assert_eq!(name, format!("已注销用户 #{id}"));
    let left: i64 = c.query_row("SELECT COUNT(*) FROM holdings WHERE account_id=?", [id], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
    for t in ["account_agreements", "risk_assessments", "account_logins", "api_keys"] {
        let n: i64 = c.query_row(&format!("SELECT COUNT(*) FROM {t} WHERE account_id=?"), [id], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "{t}");
    }
    a.st.actor.call(|ctx| ctx.engine.check_invariants()).await.unwrap().unwrap();
}

#[tokio::test]
async fn changing_and_resetting_a_password() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    a.login().await;
    // a session from a minute ago (the test clock stands still)
    let id: i64 = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap().query_row("SELECT id FROM accounts WHERE username='alice'", [], |r| r.get(0)).unwrap();
    let old = format!("session={}", anistock::auth::make_session(&a.st.secret, id, a.st.unix_now() - 60.0));
    let h = [("cookie", old.as_str())];
    let change = |cur: &str, new: &str| Some(json!({"current_password": cur, "new_password": new}));
    let r = a.req("POST", "/api/v1/me/password", change("wrong!!", "newpass1"), &h).await;
    err(&r, 400, "invalid_request");
    assert_eq!(r.json["message"], "当前密码不正确");
    err(&a.req("POST", "/api/v1/me/password", change("secret1", "123"), &h).await, 400, "invalid_request");
    let r = a.req("POST", "/api/v1/me/password", change("secret1", "newpass1"), &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let fresh = r.headers.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string();
    // the cookie from before the change no longer works; the new one does
    err(&a.get("/api/v1/me", Some(&old)).await, 401, "unauthenticated");
    assert_eq!(a.get("/api/v1/me", Some(&fresh)).await.status, 200);
    err(&a.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "secret1"})), &[]).await, 401, "bad_credentials");

    // forgotten: the bot asks for a code for the QQ bound to alice
    let bot = [("authorization", &*format!("Bearer {BOT}"))];
    let openid = "E1F2A3B4C5D6E7F8A9B0C1D2E3F4A5B6";
    let r = a.req("POST", "/api/v1/bot/password-resets", Some(json!({"platform": "qq", "external_id": openid})), &bot).await;
    err(&r, 403, "not_bound");
    rusqlite::Connection::open(&a.st.cfg.game_db)
        .unwrap()
        .execute("INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', ?, 'read:account', ?)", rusqlite::params![id, openid, TS])
        .unwrap();
    let r = a.req("POST", "/api/v1/bot/password-resets", Some(json!({"platform": "qq", "external_id": openid})), &bot).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/bot/password-resets", 201), &r.json, "reset code");
    assert_eq!(r.json["account"]["username"], "alice");
    let code = r.json["code"].as_str().unwrap().to_string();
    let reset = |code: &str| Some(json!({"username": "alice", "code": code, "new_password": "third333"}));
    err(&a.req("POST", "/api/v1/auth/password-reset", reset("AAAA-AAAA"), &[]).await, 422, "reset_code_invalid");
    let r = a.req("POST", "/api/v1/auth/password-reset", reset(&code.to_lowercase()), &[]).await;
    assert_eq!(r.status, 200, "{}", r.json);
    // signed in, the code is spent
    err(&a.req("POST", "/api/v1/auth/password-reset", reset(&code), &[]).await, 422, "reset_code_invalid");
    let r = a.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "third333"})), &[]).await;
    assert_eq!(r.status, 200, "{}", r.json);
}

#[tokio::test]
async fn reset_codes_take_five_wrong_tries() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let mut c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    let id: i64 = c.query_row("SELECT id FROM accounts WHERE username='alice'", [], |r| r.get(0)).unwrap();
    let now = a.st.clock.now();
    let (code, _) = anistock::api::v1::password::issue_code(&mut c, id, "operator", &now).unwrap();
    let reset = |code: &str| Some(json!({"username": "alice", "code": code, "new_password": "third333"}));
    for _ in 0..5 {
        err(&a.req("POST", "/api/v1/auth/password-reset", reset("ZZZZZZZZ"), &[]).await, 422, "reset_code_invalid");
    }
    // the right code, too late
    err(&a.req("POST", "/api/v1/auth/password-reset", reset(&code), &[]).await, 422, "reset_code_invalid");
}

#[tokio::test]
async fn static_assets_are_immutable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/app-abc123.js"), "x").unwrap();
    std::fs::write(dir.path().join("index.html"), "<html></html>").unwrap();
    let r = anistock::api::spa::asset(dir.path(), "app-abc123.js", &axum::http::Method::GET, &Default::default()).await;
    assert_eq!(r.headers().get("cache-control").unwrap(), "public, max-age=31536000, immutable");
    let r = anistock::api::spa::catch_all(dir.path(), "/stocks/1").await;
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-cache");
}

#[tokio::test]
async fn auth_bindings_and_routing_errors() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    err(&a.get("/api/v1/me", None).await, 401, "unauthenticated");
    err(&a.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "nope!!"})), &[]).await, 401, "bad_credentials");
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "x", "agree": true})), &[]).await, 422, "bad_invite");
    // registration needs a proof of work; login does not (no surge)
    let r = a.get("/api/v1/auth/challenge?purpose=register", None).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("GET", "/api/v1/auth/challenge", 200), &r.json, "challenge");
    assert_eq!(r.json["registration"], "invite");
    assert_eq!(r.headers.get("cache-control").unwrap(), "no-store");
    let r = a.get("/api/v1/auth/challenge?purpose=login", None).await;
    assert_eq!(r.json["challenge"], Value::Null);
    err(&a.get("/api/v1/auth/challenge?purpose=x", None).await, 400, "invalid_request");
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club", "agree": true})), &[]).await, 422, "pow_required");
    let pow = solved(&a).await;
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "alice", "password": "secret1", "invite": "club", "agree": true, "pow": pow})), &[]).await, 409, "username_taken");
    // that challenge is spent
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club", "agree": true, "pow": pow})), &[]).await, 422, "pow_invalid");
    let mut wrong = solved(&a).await;
    wrong["number"] = json!((wrong["number"].as_u64().unwrap() + 1) % wrong["challenge"]["max"].as_u64().unwrap());
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club", "agree": true, "pow": wrong})), &[]).await, 422, "pow_invalid");
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club", "agree": true, "pow": 3})), &[]).await, 400, "invalid_request");
    let r = a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club", "agree": true, "pow": solved(&a).await})), &[]).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/auth/register", 201), &r.json, "register");
    err(&a.get("/api/v1/nope", None).await, 404, "not_found");
    err(&a.get("/api/status", None).await, 404, "not_found");
    err(&a.req("PUT", "/api/v1/me/orders", None, &[]).await, 405, "method_not_allowed");
    err(&a.get("/api/v1/stocks/77", None).await, 404, "stock_not_found");
    err(&a.get("/api/v1/stocks/x", None).await, 400, "invalid_request");
    err(&a.get("/api/v1/ipos/1", None).await, 404, "ipo_not_found");
    err(&a.get("/api/v1/news/999", None).await, 404, "news_not_found");
    err(&a.get("/api/v1/stocks/1/cover", None).await, 404, "cover_not_found");
    // the static path (`cover_url`): the same answer, outside the API; anything but `<digits>.jpg` is not a cover
    err(&a.get("/covers/1.jpg", None).await, 404, "cover_not_found");
    assert_eq!(a.get("/covers/1.png", None).await.status, 404);
    assert_eq!(a.get("/covers/../game.db", None).await.status, 404);

    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let r = a.req("POST", "/api/v1/me/bind-codes", Some(json!({})), &h).await;
    assert_eq!(r.status, 201, "{}", r.json);
    let code = r.json["code"].as_str().unwrap().to_string();
    let bot = format!("Bearer {BOT}");
    let bh = [("authorization", bot.as_str())];
    err(&a.req("POST", "/api/v1/bot/bindings", Some(json!({"code": "ZZZZ-ZZZZ", "platform": "qq", "external_id": "10001"})), &bh).await, 422, "bind_code_invalid");
    let r = a.req("POST", "/api/v1/bot/bindings", Some(json!({"code": code, "platform": "qq", "external_id": "10001"})), &bh).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/bot/bindings", 201), &r.json, "redeem");
    err(&a.req("POST", "/api/v1/me/bind-codes", Some(json!({})), &h).await, 409, "already_bound");
    let act = [("authorization", bot.as_str()), ("x-act-as", "qq:10001")];
    let r = a.req("GET", "/api/v1/me", None, &act).await;
    assert_eq!(r.json["username"], "alice");
    err(&a.req("GET", "/api/v1/me", None, &[("authorization", bot.as_str()), ("x-act-as", "qq:2")]).await, 403, "not_bound");
    err(&a.req("POST", "/api/v1/me/orders", Some(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 1, "quantity": 1})), &act).await, 403, "forbidden");
    err(&a.req("GET", "/api/v1/me", None, &[("authorization", "Bearer wrong")]).await, 401, "unauthenticated");
    let r = a.req("DELETE", "/api/v1/bot/bindings/qq/10001", None, &bh).await;
    assert_eq!(r.status, 204);
    err(&a.req("DELETE", "/api/v1/bot/bindings/qq/10001", None, &bh).await, 404, "binding_not_found");
}

#[tokio::test]
async fn closed_market_and_rate_limits() {
    let a = app("2026-11-06T05:00:00+08:00", false);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let r = a.req("POST", "/api/v1/me/orders", Some(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 1})), &h).await;
    err(&r, 422, "market_closed");
    assert_eq!(r.json["details"]["next_open_at"], "2026-11-06T08:45:00+08:00");
    let s = a.checked("/api/v1/market/status", "/api/v1/market/status", None).await;
    assert_eq!(s["phase"], "closed");
    assert_eq!(s["trading_day"], "2026-11-06");
    assert_eq!(s["phase_ends_at"], "2026-11-06T08:45:00+08:00");

    let b = app("2026-11-05T08:50:00+08:00", false);
    let ck = b.login().await;
    let r = b
        .req("POST", "/api/v1/me/orders", Some(json!({"stock_id": 1, "side": "buy", "type": "market", "quantity": 1})), &[("cookie", ck.as_str())])
        .await;
    err(&r, 422, "auction_limit_only");

    // the anonymous bucket: 20 at once, then 429 with Retry-After (refilling slowly enough that a slow CI machine
    // cannot earn a token back during the 21 requests)
    let c = app("2026-11-05T10:00:00+08:00", true);
    let mut limits = c.st.v1.limits.clone();
    limits.anon = (0.01, 20.0);
    let st = Arc::new(AppState {
        actor: c.st.actor.clone(),
        cfg: c.st.cfg.clone(),
        clock: c.st.clock.clone(),
        secret: c.st.secret.clone(),
        reads: ReadPool::new(c.st.cfg.game_db.clone(), 2),
        dist: None,
        live: None,
        v1: V1State { limits, ..V1State::new(&c.st.cfg) },
    });
    let limited = App { _dir: tempfile::tempdir().unwrap(), st, doc: c.doc.clone() };
    let mut last = None;
    for _ in 0..21 {
        last = Some(limited.get("/api/v1/rules", None).await);
    }
    let r = last.unwrap();
    err(&r, 429, "rate_limited");
    assert!(r.headers.get("retry-after").is_some());
}

#[tokio::test]
async fn credit_account_and_credit_orders() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let c = Some(ck.as_str());
    let cr = a.checked("/api/v1/me/credit", "/api/v1/me/credit", c).await;
    assert_eq!(cr["credit"]["opened"], false);
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    err(&r, 422, "credit_not_eligible");
    assert_eq!(r.json["message"], "完成身份验证后才能开通");
    let id = a.get("/api/v1/me", c).await.json["id"].as_i64().unwrap();
    rusqlite::Connection::open(&a.st.cfg.game_db)
        .unwrap()
        .execute("INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', 'OPENID3', 'read:account', ?)", rusqlite::params![id, TS])
        .unwrap();
    let r = a.req("POST", "/api/v1/me/credit", None, &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/credit", 200), &r.json, "open");
    assert_eq!(r.json["credit"]["status"], "normal");
    err(&a.req("POST", "/api/v1/me/credit", None, &h).await, 409, "credit_open");
    let sm = a.checked("/api/v1/stocks/{id}/margin", "/api/v1/stocks/1/margin", None).await;
    assert_eq!(sm["eligible"], true);

    let place = |b: Value| a.req("POST", "/api/v1/me/orders", Some(b), &h);
    let r = place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 10, "credit": "margin_buy"})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/orders", 201), &r.json, "margin buy");
    assert_eq!(r.json["order"]["credit"], "margin_buy");
    let id = r.json["order"]["id"].as_i64().unwrap();
    let cr = a.checked("/api/v1/me/credit", "/api/v1/me/credit", c).await;
    assert_eq!(cr["credit"]["debt"], 5_012, "drawn for the resting order");
    let me = a.checked("/api/v1/me", "/api/v1/me", c).await;
    assert_eq!(me["liabilities"], 5_012);
    assert_eq!(me["credit_status"], "normal");
    // cancelling repays the loan
    a.req("DELETE", &format!("/api/v1/me/orders/{id}"), None, &h).await;
    let cr = a.checked("/api/v1/me/credit", "/api/v1/me/credit", c).await;
    assert_eq!(cr["credit"]["debt"], 0);
    err(&a.req("POST", "/api/v1/me/credit/repay", None, &h).await, 422, "no_debt");
    // alice holds the stock: no short sale of it
    err(&place(json!({"stock_id": 1, "side": "sell", "type": "limit", "price": 500, "quantity": 1, "credit": "short_sell"})).await, 422, "holds_long");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "market", "quantity": 1, "credit": "margin_buy"})).await, 422, "credit_limit_only");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 1, "credit": "short_sell"})).await, 400, "invalid_request");
    err(&place(json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 500, "quantity": 1, "credit": "buy_return"})).await, 422, "no_short");
    let orders = a.checked("/api/v1/me/orders", "/api/v1/me/orders?status=all", c).await;
    assert_eq!(orders["items"][0]["credit"], "margin_buy");
    a.checked("/api/v1/me/ledger", "/api/v1/me/ledger", c).await;
}

#[tokio::test]
async fn announcements_pop_up_once_for_existing_accounts() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    // posted before alice opened her account (TS): not for her
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    c.execute("INSERT INTO announcements(created_at, title, body, popup) VALUES ('2026-11-05T09:00:00+08:00', '更早', '零', 1)", []).unwrap();
    let r = a.req("GET", "/api/v1/me", None, &h).await;
    assert_eq!(r.json["announcement"], Value::Null);
    // posted after it, as `anistock announce` writes it
    {
        c.execute("INSERT INTO announcements(created_at, title, body, popup) VALUES ('2026-11-05T11:00:00+08:00', '旧的', '一', 1)", []).unwrap();
        c.execute("INSERT INTO announcements(created_at, title, body, popup) VALUES ('2026-11-05T12:00:00+08:00', '不弹', '二', 0)", []).unwrap();
        c.execute("INSERT INTO announcements(created_at, title, body, popup) VALUES ('2026-11-05T13:00:00+08:00', '重新开服', '三\n\n四', 1)", []).unwrap();
    }
    let r = a.req("GET", "/api/v1/me", None, &h).await;
    a.validate(&a.schema("GET", "/api/v1/me", 200), &r.json, "me");
    assert_eq!(r.json["announcement"]["title"], "重新开服");
    assert_eq!(r.json["announcement"]["body"], "三\n\n四");
    let id = r.json["announcement"]["id"].as_i64().unwrap();
    let r = a.req("POST", &format!("/api/v1/me/announcements/{id}/read"), None, &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/announcements/{id}/read", 200), &r.json, "read");
    // the older unread popup comes next; reading it twice is fine
    assert_eq!(r.json["announcement"]["title"], "旧的");
    let old = r.json["announcement"]["id"].as_i64().unwrap();
    a.req("POST", &format!("/api/v1/me/announcements/{old}/read"), None, &h).await;
    let r = a.req("POST", &format!("/api/v1/me/announcements/{old}/read"), None, &h).await;
    assert_eq!(r.json["announcement"], Value::Null);
    err(&a.req("POST", "/api/v1/me/announcements/999/read", None, &h).await, 404, "not_found");
    err(&a.req("POST", &format!("/api/v1/me/announcements/{id}/read"), None, &[]).await, 401, "unauthenticated");
}

#[tokio::test]
async fn api_keys_read_but_do_not_write() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let (a, h) = (&a, &h);
    let make = |body: Value| async move { a.req("POST", "/api/v1/me/api-keys", Some(body), h).await };
    err(&make(json!({})).await, 400, "invalid_request");
    err(&make(json!({"name": "x".repeat(21)})).await, 400, "invalid_request");
    err(&make(json!({"name": "行情", "scopes": ["trade"]})).await, 400, "invalid_request");
    err(&make(json!({"name": "行情", "expires_in_days": 7})).await, 400, "invalid_request");
    let r = make(json!({"name": "行情", "expires_in_days": 30})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/api-keys", 201), &r.json, "api key");
    let token = r.json["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("anx_") && token.len() == 44, "{token}");
    assert_eq!(r.json["key"]["prefix"].as_str().unwrap(), &token[..8]);
    assert_eq!(r.json["key"]["scopes"], json!(["read:account"]));
    let id = r.json["key"]["id"].as_i64().unwrap();
    let bearer = format!("Bearer {token}");
    let k = [("authorization", bearer.as_str())];

    // reads the account like the browser
    let r = a.req("GET", "/api/v1/me", None, &k).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["username"], "alice");
    assert_eq!(a.req("GET", "/api/v1/me/portfolio", None, &k).await.status, 200);
    assert_eq!(a.req("GET", "/api/v1/me/orders", None, &k).await.status, 200);
    // orders need the `trade` scope; web-only endpoints stay with the browser
    let order = json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 100, "quantity": 1});
    let r = a.req("POST", "/api/v1/me/orders", Some(order), &k).await;
    err(&r, 403, "forbidden");
    assert_eq!(r.json["details"]["required_scope"], "trade");
    let r = a.req("POST", "/api/v1/me/restructure", Some(json!({})), &k).await;
    err(&r, 403, "forbidden");
    assert_eq!(r.json["details"]["required_scope"], "web");
    err(&a.req("POST", "/api/v1/me/api-keys", Some(json!({"name": "再来"})), &k).await, 403, "forbidden");
    err(&a.req("GET", "/api/v1/me/api-keys", None, &k).await, 403, "forbidden");
    err(&a.req("DELETE", &format!("/api/v1/me/api-keys/{id}"), None, &k).await, 403, "forbidden");
    err(&a.req("GET", "/api/v1/me", None, &[("authorization", "Bearer anx_nope")]).await, 401, "unauthenticated");
    // with `[api] trading` off even a key carrying `trade` cannot trade
    {
        let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
        let tok = "anx_TradeKeyWhileTheSwitchIsOff0000000000";
        c.execute(
            "INSERT INTO api_keys(account_id, name, prefix, key_hash, scopes, created_at, expires_at) SELECT id, 't', 'anx_Trad', ?, 'read:account,trade', ?, '2027-01-01T00:00:00+08:00' FROM accounts WHERE username='alice'",
            rusqlite::params![v1::keys::hash(tok), TS],
        )
        .unwrap();
        let b = format!("Bearer {tok}");
        let order = json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 100, "quantity": 1});
        let r = a.req("POST", "/api/v1/me/orders", Some(order), &[("authorization", b.as_str())]).await;
        err(&r, 403, "forbidden");
        assert_eq!(r.json["details"]["required"], json!(["api_trading"]));
        c.execute("DELETE FROM api_keys WHERE name='t'", []).unwrap();
    }

    // five in force at most; the list never shows the token
    for i in 0..4 {
        assert_eq!(make(json!({"name": format!("k{i}")})).await.status, 201);
    }
    let r = make(json!({"name": "第六个"})).await;
    err(&r, 409, "too_many_api_keys");
    let r = a.get("/api/v1/me/api-keys", Some(&ck)).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("GET", "/api/v1/me/api-keys", 200), &r.json, "api keys");
    assert_eq!(r.json["items"].as_array().unwrap().len(), 5);
    assert!(!r.json.to_string().contains(&token));
    // the first use is recorded in the background
    let mut used = Value::Null;
    for _ in 0..50 {
        let r = a.get("/api/v1/me/api-keys", Some(&ck)).await;
        used = r.json["items"].as_array().unwrap().iter().find(|x| x["id"] == id).unwrap()["last_used_at"].clone();
        if used.is_string() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(used.is_string(), "{used}");

    // an expired key stops working and frees its place
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    c.execute("UPDATE api_keys SET expires_at='2026-11-01T00:00:00+08:00' WHERE id=?", [id]).unwrap();
    err(&a.req("GET", "/api/v1/me", None, &k).await, 401, "unauthenticated");
    let r = make(json!({"name": "第六个"})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    let token6 = format!("Bearer {}", r.json["token"].as_str().unwrap());
    let id6 = r.json["key"]["id"].as_i64().unwrap();
    assert_eq!(a.req("GET", "/api/v1/me", None, &[("authorization", token6.as_str())]).await.status, 200);

    // revoked: gone at once
    let r = a.req("DELETE", &format!("/api/v1/me/api-keys/{id6}"), None, h).await;
    assert_eq!(r.status, 204);
    err(&a.req("GET", "/api/v1/me", None, &[("authorization", token6.as_str())]).await, 401, "unauthenticated");
    err(&a.req("DELETE", &format!("/api/v1/me/api-keys/{id6}"), None, h).await, 404, "api_key_not_found");

    // a password reset revokes the rest
    let openid = "E1F2A3B4C5D6E7F8A9B0C1D2E3F4A5B6";
    let acct: i64 = c.query_row("SELECT id FROM accounts WHERE username='alice'", [], |r| r.get(0)).unwrap();
    c.execute("INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', ?, 'read:account', ?)", rusqlite::params![acct, openid, TS]).unwrap();
    let bot = [("authorization", &*format!("Bearer {BOT}"))];
    let r = a.req("POST", "/api/v1/bot/password-resets", Some(json!({"platform": "qq", "external_id": openid})), &bot).await;
    let code = r.json["code"].as_str().unwrap().to_string();
    let r = a.req("POST", "/api/v1/auth/password-reset", Some(json!({"username": "alice", "code": code, "new_password": "third333"})), &[]).await;
    assert_eq!(r.status, 200, "{}", r.json);
    let live: i64 = c.query_row("SELECT COUNT(*) FROM api_keys WHERE account_id=? AND revoked_at IS NULL", [acct], |r| r.get(0)).unwrap();
    assert_eq!(live, 0);
}

#[tokio::test]
async fn media_news_is_archived_and_published_by_mode() {
    let push = |id: &str| {
        json!({"event_id": id, "source": "ann", "url": "https://www.animenewsnetwork.com/news/x",
               "title": "公布续作", "original_title": "Test Anime Gets 2nd Season",
               "published_at": "2026-11-05T09:30:00+09:00", "fetched_at": "2026-11-05T08:35:00+08:00",
               "stock_id": 1, "event": "sequel", "direction": "positive", "magnitude": 6.5, "credibility": "official",
               "model": "jev-1.14.0", "probabilities": {"works": {"1": 0.97, "none": 0.03}}})
    };
    let bot = [("authorization", &*format!("Bearer {BOT}"))];
    // shadow (the default): archived, not published
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    err(&a.req("POST", "/api/v1/bot/news", Some(push("e1")), &[("cookie", ck.as_str())]).await, 403, "forbidden");
    let r = a.req("POST", "/api/v1/bot/news", Some(push("e1")), &bot).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/bot/news", 200), &r.json, "media news");
    assert_eq!(r.json["status"], "stored");
    assert_eq!(r.json["mode"], "shadow");
    assert!(r.json["news_id"].is_null());
    let mut bad = push("e2");
    bad["event"] = json!("explosion");
    err(&a.req("POST", "/api/v1/bot/news", Some(bad), &bot).await, 400, "invalid_request");
    let mut bad = push("e2");
    bad["magnitude"] = json!(11);
    err(&a.req("POST", "/api/v1/bot/news", Some(bad), &bot).await, 400, "invalid_request");
    let mut bad = push("e2");
    bad["stock_id"] = json!(999999);
    err(&a.req("POST", "/api/v1/bot/news", Some(bad), &bot).await, 404, "stock_not_found");
    let r = a.get("/api/v1/news?category=media", None).await;
    assert_eq!(r.json["items"].as_array().unwrap().len(), 0, "{}", r.json);
    // switching to publish publishes what shadow archived, once (at startup), with the day the source ran it
    let db = a.st.cfg.game_db.clone();
    let n = a.st.actor.call(move |ctx| anistock::api::v1::media::publish_backlog(&mut ctx.engine, &db, "2026-11-07", "2026-11-07T09:00:00+08:00")).await.unwrap().unwrap();
    assert_eq!(n, 1);
    let r = a.get("/api/v1/news?category=media", None).await;
    let items = r.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{}", r.json);
    assert!(items[0]["body"].as_str().unwrap().contains("11月5日报道"), "{}", items[0]);
    let db = a.st.cfg.game_db.clone();
    let n = a.st.actor.call(move |ctx| anistock::api::v1::media::publish_backlog(&mut ctx.engine, &db, "2026-11-07", "2026-11-07T09:00:00+08:00")).await.unwrap().unwrap();
    assert_eq!(n, 0);

    // publish: a game news item once; a refined judgement updates the archive, not the news
    let a = app_env("2026-11-05T10:00:00+08:00", true, &[("ANISTOCK_NEWS_MODE", "publish")]);
    let r = a.req("POST", "/api/v1/bot/news", Some(push("e1")), &bot).await;
    assert_eq!(r.json["status"], "published", "{}", r.json);
    let nid = r.json["news_id"].as_i64().unwrap();
    let mut again = push("e1");
    again["magnitude"] = json!(4.0);
    let r = a.req("POST", "/api/v1/bot/news", Some(again), &bot).await;
    assert_eq!(r.json["news_id"].as_i64(), Some(nid), "{}", r.json);
    let r = a.get("/api/v1/news?category=media", None).await;
    let items = r.json["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{}", r.json);
    assert_eq!(items[0]["kind"], "media");
    assert_eq!(items[0]["category"], "media");
    assert!(items[0]["title"].as_str().unwrap().contains("公布续作"));
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    let m: f64 = c.query_row("SELECT magnitude FROM media_news WHERE event_id='e1'", [], |r| r.get(0)).unwrap();
    assert_eq!(m, 4.0);
    // merged into another event
    let r = a.req("POST", "/api/v1/bot/news", Some(json!({"event_id": "e1", "merged_into": "e9"})), &bot).await;
    assert_eq!(r.json["status"], "merged", "{}", r.json);
    err(&a.req("POST", "/api/v1/bot/news", Some(json!({"event_id": "nope", "merged_into": "e9"})), &bot).await, 404, "media_news_not_found");
}

#[tokio::test]
async fn trade_keys_need_verification_and_the_disclosure() {
    let mut a = app_env("2026-11-05T10:00:00+08:00", true, &[("ANISTOCK_API_TRADING", "1")]);
    {
        let l = &mut Arc::get_mut(&mut a.st).unwrap().v1.limits;
        l.max_open_orders_per_stock = 2;
        l.orders_per_day = 3;
    }
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let (a, h) = (&a, &h);
    let make = |body: Value| async move { a.req("POST", "/api/v1/me/api-keys", Some(body), h).await };
    let c = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    let acct: i64 = c.query_row("SELECT id FROM accounts WHERE username='alice'", [], |r| r.get(0)).unwrap();

    // not verified, not signed
    let me = a.get("/api/v1/me", Some(&ck)).await.json;
    assert_eq!(me["api_trading"], json!({"enabled": true, "verified": false, "disclosure_signed": false, "disclosure_version": 1}));
    let r = make(json!({"name": "交易", "scopes": ["trade"]})).await;
    err(&r, 403, "forbidden");
    assert_eq!(r.json["details"]["required"], json!(["verification", "api_disclosure"]));
    c.execute("INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?, 'qq', 'OPENIDT', 'read:account', ?)", rusqlite::params![acct, TS]).unwrap();
    err(&make(json!({"name": "交易", "scopes": ["trade"]})).await, 403, "forbidden");
    // the disclosure is signed with the username, like the other two
    let r = a.req("POST", "/api/v1/me/agreements", Some(json!({"doc": "api_disclosure", "version": 1, "signature": "bob"})), h).await;
    err(&r, 400, "invalid_request");
    let r = a.req("POST", "/api/v1/me/agreements", Some(json!({"doc": "api_disclosure", "version": 1, "signature": "alice"})), h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    assert_eq!(r.json["api_trading"]["disclosure_signed"], true);
    assert_eq!(r.json["pending_documents"], json!([]));

    let r = make(json!({"name": "交易", "scopes": ["trade"]})).await;
    assert_eq!(r.status, 201, "{}", r.json);
    assert_eq!(r.json["key"]["scopes"], json!(["read:account", "trade"]));
    let key_id = r.json["key"]["id"].as_i64().unwrap();
    let bearer = format!("Bearer {}", r.json["token"].as_str().unwrap());
    let k = [("authorization", bearer.as_str())];
    let r = make(json!({"name": "只读"})).await;
    let ro = format!("Bearer {}", r.json["token"].as_str().unwrap());

    // the trade key places and cancels; the read-only one still cannot
    let order = json!({"stock_id": 1, "side": "buy", "type": "limit", "price": 100, "quantity": 1});
    let r = a.req("POST", "/api/v1/me/orders", Some(order.clone()), &[("authorization", ro.as_str())]).await;
    err(&r, 403, "forbidden");
    assert_eq!(r.json["details"]["required_scope"], "trade");
    let r = a.req("POST", "/api/v1/me/orders", Some(order.clone()), &k).await;
    assert_eq!(r.status, 201, "{}", r.json);
    let first = r.json["order"]["id"].as_i64().unwrap();
    let marked: i64 = c.query_row("SELECT key_id FROM api_orders WHERE order_id=?", [first], |r| r.get(0)).unwrap();
    assert_eq!(marked, key_id);
    // a browser order is not marked
    let r = a.req("POST", "/api/v1/me/orders", Some(order.clone()), h).await;
    assert_eq!(r.status, 201, "{}", r.json);
    let web = r.json["order"]["id"].as_i64().unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM api_orders WHERE order_id=?", [web], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // two open orders in one stock at most (web and keys together)
    let r = a.req("POST", "/api/v1/me/orders", Some(order.clone()), &k).await;
    err(&r, 409, "too_many_open_orders");
    assert_eq!(r.json["details"], json!({"scope": "stock", "max": 2}));
    assert_eq!(a.req("DELETE", &format!("/api/v1/me/orders/{first}"), None, &k).await.status, 200);
    // three orders a day: the third goes in, the fourth does not
    assert_eq!(a.req("POST", "/api/v1/me/orders", Some(order.clone()), &k).await.status, 201);
    a.req("DELETE", &format!("/api/v1/me/orders/{web}"), None, h).await;
    let r = a.req("POST", "/api/v1/me/orders", Some(order.clone()), &k).await;
    err(&r, 429, "daily_order_limit");

    // the leaderboard row carries the 程序化 mark
    let rows = v1::account::ranked(&c, 10_000_000, None, None).unwrap();
    assert!(rows.iter().find(|r| r.user.id == acct).unwrap().program);
    assert!(rows.iter().filter(|r| r.user.id != acct).all(|r| !r.program));

    // losing 身份验证 stops the key at once; the browser keeps trading
    c.execute("DELETE FROM bindings WHERE account_id=?", [acct]).unwrap();
    let r = a.req("DELETE", &format!("/api/v1/me/orders/{web}"), None, &k).await;
    err(&r, 403, "forbidden");
    assert_eq!(r.json["details"]["required"], json!(["verification"]));
}

#[tokio::test]
async fn key_connections_follow_the_key_not_the_session() {
    use anistock::live::{Ctl, Fanout};
    let mut fx = Fx::new();
    fx.seed_rest();
    let db = fx.dir.path().join("game.db");
    let coll = fx.dir.path().join("collector.db");
    let cfg = Config::load_from(&root(), move |k| match k {
        "ANISTOCK_GAME_DB" => Some(db.display().to_string()),
        "ANISTOCK_DB" => Some(coll.display().to_string()),
        _ => None,
    })
    .unwrap();
    let cfg = Arc::new(cfg);
    let clock = Clock::fixed(cfg.tz, "2026-11-05T10:00:00+08:00");
    let (_ctx, crx) = crossbeam_channel::unbounded();
    let (_ltx, lrx) = crossbeam_channel::unbounded();
    let mut f = Fanout::new(clock, cfg, crx, lrx, Default::default());
    let mut add = |id: u64, key: Option<i64>| {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let (kill, killed) = tokio::sync::oneshot::channel::<u16>();
        f.control(Ctl::Add { id, account: Some(5), bot: false, key, exp: None, tx, kill }, &fx.e);
        (killed, _rx)
    };
    let (mut browser, _b) = add(1, None);
    let (mut script, _s) = add(2, Some(7));
    let (mut other, _o) = add(3, Some(8));
    // logging out of the browser leaves the scripts alone
    f.control(Ctl::Logout { account: 5 }, &fx.e);
    assert_eq!(browser.try_recv(), Ok(4001));
    assert!(script.try_recv().is_err());
    // revoking one key closes only its connections
    f.control(Ctl::Revoke { key: 7 }, &fx.e);
    assert_eq!(script.try_recv(), Ok(4001));
    assert!(other.try_recv().is_err());
}

#[tokio::test]
async fn watchlist_and_onboarding_marks() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    let ck = a.login().await;
    let h = [("cookie", ck.as_str())];
    let c = Some(ck.as_str());
    // two more companies, copies of the fixture's one (the watchlist only checks that the stock exists)
    let db = rusqlite::Connection::open(&a.st.cfg.game_db).unwrap();
    db.execute_batch(
        "CREATE TEMP TABLE c AS SELECT * FROM companies WHERE id=1;
         UPDATE c SET id=2, code=code || 'b'; INSERT INTO companies SELECT * FROM c;
         UPDATE c SET id=3, code=code || 'c'; INSERT INTO companies SELECT * FROM c;",
    )
    .unwrap();
    let me = a.checked("/api/v1/me", "/api/v1/me", c).await;
    assert_eq!(me["onboarding"]["watched"], false);
    assert_eq!(me["onboarding"]["traded"], false);
    assert_eq!(me["onboarding"]["marks"], json!([]));
    let w = a.checked("/api/v1/me/watchlist", "/api/v1/me/watchlist", c).await;
    assert_eq!(w["items"], json!([]));

    // add (twice is the same), newest first
    for _ in 0..2 {
        let r = a.req("PUT", "/api/v1/me/watchlist/1", None, &h).await;
        assert_eq!(r.status, 200, "{}", r.json);
        a.validate(&a.schema("PUT", "/api/v1/me/watchlist/{id}", 200), &r.json, "watch");
    }
    let r = a.req("PUT", "/api/v1/me/watchlist/2", None, &h).await;
    assert_eq!(r.json["items"], json!([2, 1]), "{}", r.json);
    err(&a.req("PUT", "/api/v1/me/watchlist/999999", None, &h).await, 404, "stock_not_found");
    err(&a.req("PUT", "/api/v1/me/watchlist/1", None, &[]).await, 401, "unauthenticated");

    // removing keeps the 加入过自选 step done
    let r = a.req("DELETE", "/api/v1/me/watchlist/2", None, &h).await;
    assert_eq!(r.json["items"], json!([1]));
    a.req("DELETE", "/api/v1/me/watchlist/1", None, &h).await;
    let me = a.checked("/api/v1/me", "/api/v1/me", c).await;
    assert_eq!(me["onboarding"]["watched"], true);

    // import a browser's list (newest first): unknown ids dropped, duplicates once, order kept, on top
    a.req("PUT", "/api/v1/me/watchlist/1", None, &h).await;
    let r = a.req("POST", "/api/v1/me/watchlist/import", Some(json!({"items": [3, 999999, 2, 1, 3]})), &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/watchlist/import", 200), &r.json, "import");
    assert_eq!(r.json["items"], json!([3, 2, 1]));
    err(&a.req("POST", "/api/v1/me/watchlist/import", Some(json!({"items": "1"})), &h).await, 400, "invalid_request");

    // marks
    let r = a.req("POST", "/api/v1/me/marks/guide.stock", None, &h).await;
    assert_eq!(r.status, 200, "{}", r.json);
    a.validate(&a.schema("POST", "/api/v1/me/marks/{key}", 200), &r.json, "mark");
    let marks = r.json["onboarding"]["marks"].as_array().unwrap().clone();
    assert!(marks.contains(&json!("guide.stock")) && marks.contains(&json!("watch")), "{marks:?}");
    err(&a.req("POST", "/api/v1/me/marks/watch", None, &h).await, 400, "invalid_request");
}
