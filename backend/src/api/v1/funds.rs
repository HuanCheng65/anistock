//! 指数基金 (docs/design/funds.md): the funds, one fund's net values and holdings, the caller's units and requests.
//! A request is submitted any time and executes in the next session (the batches), confirmed at its close: a
//! subscription's units and a redemption's cash are known only then. Net values are 星辉 a unit; units are counted in
//! hundredths (`units: 120355` is 1,203.55 份).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{body_int, body_str, engine, json_body, need, read, Caller};
use crate::api::AppState;
use crate::engine::Engine;
use crate::funds::{fund_name, Fund, ReqKind, UNIT};

#[derive(Serialize, ToSchema, Clone)]
pub struct FundView {
    pub key: String,
    /// 基金名（跟踪的指数的简称）
    pub name: String,
    /// 跟踪的指数的全称，如「安易漫改指数」
    pub index_name: String,
    /// 开放枚举：market（市场）、source（原作）、theme（题材）
    pub group: String,
    /// 单位净值，星辉
    pub nav: f64,
    /// 净值日期；成立后第一次收盘前为 null
    pub nav_day: Option<TradingDay>,
    /// 今日估算：跟踪的指数今天的涨跌
    pub estimate: f64,
    /// 近一月净值涨跌；成立不满一个月为 null
    pub month_change: Option<f64>,
    /// 成立以来净值涨跌（从 1.0000 起）
    pub since_start: f64,
    /// 成分股数量
    pub members: i64,
    /// 单只成分股的权重上限；null 为不设上限
    pub cap: Option<f64>,
    /// 基金规模（按最新价）
    pub aum: Money,
    /// 总份额，0.01 份
    pub units: i64,
}

