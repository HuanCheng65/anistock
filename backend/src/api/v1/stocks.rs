//! One stock (docs/api-v1.md §4.2) and the IPO reads (§4.4.1, §4.4.2).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderValue, Response};
use axum::response::IntoResponse;
use axum::Json;
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use super::error::{self, sql, ErrorBody, R};
use super::market::{collector, days_before, next_airings, param_f64, ratio, ratios, snapshot, unix_ts};
use super::types::*;
use super::{read, Caller, Page, Q};
use crate::api::AppState;
use crate::config::Config;
use crate::model::Company;

pub fn one_company(c: &Connection, id: i64) -> R<Option<Company>> {
    c.query_row("SELECT * FROM companies WHERE id=?", [id], crate::store::company_from_row).optional().map_err(sql)
}

pub fn sref(co: &Company) -> StockRef {
    StockRef { id: co.id, code: co.code.clone(), title: co.title.clone() }
}

pub fn vcm_of(co: &Company, now: &str) -> Vcm {
    match crate::exchange::cooling(co, now) {
        Some((lo, hi)) => Vcm {
            active: true,
            until: co.vcm_until.clone().map(ts),
            low: Some(m(lo)),
            high: Some(m(hi)),
            count_today: co.vcm_count,
        },
        None => Vcm { active: false, until: None, low: None, high: None, count_today: co.vcm_count },
    }
}

// ---------------------------------------------------------------- audiences

fn audiences(cfg: &Config, id: i64) -> Audiences {
    let mut out = Audiences { anilist: None, bangumi: None, annict: None };
    let Some(cdb) = collector(cfg) else { return out };
    if let Some(mm) = snapshot(&cdb, "anilist", "media", &id.to_string()) {
        out.anilist = Some(AnilistAudience {
            popularity: mm.get("popularity").and_then(Value::as_i64),
            score: mm.get("averageScore").and_then(Value::as_f64),
            genres: mm
                .get("genres")
                .and_then(Value::as_array)
                .map(|g| g.iter().take(4).filter_map(|x| x.as_str().map(str::to_string)).collect())
                .unwrap_or_default(),
        });
    }
    let matches = matches(cfg);
    let entry = matches.get(&id.to_string());
    let id_of = |k: &str| entry.and_then(|e| e.get(k)).and_then(id_str);
    if let Some(bgm) = id_of("bgm") {
        if let Some(s) = snapshot(&cdb, "bgm", "subject", &bgm) {
            let col = s.get("collection");
            let n = |k: &str| col.and_then(|c| c.get(k)).and_then(Value::as_i64).unwrap_or(0);
            out.bangumi = Some(BangumiAudience {
                id: bgm.parse().ok(),
                collection: col.filter(|c| c.is_object()).map(|_| BangumiCollection {
                    wish: n("wish"),
                    doing: n("doing"),
                    collect: n("collect"),
                    on_hold: n("on_hold"),
                    dropped: n("dropped"),
                }),
                score: s.get("rating").and_then(|r| r.get("score")).and_then(Value::as_f64),
            });
        }
    }
    if let Some(annict) = id_of("annict") {
        if let Some(w) = snapshot(&cdb, "annict", "work", &annict) {
            out.annict = Some(AnnictAudience {
                id: annict.parse().ok(),
                watchers: w.get("watchersCount").and_then(Value::as_i64),
            });
        }
    }
    out
}

/// `sim/matches.json` "matches" merged with "manual" (AniList id -> {bgm, annict, ...}).
pub fn matches(cfg: &Config) -> std::collections::HashMap<String, Value> {
    let mut out = std::collections::HashMap::new();
    let Ok(text) = std::fs::read_to_string(cfg.root.join("sim").join("matches.json")) else { return out };
    let Ok(all) = serde_json::from_str::<Value>(&text) else { return out };
    if let Some(m) = all.get("matches").and_then(Value::as_object) {
        for (k, v) in m {
            out.insert(k.clone(), v.clone());
        }
    }
    if let Some(m) = all.get("manual").and_then(Value::as_object) {
        for (k, v) in m {
            let e = out.entry(k.clone()).or_insert_with(|| serde_json::json!({}));
            if let (Some(e), Some(v)) = (e.as_object_mut(), v.as_object()) {
                for (kk, vv) in v {
                    e.insert(kk.clone(), vv.clone());
                }
            }
        }
    }
    out
}

