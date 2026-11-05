//! Market-wide reads (docs/api-v1.md §4.1, §4.3): status, rules, quotes, indices, airings, macro, news.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, RawQuery, State};
use axum::Json;
use chrono::{Datelike, NaiveDate, TimeZone};
use rusqlite::{Connection, OptionalExtension};

use super::error::{self, sql, ErrorBody, R};
use super::types::*;
use super::{read, Page, Q};
use crate::api::AppState;
use crate::config::{hm, next_daily_step, Config};

// ---------------------------------------------------------------- shared helpers

pub fn date(s: &str) -> R<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| error::internal(format!("date {s:?}: {e}")))
}

pub fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn days_before(day: &str, n: i64) -> R<String> {
    Ok(iso(date(day)? - chrono::Duration::days(n)))
}

pub fn param_f64(cfg: &Config, path: &str, default: f64) -> f64 {
    match cfg.param(path) {
        Some(toml::Value::Float(f)) => *f,
        Some(toml::Value::Integer(i)) => *i as f64,
        _ => default,
    }
}

pub fn state(c: &Connection, key: &str) -> R<Option<String>> {
    c.query_row("SELECT value FROM state WHERE key=?", [key], |r| r.get::<_, String>(0)).optional().map_err(sql)
}

pub fn policy_rate(c: &Connection) -> R<f64> {
    Ok(state(c, "rate")?.and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0))
}

pub fn deposit_rate(cfg: &Config, rate: f64) -> f64 {
    (rate - param_f64(cfg, "central_bank.deposit_spread", 0.0)).max(0.0)
}

/// Start of the calendar quarter of `day` (the anime season).
pub fn quarter_start(day: &str) -> String {
    let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap_or_else(|_| NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    format!("{:04}-{:02}-01", d.year(), (d.month() - 1) / 3 * 3 + 1)
}

pub fn cover_url(id: i64) -> String {
    format!("/api/v1/stocks/{id}/cover")
}

/// `StockRef` for every company (any status).
pub fn stock_refs(c: &Connection) -> R<HashMap<i64, StockRef>> {
    let mut s = c.prepare_cached("SELECT id, code, title FROM companies").map_err(sql)?;
    let rows = s
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, StockRef { id: r.get(0)?, code: r.get(1)?, title: r.get(2)? })))
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

pub fn stock_ref(c: &Connection, id: i64) -> R<Option<StockRef>> {
    c.query_row("SELECT id, code, title FROM companies WHERE id=?", [id], |r| {
        Ok(StockRef { id: r.get(0)?, code: r.get(1)?, title: r.get(2)? })
    })
    .optional()
    .map_err(sql)
}

pub fn ratio(a: i64, b: i64) -> f64 {
    if b != 0 {
        r6(a as f64 / b as f64 - 1.0)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------- status and rules

fn phase_times(st: &AppState) -> (Option<String>, Option<Phase>, Option<String>) {
    if st.cfg.always_open {
        return (None, None, None);
    }
    let g = &st.cfg.game;
    let now = st.clock.now();
    let next = |at: &str| crate::config::format_ts(&next_daily_step(&now, at));
    let next_open = next(&g.auction_start);
    match Phase::from(st.phase()) {
        Phase::Auction => (Some(next(&g.trading_open)), Some(Phase::Continuous), Some(next_open)),
        Phase::Continuous => (Some(next(&g.trading_close)), Some(Phase::Closed), Some(next_open)),
        Phase::Closed => (Some(next_open.clone()), Some(Phase::Auction), Some(next_open)),
    }
}

pub fn market_status(st: &AppState, c: &Connection) -> R<MarketStatus> {
    let rate = policy_rate(c)?;
    let (ends, next_phase, next_open) = phase_times(st);
    Ok(MarketStatus {
        now: ts(st.clock.ts()),
        trading_day: day(st.clock.today()),
        phase: st.phase().into(),
        phase_ends_at: ends.map(ts),
        next_phase,
        next_open_at: next_open.map(ts),
        last_closed_day: state(c, "last_closed")?.filter(|s| !s.is_empty()).map(day),
        launched_on: state(c, "launched_on")?.filter(|s| !s.is_empty()).map(day),
        policy_rate: rate,
        deposit_rate: r6(deposit_rate(&st.cfg, rate)),
    })
}

/// 市场状态
#[utoipa::path(get, path = "/api/v1/market/status", tag = "market",
    responses((status = 200, body = MarketStatus), (status = "default", body = ErrorBody)))]
