//! Market data and community endpoints (v1 `app.py` "market data", "IPO subscription" reads,
//! "community"): status, market, stocks/{id} (+ candles, intraday), ipos, leaderboard, news, macro.
//! Every query is v1's SQL, so ordering and sums agree.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::NaiveDate;
use rusqlite::{Connection, OptionalExtension, Row};

use super::account::{optional_player, portfolio_rows};
use super::json::{loads, J};
use super::orders::order_json;
use super::validate::{param_int, BigInt};
use super::{
    col, deposit_rate, div, game, get_i64, get_str, http, sql, sql_int, state, state_rate, yuan, ApiError,
    ApiResult, AppState, Reply, Req,
};
use crate::config::Phase;
use crate::model::Company;
use crate::obj;

fn date(s: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| super::internal(format!("date {s:?}: {e}")))
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// `GET /api/status`
pub async fn status(st: &Arc<AppState>) -> ApiResult {
    let st2 = st.clone();
    st.read(move |c| {
        let cfg = &st2.cfg;
        let rate = state_rate(c)?;
        let ph = st2.phase();
        Ok(Reply::ok(obj! {
            "now" => st2.clock.ts(), "day" => st2.clock.today(), "trading" => ph != Phase::Closed, "phase" => ph.as_str(),
            "auction_start" => game(cfg, "auction_start"), "open" => game(cfg, "trading_open"),
            "close" => game(cfg, "trading_close"),
            "last_closed" => state(c, "last_closed")?, "launched_on" => state(c, "launched_on")?,
            "rate" => rate, "deposit_rate" => deposit_rate(cfg, rate),
            "rules" => obj! {
                "vcm_threshold" => game(cfg, "vcm_threshold"), "vcm_window_min" => game(cfg, "vcm_window_min"),
                "vcm_cool_min" => game(cfg, "vcm_cool_min"),
                "commission" => game(cfg, "commission"), "stamp_duty" => game(cfg, "stamp_duty"),
                "dividend_tax" => game(cfg, "dividend_tax"), "initial_cash" => game(cfg, "initial_cash"),
            },
        }))
    })
    .await
}

/// v1 `_week_revenue(company_id, end)`
fn week_revenue(c: &Connection, company_id: i64, end: &str) -> Result<(i64, i64), ApiError> {
    let d = date(end)?;
    let q = "SELECT COALESCE(SUM(revenue),0) FROM company_days WHERE company_id=? AND day>? AND day<=?";
    let sum = |a: NaiveDate, b: &str| -> Result<i64, ApiError> {
        c.query_row(q, rusqlite::params![company_id, iso(a), b], |r| r.get(0)).map_err(sql)
    };
    let this = sum(d - chrono::Duration::days(7), end)?;
    let prev = sum(d - chrono::Duration::days(14), &iso(d - chrono::Duration::days(7)))?;
    Ok((this, prev))
}

