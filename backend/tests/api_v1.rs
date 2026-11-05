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
        _ => None,
    })
    .unwrap();
    cfg.always_open = always_open;
    let cfg = Arc::new(cfg);
    let clock = Clock::fixed(cfg.tz, at).with_rollover(&cfg.game.day_rollover);
    let Fx { dir, e } = fx;
    let (actor, _h) = Actor::start(Ctx { engine: e, cfg: cfg.clone(), ops: Box::new(EngineOps::default()) });
    let mut v1s = V1State::new(&cfg);
    v1s.bots = v1::parse_bot_tokens(&format!("koishi:{BOT}"));
    v1s.limits.anon = (1e9, 1e9);
    v1s.limits.player = (1e9, 1e9);
    v1s.limits.auth = (1e9, 1e9);
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
        assert_eq!(parts.headers.get("aniex-version").unwrap(), v1::VERSION, "{path}");
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
async fn auth_bindings_and_routing_errors() {
    let a = app("2026-11-05T10:00:00+08:00", true);
    err(&a.get("/api/v1/me", None).await, 401, "unauthenticated");
    err(&a.req("POST", "/api/v1/auth/login", Some(json!({"username": "alice", "password": "nope!!"})), &[]).await, 401, "bad_credentials");
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "x"})), &[]).await, 422, "bad_invite");
    err(&a.req("POST", "/api/v1/auth/register", Some(json!({"username": "alice", "password": "secret1", "invite": "club"})), &[]).await, 409, "username_taken");
    let r = a.req("POST", "/api/v1/auth/register", Some(json!({"username": "bob", "password": "secret1", "invite": "club"})), &[]).await;
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

    // the anonymous bucket: 20 at once, then 429 with Retry-After
    let c = app("2026-11-05T10:00:00+08:00", true);
    let mut limits = c.st.v1.limits.clone();
    limits.anon = (10.0, 20.0);
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