#[derive(Serialize, ToSchema)]
pub struct FundList {
    pub funds: Vec<FundView>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FundNavPoint {
    pub day: TradingDay,
    pub nav: f64,
    pub aum: Money,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FundHolding {
    pub stock: StockRef,
    pub quantity: i64,
    pub value: Money,
    /// 占基金资产的比例
    pub weight: f64,
    /// 在指数里的权重
    pub index_weight: f64,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FundTerms {
    /// 管理费，年化
    pub fee_rate: f64,
    /// 赎回费：持有不满 redeem_fee_days 天的份额收取
    pub redeem_fee: f64,
    pub redeem_fee_days: i64,
    pub min_subscribe: Money,
}

#[derive(Serialize, ToSchema)]
pub struct FundDetail {
    pub fund: FundView,
    /// 每个交易日收盘后的净值
    pub history: Vec<FundNavPoint>,
    /// 前十大持仓
    pub holdings: Vec<FundHolding>,
    /// 现金占基金资产的比例
    pub cash_weight: f64,
    pub terms: FundTerms,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct MyFundHolding {
    pub fund: FundView,
    /// 0.01 份
    pub units: i64,
    /// 按最新净值
    pub value: Money,
    /// 持有成本（申购金额，赎回时按份额比例扣除）
    pub cost: Money,
    /// 持有不满 7 天、赎回要收赎回费的份额，0.01 份
    pub young_units: i64,
    /// 全部份额免赎回费的日期；没有这样的份额为 null
    pub free_from: Option<TradingDay>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FundRequest {
    pub id: i64,
    pub fund: String,
    pub fund_name: String,
    /// 开放枚举：subscribe、redeem
    pub kind: String,
    /// 开放枚举：pending（等待成交）、executing（成交中）、confirmed、cancelled
    pub status: String,
    /// 申购金额；赎回为 null
    pub amount: Option<Money>,
    /// 赎回份额，0.01 份；申购在确认前为 null
    pub units: Option<i64>,
    /// 提交时所在的交易日；下一个交易日成交
    pub day: TradingDay,
    pub created_at: Timestamp,
    pub confirmed_on: Option<TradingDay>,
    /// 确认净值
    pub nav: Option<f64>,
    /// 确认金额：申购为扣除成本后投入的金额，赎回为到账金额
    pub cash: Option<Money>,
    /// 计入这笔申购或赎回的交易成本
    pub cost: Option<Money>,
    /// 赎回费（确认前为估算）
    pub fee: Option<Money>,
}

#[derive(Serialize, ToSchema)]
pub struct MyFunds {
    pub holdings: Vec<MyFundHolding>,
    /// 未结束的申购和赎回，以及最近 30 笔已结束的
    pub requests: Vec<FundRequest>,
    /// 赎回已确认、还没到账的金额
    pub owed: Money,
    pub fund_value: Money,
}

#[derive(Serialize, ToSchema)]
pub struct FundRequestResponse {
    pub request: FundRequest,
    pub funds: MyFunds,
}

fn not_found(key: &str) -> E {
    E::new(404, "fund_not_found", format!("找不到基金 {key}"))
}

/// An index's full name: 番剧综合指数, 安易 20 指数, 安易漫改指数.
fn index_title(name: &str) -> String {
    if name.ends_with("指数") {
        name.into()
    } else if name.starts_with("安易") {
        format!("{name} 指数")
    } else {
        format!("安易{name}指数")
    }
}

fn nav_of(cents: f64) -> f64 {
    (cents / 100.0 * 10_000.0).round() / 10_000.0
}

/// Engine-side view of a fund; `month_ago` is the net value (cents a unit) about a month before, from the history.
fn fund_view(e: &Engine, f: &Fund, today: &str, month_ago: Option<f64>) -> FundView {
    let def = crate::indices::def(&f.key);
    let (name, group) = fund_name(&f.key);
    let weights = e.index_weights(&f.key, today);
    let estimate: f64 = weights
        .iter()
        .filter_map(|(c, w)| {
            let co = e.company(*c)?;
            (co.prev_close > 0).then(|| w * (co.last_price as f64 / co.prev_close as f64 - 1.0))
        })
        .sum();
    let last = f.history.last();
    let aum = last.map_or(0, |d| d.aum);
    FundView {
        key: f.key.clone(),
        name: name.into(),
        index_name: def.map_or(String::new(), |d| index_title(d.name)),
        group: group.into(),
        nav: nav_of(f.nav),
        nav_day: last.map(|d| day(d.day.clone())),
        estimate: r6(estimate),
        month_change: month_ago.filter(|x| *x > 0.0).map(|x| r6(f.nav / x - 1.0)),
        since_start: r6(f.nav / crate::funds::NAV0 - 1.0),
        members: weights.len() as i64,
        cap: def.and_then(|d| d.cap),
        aum: m(aum),
        units: f.units,
    }
}

/// Each fund's net value a month before its last one (the last on or before that day), from `fund_nav`.
fn month_ago(c: &rusqlite::Connection) -> R<BTreeMap<String, f64>> {
    let mut s = c
        .prepare_cached(
            "SELECT n.fund, (SELECT nav FROM fund_nav WHERE fund=n.fund AND day <= date(n.day, '-1 month') ORDER BY day DESC LIMIT 1)
             FROM (SELECT fund, MAX(day) AS day FROM fund_nav GROUP BY fund) n",
        )
        .map_err(sql)?;
    let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?))).map_err(sql)?;
    let mut out = BTreeMap::new();
    for x in rows {
        let (k, v) = x.map_err(sql)?;
        if let Some(v) = v {
            out.insert(k, v);
        }
    }
    Ok(out)
}

fn enabled(st: &AppState) -> R<()> {
    if st.cfg.funds.enabled {
        Ok(())
    } else {
        Err(E::new(404, "funds_closed", "基金还没有开放"))
    }
}

/// 指数基金
#[utoipa::path(get, path = "/api/v1/funds", operation_id = "funds_list", tag = "funds",
    responses((status = 200, body = FundList), (status = "default", body = ErrorBody)))]
pub async fn list(State(st): State<Arc<AppState>>) -> R<Json<FundList>> {
    enabled(&st)?;
    let ago = read(&st, month_ago).await?;
    let today = st.clock.today();
    let funds = engine(&st, move |ctx| {
        let e = &ctx.engine;
        // in the order of [funds] keys (市场, 原作, 题材)
        e.funds
            .keys
            .iter()
            .filter_map(|k| e.fund(k))
            .map(|f| fund_view(e, f, &today, ago.get(&f.key).copied()))
            .collect()
    })
    .await?;
    Ok(Json(FundList { funds }))
}

/// 基金详情
#[utoipa::path(get, path = "/api/v1/funds/{key}", operation_id = "funds_detail", tag = "funds",
    params(("key" = String, Path, description = "基金，如 manga")),
    responses((status = 200, body = FundDetail), (status = "default", body = ErrorBody)))]