pub async fn status(State(st): State<Arc<AppState>>) -> R<Json<MarketStatus>> {
    let st2 = st.clone();
    read(&st, move |c| market_status(&st2, c)).await.map(Json)
}

/// 游戏规则
#[utoipa::path(get, path = "/api/v1/rules", tag = "market",
    responses((status = 200, body = Rules), (status = "default", body = ErrorBody)))]
pub async fn rules(State(st): State<Arc<AppState>>) -> Json<Rules> {
    Json(rules_of(&st.cfg))
}

pub fn initial_cents(cfg: &Config) -> i64 {
    (cfg.game.initial_cash * 100.0).round_ties_even() as i64
}

pub fn rules_of(cfg: &Config) -> Rules {
    let g = &cfg.game;
    let n = g.dividend_tax.len();
    let rules = cfg.rules();
    Rules {
        timezone: g.timezone.clone(),
        session: RulesSession {
            auction_start: g.auction_start.clone(),
            open: g.trading_open.clone(),
            close: g.trading_close.clone(),
            close_is_next_day: hm(&g.trading_close) < hm(&g.trading_open),
            day_rollover: g.day_rollover.clone(),
            daily_close: g.daily_step_at.clone(),
        },
        fees: RulesFees { commission: g.commission, stamp_duty: g.stamp_duty, stamp_duty_on: OrderSide::Sell },
        dividend_tax: g
            .dividend_tax
            .iter()
            .enumerate()
            .map(|(i, (days, rate))| DividendTaxBand {
                max_holding_days: if i + 1 == n && *days >= 10000.0 { None } else { Some(*days as i64) },
                rate: *rate,
            })
            .collect(),
        vcm: RulesVcm {
            threshold: g.vcm_threshold,
            window_minutes: g.vcm_window_min,
            cool_minutes: g.vcm_cool_min,
            later_bands: vec![0.2, 0.3],
        },
        price_limit_multiple: rules.sanity_mult,
        initial_cash: m(initial_cents(cfg)),
        restructure: RulesRestructure { threshold_ratio: g.restructure_threshold, per_season: 1 },
        ipo: RulesIpo {
            subscription_days: g.ipo_subscription_days,
            public_base: g.ipo_public_base,
            clawback: g.ipo_clawback.iter().map(|(o, s)| ClawbackBand { oversubscribed: *o, public_share: *s }).collect(),
        },
        order_quantity_max: ORDER_QTY_MAX,
    }
}

pub const ORDER_QTY_MAX: i64 = 10_000_000;

// ---------------------------------------------------------------- quotes

/// Today's per-stock trade aggregate: (open, high, low, volume, turnover).
pub type DayAgg = (i64, i64, i64, i64, i64);