pub fn id_str(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------- holders

fn holders(c: &Connection, id: i64, shares: i64) -> R<Holders> {
    let mut h = Holders {
        player: 0,
        committee: 0,
        unissued: 0,
        market_maker: 0,
        other: 0,
        npc: NpcHolders { total: 0, by_style: None },
    };
    let mut styles: BTreeMap<String, i64> = BTreeMap::new();
    let mut s = c
        .prepare_cached(
            "SELECT a.kind, a.name, SUM(h.qty) FROM holdings h JOIN accounts a ON a.id=h.account_id
             WHERE h.company_id=? GROUP BY a.kind, CASE WHEN a.kind='npc' THEN substr(a.name, 1, 3) ELSE '' END",
        )
        .map_err(sql)?;
    let mut rows = s.query([id]).map_err(sql)?;
    let mut any_npc_accounts = false;
    while let Some(r) = rows.next().map_err(sql)? {
        let kind: String = r.get(0).map_err(sql)?;
        let name: String = r.get(1).map_err(sql)?;
        let q: i64 = r.get::<_, Option<i64>>(2).map_err(sql)?.unwrap_or(0);
        match kind.as_str() {
            "player" => h.player += q,
            "committee" => h.committee += q,
            "company" => h.unissued += q,
            "mm" => h.market_maker += q,
            "noise" => h.npc.total += q,
            "npc" => {
                any_npc_accounts = true;
                h.npc.total += q;
                let style = if name.starts_with("价值型") {
                    "value"
                } else if name.starts_with("趋势型") {
                    "trend"
                } else if name.starts_with("消息型") {
                    "news"
                } else {
                    "retail"
                };
                *styles.entry(style.into()).or_default() += q;
            }
            _ => h.other += q,
        }
    }
    let total = h.player + h.committee + h.unissued + h.market_maker + h.other + h.npc.total;
    if total != shares && total > 0 {
        // shares never issued sit nowhere; count them as unissued so the parts add up to `shares`
        h.unissued += shares - total;
    }
    if !any_npc_accounts {
        any_npc_accounts = c
            .query_row("SELECT 1 FROM accounts WHERE kind='npc' LIMIT 1", [], |_| Ok(()))
            .optional()
            .map_err(sql)?
            .is_some();
    }
    if any_npc_accounts {
        for k in ["value", "trend", "news", "retail"] {
            styles.entry(k.into()).or_insert(0);
        }
        h.npc.by_style = Some(styles);
    }
    Ok(h)
}

// ---------------------------------------------------------------- IPO objects

pub fn ipo_status(co: &Company, today: &str) -> &'static str {
    match co.status.as_str() {
        "subscribing" if co.sub_start.as_deref().is_some_and(|s| s > today) => "upcoming",
        "subscribing" => "open",
        _ if co.listed_on.as_str() > today => "allotted",
        _ => "listed",
    }
}