pub async fn detail(State(st): State<Arc<AppState>>, Path(key): Path<String>) -> R<Json<FundDetail>> {
    enabled(&st)?;
    let k = key.clone();
    let (ago, history) = read(&st, move |c| {
        let mut s = c.prepare_cached("SELECT day, nav, aum FROM fund_nav WHERE fund=? ORDER BY day").map_err(sql)?;
        let history = s
            .query_map([&k], |r| {
                Ok(FundNavPoint { day: day(r.get::<_, String>(0)?), nav: nav_of(r.get(1)?), aum: m(r.get(2)?) })
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok((month_ago(c)?, history))
    })
    .await?;
    let today = st.clock.today();
    let terms = FundTerms {
        fee_rate: st.cfg.funds.fee_rate,
        redeem_fee: st.cfg.funds.redeem_fee,
        redeem_fee_days: st.cfg.funds.redeem_fee_days,
        min_subscribe: m(st.cfg.funds.min_subscribe),
    };
    let out = engine(&st, move |ctx| -> R<FundDetail> {
        let e = &ctx.engine;
        let f = e.fund(&key).ok_or_else(|| not_found(&key))?;
        let fund = fund_view(e, f, &today, ago.get(&key).copied());
        let weights = e.index_weights(&key, &today);
        let mut held: Vec<(i64, i64, i64)> = e
            .holdings_of(f.account)
            .filter(|h| h.qty > 0)
            .map(|h| (h.company_id, h.qty, h.qty * e.company(h.company_id).map_or(0, |c| c.last_price)))
            .collect();
        let cash = e.account(f.account).map_or(0, |a| a.cash) - f.pending_cash - f.owed.values().sum::<i64>();
        let total = (held.iter().map(|x| x.2).sum::<i64>() + cash).max(1) as f64;
        held.sort_by_key(|x| (-x.2, x.0));
        let holdings = held
            .iter()
            .take(10)
            .map(|&(c, q, v)| FundHolding {
                stock: e.company(c).map_or(StockRef { id: c, code: String::new(), title: String::new() }, |co| StockRef {
                    id: co.id,
                    code: co.code.clone(),
                    title: co.title.clone(),
                }),
                quantity: q,
                value: m(v),
                weight: r6(v as f64 / total),
                index_weight: r6(weights.get(&c).copied().unwrap_or(0.0)),
            })
            .collect();
        Ok(FundDetail { fund, history, holdings, cash_weight: r6(cash.max(0) as f64 / total), terms })
    })
    .await??;
    Ok(Json(out))
}

/// The open requests of `me` (engine), as the API shows them.
fn open_requests(e: &Engine, me: i64) -> Vec<FundRequest> {
    let mut out = Vec::new();
    for f in e.funds_iter() {
        let all = f.pending.iter().map(|r| (r, "pending")).chain(f.executing().iter().map(|r| (r, "executing")));
        for (r, status) in all.filter(|(r, _)| r.account == me) {
            let (kind, amount, units, fee) = match r.kind {
                ReqKind::Subscribe(a) => ("subscribe", Some(m(a)), None, None),
                ReqKind::Redeem(u) => (
                    "redeem",
                    None,
                    Some(u),
                    Some(m((r.young as f64 / UNIT as f64 * f.nav * e.funds.redeem_fee).round() as i64)),
                ),
            };
            out.push(FundRequest {
                id: r.id,
                fund: f.key.clone(),
                fund_name: fund_name(&f.key).0.into(),
                kind: kind.into(),
                status: status.into(),
                amount,
                units,
                day: day(r.day.clone()),
                created_at: ts(r.created_at.clone()),
                confirmed_on: None,
                nav: None,
                cash: None,
                cost: None,
                fee,
            });
        }
    }
    out.sort_by_key(|r| -r.id);
    out
}

/// What the database has of `me`'s funds: the ended requests (latest 30) and the cost of the units held, per fund
/// (average cost: a redemption takes its share of the cost).
fn my_history(c: &rusqlite::Connection, me: i64) -> R<(Vec<FundRequest>, BTreeMap<String, i64>)> {
    let mut s = c
        .prepare_cached(
            "SELECT id, fund, kind, amount, status, day, created_at, confirmed_on, nav, units, cash, cost FROM fund_requests
             WHERE account_id=? AND status IN ('confirmed', 'cancelled') ORDER BY id",
        )
        .map_err(sql)?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(i64, String, String, i64, String, String, String, Option<String>, Option<f64>, Option<i64>, Option<i64>, Option<i64>)> = s
        .query_map([me], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
            ))
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    // (units held, cost) per fund
    let mut basis: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    let mut out = Vec::new();
    for (id, fund, kind, amount, status, d, created, confirmed_on, nav, units, cash, cost) in rows {
        let sub = kind == "subscribe";
        if status == "confirmed" {
            let b = basis.entry(fund.clone()).or_default();
            let u = units.unwrap_or(0);
            if sub {
                *b = (b.0 + u, b.1 + amount);
            } else if b.0 > 0 {
                let out_of = (b.1 as i128 * (-u).min(b.0) as i128 / b.0 as i128) as i64;
                *b = (b.0 + u, b.1 - out_of);
            }
        }
        // the fee: what the redemption was worth at its net value, less its cost and what it paid out
        let fee = match (sub, nav, units, cash, cost) {
            (false, Some(n), Some(u), Some(c), Some(k)) => Some(m(((-u) as f64 / UNIT as f64 * n).round() as i64 - k + c)),
            _ => None,
        };
        out.push(FundRequest {
            id,
            fund_name: fund_name(&fund).0.into(),
            fund,
            kind: kind.clone(),
            status,
            amount: sub.then(|| m(amount)),
            units: if sub { units } else { Some(amount) },
            day: day(d),
            created_at: ts(created),
            confirmed_on: confirmed_on.map(day),
            nav: nav.map(nav_of),
            cash: cash.map(|c| m(c.abs())),
            cost: cost.map(m),
            fee: fee.map(|f| m(f.0.max(0))),
        });
    }
    out.reverse();
    out.truncate(30);
    Ok((out, basis.into_iter().map(|(k, v)| (k, v.1.max(0))).collect()))
}

fn my_funds(
    e: &Engine,
    me: i64,
    today: &str,
    ago: &BTreeMap<String, f64>,
    history: Vec<FundRequest>,
    cost: &BTreeMap<String, i64>,
) -> MyFunds {
    let since = crate::vclose::day_no(today).unwrap_or(0) - e.funds.redeem_fee_days;
    let holdings = e
        .funds_iter()
        .filter(|f| f.units_of(me) > 0)
        .map(|f| {
            let units = f.units_of(me);
            let lots = f.lots.get(&me).cloned().unwrap_or_default();
            let young: Vec<&(String, i64)> =
                lots.iter().filter(|l| crate::vclose::day_no(&l.0).unwrap_or(0) > since).collect();
            let free_from = young.last().and_then(|l| {
                let d = chrono::NaiveDate::parse_from_str(&l.0, "%Y-%m-%d").ok()?;
                Some(day((d + chrono::Duration::days(e.funds.redeem_fee_days)).format("%Y-%m-%d").to_string()))
            });
            MyFundHolding {
                fund: fund_view(e, f, today, ago.get(&f.key).copied()),
                units,
                value: m((units as f64 / UNIT as f64 * f.nav).round() as i64),
                cost: m(cost.get(&f.key).copied().unwrap_or(0)),
                young_units: young.iter().map(|l| l.1).sum(),
                free_from,
            }
        })
        .collect();
    let mut requests = open_requests(e, me);
    requests.extend(history);
    MyFunds {
        holdings,
        requests,
        owed: m(e.funds_iter().map(|f| f.owed.get(&me).copied().unwrap_or(0)).sum()),
        fund_value: m(e.fund_value_of(me)),
    }
}

async fn load_mine(st: &Arc<AppState>, me: i64) -> R<MyFunds> {
    let (ago, (history, cost)) = read(st, move |c| Ok((month_ago(c)?, my_history(c, me)?))).await?;
    let today = st.clock.today();
    engine(st, move |ctx| my_funds(&ctx.engine, me, &today, &ago, history, &cost)).await
}

/// 我的基金
#[utoipa::path(get, path = "/api/v1/me/funds", operation_id = "my_funds", tag = "account",
    responses((status = 200, body = MyFunds), (status = "default", body = ErrorBody)))]