pub fn day_aggs(c: &Connection, today: &str) -> R<HashMap<i64, DayAgg>> {
    let mut s = c
        .prepare_cached(
            "SELECT a.company_id, (SELECT price FROM trades WHERE id=a.first), a.hi, a.lo, a.v, a.tv FROM
               (SELECT company_id, MIN(id) first, MAX(price) hi, MIN(price) lo, SUM(qty) v, SUM(qty*price) tv
                FROM trades WHERE day=? GROUP BY company_id) a",
        )
        .map_err(sql)?;
    let rows = s
        .query_map([today], |r| Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))))
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// Ratios of every company (or one), definitions of docs/api-v1.md §4.0 `Ratios`.
pub fn ratios(c: &Connection, cfg: &Config, today: &str, only: Option<i64>) -> R<HashMap<i64, Ratios>> {
    let s365 = days_before(today, 365)?;
    let s28 = days_before(today, 28)?;
    let cost = param_f64(cfg, "company.operating_cost_ratio", 0.5);
    let tax = param_f64(cfg, "company.corp_tax", 0.25);
    let filter = match only {
        Some(id) => format!("co.id={id}"),
        None => "co.status='listed'".into(),
    };
    let q = format!(
        "SELECT co.id, co.shares, co.last_price,
           COALESCE((SELECT SUM(revenue) FROM company_days d WHERE d.company_id=co.id AND d.day>?1), 0),
           COALESCE((SELECT SUM(dividend_ps) FROM company_days d WHERE d.company_id=co.id AND d.day>?1), 0),
           COALESCE((SELECT SUM(revenue) FROM company_days d WHERE d.company_id=co.id AND d.day>?2), 0),
           COALESCE((SELECT GROUP_CONCAT(dividend_ps) FROM company_days d WHERE d.company_id=co.id AND d.day>?2 AND d.dividend_ps>0), ''),
           (SELECT cash FROM accounts a WHERE a.kind='company' AND a.company_id=co.id)
         FROM companies co WHERE {filter}"
    );
    let mut s = c.prepare(&q).map_err(sql)?;
    let rows = s
        .query_map([&s365, &s28], |r| {
            let (id, shares, price, rev, dps): (i64, i64, i64, i64, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?);
            let (rev28, divs, cash): (i64, String, Option<i64>) = (r.get(5)?, r.get(6)?, r.get(7)?);
            let divs: Vec<i64> = divs.split(',').filter_map(|x| x.parse().ok()).collect();
            let sh = shares.max(1) as f64;
            let eps = rev as f64 * (1.0 - cost) * (1.0 - tax) / sh;
            let eps28 = rev28 as f64 * 365.0 / 28.0 * (1.0 - cost) * (1.0 - tax) / sh;
            let p = price as f64;
            let dyd = if !divs.is_empty() && price > 0 {
                Some(r6(divs.iter().sum::<i64>() as f64 / divs.len() as f64 * 52.0 / p))
            } else {
                None
            };
            let bvps = match cash {
                Some(cash) if shares > 0 => Some(crate::exchange::py_round_div(cash, shares)),
                _ => None,
            };
            Ok((
                id,
                Ratios {
                    pe_ttm: (eps > 0.0 && price > 0).then(|| r6(p / eps)),
                    pe_dynamic: (eps28 > 0.0 && price > 0).then(|| r6(p / eps28)),
                    dividend_yield_ttm: (dps > 0 && price > 0).then(|| r6(dps as f64 / p)),
                    dividend_yield_dynamic: dyd,
                    pb: match cash {
                        Some(cash) if cash > 0 && shares > 0 && price > 0 => Some(r6(p / (cash as f64 / sh))),
                        _ => None,
                    },
                    book_value_per_share: bvps.map(Money),
                },
            ))
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// The stocks in quote order with everything `Quote` needs; `ids` limits the set.
pub fn load_quotes(st: &AppState, c: &Connection, ids: Option<&HashSet<i64>>) -> R<Vec<Quote>> {
    let today = st.clock.today();
    let now = st.clock.ts();
    let last_closed = state(c, "last_closed")?.unwrap_or_else(|| today.clone());
    let aggs = day_aggs(c, &today)?;
    let ratios = ratios(c, &st.cfg, &today, None)?;
    // 7-day revenue to the last close, and the 7 days before
    let (a, b) = (days_before(&last_closed, 7)?, days_before(&last_closed, 14)?);
    let mut week: HashMap<i64, (i64, i64)> = HashMap::new();
    {
        let mut s = c
            .prepare_cached(
                "SELECT company_id,
                   COALESCE(SUM(CASE WHEN day>?1 AND day<=?3 THEN revenue END),0),
                   COALESCE(SUM(CASE WHEN day>?2 AND day<=?1 THEN revenue END),0)
                 FROM company_days WHERE day>?2 AND day<=?3 GROUP BY company_id",
            )
            .map_err(sql)?;
        let mut rows = s.query([&a, &b, &last_closed]).map_err(sql)?;
        while let Some(r) = rows.next().map_err(sql)? {
            week.insert(r.get(0).map_err(sql)?, (r.get(1).map_err(sql)?, r.get(2).map_err(sql)?));
        }
    }
    let aired = latest_aired(c)?;
    let mut spark: HashMap<i64, Vec<Money>> = HashMap::new();
    {
        let mut s = c
            .prepare_cached(
                "SELECT company_id, close FROM (SELECT company_id, day, close,
                   ROW_NUMBER() OVER (PARTITION BY company_id ORDER BY day DESC) rn FROM candles)
                 WHERE rn<=20 ORDER BY company_id, day",
            )
            .map_err(sql)?;
        let mut rows = s.query([]).map_err(sql)?;
        while let Some(r) = rows.next().map_err(sql)? {
            spark.entry(r.get(0).map_err(sql)?).or_default().push(Money(r.get(1).map_err(sql)?));
        }
    }
    let ipo_left = ipo_remaining(c)?;
    let t = date(&today)?;
    let mut out = Vec::new();
    let mut s = c.prepare_cached("SELECT * FROM companies WHERE status='listed' ORDER BY code").map_err(sql)?;
    let cos = s.query_map([], crate::store::company_from_row).map_err(sql)?;
    for co in cos {
        let co = co.map_err(sql)?;
        if ids.is_some_and(|s| !s.contains(&co.id)) {
            continue;
        }
        let agg = aggs.get(&co.id);
        let (this, prev) = week.get(&co.id).copied().unwrap_or((0, 0));
        out.push(Quote {
            stock: StockRef { id: co.id, code: co.code.clone(), title: co.title.clone() },
            title_ja: co.title_ja.clone(),
            title_en: co.title_en.clone(),
            price: m(co.last_price),
            prev_close: m(co.prev_close),
            change: ratio(co.last_price, co.prev_close),
            open: agg.map(|a| m(a.0)),
            high: agg.map(|a| m(a.1)),
            low: agg.map(|a| m(a.2)),
            volume: agg.map_or(0, |a| a.3),
            turnover: m(agg.map_or(0, |a| a.4)),
            market_cap: m(co.last_price * co.shares),
            week_revenue: m(this),
            week_revenue_change: (prev != 0).then(|| r6((this - prev) as f64 / prev as f64)),
            episodes_aired: aired.get(&co.id).copied().unwrap_or(0),
            episodes: co.episodes,
            is_long_runner: co.long_runner != 0,
            first_air: co.first_air.clone(),
            listed_on: co.listed_on.clone(),
            is_new: (t - date(&co.listed_on)?).num_days() < 5,
            ipo_remaining: ipo_left.get(&co.id).copied().unwrap_or(0),
            ipo_price: m(co.ipo_price),
            vcm_active: crate::exchange::cooling(&co, &now).is_some(),
            sparkline: spark.remove(&co.id).unwrap_or_default(),
            ratios: ratios.get(&co.id).cloned().unwrap_or(Ratios {
                pe_ttm: None,
                pe_dynamic: None,
                dividend_yield_ttm: None,
                dividend_yield_dynamic: None,
                pb: None,
                book_value_per_share: None,
            }),
            cover_url: cover_url(co.id),
        });
    }
    Ok(out)
}

/// The latest `company_days.aired` per company.
pub fn latest_aired(c: &Connection) -> R<HashMap<i64, i64>> {
    let mut s = c
        .prepare_cached(
            "SELECT company_id, aired FROM (SELECT company_id, aired,
               ROW_NUMBER() OVER (PARTITION BY company_id ORDER BY day DESC) rn FROM company_days) WHERE rn=1",
        )
        .map_err(sql)?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// What is left of each company's first-day IPO sell order.
pub fn ipo_remaining(c: &Connection) -> R<HashMap<i64, i64>> {
    let mut s = c
        .prepare_cached(
            "SELECT o.company_id, SUM(o.qty - o.filled) FROM orders o JOIN accounts a ON a.id=o.account_id
             WHERE o.status='open' AND a.kind='company' GROUP BY o.company_id",
        )
        .map_err(sql)?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))).map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// 全部上市股票的行情