fn company_rows(c: &Connection, q: &str, params: impl rusqlite::Params) -> Result<Vec<Company>, ApiError> {
    let mut s = c.prepare(q).map_err(sql)?;
    let rows = s.query_map(params, crate::store::company_from_row).map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// The open IPO sell order's remaining quantity (`ipo_open["left"] if ipo_open else 0`).
fn ipo_left(c: &Connection, company_id: i64) -> Result<J, ApiError> {
    Ok(c.query_row(
        "SELECT o.qty - o.filled AS left FROM orders o JOIN accounts a ON a.id=o.account_id
         WHERE o.company_id=? AND o.status='open' AND a.kind='company'",
        [company_id],
        |r| Ok(col(r, "left")),
    )
    .optional()
    .map_err(sql)?
    .unwrap_or(J::Int(0)))
}

/// `ex.cooling(co, ts) is not None`
fn cooling(co: &Company, ts: &str) -> Option<(J, J)> {
    match co.vcm_until.as_deref() {
        Some(u) if !u.is_empty() && ts < u => Some((J::from(co.vcm_low), J::from(co.vcm_high))),
        _ => None,
    }
}

/// `GET /api/market`
pub async fn market(st: &Arc<AppState>) -> ApiResult {
    let st2 = st.clone();
    st.read(move |c| {
        let today = st2.clock.today();
        let now = st2.clock.ts();
        let last_closed = state(c, "last_closed")?.ok_or_else(|| super::internal("last_closed missing"))?;
        let mut vol: BTreeMap<i64, (J, J)> = BTreeMap::new();
        {
            let mut s = c
                .prepare("SELECT company_id, SUM(qty) v, SUM(qty*price) t FROM trades WHERE day=? GROUP BY company_id")
                .map_err(sql)?;
            let mut rows = s.query([&today]).map_err(sql)?;
            while let Some(r) = rows.next().map_err(sql)? {
                vol.insert(get_i64(r, "company_id"), (col(r, "v"), col(r, "t")));
            }
        }
        let mut out = Vec::new();
        for co in company_rows(c, "SELECT * FROM companies WHERE status='listed' ORDER BY code", [])? {
            let (this, prev) = week_revenue(c, co.id, &last_closed)?;
            let aired = c
                .query_row("SELECT aired FROM company_days WHERE company_id=? ORDER BY day DESC LIMIT 1", [co.id], |r| {
                    Ok(col(r, "aired"))
                })
                .optional()
                .map_err(sql)?;
            let spark: Vec<J> = {
                let mut s = c
                    .prepare(
                        "SELECT close FROM (SELECT day, close FROM candles WHERE company_id=? ORDER BY day DESC LIMIT 20) ORDER BY day",
                    )
                    .map_err(sql)?;
                let rows = s.query_map([co.id], |r| Ok(yuan(col(r, "close")))).map_err(sql)?;
                rows.collect::<Result<_, _>>().map_err(sql)?
            };
            let (v, t) = vol.get(&co.id).cloned().unwrap_or((J::Int(0), J::Int(0)));
            let or0 = |x: J| if x.truthy() { x } else { J::Int(0) };
            let listed = date(&co.listed_on)?;
            out.push(obj! {
                "id" => co.id, "code" => co.code.clone(), "title" => co.title.clone(), "title_ja" => co.title_ja.clone(),
                "title_en" => co.title_en.clone(),
                "price" => yuan(co.last_price), "prev_close" => yuan(co.prev_close),
                "change" => if co.prev_close != 0 { J::Float(div(co.last_price - co.prev_close, co.prev_close)) } else { J::Int(0) },
                "volume" => or0(v), "turnover" => yuan(or0(t)),
                "market_cap" => yuan(co.last_price * co.shares),
                "week_revenue" => yuan(this), "revenue_change" => if prev != 0 { J::Float(div(this - prev, prev)) } else { J::Null },
                "aired" => aired.unwrap_or(J::Int(0)), "episodes" => co.episodes, "long_runner" => co.long_runner != 0,
                "first_air" => co.first_air.clone(), "listed_on" => co.listed_on.clone(),
                "ipo_left" => ipo_left(c, co.id)?, "ipo_price" => yuan(co.ipo_price),
                "new" => (date(&today)? - listed).num_days() < 5,
                "cooling" => cooling(&co, &now).is_some(),
                "spark" => J::Arr(spark),
            });
        }
        Ok(Reply::ok(obj! {"stocks" => J::Arr(out)}))
    })
    .await
}

// ---------------------------------------------------------------- audiences (collector database)

/// Python `dict.get` on a JSON value that must be a dict (anything else raises: AttributeError).
fn pyget<'a>(v: &'a J, key: &str) -> Result<Option<&'a J>, ()> {
    match v {
        J::Obj(_) => Ok(v.get(key)),
        _ => Err(()),
    }
}

/// `x[:4]` on a list or a string.
fn slice4(v: &J) -> Result<J, ()> {
    match v {
        J::Arr(a) => Ok(J::Arr(a.iter().take(4).cloned().collect())),
        J::Str(s) => Ok(J::Str(s.chars().take(4).collect())),
        _ => Err(()),
    }
}

/// `str(x)` of a JSON id from matches.json, as the snapshot key.
fn py_str(v: &J) -> String {
    match v {
        J::Str(s) => s.clone(),
        J::Int(i) => i.to_string(),
        J::Big(s) => s.clone(),
        J::Float(f) => crate::pyfmt::repr(*f),
        J::Bool(b) => if *b { "True" } else { "False" }.into(),
        J::Null => "None".into(),
        other => other.dumps().unwrap_or_default(),
    }
}

/// v1 `_audiences`: public audience numbers from the collector. Any failure stops it and returns what
/// was gathered so far (v1 catches every exception and logs it).
fn audiences(cfg: &crate::config::Config, company_id: i64) -> J {
    let mut out = J::Obj(vec![]);
    if let Err(why) = audiences_into(cfg, company_id, &mut out) {
        eprintln!("audiences: {why}");
    }
    out
}

