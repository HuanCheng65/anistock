//! Read-only endpoints added for the redesigned frontend. They sit outside v1's API (and the difftest):
//! nothing here writes the game database, and none of v1's responses change.
//!
//! - `GET /api/v2/indices`               market indices and advance/decline counts
//! - `GET /api/v2/tonight`               episodes airing in today's broadcast day
//! - `GET /api/v2/stocks/{id}/profile`   episode air days, dividends, ratios, holders by kind
//! - `GET /api/v2/listings`              recent listings (IPO price, first-day close, allotment ratio)
//! - `GET /api/covers/{id}`              the work's Bangumi cover, fetched once and cached on disk
//!
//! ## Indices
//! Cap-weighted (close × shares), chain-linked day by day so listings joining later do not jump the
//! level: the return of day t uses only companies with a close on both t−1 and t. Each index starts at
//! 1000 on its first day with constituents (the first candle day for 综合). Today's point, when today
//! has no candle yet, is `last_price` against the last close.
//! - `all`    综合: every listed company.
//! - `season` 本季新番: not a long runner, first aired on or after the start of the current calendar
//!   quarter (Jan / Apr / Jul / Oct 1 in the game time zone), including works listed before they air.
//! - `long`   长篇连载: `long_runner = 1`.
//! Membership is today's classification applied to the whole history.
//!
//! ## Ratios (approximations from public data only)
//! - 市盈率 = price / EPS, EPS = revenue of the trailing 365 days × (1 − operating_cost_ratio) ×
//!   (1 − corp_tax) / shares — the company's own cost and tax rules applied to its published revenue.
//!   A season's earnings arrive within a few months, so seasonal works show low ratios.
//! - 股息率 = dividends per share paid in the trailing 365 days / price (null until one is paid).
//! - 市净率 = price / (company cash / shares), the same book value v1 shows as `book_value_ps`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderValue, Method, Response, StatusCode};
use chrono::{Datelike, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

use super::{AppState, ApiError, Req};

/// Handles the paths above; `None` lets the regular router continue.
pub async fn route(st: &Arc<AppState>, r: &Req) -> Option<Response<Body>> {
    if r.method != Method::GET {
        return None;
    }
    let path = r.path.as_str();
    if let Some(id) = path.strip_prefix("/api/covers/") {
        return Some(cover(st, id).await);
    }
    let rest = path.strip_prefix("/api/v2/")?;
    let out = match rest {
        "indices" => indices(st).await,
        "tonight" => tonight(st).await,
        "listings" => listings(st).await,
        _ => match rest.strip_prefix("stocks/").and_then(|s| s.strip_suffix("/profile")) {
            Some(id) => match id.parse::<i64>() {
                Ok(id) => profile(st, id).await,
                Err(_) => Err(super::http(404, "没有这支股票")),
            },
            None => Err(super::http(404, "没有这个接口")),
        },
    };
    Some(match out {
        Ok(v) => json_reply(&v, "no-store"),
        Err(super::ApiError::Http(s, m)) => {
            let mut r = json_reply(&json!({ "detail": m }), "no-store");
            *r.status_mut() = StatusCode::from_u16(s).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            r
        }
        Err(e) => {
            eprintln!("v2: {e:?}");
            let mut r = Response::new(Body::from("Internal Server Error"));
            *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            r
        }
    })
}

fn json_reply(v: &Value, cache: &'static str) -> Response<Body> {
    let mut r = Response::new(Body::from(v.to_string()));
    r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
    r.headers_mut().insert("cache-control", HeaderValue::from_static(cache));
    r
}

fn sql(e: rusqlite::Error) -> ApiError {
    super::sql(e)
}

fn cents(v: i64) -> f64 {
    v as f64 / 100.0
}

fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn param_f64(cfg: &crate::config::Config, path: &str, default: f64) -> f64 {
    match cfg.param(path) {
        Some(toml::Value::Float(f)) => *f,
        Some(toml::Value::Integer(i)) => *i as f64,
        _ => default,
    }
}

fn quarter_start(today: &str) -> String {
    let d = NaiveDate::parse_from_str(today, "%Y-%m-%d").unwrap_or_else(|_| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    let m = (d.month() - 1) / 3 * 3 + 1;
    format!("{:04}-{:02}-01", d.year(), m)
}

// ---------------------------------------------------------------- indices

struct Co {
    id: i64,
    shares: i64,
    last: i64,
    prev: i64,
    long: bool,
    first_air: String,
}

fn listed(c: &Connection) -> Result<Vec<Co>, ApiError> {
    let mut s = c
        .prepare("SELECT id, shares, last_price, prev_close, long_runner, first_air FROM companies WHERE status='listed'")
        .map_err(sql)?;
    let rows = s
        .query_map([], |r| {
            Ok(Co {
                id: r.get(0)?,
                shares: r.get(1)?,
                last: r.get(2)?,
                prev: r.get(3)?,
                long: r.get::<_, i64>(4)? != 0,
                first_air: r.get(5)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

async fn indices(st: &Arc<AppState>) -> Result<Value, ApiError> {
    let st2 = st.clone();
    st.read(move |c| {
        let today = st2.clock.today();
        let qs = quarter_start(&today);
        let cos = listed(c)?;
        let by_id: HashMap<i64, &Co> = cos.iter().map(|co| (co.id, co)).collect();
        // day -> company -> close
        let mut days: BTreeMap<String, HashMap<i64, i64>> = BTreeMap::new();
        {
            let mut s = c.prepare("SELECT company_id, day, close FROM candles ORDER BY day").map_err(sql)?;
            let mut rows = s.query([]).map_err(sql)?;
            while let Some(r) = rows.next().map_err(sql)? {
                let id: i64 = r.get(0).map_err(sql)?;
                if by_id.contains_key(&id) {
                    days.entry(r.get(1).map_err(sql)?).or_default().insert(id, r.get(2).map_err(sql)?);
                }
            }
        }
        let defs: [(&str, &str, Box<dyn Fn(&Co) -> bool>); 3] = [
            ("all", "番剧综合指数", Box::new(|_| true)),
            ("season", "本季新番", Box::new(|co: &Co| !co.long && co.first_air.as_str() >= qs.as_str())),
            ("long", "长篇连载", Box::new(|co: &Co| co.long)),
        ];
        let mut out = Vec::new();
        for (key, name, member) in defs.iter() {
            let members: HashSet<i64> = cos.iter().filter(|co| member(co)).map(|co| co.id).collect();
            let mut series: Vec<(String, f64)> = Vec::new();
            let mut prev: Option<&HashMap<i64, i64>> = None;
            let mut level = 1000.0;
            for (day, closes) in &days {
                let has = closes.keys().any(|id| members.contains(id));
                if !has {
                    continue;
                }
                if let Some(p) = prev {
                    let (mut num, mut den) = (0i128, 0i128);
                    for (id, close) in closes {
                        if !members.contains(id) {
                            continue;
                        }
                        if let Some(pc) = p.get(id) {
                            let sh = by_id[id].shares as i128;
                            num += *close as i128 * sh;
                            den += *pc as i128 * sh;
                        }
                    }
                    if den > 0 {
                        level *= num as f64 / den as f64;
                    }
                }
                series.push((day.clone(), level));
                prev = Some(closes);
            }
            if series.is_empty() {
                continue;
            }
            // today's live point from last prices
            let last_day = series.last().map(|s| s.0.clone()).unwrap_or_default();
            if last_day < today {
                let (mut num, mut den) = (0i128, 0i128);
                let lastc = &days[&last_day];
                for id in &members {
                    if let Some(pc) = lastc.get(id) {
                        let sh = by_id[id].shares as i128;
                        num += by_id[id].last as i128 * sh;
                        den += *pc as i128 * sh;
                    }
                }
                if den > 0 {
                    let lv = level * num as f64 / den as f64;
                    series.push((today.clone(), lv));
                }
            }
            let n = series.len();
            let value = series[n - 1].1;
            let prev_v = if n >= 2 { series[n - 2].1 } else { value };
            out.push(json!({
                "key": key, "name": name, "members": members.len(),
                "base_day": series[0].0,
                "value": r4(value), "prev": r4(prev_v),
                "change": if prev_v > 0.0 { r4(value / prev_v - 1.0) } else { 0.0 },
                "series": series.iter().map(|(d, v)| json!([d, r4(*v)])).collect::<Vec<_>>(),
            }));
        }
        let (mut up, mut flat, mut down) = (0, 0, 0);
        for co in &cos {
            match co.last.cmp(&co.prev) {
                std::cmp::Ordering::Greater => up += 1,
                std::cmp::Ordering::Less => down += 1,
                std::cmp::Ordering::Equal => flat += 1,
            }
        }
        Ok(json!({
            "indices": out,
            "breadth": {"up": up, "flat": flat, "down": down},
            "season_start": qs,
        }))
    })
    .await
}

// ---------------------------------------------------------------- collector data

fn collector(cfg: &crate::config::Config) -> Option<Connection> {
    Connection::open_with_flags(
        &cfg.collector_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn snapshot(cdb: &Connection, source: &str, kind: &str, id: &str) -> Option<Value> {
    let t: Option<String> = cdb
        .query_row(
            "SELECT data FROM snapshots WHERE source=? AND kind=? AND external_id=? ORDER BY day DESC LIMIT 1",
            [source, kind, id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    t.and_then(|t| serde_json::from_str(&t).ok())
}

/// `sim/matches.json` "matches" merged with "manual" (AniList id -> {bgm, annict, ...}).
fn matches(cfg: &crate::config::Config) -> HashMap<String, Value> {
    let Ok(text) = std::fs::read_to_string(cfg.root.join("sim").join("matches.json")) else { return HashMap::new() };
    let Ok(all) = serde_json::from_str::<Value>(&text) else { return HashMap::new() };
    let mut out: HashMap<String, Value> = HashMap::new();
    if let Some(m) = all.get("matches").and_then(Value::as_object) {
        for (k, v) in m {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(m) = all.get("manual").and_then(Value::as_object) {
        for (k, v) in m {
            let e = out.entry(k.clone()).or_insert_with(|| json!({}));
            if let (Some(e), Some(v)) = (e.as_object_mut(), v.as_object()) {
                for (kk, vv) in v {
                    e.insert(kk.clone(), vv.clone());
                }
            }
        }
    }
    out
}

fn id_str(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Next airing per company from the latest AniList snapshot, cached for ten minutes.
fn next_airings(cfg: &crate::config::Config, ids: &[i64]) -> HashMap<i64, (i64, i64)> {
    static CACHE: OnceLock<Mutex<Option<(Instant, HashMap<i64, (i64, i64)>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, m)) = cache.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(600) && ids.iter().all(|i| m.contains_key(i) || m.contains_key(&-i)) {
            return m.clone();
        }
    }
    let mut out = HashMap::new();
    if let Some(cdb) = collector(cfg) {
        for id in ids {
            let next = snapshot(&cdb, "anilist", "media", &id.to_string())
                .and_then(|m| m.get("nextAiringEpisode").cloned())
                .and_then(|n| Some((n.get("episode")?.as_i64()?, n.get("airingAt")?.as_i64()?)));
            match next {
                Some(x) => out.insert(*id, x),
                None => out.insert(-id, (0, 0)), // remembered as "none"
            };
        }
    }
    *cache.lock().unwrap() = Some((Instant::now(), out.clone()));
    out
}

/// Episodes airing in today's broadcast day: from 06:00 today (game time zone) to 06:00 tomorrow, or,
/// before 06:00, the broadcast day that started yesterday.
async fn tonight(st: &Arc<AppState>) -> Result<Value, ApiError> {
    let st2 = st.clone();
    st.read(move |c| {
        let cfg = &st2.cfg;
        let now = st2.clock.now();
        let mut start = now.date_naive().and_hms_opt(6, 0, 0).unwrap();
        if now.naive_local() < start {
            start -= chrono::Duration::days(1);
        }
        let tz = st2.clock.tz;
        let start = tz.from_local_datetime(&start).single().map(|d| d.timestamp()).unwrap_or(now.timestamp());
        let end = start + 86400;
        let ids: Vec<i64> = {
            let mut s = c.prepare("SELECT id FROM companies WHERE status IN ('listed','subscribing')").map_err(sql)?;
            let rows = s.query_map([], |r| r.get(0)).map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let next = next_airings(cfg, &ids);
        let mut items: Vec<(i64, i64, i64)> = ids
            .iter()
            .filter_map(|id| next.get(id).map(|(ep, at)| (*at, *id, *ep)))
            .filter(|(at, _, _)| *at >= start && *at < end)
            .collect();
        items.sort();
        let out: Vec<Value> = items
            .into_iter()
            .map(|(at, id, ep)| {
                let when = tz.timestamp_opt(at, 0).single().map(|d| crate::config::format_ts(&d)).unwrap_or_default();
                json!({"company_id": id, "episode": ep, "airing_at": when, "aired": at <= now.timestamp()})
            })
            .collect();
        Ok(json!({"airings": out}))
    })
    .await
}

// ---------------------------------------------------------------- one company's profile

async fn profile(st: &Arc<AppState>, id: i64) -> Result<Value, ApiError> {
    let st2 = st.clone();
    st.read(move |c| {
        let cfg = &st2.cfg;
        let co: Option<(i64, i64, String)> = c
            .query_row("SELECT shares, last_price, first_air FROM companies WHERE id=?", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(sql)?;
        let (shares, price, first_air) = co.ok_or_else(|| super::http(404, "没有这支股票"))?;
        let mut episodes = Vec::new();
        let mut dividends = Vec::new();
        let mut daily: Vec<(String, i64, i64)> = Vec::new(); // day, revenue, dps
        {
            let mut s = c
                .prepare("SELECT day, aired, dividend_ps, revenue FROM company_days WHERE company_id=? ORDER BY day")
                .map_err(sql)?;
            let mut rows = s.query([id]).map_err(sql)?;
            // the first row's count is history from before the game's window, not an airing that day
            let mut last_aired: Option<i64> = None;
            while let Some(r) = rows.next().map_err(sql)? {
                let day: String = r.get(0).map_err(sql)?;
                let aired: i64 = r.get(1).map_err(sql)?;
                let dps: i64 = r.get(2).map_err(sql)?;
                let rev: i64 = r.get(3).map_err(sql)?;
                match last_aired {
                    Some(prev) if aired > prev => episodes.push(json!({"n": aired, "day": day})),
                    None if aired > 0 && day == first_air => episodes.push(json!({"n": aired, "day": day})),
                    _ => {}
                }
                last_aired = Some(last_aired.map_or(aired, |p| p.max(aired)));
                if dps > 0 {
                    dividends.push(json!({"day": day, "ps": cents(dps)}));
                }
                daily.push((day, rev, dps));
            }
        }
        let today = st2.clock.today();
        let t = NaiveDate::parse_from_str(&today, "%Y-%m-%d").map_err(|e| super::internal(e.to_string()))?;
        let since = |n: i64| (t - chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
        let s365 = since(365);
        let rev_ttm: i64 = daily.iter().filter(|d| d.0 > s365).map(|d| d.1).sum();
        let dps_ttm: i64 = daily.iter().filter(|d| d.0 > s365).map(|d| d.2).sum();
        let cost = param_f64(cfg, "company.operating_cost_ratio", 0.5);
        let tax = param_f64(cfg, "company.corp_tax", 0.25);
        let eps = rev_ttm as f64 * (1.0 - cost) * (1.0 - tax) / shares.max(1) as f64;
        let pe = if eps > 0.0 && price > 0 { Some(r4(price as f64 / eps)) } else { None };
        let dy = if dps_ttm > 0 && price > 0 { Some(r4(dps_ttm as f64 / price as f64)) } else { None };
        let cash: Option<i64> = c
            .query_row("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", [id], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        let pb = match cash {
            Some(cash) if cash > 0 && shares > 0 && price > 0 => Some(r4(price as f64 / (cash as f64 / shares as f64))),
            _ => None,
        };
        let mut holders = json!({"committee": 0, "npc": 0, "player": 0, "unissued": 0});
        {
            let mut s = c
                .prepare(
                    "SELECT a.kind, SUM(h.qty) FROM holdings h JOIN accounts a ON a.id=h.account_id
                     WHERE h.company_id=? GROUP BY a.kind",
                )
                .map_err(sql)?;
            let mut rows = s.query([id]).map_err(sql)?;
            while let Some(r) = rows.next().map_err(sql)? {
                let kind: String = r.get(0).map_err(sql)?;
                let q: i64 = r.get::<_, Option<i64>>(1).map_err(sql)?.unwrap_or(0);
                let key = match kind.as_str() {
                    "committee" => "committee",
                    "player" => "player",
                    "company" => "unissued",
                    _ => "npc",
                };
                let cur = holders[key].as_i64().unwrap_or(0);
                holders[key] = json!(cur + q);
            }
        }
        let next = next_airings(cfg, &[id]).get(&id).map(|(ep, at)| {
            let when = st2.clock.tz.timestamp_opt(*at, 0).single().map(|d| crate::config::format_ts(&d)).unwrap_or_default();
            json!({"episode": ep, "airing_at": when})
        });
        Ok(json!({
            "episodes": episodes,
            "next_episode": next,
            "dividends": dividends,
            "ratios": {"pe": pe, "dividend_yield": dy, "pb": pb},
            "holders": holders,
            "shares": shares,
        }))
    })
    .await
}

// ---------------------------------------------------------------- listings

async fn listings(st: &Arc<AppState>) -> Result<Value, ApiError> {
    let st2 = st.clone();
    st.read(move |c| {
        let today = st2.clock.today();
        let t = NaiveDate::parse_from_str(&today, "%Y-%m-%d").map_err(|e| super::internal(e.to_string()))?;
        let since = (t - chrono::Duration::days(60)).format("%Y-%m-%d").to_string();
        let mut s = c
            .prepare(
                "SELECT co.id, co.title, co.listed_on, co.ipo_price, co.sub_start,
                   (SELECT close FROM candles k WHERE k.company_id=co.id AND k.day>=co.listed_on AND k.source='live' ORDER BY k.day LIMIT 1) first_close,
                   (SELECT SUM(qty) FROM ipo_applications a WHERE a.company_id=co.id) applied,
                   (SELECT SUM(allotted) FROM ipo_applications a WHERE a.company_id=co.id) allotted
                 FROM companies co WHERE co.status='listed' AND co.sub_start IS NOT NULL AND co.listed_on >= ?
                 ORDER BY co.listed_on DESC LIMIT 20",
            )
            .map_err(sql)?;
        let rows = s
            .query_map([&since], |r| {
                let ipo: i64 = r.get(3)?;
                let first: Option<i64> = r.get(5)?;
                let applied: Option<i64> = r.get(6)?;
                let allotted: Option<i64> = r.get(7)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?, "listed_on": r.get::<_, String>(2)?,
                    "ipo_price": cents(ipo),
                    "first_close": first.map(cents),
                    "first_change": first.filter(|_| ipo > 0).map(|f| r4(f as f64 / ipo as f64 - 1.0)),
                    "ratio": match (applied, allotted) { (Some(a), Some(b)) if a > 0 => Some(r4(b as f64 / a as f64)), _ => None },
                }))
            })
            .map_err(sql)?;
        let recent: Vec<Value> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let mut s = c
            .prepare(
                "SELECT id, title, sub_start, first_air FROM companies WHERE status='subscribing' AND sub_start > ? ORDER BY sub_start",
            )
            .map_err(sql)?;
        let rows = s
            .query_map([&today], |r| {
                Ok(json!({"id": r.get::<_, i64>(0)?, "title": r.get::<_, String>(1)?, "sub_start": r.get::<_, String>(2)?,
                          "first_air": r.get::<_, String>(3)?}))
            })
            .map_err(sql)?;
        let upcoming: Vec<Value> = rows.collect::<Result<_, _>>().map_err(sql)?;
        Ok(json!({"recent": recent, "upcoming": upcoming}))
    })
    .await
}

// ---------------------------------------------------------------- covers

fn covers_dir(st: &AppState) -> PathBuf {
    std::env::var("ANISTOCK_COVERS").map(PathBuf::from).unwrap_or_else(|_| st.cfg.root.join("data").join("covers"))
}

fn image_type(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

fn not_found() -> Response<Body> {
    let mut r = Response::new(Body::from("{\"detail\":\"没有封面\"}"));
    *r.status_mut() = StatusCode::NOT_FOUND;
    r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
    r.headers_mut().insert("cache-control", HeaderValue::from_static("public, max-age=3600"));
    r
}

/// Ids whose cover could not be found or fetched, with when; retried after six hours.
fn misses() -> &'static Mutex<HashMap<i64, Instant>> {
    static M: OnceLock<Mutex<HashMap<i64, Instant>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// At most four downloads at a time.
fn fetch_slots() -> &'static tokio::sync::Semaphore {
    static S: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(4))
}

async fn cover(st: &Arc<AppState>, raw: &str) -> Response<Body> {
    let Ok(id) = raw.parse::<i64>() else { return not_found() };
    let path = covers_dir(st).join(format!("{id}.img"));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return image_reply(bytes);
    }
    if misses().lock().unwrap().get(&id).is_some_and(|t| t.elapsed() < Duration::from_secs(6 * 3600)) {
        return not_found();
    }
    let _slot = fetch_slots().acquire().await;
    // another request may have fetched it while we waited
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return image_reply(bytes);
    }
    let st2 = st.clone();
    let got = tokio::task::spawn_blocking(move || fetch_cover(&st2, id, &path)).await.ok().flatten();
    match got {
        Some(bytes) => image_reply(bytes),
        None => {
            misses().lock().unwrap().insert(id, Instant::now());
            not_found()
        }
    }
}

fn image_reply(bytes: Vec<u8>) -> Response<Body> {
    let Some(t) = image_type(&bytes) else { return not_found() };
    let mut r = Response::new(Body::from(bytes));
    r.headers_mut().insert("content-type", HeaderValue::from_static(t));
    r.headers_mut().insert("cache-control", HeaderValue::from_static("public, max-age=604800"));
    r
}

/// The Bangumi subject's 200 px cover URL (from the collector's latest snapshot), downloaded with curl
/// (which honours `https_proxy`), validated as an image and stored under `data/covers/{id}.img`.
fn fetch_cover(st: &AppState, id: i64, path: &std::path::Path) -> Option<Vec<u8>> {
    let m = matches(&st.cfg);
    let bgm = m.get(&id.to_string()).and_then(|e| e.get("bgm")).and_then(id_str)?;
    let cdb = collector(&st.cfg)?;
    let subject = snapshot(&cdb, "bgm", "subject", &bgm)?;
    let images = subject.get("images")?;
    let url = ["small", "common", "medium", "large"].iter().find_map(|k| images.get(*k).and_then(Value::as_str))?.to_string();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return None;
    }
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let tmp = dir.join(format!(".{id}.part"));
    let status = std::process::Command::new("curl")
        .args(["-sSfL", "--max-time", "20", "--max-filesize", "5000000", "-A", "AniEx/0.2 (cover cache)", "-o"])
        .arg(&tmp)
        .arg(&url)
        .status()
        .ok()?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    let bytes = std::fs::read(&tmp).ok()?;
    if image_type(&bytes).is_none() {
        let _ = std::fs::remove_file(&tmp);
        return None;
    }
    std::fs::rename(&tmp, path).ok()?;
    Some(bytes)
}