#[utoipa::path(get, path = "/api/v1/market/quotes", tag = "market",
    params(("ids" = Option<String>, Query, description = "逗号分隔的股票 id")),
    responses((status = 200, body = QuotesResponse), (status = "default", body = ErrorBody)))]
pub async fn quotes(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<QuotesResponse>> {
    let q = Q::parse(q.as_deref());
    let ids: Option<HashSet<i64>> = match q.list("ids") {
        None => None,
        Some(v) => Some(
            v.iter()
                .map(|s| s.parse::<i64>())
                .collect::<Result<_, _>>()
                .map_err(|_| error::invalid(vec![error::bad_format("ids")]))?,
        ),
    };
    let st2 = st.clone();
    read(&st, move |c| {
        let today = st2.clock.today();
        let items = load_quotes(&st2, c, ids.as_ref())?;
        let as_of: Option<String> =
            c.query_row("SELECT ts FROM trades ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).optional().map_err(sql)?;
        Ok(QuotesResponse {
            as_of: ts(as_of.unwrap_or_else(|| format!("{today}T{}:00+08:00", st2.cfg.game.day_rollover))),
            trading_day: day(today),
            items,
        })
    })
    .await
    .map(Json)
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

/// The three indices and the market breadth (definitions in the module doc of the old v2 read-only
/// API, kept: cap-weighted, chain-linked day by day, today's point from last prices).
pub fn compute_indices(c: &Connection, today: &str, with_series: bool) -> R<IndicesResponse> {
    let qs = quarter_start(today);
    let cos: Vec<Co> = {
        let mut s = c
            .prepare_cached(
                "SELECT id, shares, last_price, prev_close, long_runner, first_air FROM companies WHERE status='listed'",
            )
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
        rows.collect::<Result<_, _>>().map_err(sql)?
    };
    let by_id: HashMap<i64, &Co> = cos.iter().map(|co| (co.id, co)).collect();
    let mut days: BTreeMap<String, HashMap<i64, i64>> = BTreeMap::new();
    {
        let mut s = c.prepare_cached("SELECT company_id, day, close FROM candles ORDER BY day").map_err(sql)?;
        let mut rows = s.query([]).map_err(sql)?;
        while let Some(r) = rows.next().map_err(sql)? {
            let id: i64 = r.get(0).map_err(sql)?;
            if by_id.contains_key(&id) {
                days.entry(r.get(1).map_err(sql)?).or_default().insert(id, r.get(2).map_err(sql)?);
            }
        }
    }
    type Member<'a> = Box<dyn Fn(&Co) -> bool + 'a>;
    let defs: [(&str, &str, Member); 3] = [
        ("all", "番剧综合指数", Box::new(|_| true)),
        ("season", "本季新番", Box::new(|co: &Co| !co.long && co.first_air.as_str() >= qs.as_str())),
        ("long", "长篇连载", Box::new(|co: &Co| co.long)),
    ];
    let r4 = |v: f64| (v * 10000.0).round() / 10000.0;
    let mut items = Vec::new();
    for (key, name, member) in defs.iter() {
        let members: HashSet<i64> = cos.iter().filter(|co| member(co)).map(|co| co.id).collect();
        let mut series: Vec<(String, f64)> = Vec::new();
        let mut prev: Option<&HashMap<i64, i64>> = None;
        let mut level = 1000.0;
        for (d, closes) in &days {
            if !closes.keys().any(|id| members.contains(id)) {
                continue;
            }
            if let Some(p) = prev {
                let (mut num, mut den) = (0i128, 0i128);
                for (id, close) in closes {
                    if let (true, Some(pc)) = (members.contains(id), p.get(id)) {
                        let sh = by_id[id].shares as i128;
                        num += *close as i128 * sh;
                        den += *pc as i128 * sh;
                    }
                }
                if den > 0 {
                    level *= num as f64 / den as f64;
                }
            }
            series.push((d.clone(), level));
            prev = Some(closes);
        }
        if series.is_empty() {
            continue;
        }
        let last_day = series.last().map(|s| s.0.clone()).unwrap_or_default();
        if last_day.as_str() < today {
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
                series.push((today.to_string(), level * num as f64 / den as f64));
            }
        }
        let n = series.len();
        let value = series[n - 1].1;
        let prev_v = if n >= 2 { series[n - 2].1 } else { value };
        items.push(MarketIndex {
            key: key.to_string(),
            name: name.to_string(),
            members: members.len() as i64,
            base_day: day(series[0].0.clone()),
            value: r4(value),
            prev_close: r4(prev_v),
            change: if prev_v > 0.0 { r6(value / prev_v - 1.0) } else { 0.0 },
            series: if with_series {
                series.iter().map(|(d, v)| IndexPoint { trading_day: day(d.clone()), value: r4(*v) }).collect()
            } else {
                vec![]
            },
        });
    }
    let (mut up, mut flat, mut down) = (0, 0, 0);
    for co in &cos {
        match co.last.cmp(&co.prev) {
            std::cmp::Ordering::Greater => up += 1,
            std::cmp::Ordering::Less => down += 1,
            std::cmp::Ordering::Equal => flat += 1,
        }
    }
    Ok(IndicesResponse { trading_day: day(today), season_start: qs.clone(), breadth: Breadth { up, flat, down }, items })
}

/// 指数与涨跌家数
#[utoipa::path(get, path = "/api/v1/market/indices", tag = "market",
    params(("series" = Option<bool>, Query, description = "false 时不带走势")),
    responses((status = 200, body = IndicesResponse), (status = "default", body = ErrorBody)))]