fn audiences_into(cfg: &crate::config::Config, company_id: i64, out: &mut J) -> Result<(), String> {
    let path = &cfg.collector_db;
    let cdb = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    let snap = |source: &str, kind: &str, id: &str| -> Result<Option<J>, String> {
        let r: Option<String> = cdb
            .query_row(
                "SELECT data FROM snapshots WHERE source=? AND kind=? AND external_id=? ORDER BY day DESC LIMIT 1",
                [source, kind, id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        r.map(|t| loads(&t).map_err(|e| format!("{e:?}"))).transpose()
    };
    let bad = |_| "not a dict".to_string();
    if let Some(m) = snap("anilist", "media", &company_id.to_string())? {
        let get = |k: &str| pyget(&m, k).map(|v| v.cloned().unwrap_or(J::Null)).map_err(bad);
        let genres = match pyget(&m, "genres").map_err(bad)? {
            None => J::Arr(vec![]),
            Some(g) => slice4(g).map_err(|_| "genres not sliceable".to_string())?,
        };
        let cover = match pyget(&m, "coverImage").map_err(bad)? {
            Some(ci) if ci.truthy() => pyget(ci, "large").map_err(bad)?.cloned().unwrap_or(J::Null),
            _ => J::Null,
        };
        out.set(
            "anilist",
            obj! {"popularity" => get("popularity")?, "score" => get("averageScore")?,
                  "next_episode" => get("nextAiringEpisode")?, "genres" => genres, "cover" => cover},
        );
    }
    let text = std::fs::read_to_string(cfg.root.join("sim").join("matches.json")).map_err(|e| e.to_string())?;
    let all = loads(&text).map_err(|e| format!("{e:?}"))?;
    let matches = pyget(&all, "matches").map_err(bad)?.ok_or("KeyError: matches")?;
    let matches = pyget(matches, &company_id.to_string()).map_err(bad)?.cloned().unwrap_or(J::Obj(vec![]));
    let mget = |k: &str| pyget(&matches, k).map(|v| v.cloned()).map_err(bad);
    if let Some(bgm) = mget("bgm")?.filter(J::truthy) {
        if let Some(s) = snap("bgm", "subject", &py_str(&bgm))? {
            let score = match pyget(&s, "rating").map_err(bad)? {
                Some(rt) if rt.truthy() => pyget(rt, "score").map_err(bad)?.cloned().unwrap_or(J::Null),
                _ => J::Null,
            };
            let get = |k: &str| pyget(&s, k).map(|v| v.cloned().unwrap_or(J::Null)).map_err(bad);
            out.set(
                "bangumi",
                obj! {"collection" => get("collection")?, "score" => score, "name_cn" => get("name_cn")?, "id" => bgm.clone()},
            );
        }
    }
    if let Some(annict) = mget("annict")?.filter(J::truthy) {
        if let Some(w) = snap("annict", "work", &py_str(&annict))? {
            let watchers = pyget(&w, "watchersCount").map_err(bad)?.cloned().unwrap_or(J::Null);
            out.set("annict", obj! {"watchers" => watchers, "id" => annict.clone()});
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- one stock

/// v1 `ipo_json(co, player_id)`
pub fn ipo_json(c: &Connection, cfg: &crate::config::Config, co: &Company, player_id: Option<i64>) -> Result<J, ApiError> {
    let float_share = match cfg.param("company.ipo_float") {
        Some(toml::Value::Float(f)) => *f,
        Some(toml::Value::Integer(i)) => *i as f64,
        _ => return Err(super::internal("company.ipo_float")),
    };
    let float_qty = (co.shares as f64 * float_share).round_ties_even() as i64;
    let base = (float_qty as f64 * cfg.game.ipo_public_base).round_ties_even() as i64;
    let (q, n, a): (i64, i64, i64) = c
        .query_row(
            "SELECT COALESCE(SUM(qty),0) q, COUNT(*) n, COALESCE(SUM(allotted),0) a FROM ipo_applications WHERE company_id=?",
            [co.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(sql)?;
    let mine = match player_id {
        Some(p) => c
            .query_row(
                "SELECT qty, allotted FROM ipo_applications WHERE company_id=? AND account_id=?",
                [co.id, p],
                |r| Ok(obj! {"qty" => col(r, "qty"), "allotted" => col(r, "allotted")}),
            )
            .optional()
            .map_err(sql)?,
        None => None,
    };
    let allotted = co.status != "subscribing";
    let over = if base != 0 { div(q, base) } else { 0.0 };
    Ok(obj! {
        "id" => co.id, "code" => co.code.clone(), "title" => co.title.clone(), "title_ja" => co.title_ja.clone(),
        "ipo_price" => yuan(co.ipo_price), "sub_start" => co.sub_start.clone(), "sub_end" => co.sub_end.clone(),
        "listed_on" => co.listed_on.clone(), "first_air" => co.first_air.clone(), "float_qty" => float_qty,
        "base_public_qty" => base,
        "applied_qty" => q, "applicants" => n, "status" => if allotted { "allotted" } else { "open" },
        "my" => mine,
        "result" => if allotted {
            obj! {"oversubscribed" => over, "public_qty" => a, "ratio" => if q != 0 { J::Float(div(a, q)) } else { J::Null }}
        } else { J::Null },
    })
}

/// v1 `ex.book(conn, company_id)`
fn book(c: &Connection, company_id: i64) -> Result<J, ApiError> {
    let side = |s: &str, order: &str| -> Result<J, ApiError> {
        let q = format!(
            "SELECT price, SUM(qty - filled) AS qty, COUNT(*) AS n FROM orders
             WHERE company_id=? AND status='open' AND side=? GROUP BY price ORDER BY price {order} LIMIT ?"
        );
        let mut st = c.prepare_cached(&q).map_err(sql)?;
        let rows = st
            .query_map(rusqlite::params![company_id, s, 5], |r| {
                Ok(obj! {"price" => yuan(col(r, "price")), "qty" => col(r, "qty"), "orders" => col(r, "n")})
            })
            .map_err(sql)?;
        Ok(J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?))
    };
    Ok(obj! {"bids" => side("buy", "DESC")?, "asks" => side("sell", "ASC")?})
}

/// v1 `ex.auction_price(conn, company_id, ref)`
fn auction_price(c: &Connection, company_id: i64, reference: i64) -> Result<(Option<i64>, i64), ApiError> {
    let levels = |s: &str| -> Result<Vec<(i64, i64)>, ApiError> {
        let mut st = c
            .prepare_cached(
                "SELECT price, SUM(qty-filled) q FROM orders WHERE company_id=? AND status='open'
                 AND side=? AND kind='limit' GROUP BY price",
            )
            .map_err(sql)?;
        let rows = st.query_map(rusqlite::params![company_id, s], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?;
        rows.collect::<Result<_, _>>().map_err(sql)
    };
    let (buys, sells) = (levels("buy")?, levels("sell")?);
    if buys.is_empty() || sells.is_empty() {
        return Ok((None, 0));
    }
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
    Ok(best.map_or((None, 0), |(_, p, v)| (Some(p), v)))
}

fn company_id_param(raw: &str) -> Result<BigInt, ApiError> {
    param_int("path", "company_id", raw).map_err(|e| ApiError::Validation(vec![e]))
}

fn one_company(c: &Connection, id: i64) -> Result<Option<Company>, ApiError> {
    c.query_row("SELECT * FROM companies WHERE id=?", [id], crate::store::company_from_row).optional().map_err(sql)
}

/// `GET /api/stocks/{company_id}`
pub async fn stock(st: &Arc<AppState>, r: &Req, raw_id: &str) -> ApiResult {
    let player = optional_player(st, r).await?;
    let id = sql_int(company_id_param(raw_id)?)?;
    let st2 = st.clone();
    st.read(move |c| {
        let cfg = &st2.cfg;
        let co = one_company(c, id)?.ok_or_else(|| http(404, "没有这支股票"))?;
        let today = st2.clock.today();
        let (lo, hi, v, tv) = c
            .query_row(
                "SELECT MIN(price) lo, MAX(price) hi, SUM(qty) v, SUM(qty*price) tv FROM trades WHERE company_id=? AND day=?",
                rusqlite::params![id, today],
                |r| Ok((col(r, "lo"), col(r, "hi"), col(r, "v"), col(r, "tv"))),
            )
            .map_err(sql)?;
        let first = c
            .query_row(
                "SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id LIMIT 1",
                rusqlite::params![id, today],
                |r| Ok(col(r, "price")),
            )
            .optional()
            .map_err(sql)?;
        let now_ts = st2.clock.ts();
        let band = cooling(&co, &now_ts);
        let auction = if st2.phase() == Phase::Auction {
            let (ap, av) = auction_price(c, id, co.prev_close)?;
            obj! {"price" => yuan(ap), "volume" => av}
        } else {
            J::Null
        };
        let trades: Vec<J> = {
            let mut s = c
                .prepare("SELECT price, qty, ts FROM trades WHERE company_id=? ORDER BY id DESC LIMIT 30")
                .map_err(sql)?;
            let rows = s
                .query_map([id], |r| Ok(obj! {"price" => yuan(col(r, "price")), "qty" => col(r, "qty"), "ts" => col(r, "ts")}))
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let weeks: Vec<J> = {
            let mut s = c
                .prepare(
                    "SELECT strftime('%Y-%W', day) wk, MIN(day) start, SUM(revenue) rev, SUM(rev_us) us, SUM(rev_cn) cn,
                     SUM(rev_jp) jp, SUM(dividend_ps) dps, MAX(aired) aired
                     FROM company_days WHERE company_id=? GROUP BY wk ORDER BY wk",
                )
                .map_err(sql)?;
            let rows = s
                .query_map([id], |r| {
                    Ok(obj! {"week" => col(r, "start"), "revenue" => yuan(col(r, "rev")), "us" => yuan(col(r, "us")),
                             "cn" => yuan(col(r, "cn")), "jp" => yuan(col(r, "jp")), "dividend_ps" => yuan(col(r, "dps")),
                             "aired" => col(r, "aired")})
                })
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let holders: Vec<(String, J)> = {
            let mut s = c
                .prepare(
                    "SELECT a.kind, SUM(h.qty) q FROM holdings h JOIN accounts a ON a.id=h.account_id
                     WHERE h.company_id=? GROUP BY a.kind",
                )
                .map_err(sql)?;
            let rows = s.query_map([id], |r| Ok((r.get::<_, String>(0)?, col(r, "q")))).map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let company_cash: Option<i64> = c
            .query_row("SELECT cash FROM accounts WHERE kind='company' AND company_id=?", [id], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        let book_value_ps = match company_cash {
            Some(_) if co.shares == 0 => return Err(super::internal("ZeroDivisionError: company has no shares")),
            Some(cash) => yuan(crate::exchange::py_round_div(cash, co.shares)),
            None => J::Null,
        };
        let news: Vec<J> = {
            let mut s = c
                .prepare("SELECT id, day, kind, title, body FROM news WHERE company_id=? ORDER BY id DESC LIMIT 20")
                .map_err(sql)?;
            let rows = s.query_map([id], news_row(&["id", "day", "kind", "title", "body"])).map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let truthy = |s: &Option<String>| s.as_deref().is_some_and(|x| !x.is_empty());
        let or0 = |x: J| if x.truthy() { x } else { J::Int(0) };
        let mut out = obj! {
            "id" => co.id, "code" => co.code.clone(), "title" => co.title.clone(), "title_ja" => co.title_ja.clone(),
            "title_en" => co.title_en.clone(), "studio" => co.studio.clone(),
            "status" => co.status.clone(), "episodes" => co.episodes, "long_runner" => co.long_runner != 0,
            "first_air" => co.first_air.clone(), "listed_on" => co.listed_on.clone(), "shares" => co.shares,
            "ipo_price" => yuan(co.ipo_price), "ipo_left" => ipo_left(c, id)?,
            "quote" => obj! {"price" => yuan(co.last_price), "prev_close" => yuan(co.prev_close),
                             "open" => first.map_or(J::Null, yuan), "high" => yuan(hi), "low" => yuan(lo),
                             "volume" => or0(v), "turnover" => yuan(or0(tv)), "open_ref" => yuan(co.open_ref)},
            "vcm" => obj! {"active" => band.is_some(),
                           "until" => if band.is_some() { J::from(co.vcm_until.clone()) } else { J::Null },
                           "low" => band.as_ref().map_or(J::Null, |b| yuan(b.0.clone())),
                           "high" => band.as_ref().map_or(J::Null, |b| yuan(b.1.clone())),
                           "count" => co.vcm_count},
            "auction" => auction,
            "book" => book(c, id)?,
            "trades" => J::Arr(trades), "weeks" => J::Arr(weeks),
            "book_value_ps" => book_value_ps,
            "holders" => J::Obj(holders),
            "audiences" => audiences(cfg, id),
            "news" => J::Arr(news),
            "ipo" => if truthy(&co.sub_start) { ipo_json(c, cfg, &co, player.as_ref().map(|p| p.id))? } else { J::Null },
        };
        if let Some(p) = &player {
            let pos = portfolio_rows(c, p.id)?.into_iter().find(|row| row.get("company_id") == Some(&J::Int(id)));
            out.set("position", pos.unwrap_or(J::Null));
            let mut s = c
                .prepare("SELECT * FROM orders WHERE account_id=? AND company_id=? AND status='open' ORDER BY id DESC")
                .map_err(sql)?;
            let rows = s.query_map([p.id, id], |r| Ok(order_json(r))).map_err(sql)?;
            out.set("my_orders", J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?));
            let avail: i64 = c
                .query_row("SELECT cash - cash_frozen FROM accounts WHERE id=?", [p.id], |r| r.get(0))
                .optional()
                .map_err(sql)?
                .unwrap_or(0);
            out.set("available_cash", yuan(avail));
        }
        Ok(Reply::ok(out))
    })
    .await
}

/// `dict(row)` for the named columns.
fn news_row(cols: &'static [&'static str]) -> impl Fn(&Row) -> rusqlite::Result<J> {
    move |r| Ok(J::Obj(cols.iter().map(|k| (k.to_string(), col(r, k))).collect()))
}

/// `GET /api/stocks/{company_id}/candles`
pub async fn candles(st: &Arc<AppState>, raw_id: &str) -> ApiResult {
    let id = sql_int(company_id_param(raw_id)?)?;
    let st2 = st.clone();
    st.read(move |c| {
        let mut rows: Vec<J> = {
            let mut s = c.prepare("SELECT * FROM candles WHERE company_id=? ORDER BY day").map_err(sql)?;
            let it = s
                .query_map([id], |r| {
                    Ok(obj! {"time" => col(r, "day"), "open" => yuan(col(r, "open")), "high" => yuan(col(r, "high")),
                             "low" => yuan(col(r, "low")), "close" => yuan(col(r, "close")), "volume" => col(r, "volume"),
                             "replay" => get_str(r, "source").as_deref() == Some("replay")})
                })
                .map_err(sql)?;
            it.collect::<Result<_, _>>().map_err(sql)?
        };
        let today = st2.clock.today();
        let (lo, hi, v, op, cl) = c
            .query_row(
                "SELECT MIN(price) lo, MAX(price) hi, SUM(qty) v,
                 (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id LIMIT 1) op,
                 (SELECT price FROM trades WHERE company_id=? AND day=? ORDER BY id DESC LIMIT 1) cl
                 FROM trades WHERE company_id=? AND day=?",
                rusqlite::params![id, today, id, today, id, today],
                |r| Ok((col(r, "lo"), col(r, "hi"), col(r, "v"), col(r, "op"), col(r, "cl"))),
            )
            .map_err(sql)?;
        let last_time = rows.last().and_then(|r| r.get("time")).and_then(|t| match t {
            J::Str(s) => Some(s.clone()),
            _ => None,
        });
        if v.truthy() && last_time.is_none_or(|t| t < today) {
            rows.push(obj! {"time" => today.clone(), "open" => yuan(op), "high" => yuan(hi), "low" => yuan(lo),
                            "close" => yuan(cl), "volume" => v, "replay" => false, "live" => true});
        }
        Ok(Reply::ok(obj! {"candles" => J::Arr(rows)}))
    })
    .await
}

/// `GET /api/stocks/{company_id}/intraday`
pub async fn intraday(st: &Arc<AppState>, raw_id: &str) -> ApiResult {
    let id = sql_int(company_id_param(raw_id)?)?;
    let st2 = st.clone();
    st.read(move |c| {
        let mut s = c
            .prepare(
                "SELECT substr(ts, 1, 16) minute, SUM(qty) v,
                 (SELECT t2.price FROM trades t2 WHERE t2.company_id=t.company_id AND substr(t2.ts,1,16)=substr(t.ts,1,16)
                  ORDER BY t2.id DESC LIMIT 1) last
                 FROM trades t WHERE company_id=? AND day=? GROUP BY minute ORDER BY minute",
            )
            .map_err(sql)?;
        let rows = s
            .query_map(rusqlite::params![id, st2.clock.today()], |r| {
                Ok(obj! {"minute" => col(r, "minute"), "price" => yuan(col(r, "last")), "volume" => col(r, "v")})
            })
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"points" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

/// `GET /api/ipos`: open subscriptions, plus allotments of the last two weeks.
pub async fn ipos(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let player = optional_player(st, r).await?;
    let st2 = st.clone();
    st.read(move |c| {
        let since = iso(date(&st2.clock.today())? - chrono::Duration::days(14));
        let rows = company_rows(
            c,
            "SELECT * FROM companies WHERE sub_start IS NOT NULL AND (status='subscribing' OR sub_end >= ?)
             ORDER BY status='subscribing' DESC, sub_end DESC",
            [since],
        )?;
        let pid = player.map(|p| p.id);
        let out = rows.iter().map(|co| ipo_json(c, &st2.cfg, co, pid)).collect::<Result<Vec<_>, _>>()?;
        Ok(Reply::ok(obj! {"ipos" => J::Arr(out)}))
    })
    .await
}

/// `GET /api/leaderboard`
pub async fn leaderboard(st: &Arc<AppState>) -> ApiResult {
    let st2 = st.clone();
    st.read(move |c| {
        let initial = match game(&st2.cfg, "initial_cash") {
            J::Int(i) => i as f64,
            J::Float(f) => f,
            _ => return Err(super::internal("initial_cash")),
        };
        let accts: Vec<(i64, J, i64)> = {
            let mut s = c.prepare("SELECT id, username, cash FROM accounts WHERE kind='player'").map_err(sql)?;
            let it = s.query_map([], |r| Ok((get_i64(r, "id"), col(r, "username"), get_i64(r, "cash")))).map_err(sql)?;
            it.collect::<Result<_, _>>().map_err(sql)?
        };
        let mut rows: Vec<(f64, J)> = Vec::new();
        for (id, username, cash) in accts {
            let stock: i64 = c
                .query_row(
                    "SELECT COALESCE(SUM(h.qty * co.last_price),0) FROM holdings h
                     JOIN companies co ON co.id=h.company_id WHERE h.account_id=?",
                    [id],
                    |r| r.get(0),
                )
                .map_err(sql)?;
            let total = cash + stock;
            let t = total as f64 / 100.0;
            rows.push((t, obj! {"username" => username, "total" => yuan(total),
                                "return_pct" => (div(total, 100) - initial) / initial}));
        }
        // rows.sort(key=lambda r: -r["total"]): stable, descending
        rows.sort_by(|a, b| (-a.0).partial_cmp(&-b.0).unwrap_or(std::cmp::Ordering::Equal));
        let out: Vec<J> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (_, mut j))| {
                j.set("rank", (i + 1) as i64);
                j
            })
            .collect();
        Ok(Reply::ok(obj! {"players" => J::Arr(out)}))
    })
    .await
}

/// `GET /api/news?limit=50`
pub async fn news(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let limit = match r.query("limit") {
        None => BigInt::Small(50),
        Some(v) => param_int("query", "limit", v).map_err(|e| ApiError::Validation(vec![e]))?,
    };
    // min(limit, 200); a huge negative limit overflows SQLite's integer (500); negative = no limit
    let limit = match limit {
        BigInt::Small(v) => v.min(200),
        BigInt::Huge(true) => 200,
        BigInt::Huge(false) => return Err(super::internal("Python int too large to convert to SQLite INTEGER")),
    };
    st.read(move |c| {
        let mut s = c
            .prepare(
                "SELECT n.id, n.day, n.ts, n.kind, n.title, n.body, n.company_id, c.code FROM news n LEFT JOIN companies c ON c.id=n.company_id
                 ORDER BY n.id DESC LIMIT ?",
            )
            .map_err(sql)?;
        let rows = s
            .query_map([limit], news_row(&["id", "day", "ts", "kind", "title", "body", "company_id", "code"]))
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"news" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

/// `GET /api/macro`
pub async fn macro_days(st: &Arc<AppState>) -> ApiResult {
    st.read(move |c| {
        let mut s = c.prepare("SELECT * FROM macro_days ORDER BY day").map_err(sql)?;
        let rows = s
            .query_map([], |r| {
                Ok(obj! {"day" => col(r, "day"), "rate" => col(r, "rate"), "m2" => yuan(col(r, "m2")),
                         "m2_growth" => col(r, "m2_growth"), "injected" => yuan(col(r, "injected")),
                         "exported" => yuan(col(r, "exported")), "taxes" => yuan(col(r, "taxes"))})
            })
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"days" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