pub async fn mine(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<MyFunds>> {
    let me = caller.reader()?;
    enabled(&st)?;
    Ok(Json(load_mine(&st, me).await?))
}

/// The body of 申购 / 赎回 (documentation: the handler reads the fields itself).
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FundRequestBody {
    pub fund: String,
    /// subscribe | redeem
    pub kind: String,
    /// 申购金额，分
    pub amount: Option<Money>,
    /// 赎回份额，0.01 份
    pub units: Option<i64>,
}

/// The trading day a request counts as submitted on: before the session's requests start executing (08:50), the
/// day before, so it executes in this session.
fn request_day(e: &Engine, today: &str) -> String {
    if e.st.funds.auction_day.as_str() < today {
        crate::config::prev_day(today)
    } else {
        today.to_string()
    }
}

/// 申购或赎回
#[utoipa::path(post, path = "/api/v1/me/fund-requests", operation_id = "fund_request", tag = "account",
    request_body = FundRequestBody,
    responses((status = 201, body = FundRequestResponse), (status = "default", body = ErrorBody)))]
pub async fn submit(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<axum::response::Response> {
    let me = caller.player()?;
    enabled(&st)?;
    super::account::require_signed(&st, me).await?;
    let b = json_body(&headers, &body)?;
    let key = need(body_str(&b, "fund")?, "fund")?;
    let kind = need(body_str(&b, "kind")?, "kind")?;
    let min = st.cfg.funds.min_subscribe;
    let qty = match kind.as_str() {
        "subscribe" => {
            let a = need(body_int(&b, "amount")?, "amount")?;
            if a < min {
                return Err(error::invalid(vec![error::field(
                    "amount",
                    "too_small",
                    format!("申购金额不能低于 {} 星辉", error::coins(min).trim_end_matches(".00")),
                )]));
            }
            a
        }
        "redeem" => {
            let u = need(body_int(&b, "units")?, "units")?;
            if u < 1 {
                return Err(error::invalid(vec![error::too_small("units", 1)]));
            }
            u
        }
        _ => return Err(error::invalid(vec![error::not_allowed("kind")])),
    };
    let (today, t) = (st.clock.today(), st.clock.ts());
    let id = engine(&st, move |ctx| -> R<i64> {
        let e = &mut ctx.engine;
        if e.fund(&key).is_none() {
            return Err(not_found(&key));
        }
        let d = request_day(e, &today);
        let r = if kind == "subscribe" {
            // the margin account's collateral would leave for the fund
            if e.liabilities(me) > 0 {
                return Err(E::new(422, "margin_debt", "先了结融资融券负债才能申购基金"));
            }
            e.fund_subscribe(me, &key, qty, &d, &t)
        } else {
            e.fund_redeem(me, &key, qty, &d, &t)
        };
        let id = r.map_err(|why| match why.as_str() {
            "not enough cash" => E::new(422, "insufficient_cash", "可用资金不足"),
            "not enough units" => E::new(422, "insufficient_units", "可赎回的份额不足"),
            _ => E::new(422, "fund_request_rejected", why),
        })?;
        e.flush().map_err(error::internal)?;
        Ok(id)
    })
    .await??;
    let funds = load_mine(&st, me).await?;
    let request = funds.requests.iter().find(|r| r.id == id).cloned().ok_or_else(|| error::internal("request"))?;
    Ok(super::created(FundRequestResponse { request, funds }))
}

/// 撤销申购或赎回（开始成交前）
#[utoipa::path(delete, path = "/api/v1/me/fund-requests/{id}", operation_id = "fund_request_cancel", tag = "account",
    params(("id" = i64, Path, description = "申购或赎回的 id")),
    responses((status = 200, body = MyFunds), (status = "default", body = ErrorBody)))]
pub async fn cancel(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<MyFunds>> {
    let me = caller.player()?;
    enabled(&st)?;
    let id = super::path_id(&raw, "id")?;
    let (today, t) = (st.clock.today(), st.clock.ts());
    engine(&st, move |ctx| -> R<()> {
        let e = &mut ctx.engine;
        e.fund_cancel(me, id, &today, &t)
            .map_err(|_| E::new(404, "request_not_cancellable", "找不到这笔申购或赎回，或它已经开始成交"))?;
        e.flush().map_err(error::internal)
    })
    .await??;
    Ok(Json(load_mine(&st, me).await?))
}