pub fn ipo_of(c: &Connection, cfg: &Config, co: &Company, today: &str, viewer: Option<i64>) -> R<Ipo> {
    let float_share = param_f64(cfg, "company.ipo_float", 0.25);
    let float_qty = (co.shares as f64 * float_share).round_ties_even() as i64;
    let base = (float_qty as f64 * cfg.game.ipo_public_base).round_ties_even() as i64;
    let (q, n, a): (i64, i64, i64) = c
        .query_row(
            "SELECT COALESCE(SUM(qty),0), COUNT(*), COALESCE(SUM(allotted),0) FROM ipo_applications WHERE company_id=?",
            [co.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(sql)?;
    let status = ipo_status(co, today);
    let allotted = matches!(status, "allotted" | "listed");
    let viewer = match viewer {
        Some(p) => Some(
            c.query_row(
                "SELECT qty, frozen, allotted FROM ipo_applications WHERE company_id=? AND account_id=?",
                [co.id, p],
                |r| Ok(IpoViewer { quantity: r.get(0)?, frozen: m(r.get(1)?), allotted: r.get(2)? }),
            )
            .optional()
            .map_err(sql)?
            .unwrap_or(IpoViewer { quantity: 0, frozen: m(0), allotted: None }),
        ),
        None => None,
    };
    let first_day = if status == "listed" {
        c.query_row(
            "SELECT close FROM candles WHERE company_id=? AND day>=? AND source='live' ORDER BY day LIMIT 1",
            rusqlite::params![co.id, co.listed_on],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(sql)?
        .map(|close| IpoFirstDay { close: m(close), change: (co.ipo_price > 0).then(|| ratio(close, co.ipo_price)) })
    } else {
        None
    };
    Ok(Ipo {
        stock: sref(co),
        title_ja: co.title_ja.clone(),
        status: status.into(),
        price: m(co.ipo_price),
        sub_start: co.sub_start.clone().unwrap_or_default(),
        sub_end: co.sub_end.clone().unwrap_or_default(),
        listed_on: co.listed_on.clone(),
        first_air: co.first_air.clone(),
        float_quantity: float_qty,
        base_public_quantity: base,
        applied_quantity: q,
        applicants: n,
        result: allotted.then(|| IpoResult {
            oversubscribed: if base != 0 { r6(q as f64 / base as f64) } else { 0.0 },
            public_quantity: a,
            allotment_ratio: (q != 0).then(|| r6(a as f64 / q as f64)),
        }),
        first_day,
        viewer,
    })
}

// ---------------------------------------------------------------- detail

/// 一支股票的完整资料
#[utoipa::path(get, path = "/api/v1/stocks/{id}", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id（AniList 作品 id）")),
    responses((status = 200, body = StockDetail), (status = "default", body = ErrorBody)))]
pub async fn detail(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<StockDetail>> {
    let id = super::path_id(&raw, "id")?;
    let viewer = caller.viewer();
    let st2 = st.clone();
    read(&st, move |c| {
        let cfg = &st2.cfg;
        let co = one_company(c, id)?.ok_or_else(|| error::stock_not_found(id))?;
        let today = st2.clock.today();
        let now = st2.clock.ts();
        let agg = super::market::day_aggs(c, &today)?.remove(&id);
        let left = super::market::ipo_remaining(c)?.remove(&id).unwrap_or(0);
        // per-share profit, the `ratios` basis (current shares; forward-adjusted like the K-line)
        let cost = param_f64(cfg, "company.operating_cost_ratio", 0.5);
        let tax = param_f64(cfg, "company.corp_tax", 0.25);
        let sh = co.shares.max(1) as f64;
        let eps = move |rev: i64| m((rev as f64 * (1.0 - cost) * (1.0 - tax) / sh).round() as i64);
        let weeks: Vec<RevenueWeek> = {
            let mut s = c
                .prepare_cached(
                    "SELECT strftime('%Y-%W', day) wk, MIN(day), SUM(revenue), SUM(rev_us), SUM(rev_cn), SUM(rev_jp),
                     SUM(dividend_ps), MAX(aired)
                     FROM company_days WHERE company_id=? GROUP BY wk ORDER BY wk",
                )
                .map_err(sql)?;
            let rows = s
                .query_map([id], |r| {
                    Ok(RevenueWeek {
                        week_start: r.get(1)?,
                        revenue: m(r.get(2)?),
                        by_market: ByMarket { us: m(r.get(3)?), cn: m(r.get(4)?), jp: m(r.get(5)?) },
                        dividend_per_share: m(r.get(6)?),
                        earnings_per_share: eps(r.get(2)?),
                        episodes_aired: r.get(7)?,
                    })
                })
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let mut episode_days = Vec::new();
        let mut dividends = Vec::new();
        let mut aired_now = 0;
        {
            let mut s = c
                .prepare_cached("SELECT day, aired, dividend_ps FROM company_days WHERE company_id=? ORDER BY day")
                .map_err(sql)?;
            let mut rows = s.query([id]).map_err(sql)?;
            let mut last: Option<i64> = None;
            while let Some(r) = rows.next().map_err(sql)? {
                let d: String = r.get(0).map_err(sql)?;
                let aired: i64 = r.get(1).map_err(sql)?;
                let dps: i64 = r.get(2).map_err(sql)?;
                match last {
                    Some(p) if aired > p => episode_days.push(EpisodeDay { episode: aired, trading_day: day(d.clone()) }),
                    None if aired > 0 && d == co.first_air => {
                        episode_days.push(EpisodeDay { episode: aired, trading_day: day(d.clone()) })
                    }
                    _ => {}
                }
                last = Some(last.map_or(aired, |p| p.max(aired)));
                aired_now = aired;
                if dps > 0 {
                    dividends.push(DividendPaid { trading_day: day(d), per_share: m(dps) });
                }
            }
        }
        let next_episode = next_airings(cfg, &[id])
            .get(&id)
            .map(|(ep, at)| NextEpisode { episode: *ep, airs_at: ts(unix_ts(&st2, *at)) });
        let r = ratios(c, cfg, &today, Some(id))?.remove(&id).ok_or_else(|| error::internal("ratios"))?;
        // the last 28 days, cut like `ratios`: actual sums, never annualised
        let trailing_4w = {
            let since = days_before(&today, 28)?;
            let (rev, dps): (i64, i64) = c
                .query_row(
                    "SELECT COALESCE(SUM(revenue),0), COALESCE(SUM(dividend_ps),0) FROM company_days WHERE company_id=? AND day>?",
                    rusqlite::params![id, since],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(sql)?;
            Trailing4w { earnings_per_share: eps(rev), dividends_per_share: m(dps) }
        };
        let has_ipo = co.sub_start.as_deref().is_some_and(|s| !s.is_empty());
        Ok(StockDetail {
            stock: sref(&co),
            title_ja: co.title_ja.clone(),
            title_en: co.title_en.clone(),
            studio: co.studio.clone(),
            status: co.status.clone(),
            trading_day: day(today.clone()),
            shares: co.shares,
            first_air: co.first_air.clone(),
            listed_on: co.listed_on.clone(),
            delisted_on: co.delisted_on.clone(),
            episodes: co.episodes,
            episodes_aired: aired_now,
            is_long_runner: co.long_runner != 0,
            ipo_price: m(co.ipo_price),
            quote: StockQuote {
                price: m(co.last_price),
                prev_close: m(co.prev_close),
                change: ratio(co.last_price, co.prev_close),
                open: agg.map(|a| m(a.0)),
                high: agg.map(|a| m(a.1)),
                low: agg.map(|a| m(a.2)),
                open_reference: om(co.open_ref),
                volume: agg.map_or(0, |a| a.3),
                turnover: m(agg.map_or(0, |a| a.4)),
                market_cap: m(co.last_price * co.shares),
                ipo_remaining: left,
            },
            vcm: vcm_of(&co, &now),
            ratios: r,
            revenue_weeks: weeks,
            trailing_4w,
            episode_days,
            next_episode,
            dividends,
            holders: holders(c, id, co.shares)?,
            audiences: audiences(cfg, id),
            ipo: if has_ipo { Some(ipo_of(c, cfg, &co, &today, viewer)?) } else { None },
            cover_url: super::market::cover_url(id),
        })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- book, trades, candles, intraday

pub fn book_side(c: &Connection, id: i64, side: &str, depth: i64) -> R<Vec<BookLevel>> {
    let order = if side == "buy" { "DESC" } else { "ASC" };
    let q = format!(
        "SELECT price, SUM(qty - filled), COUNT(*) FROM orders
         WHERE company_id=? AND status='open' AND side=? AND price IS NOT NULL GROUP BY price ORDER BY price {order} LIMIT ?"
    );
    let mut s = c.prepare_cached(&q).map_err(sql)?;
    let rows = s
        .query_map(rusqlite::params![id, side, depth], |r| {
            Ok(BookLevel { price: m(r.get(0)?), quantity: r.get(1)?, orders: r.get(2)? })
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// The opening auction's indicative price and volume (engine `auction_price`, from the database).
pub fn auction_indication(c: &Connection, id: i64, reference: i64) -> R<AuctionIndication> {
    let levels = |side: &str| -> R<Vec<(i64, i64)>> {
        let mut s = c
            .prepare_cached(
                "SELECT price, SUM(qty-filled) FROM orders WHERE company_id=? AND status='open'
                 AND side=? AND kind='limit' GROUP BY price",
            )
            .map_err(sql)?;
        let rows = s.query_map(rusqlite::params![id, side], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?;
        rows.collect::<Result<_, _>>().map_err(sql)
    };
    let (buys, sells) = (levels("buy")?, levels("sell")?);
    let mut prices: Vec<i64> = buys.iter().chain(&sells).map(|l| l.0).collect();
    prices.sort();
    prices.dedup();
    let mut best: Option<((i64, i64, i64), i64, i64)> = None;
    for p in prices {
        let demand: i64 = buys.iter().filter(|l| l.0 >= p).map(|l| l.1).sum();
        let supply: i64 = sells.iter().filter(|l| l.0 <= p).map(|l| l.1).sum();
        let vol = demand.min(supply);
        if vol <= 0 {
            continue;
        }
        let key = (vol, -(demand - supply).abs(), -(p - reference).abs());
        if best.as_ref().is_none_or(|b| key > b.0) {
            best = Some((key, p, vol));
        }
    }
    Ok(match best {
        Some((_, p, v)) => AuctionIndication { price: Some(m(p)), quantity: v },
        None => AuctionIndication { price: None, quantity: 0 },
    })
}

pub fn now_ms(st: &AppState) -> String {
    st.clock.now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
}

/// 盘口
#[utoipa::path(get, path = "/api/v1/stocks/{id}/book", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id"), ("depth" = Option<i64>, Query, description = "1–20，默认 5")),
    responses((status = 200, body = BookResponse), (status = "default", body = ErrorBody)))]
pub async fn book(State(st): State<Arc<AppState>>, Path(raw): Path<String>, RawQuery(q): RawQuery) -> R<Json<BookResponse>> {
    let id = super::path_id(&raw, "id")?;
    let depth = Q::parse(q.as_deref()).int("depth", 1, 20)?.unwrap_or(5);
    let st2 = st.clone();
    read(&st, move |c| {
        let co = one_company(c, id)?.ok_or_else(|| error::stock_not_found(id))?;
        let auction = if st2.phase() == crate::config::Phase::Auction {
            Some(auction_indication(c, id, co.prev_close)?)
        } else {
            None
        };
        Ok(BookResponse {
            trading_day: day(st2.clock.today()),
            as_of: ts(now_ms(&st2)),
            bids: book_side(c, id, "buy", depth)?,
            asks: book_side(c, id, "sell", depth)?,
            auction,
        })
    })
    .await
    .map(Json)
}

/// A stock's price adjustments (V10): (first day in new terms, price factor, volume factor, action kind), by day.
fn price_adjustments(c: &Connection, id: i64) -> R<Vec<(String, f64, f64, Option<String>)>> {
    let has: bool = c
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='price_adjustments'", [], |_| Ok(()))
        .optional()
        .map_err(sql)?
        .is_some();
    if !has {
        return Ok(Vec::new());
    }
    let mut s = c
        .prepare_cached(
            "SELECT p.day, p.price_factor, p.qty_factor, a.kind FROM price_adjustments p LEFT JOIN corporate_actions a ON a.id = p.action_id
             WHERE p.company_id=? ORDER BY p.day",
        )
        .map_err(sql)?;
    let rows = s.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

fn exists(c: &Connection, id: i64) -> R<()> {
    c.query_row("SELECT 1 FROM companies WHERE id=?", [id], |_| Ok(()))
        .optional()
        .map_err(sql)?
        .ok_or_else(|| error::stock_not_found(id))
}

/// 公开成交，游标分页
#[utoipa::path(get, path = "/api/v1/stocks/{id}/trades", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id"),
        ("limit" = Option<i64>, Query, description = "默认 50，最大 200"),
        ("before" = Option<String>, Query, description = "游标"), ("after" = Option<String>, Query, description = "游标")),
    responses((status = 200, body = PublicTradePage), (status = "default", body = ErrorBody)))]
pub async fn trades(State(st): State<Arc<AppState>>, Path(raw): Path<String>, RawQuery(q): RawQuery) -> R<Json<PublicTradePage>> {
    let id = super::path_id(&raw, "id")?;
    let page = Page::parse(&Q::parse(q.as_deref()))?;
    read(&st, move |c| {
        exists(c, id)?;
        let (cond, tail) = page.sql("id");
        let mut s = c
            .prepare(&format!("SELECT id, price, qty, ts, day FROM trades WHERE company_id=? AND {cond} {tail}"))
            .map_err(sql)?;
        let rows = s
            .query_map([id], |r| {
                Ok(PublicTrade {
                    id: r.get(0)?,
                    price: m(r.get(1)?),
                    quantity: r.get(2)?,
                    at: ts(r.get::<_, String>(3)?),
                    trading_day: day(r.get::<_, String>(4)?),
                })
            })
            .map_err(sql)?;
        let rows: Vec<PublicTrade> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let (items, next_cursor, has_more) = page.finish(rows, |t| t.id);
        Ok(PublicTradePage { items, next_cursor, has_more })
    })
    .await
    .map(Json)
}

/// 日 K 线
#[utoipa::path(get, path = "/api/v1/stocks/{id}/candles", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id"),
        ("from" = Option<String>, Query, description = "交易日，含"), ("to" = Option<String>, Query, description = "交易日，含"),
        ("interval" = Option<String>, Query, description = "只支持 1d"),
        ("adjust" = Option<String>, Query, description = "forward（默认，前复权）或 none")),
    responses((status = 200, body = CandlesResponse), (status = "default", body = ErrorBody)))]
pub async fn candles(State(st): State<Arc<AppState>>, Path(raw): Path<String>, RawQuery(q): RawQuery) -> R<Json<CandlesResponse>> {
    let id = super::path_id(&raw, "id")?;
    let q = Q::parse(q.as_deref());
    if q.get("interval").is_some_and(|i| i != "1d") {
        return Err(error::invalid(vec![error::not_allowed("interval")]));
    }
    let from = q.day("from")?.unwrap_or_else(|| "0000-01-01".into());
    let to = q.day("to")?.unwrap_or_else(|| "9999-12-31".into());
    let forward = match q.get("adjust") {
        None | Some("forward") => true,
        Some("none") => false,
        Some(_) => return Err(error::invalid(vec![error::not_allowed("adjust")])),
    };
    let today = st.clock.today();
    read(&st, move |c| {
        exists(c, id)?;
        let mut items: Vec<Candle> = {
            let mut s = c
                .prepare_cached(
                    "SELECT day, open, high, low, close, volume, source FROM candles WHERE company_id=? AND day>=? AND day<=? ORDER BY day",
                )
                .map_err(sql)?;
            let rows = s
                .query_map(rusqlite::params![id, from, to], |r| {
                    Ok(Candle {
                        trading_day: day(r.get::<_, String>(0)?),
                        open: m(r.get(1)?),
                        high: m(r.get(2)?),
                        low: m(r.get(3)?),
                        close: m(r.get(4)?),
                        volume: r.get(5)?,
                        source: if r.get::<_, Option<String>>(6)?.as_deref() == Some("replay") {
                            CandleSource::Replay
                        } else {
                            CandleSource::Live
                        },
                        is_partial: false,
                    })
                })
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        if today.as_str() >= from.as_str() && today.as_str() <= to.as_str() {
            let agg = super::market::day_aggs(c, &today)?.remove(&id);
            let last = c
                .query_row("SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id DESC LIMIT 1", rusqlite::params![id, today], |r| r.get::<_, i64>(0))
                .optional()
                .map_err(sql)?;
            if let (Some(a), Some(close)) = (agg, last) {
                if items.last().is_none_or(|k| k.trading_day.0 < today) {
                    items.push(Candle {
                        trading_day: day(today),
                        open: m(a.0),
                        high: m(a.1),
                        low: m(a.2),
                        close: m(close),
                        volume: a.3,
                        source: CandleSource::Live,
                        is_partial: true,
                    });
                }
            }
        }
        // 前复权 (docs/v2-corporate.md §5): bars before each split / ex-rights day × its factor
        let adj = price_adjustments(c, id)?;
        if forward && !adj.is_empty() {
            for k in items.iter_mut() {
                let (mut pf, mut qf) = (1.0, 1.0);
                for (d, p, q, _) in &adj {
                    if d.as_str() > k.trading_day.0.as_str() {
                        pf *= p;
                        qf *= q;
                    }
                }
                if pf != 1.0 || qf != 1.0 {
                    let f = |x: Money| m(((x.0 as f64) * pf).round() as i64);
                    k.open = f(k.open);
                    k.high = f(k.high);
                    k.low = f(k.low);
                    k.close = f(k.close);
                    k.volume = ((k.volume as f64) * qf).round() as i64;
                }
            }
        }
        let ex_days = adj
            .into_iter()
            .filter(|(d, ..)| d.as_str() >= from.as_str() && d.as_str() <= to.as_str())
            .map(|(d, p, q, kind)| ExDay { trading_day: day(d), kind: if q != 1.0 { kind.unwrap_or_else(|| "split".into()) } else { kind.unwrap_or_else(|| "rights".into()) }, price_factor: r6(p) })
            .collect();
        Ok(CandlesResponse { items, adjust: if forward { "forward".into() } else { "none".into() }, ex_days })
    })
    .await
    .map(Json)
}

/// 分时
#[utoipa::path(get, path = "/api/v1/stocks/{id}/intraday", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id"), ("trading_day" = Option<String>, Query, description = "默认本交易日")),
    responses((status = 200, body = IntradayResponse), (status = "default", body = ErrorBody)))]
pub async fn intraday(State(st): State<Arc<AppState>>, Path(raw): Path<String>, RawQuery(q): RawQuery) -> R<Json<IntradayResponse>> {
    let id = super::path_id(&raw, "id")?;
    let today = st.clock.today();
    let d = Q::parse(q.as_deref()).day("trading_day")?.unwrap_or_else(|| today.clone());
    read(&st, move |c| {
        let co = one_company(c, id)?.ok_or_else(|| error::stock_not_found(id))?;
        let prev_close = if d == today {
            co.prev_close
        } else {
            c.query_row("SELECT close FROM candles WHERE company_id=? AND day<? ORDER BY day DESC LIMIT 1", rusqlite::params![id, d], |r| r.get(0))
                .optional()
                .map_err(sql)?
                .unwrap_or(co.ipo_price)
        };
        let mut s = c
            .prepare_cached(
                "SELECT substr(ts, 1, 16) minute, SUM(qty), substr(MAX(ts), 20),
                 (SELECT t2.price FROM trades t2 WHERE t2.company_id=t.company_id AND t2.day=t.day AND substr(t2.ts,1,16)=substr(t.ts,1,16)
                  ORDER BY t2.id DESC LIMIT 1)
                 FROM trades t WHERE company_id=? AND day=? GROUP BY minute ORDER BY MIN(id)",
            )
            .map_err(sql)?;
        let rows = s
            .query_map(rusqlite::params![id, d], |r| {
                let minute: String = r.get(0)?;
                let offset: String = r.get(2)?;
                Ok(IntradayPoint { minute: ts(format!("{minute}:00{offset}")), volume: r.get(1)?, price: m(r.get(3)?) })
            })
            .map_err(sql)?;
        Ok(IntradayResponse { trading_day: day(d.clone()), prev_close: m(prev_close), items: rows.collect::<Result<_, _>>().map_err(sql)? })
    })
    .await
    .map(Json)
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

fn no_cover() -> Response<Body> {
    let mut r = error::cover_not_found().into_response();
    r.headers_mut().insert("cache-control", HeaderValue::from_static("public, max-age=3600"));
    r
}

fn misses() -> &'static Mutex<std::collections::HashMap<i64, Instant>> {
    static M: OnceLock<Mutex<std::collections::HashMap<i64, Instant>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

fn fetch_slots() -> &'static tokio::sync::Semaphore {
    static S: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    S.get_or_init(|| tokio::sync::Semaphore::new(4))
}

/// 作品封面（图片）
#[utoipa::path(get, path = "/api/v1/stocks/{id}/cover", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, description = "图片", content_type = "image/jpeg"), (status = "default", body = ErrorBody)))]
pub async fn cover(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> Response<Body> {
    let Ok(id) = raw.trim().parse::<i64>() else { return no_cover() };
    let path = covers_dir(&st).join(format!("{id}.img"));
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return image_reply(bytes);
    }
    if misses().lock().unwrap().get(&id).is_some_and(|t| t.elapsed() < Duration::from_secs(6 * 3600)) {
        return no_cover();
    }
    let _slot = fetch_slots().acquire().await;
    if let Ok(bytes) = tokio::fs::read(&path).await {
        return image_reply(bytes);
    }
    let st2 = st.clone();
    match tokio::task::spawn_blocking(move || fetch_cover(&st2, id, &path)).await.ok().flatten() {
        Some(bytes) => image_reply(bytes),
        None => {
            misses().lock().unwrap().insert(id, Instant::now());
            no_cover()
        }
    }
}

fn image_reply(bytes: Vec<u8>) -> Response<Body> {
    let Some(t) = image_type(&bytes) else { return no_cover() };
    let mut r = Response::new(Body::from(bytes));
    r.headers_mut().insert("content-type", HeaderValue::from_static(t));
    r.headers_mut().insert("cache-control", HeaderValue::from_static("public, max-age=604800, immutable"));
    r
}

/// The Bangumi subject's 200 px cover (collector snapshot), downloaded once with curl (honours
/// `https_proxy`) and kept under `data/covers/{id}.img`.
fn fetch_cover(st: &AppState, id: i64, path: &std::path::Path) -> Option<Vec<u8>> {
    let mm = matches(&st.cfg);
    let bgm = mm.get(&id.to_string()).and_then(|e| e.get("bgm")).and_then(id_str)?;
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
        .args(["-sSfL", "--max-time", "20", "--max-filesize", "5000000", "-A", "AniEx/0.4 (cover cache)", "-o"])
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

// ---------------------------------------------------------------- IPO lists

/// 新股
#[utoipa::path(get, path = "/api/v1/ipos", tag = "ipos",
    params(("status" = Option<String>, Query, description = "upcoming、open、allotted、listed、cancelled，可逗号分隔；默认 upcoming,open,allotted"),
        ("listed_since" = Option<String>, Query, description = "status 含 listed 时：上市日不早于这天，默认 60 天前")),
    responses((status = 200, body = IposResponse), (status = "default", body = ErrorBody)))]
pub async fn ipos(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<IposResponse>> {
    let q = Q::parse(q.as_deref());
    let statuses = q.list("status").unwrap_or_else(|| vec!["upcoming".into(), "open".into(), "allotted".into()]);
    for s in &statuses {
        if !["upcoming", "open", "allotted", "listed", "cancelled"].contains(&s.as_str()) {
            return Err(error::invalid(vec![error::not_allowed("status")]));
        }
    }
    let today = st.clock.today();
    let since = match q.day("listed_since")? {
        Some(d) => d,
        None => days_before(&today, 60)?,
    };
    let viewer = caller.viewer();
    let st2 = st.clone();
    read(&st, move |c| {
        let mut s = c
            .prepare_cached("SELECT * FROM companies WHERE sub_start IS NOT NULL AND sub_start != '' AND status != 'delisted'")
            .map_err(sql)?;
        let cos: Vec<Company> =
            s.query_map([], crate::store::company_from_row).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
        let mut items = Vec::new();
        for co in cos {
            let status = ipo_status(&co, &today);
            if !statuses.iter().any(|s| s == status) || (status == "listed" && co.listed_on < since) {
                continue;
            }
            items.push(ipo_of(c, &st2.cfg, &co, &today, viewer)?);
        }
        let rank = |s: &str| match s {
            "open" => 0,
            "upcoming" => 1,
            "allotted" => 2,
            _ => 3,
        };
        items.sort_by(|a, b| {
            rank(&a.status).cmp(&rank(&b.status)).then_with(|| match a.status.as_str() {
                "open" => a.sub_end.cmp(&b.sub_end),
                "upcoming" => a.sub_start.cmp(&b.sub_start),
                _ => b.listed_on.cmp(&a.listed_on),
            })
        });
        Ok(IposResponse { trading_day: day(today), items })
    })
    .await
    .map(Json)
}

pub async fn load_ipo(st: &Arc<AppState>, id: i64, viewer: Option<i64>) -> R<Ipo> {
    let st2 = st.clone();
    read(st, move |c| {
        let co = one_company(c, id)?.ok_or_else(|| error::ipo_not_found(id))?;
        if co.sub_start.as_deref().is_none_or(str::is_empty) {
            return Err(error::ipo_not_found(id));
        }
        ipo_of(c, &st2.cfg, &co, &st2.clock.today(), viewer)
    })
    .await
}

/// 一支新股
#[utoipa::path(get, path = "/api/v1/ipos/{id}", tag = "ipos",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = Ipo), (status = "default", body = ErrorBody)))]
pub async fn ipo_one(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<Ipo>> {
    let id = super::path_id(&raw, "id")?;
    load_ipo(&st, id, caller.viewer()).await.map(Json)
}