pub async fn indices(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<IndicesResponse>> {
    let with_series = Q::parse(q.as_deref()).bool("series", true)?;
    let today = st.clock.today();
    read(&st, move |c| compute_indices(c, &today, with_series)).await.map(Json)
}

// ---------------------------------------------------------------- airings (collector snapshots)

pub fn collector(cfg: &Config) -> Option<Connection> {
    Connection::open_with_flags(
        &cfg.collector_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

pub fn snapshot(cdb: &Connection, source: &str, kind: &str, id: &str) -> Option<serde_json::Value> {
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

/// Next airing (episode, unix seconds) per company from the latest AniList snapshot, cached 10 minutes.
pub fn next_airings(cfg: &Config, ids: &[i64]) -> HashMap<i64, (i64, i64)> {
    static CACHE: OnceLock<Mutex<Option<(Instant, HashMap<i64, Option<(i64, i64)>>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    if let Some((at, m)) = cache.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(600) && ids.iter().all(|i| m.contains_key(i)) {
            return m.iter().filter_map(|(k, v)| v.map(|v| (*k, v))).collect();
        }
    }
    let mut all: HashMap<i64, Option<(i64, i64)>> = HashMap::new();
    if let Some(cdb) = collector(cfg) {
        for id in ids {
            let next = snapshot(&cdb, "anilist", "media", &id.to_string())
                .and_then(|m| m.get("nextAiringEpisode").cloned())
                .and_then(|n| Some((n.get("episode")?.as_i64()?, n.get("airingAt")?.as_i64()?)));
            all.insert(*id, next);
        }
    }
    *cache.lock().unwrap() = Some((Instant::now(), all.clone()));
    all.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()
}

/// Start (inclusive) and end of a trading day as unix seconds: rollover to rollover.
pub fn trading_day_window(st: &AppState, d: &str) -> R<(i64, i64)> {
    let start = date(d)?.and_time(hm(&st.cfg.game.day_rollover));
    let s = st.clock.tz.from_local_datetime(&start).earliest().ok_or_else(|| error::internal("bad local time"))?;
    Ok((s.timestamp(), s.timestamp() + 86400))
}

pub fn unix_ts(st: &AppState, t: i64) -> String {
    st.clock.tz.timestamp_opt(t, 0).single().map(|d| crate::config::format_ts(&d)).unwrap_or_default()
}

/// 本交易日要播的集数
#[utoipa::path(get, path = "/api/v1/market/airings", tag = "market",
    params(("trading_day" = Option<String>, Query, description = "本交易日（默认）或下一个交易日")),
    responses((status = 200, body = AiringsResponse), (status = "default", body = ErrorBody)))]
pub async fn airings(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<AiringsResponse>> {
    let q = Q::parse(q.as_deref());
    let today = st.clock.today();
    let tomorrow = iso(date(&today)? + chrono::Duration::days(1));
    let d = q.day("trading_day")?.unwrap_or_else(|| today.clone());
    if d != today && d != tomorrow {
        return Err(error::invalid(vec![error::field("trading_day", "not_allowed", "只能查询本交易日和下一个交易日")]));
    }
    let st2 = st.clone();
    read(&st, move |c| {
        let (start, end) = trading_day_window(&st2, &d)?;
        let now = st2.clock.now().timestamp();
        let refs = stock_refs(c)?;
        let ids: Vec<i64> = {
            let mut s = c.prepare_cached("SELECT id FROM companies WHERE status IN ('listed','subscribing')").map_err(sql)?;
            let rows = s.query_map([], |r| r.get(0)).map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let next = next_airings(&st2.cfg, &ids);
        let mut items: Vec<(i64, i64, i64)> = ids
            .iter()
            .filter_map(|id| next.get(id).map(|(ep, at)| (*at, *id, *ep)))
            .filter(|(at, _, _)| *at >= start && *at < end)
            .collect();
        items.sort();
        Ok(AiringsResponse {
            trading_day: day(d),
            from: ts(unix_ts(&st2, start)),
            to: ts(unix_ts(&st2, end)),
            items: items
                .into_iter()
                .filter_map(|(at, id, ep)| {
                    Some(Airing { stock: refs.get(&id)?.clone(), episode: ep, airs_at: ts(unix_ts(&st2, at)), has_aired: at <= now })
                })
                .collect(),
        })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- macro

/// 宏观数据，一天一行
#[utoipa::path(get, path = "/api/v1/market/macro", tag = "market",
    params(("from" = Option<String>, Query, description = "交易日，含"), ("to" = Option<String>, Query, description = "交易日，含")),
    responses((status = 200, body = MacroResponse), (status = "default", body = ErrorBody)))]
pub async fn macro_days(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<MacroResponse>> {
    let q = Q::parse(q.as_deref());
    let today = st.clock.today();
    let from = match q.day("from")? {
        Some(d) => d,
        None => days_before(&today, 90)?,
    };
    let to = q.day("to")?.unwrap_or_else(|| "9999-12-31".into());
    let st2 = st.clone();
    read(&st, move |c| {
        let base = param_f64(&st2.cfg, "revenue.demand_base_week", 0.0) * 100.0;
        let mut rows: BTreeMap<String, MacroRow> = BTreeMap::new();
        let blank = |d: &str| MacroRow {
            trading_day: day(d),
            policy_rate: None,
            m2: None,
            m2_growth: None,
            injected: None,
            exported: None,
            taxes: None,
            interest: None,
            revenue_7d: None,
            demand_index: None,
        };
        {
            let mut s = c
                .prepare("SELECT day, rate, m2, m2_growth, injected, exported, taxes, interest FROM macro_days WHERE day>=? AND day<=?")
                .map_err(sql)?;
            let mut it = s.query([&from, &to]).map_err(sql)?;
            while let Some(r) = it.next().map_err(sql)? {
                let d: String = r.get(0).map_err(sql)?;
                let mut row = blank(&d);
                row.policy_rate = r.get(1).map_err(sql)?;
                row.m2 = om(r.get(2).map_err(sql)?);
                row.m2_growth = r.get::<_, Option<f64>>(3).map_err(sql)?.map(r6);
                row.injected = om(r.get(4).map_err(sql)?);
                row.exported = om(r.get(5).map_err(sql)?);
                row.taxes = om(r.get(6).map_err(sql)?);
                row.interest = om(r.get(7).map_err(sql)?);
                rows.insert(d, row);
            }
        }
        let mut s = c.prepare("SELECT day, SUM(revenue) FROM company_days GROUP BY day ORDER BY day").map_err(sql)?;
        let days: Vec<(String, i64)> = s
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)?;
        for i in 6..days.len() {
            let d = &days[i].0;
            if d.as_str() < from.as_str() || d.as_str() > to.as_str() {
                continue;
            }
            let sum: i64 = days[i - 6..=i].iter().map(|d| d.1).sum();
            let row = rows.entry(d.clone()).or_insert_with(|| blank(d));
            row.revenue_7d = Some(Money(sum));
            row.demand_index = (base > 0.0).then(|| r6(sum as f64 / base));
        }
        Ok(MacroResponse { items: rows.into_values().collect() })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- news

pub fn news_from_row(r: &rusqlite::Row, refs: &HashMap<i64, StockRef>) -> rusqlite::Result<NewsItem> {
    let kind: String = r.get("kind")?;
    let cid: Option<i64> = r.get("company_id")?;
    Ok(NewsItem {
        id: r.get("id")?,
        at: ts(r.get::<_, String>("ts")?),
        trading_day: day(r.get::<_, String>("day")?),
        category: NewsCategory::of(&kind),
        kind,
        title: r.get("title")?,
        body: r.get::<_, Option<String>>("body")?.filter(|b| !b.is_empty()),
        stock: cid.and_then(|id| refs.get(&id).cloned()),
    })
}

/// 资讯，游标分页
#[utoipa::path(get, path = "/api/v1/news", tag = "news",
    params(
        ("stock_id" = Option<i64>, Query, description = "某支股票的资讯"),
        ("category" = Option<String>, Query, description = "flash、notice、airing、close，可逗号分隔"),
        ("kind" = Option<String>, Query, description = "具体类型，可逗号分隔"),
        ("trading_day" = Option<String>, Query, description = "某个交易日"),
        ("limit" = Option<i64>, Query, description = "默认 50，最大 200"),
        ("before" = Option<String>, Query, description = "游标：更旧的一页"),
        ("after" = Option<String>, Query, description = "游标：比它更新的，从旧到新"),
    ),
    responses((status = 200, body = NewsPage), (status = "default", body = ErrorBody)))]
pub async fn news(State(st): State<Arc<AppState>>, RawQuery(q): RawQuery) -> R<Json<NewsPage>> {
    let q = Q::parse(q.as_deref());
    let page = Page::parse(&q)?;
    let mut conds: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();
    if let Some(id) = q.int("stock_id", i64::MIN, i64::MAX)? {
        conds.push(format!("company_id={id}"));
    }
    if let Some(cats) = q.list("category") {
        let mut parts = Vec::new();
        for cname in cats {
            let cat = NewsCategory::parse(&cname).ok_or_else(|| error::invalid(vec![error::not_allowed("category")]))?;
            if cat == NewsCategory::Notice {
                let known: Vec<String> = [NewsCategory::Flash, NewsCategory::Airing, NewsCategory::Close]
                    .iter()
                    .flat_map(|c| c.kinds().iter().map(|k| format!("'{k}'")))
                    .collect();
                parts.push(format!("kind NOT IN ({})", known.join(",")));
            } else {
                let ks: Vec<String> = cat.kinds().iter().map(|k| format!("'{k}'")).collect();
                parts.push(format!("kind IN ({})", ks.join(",")));
            }
        }
        if !parts.is_empty() {
            conds.push(format!("({})", parts.join(" OR ")));
        }
    }
    if let Some(kinds) = q.list("kind") {
        if !kinds.is_empty() {
            conds.push(format!("kind IN ({})", vec!["?"; kinds.len()].join(",")));
            args.extend(kinds);
        }
    }
    if let Some(d) = q.day("trading_day")? {
        conds.push("day=?".into());
        args.push(d);
    }
    let (idc, tail) = page.sql("id");
    conds.push(idc);
    let sqlq = format!("SELECT * FROM news WHERE {} {tail}", conds.join(" AND "));
    read(&st, move |c| {
        let refs = stock_refs(c)?;
        let mut s = c.prepare(&sqlq).map_err(sql)?;
        let rows = s.query_map(rusqlite::params_from_iter(args.iter()), |r| news_from_row(r, &refs)).map_err(sql)?;
        let rows: Vec<NewsItem> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let (items, next_cursor, has_more) = page.finish(rows, |n| n.id);
        Ok(NewsPage { items, next_cursor, has_more })
    })
    .await
    .map(Json)
}

/// 一条资讯
#[utoipa::path(get, path = "/api/v1/news/{id}", tag = "news",
    params(("id" = i64, Path, description = "资讯 id")),
    responses((status = 200, body = NewsItem), (status = "default", body = ErrorBody)))]
pub async fn news_one(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> R<Json<NewsItem>> {
    let id = super::path_id(&raw, "id")?;
    read(&st, move |c| {
        let refs = stock_refs(c)?;
        c.query_row("SELECT * FROM news WHERE id=?", [id], |r| news_from_row(r, &refs))
            .optional()
            .map_err(sql)?
            .ok_or_else(|| error::news_not_found(id))
    })
    .await
    .map(Json)
}
