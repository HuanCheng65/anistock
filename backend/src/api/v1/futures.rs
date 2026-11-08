//! 热度期货 (docs/design/futures.md §4–5): the contracts, one contract's market, a work's published heat index, and
//! the caller's futures: positions, orders, margin. Prices are in hundredths of an index point (`IndexPrice`);
//! a contract's `tick_value` says what one hundredth of one lot is worth. The model's forecast is hidden like
//! the stocks' value: no endpoint shows it.

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
use crate::config::Phase as CPhase;
use crate::engine::Engine;
use crate::futures::trading::FutOrderReq;
use crate::futures::Contract;
use crate::model::{Kind, Side};

/// Hundredths of an index point (90.00 = 9000).
#[derive(Serialize, ToSchema, Clone, Copy, Debug, PartialEq)]
#[schema(value_type = i64)]
pub struct IndexPrice(pub i64);

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct FuturesContract {
    pub id: i64,
    pub code: String,
    pub stock: StockRef,
    /// the AniList work whose index it settles on
    pub work_id: i64,
    /// k: weeks 4k-3 to 4k after the premiere
    pub period: i64,
    pub first_day: TradingDay,
    pub last_day: TradingDay,
    /// 开放枚举：listed、expiring（等待指数）、settled
    pub status: String,
    pub last_price: IndexPrice,
    /// 前结算价
    pub prev_settle: IndexPrice,
    /// last_price / prev_settle - 1
    pub change: f64,
    pub volume_today: i64,
    pub open_interest: i64,
    /// 一手每 0.01 点的价值
    pub tick_value: Money,
    /// 一手的初始保证金
    pub initial_margin: Money,
    /// 合约期内已公布的天数和它们的平均指数
    pub days_published: i64,
    pub days_total: i64,
    pub average_so_far: Option<f64>,
    pub final_price: Option<IndexPrice>,
}

#[derive(Serialize, ToSchema)]
pub struct FuturesList {
    pub contracts: Vec<FuturesContract>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FuturesDayRow {
    pub day: TradingDay,
    pub open: Option<IndexPrice>,
    pub high: Option<IndexPrice>,
    pub low: Option<IndexPrice>,
    pub close: Option<IndexPrice>,
    pub volume: i64,
    pub settle: IndexPrice,
    /// 开放枚举：vwap（收盘前 30 分钟成交均价）、mid（做市报价中间价）、prev（沿用前结算价）、final（到期结算）
    pub settle_source: String,
    pub open_interest: i64,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct HeatDay {
    pub day: TradingDay,
    /// 安易热度指数
    pub index: f64,
    /// 各市场当天的热度（校验后）；没有数据来源的市场为 null
    pub us: Option<f64>,
    pub cn: Option<f64>,
    pub jp: Option<f64>,
    /// 市场 → missing、filled、negative、spike（补值或替换）
    pub flags: serde_json::Value,
    /// 编制方法版本
    pub version: String,
}

#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct FuturesBookLevel {
    pub price: IndexPrice,
    pub quantity: i64,
    pub orders: i64,
}

#[derive(Serialize, ToSchema)]
pub struct FuturesDetail {
    pub contract: FuturesContract,
    pub bids: Vec<FuturesBookLevel>,
    pub asks: Vec<FuturesBookLevel>,
    pub days: Vec<FuturesDayRow>,
    /// 合约期内已公布的指数
    pub index: Vec<HeatDay>,
}

#[derive(Serialize, ToSchema)]
pub struct WorkHeat {
    pub work_id: i64,
    pub days: Vec<HeatDay>,
}

#[derive(Serialize, ToSchema)]
pub struct StockHeat {
    pub works: Vec<WorkHeat>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FuturesPosition {
    pub contract: FuturesContract,
    /// 正数为多头，负数为空头
    pub quantity: i64,
    /// 开仓均价（平仓按比例扣除，日结不改变）
    pub average_price: IndexPrice,
    /// 持仓以来的盈亏（按最新价，含已经日结划转的部分）
    pub pnl: Money,
    /// 自上次结算以来的盈亏（按最新价）；结算时划转
    pub unsettled_pnl: Money,
    /// 占用的初始保证金
    pub margin: Money,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct FuturesOrder {
    pub id: i64,
    pub contract_id: i64,
    pub side: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub price: Option<IndexPrice>,
    pub quantity: i64,
    pub filled: i64,
    /// 冻结的保证金和手续费
    pub frozen: Money,
    pub status: String,
    pub created_at: Timestamp,
}

#[derive(Serialize, ToSchema)]
pub struct MyFutures {
    pub positions: Vec<FuturesPosition>,
    pub orders: Vec<FuturesOrder>,
    /// 占用的初始保证金（持仓和委托）
    pub margin_held: Money,
    /// 自上次结算以来的亏损
    pub unsettled_loss: Money,
    /// 维持保证金
    pub maintenance_margin: Money,
    /// 信用账户的维持担保比例（计入期货）；没有负债和期货时为 null
    pub maintenance_ratio: Option<f64>,
    /// 可用保证金
    pub available_margin: Money,
}

#[derive(Serialize, ToSchema)]
pub struct FuturesOrderResponse {
    pub order: FuturesOrder,
    pub filled: i64,
    pub average_price: Option<IndexPrice>,
}

pub(crate) fn contract_view(e: &Engine, c: &Contract, today: &str) -> FuturesContract {
    let f = &e.st.futures;
    let days = crate::futures::forecast::day_range(&c.first_day, &c.last_day).map_or(0, |(a, b)| b - a + 1);
    let published: Vec<f64> =
        f.heat.range((c.work_id, c.first_day.clone())..=(c.work_id, c.last_day.clone())).map(|(_, v)| *v).collect();
    let volume = f.today.get(&c.id).filter(|(d, _)| d == today).map_or(0, |(_, t)| t.iter().map(|x| x.qty).sum());
    FuturesContract {
        id: c.id,
        code: c.code.clone(),
        stock: e.company(c.company_id).map_or(
            StockRef { id: c.company_id, code: String::new(), title: String::new() },
            |co| StockRef { id: co.id, code: co.code.clone(), title: co.title.clone() },
        ),
        work_id: c.work_id,
        period: c.period,
        first_day: day(c.first_day.clone()),
        last_day: day(c.last_day.clone()),
        status: c.status.as_str().into(),
        last_price: IndexPrice(c.last_price),
        prev_settle: IndexPrice(c.prev_settle),
        change: if c.prev_settle > 0 { r6(c.last_price as f64 / c.prev_settle as f64 - 1.0) } else { 0.0 },
        volume_today: volume,
        open_interest: f.open_interest(c.id),
        tick_value: m(c.tick_value),
        initial_margin: m(e.fut_im_lot(c)),
        days_published: published.len() as i64,
        days_total: days,
        average_so_far: (!published.is_empty()).then(|| r6(published.iter().sum::<f64>() / published.len() as f64)),
        final_price: c.final_price.map(IndexPrice),
    }
}

/// A contract and its book (10 levels a side): `GET /futures/{id}` and the `futures:{id}` push.
pub(crate) fn contract_market(
    e: &Engine,
    id: i64,
    today: &str,
) -> Option<(FuturesContract, Vec<FuturesBookLevel>, Vec<FuturesBookLevel>)> {
    let c = e.st.futures.contracts.get(&id)?;
    let f = &e.st.futures;
    let view = f.book(id).depth(10, |o| f.orders[&o].left());
    let lv = |ls: Vec<crate::matching::Level>| {
        ls.into_iter().map(|l| FuturesBookLevel { price: IndexPrice(l.price), quantity: l.qty, orders: l.orders }).collect()
    };
    Some((contract_view(e, c, today), lv(view.bids), lv(view.asks)))
}

fn heat_rows(c: &rusqlite::Connection, work: i64, from: &str, to: &str) -> R<Vec<HeatDay>> {
    let exists: bool =
        c.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='heat_index'", [], |_| Ok(())).is_ok();
    if !exists {
        return Ok(vec![]);
    }
    let mut s = c
        .prepare("SELECT day, idx, us, cn, jp, flags, version FROM heat_index WHERE work_id=? AND day>=? AND day<=? ORDER BY day")
        .map_err(sql)?;
    let rows = s
        .query_map(rusqlite::params![work, from, to], |r| {
            let flags: String = r.get(5)?;
            Ok(HeatDay {
                day: day(r.get::<_, String>(0)?),
                index: r6(r.get(1)?),
                us: r.get(2)?,
                cn: r.get(3)?,
                jp: r.get(4)?,
                flags: serde_json::from_str(&flags).unwrap_or(serde_json::json!({})),
                version: r.get(6)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

fn not_found(id: i64) -> E {
    E::new(404, "contract_not_found", format!("找不到合约 {id}"))
}

/// The engine's futures rejections as API errors.
fn order_error(msg: &str) -> E {
    let code = match msg {
        "期货交易还没有开放" => "futures_closed",
        "这个合约不存在或已停止交易" => "contract_not_tradable",
        "数量必须是正整数" => "invalid_quantity",
        "价格太离谱了：超出了允许的范围" => "price_out_of_range",
        "限价单需要价格" => "price_required",
        "集合竞价阶段只接受限价单" => "auction_limit_only",
        "可用保证金不足" => "insufficient_margin",
        "期货交易需要先开通信用账户" => "credit_not_open",
        "信用账户在追保或强平中，只能平仓" => "margin_call_active",
        "信用账户已穿仓" => "credit_defaulted",
        "找不到这笔委托，或它已经结束" => "order_not_found",
        m if m.starts_with("单个合约的持仓不能超过") => "position_cap",
        m if m.starts_with("期货保证金占用不能超过") => "margin_cap",
        _ => "order_rejected",
    };
    E::new(if code == "order_not_found" { 404 } else { 422 }, code, msg.to_string())
}

fn order_view(o: &crate::futures::FutOrder) -> FuturesOrder {
    FuturesOrder {
        id: o.id,
        contract_id: o.contract_id,
        side: o.side.as_str().into(),
        kind: o.kind.as_str().into(),
        price: o.price.map(IndexPrice),
        quantity: o.qty,
        filled: o.filled,
        frozen: m(o.frozen),
        status: o.status.as_str().into(),
        created_at: ts(o.created_at.clone()),
    }
}

/// 期货合约
#[utoipa::path(get, path = "/api/v1/futures", operation_id = "futures_list", tag = "futures",
    responses((status = 200, body = FuturesList), (status = "default", body = ErrorBody)))]
pub async fn list(State(st): State<Arc<AppState>>) -> R<Json<FuturesList>> {
    let today = st.clock.today();
    let contracts = engine(&st, move |ctx| {
        let e = &ctx.engine;
        e.st.futures.contracts.values().map(|c| contract_view(e, c, &today)).collect()
    })
    .await?;
    Ok(Json(FuturesList { contracts }))
}

/// 期货合约详情
#[utoipa::path(get, path = "/api/v1/futures/{id}", operation_id = "futures_detail", tag = "futures",
    params(("id" = i64, Path, description = "合约 id")),
    responses((status = 200, body = FuturesDetail), (status = "default", body = ErrorBody)))]
pub async fn detail(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> R<Json<FuturesDetail>> {
    let id = super::path_id(&raw, "id")?;
    let today = st.clock.today();
    let (contract, bids, asks) =
        engine(&st, move |ctx| contract_market(&ctx.engine, id, &today).ok_or_else(|| not_found(id))).await??;
    let (work, first, last) = (contract.work_id, contract.first_day.0.clone(), contract.last_day.0.clone());
    let (days, index) = read(&st, move |c| {
        let mut s = c
            .prepare("SELECT day, open, high, low, close, volume, settle, settle_source, open_interest FROM futures_days WHERE contract_id=? ORDER BY day")
            .map_err(sql)?;
        let days = s
            .query_map([id], |r| {
                let p = |i: usize| -> rusqlite::Result<Option<IndexPrice>> { Ok(r.get::<_, Option<i64>>(i)?.map(IndexPrice)) };
                Ok(FuturesDayRow {
                    day: day(r.get::<_, String>(0)?),
                    open: p(1)?,
                    high: p(2)?,
                    low: p(3)?,
                    close: p(4)?,
                    volume: r.get(5)?,
                    settle: IndexPrice(r.get(6)?),
                    settle_source: r.get(7)?,
                    open_interest: r.get(8)?,
                })
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok((days, heat_rows(c, work, &first, &last)?))
    })
    .await?;
    Ok(Json(FuturesDetail { contract, bids, asks, days, index }))
}

/// 热度指数
#[utoipa::path(get, path = "/api/v1/stocks/{id}/heat", operation_id = "stock_heat", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = StockHeat), (status = "default", body = ErrorBody)))]
pub async fn heat(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> R<Json<StockHeat>> {
    let id = super::path_id(&raw, "id")?;
    let works = engine(&st, move |ctx| -> R<Vec<i64>> {
        let e = &ctx.engine;
        e.company(id).ok_or_else(|| error::stock_not_found(id))?;
        Ok(e.works_of(id))
    })
    .await??;
    let works = read(&st, move |c| {
        works
            .into_iter()
            .map(|w| Ok(WorkHeat { work_id: w, days: heat_rows(c, w, "", "9999")? }))
            .collect::<R<Vec<_>>>()
    })
    .await?;
    Ok(Json(StockHeat { works }))
}

/// 我的期货
#[utoipa::path(get, path = "/api/v1/me/futures", operation_id = "my_futures", tag = "account",
    responses((status = 200, body = MyFutures), (status = "default", body = ErrorBody)))]
pub async fn mine(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<MyFutures>> {
    let me = caller.reader()?;
    let today = st.clock.today();
    Ok(Json(engine(&st, move |ctx| my_futures(&ctx.engine, me, &today)).await?))
}

fn my_futures(e: &Engine, me: i64, today: &str) -> MyFutures {
    let f = &e.st.futures;
    let positions = (f.positions.range((me, i64::MIN)..=(me, i64::MAX)))
        .filter_map(|(_, p)| {
            let c = f.contracts.get(&p.contract_id)?;
            Some(FuturesPosition {
                contract: contract_view(e, c, today),
                quantity: p.qty,
                average_price: IndexPrice(if p.qty != 0 { crate::exchange::py_round_div(p.cost, p.qty) } else { 0 }),
                pnl: m((p.qty * c.last_price - p.cost) * c.tick_value),
                unsettled_pnl: m(p.variation(c.last_price) * c.tick_value),
                margin: m(p.margin),
            })
        })
        .collect();
    let mut orders: Vec<FuturesOrder> =
        f.by_account.get(&me).into_iter().flatten().map(|id| order_view(&f.orders[id])).collect();
    orders.sort_by_key(|o| o.id);
    let n = e.futures_numbers(me);
    let credit = e.credit_numbers(me, today);
    MyFutures {
        positions,
        orders,
        margin_held: m(n.held),
        unsettled_loss: m(n.loss),
        maintenance_margin: m(n.maintenance),
        maintenance_ratio: credit.ratio().map(r6),
        available_margin: m(credit.available_margin.max(0)),
    }
}

/// The body of 期货下单 (documentation: the handler reads the fields itself, as `/me/orders` does).
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct FuturesOrderRequest {
    pub contract_id: i64,
    /// buy | sell
    pub side: String,
    /// limit | market
    #[schema(rename = "type")]
    pub kind: String,
    /// 手
    pub quantity: i64,
    /// 限价，0.01 点；市价委托不填
    pub price: Option<IndexPrice>,
}

/// 期货下单
#[utoipa::path(post, path = "/api/v1/me/futures/orders", operation_id = "futures_place", tag = "account",
    request_body = FuturesOrderRequest,
    responses((status = 201, body = FuturesOrderResponse), (status = "default", body = ErrorBody)))]
pub async fn place(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<axum::response::Response> {
    let me = caller.player()?;
    super::account::require_signed(&st, me).await?;
    let b = json_body(&headers, &body)?;
    let contract = need(body_int(&b, "contract_id")?, "contract_id")?;
    let side = match need(body_str(&b, "side")?, "side")?.as_str() {
        "buy" => Side::Buy,
        "sell" => Side::Sell,
        _ => return Err(error::invalid(vec![error::not_allowed("side")])),
    };
    let kind = match need(body_str(&b, "type")?, "type")?.as_str() {
        "limit" => Kind::Limit,
        "market" => Kind::Market,
        _ => return Err(error::invalid(vec![error::not_allowed("type")])),
    };
    let qty = need(body_int(&b, "quantity")?, "quantity")?;
    if qty < 1 {
        return Err(error::invalid(vec![error::too_small("quantity", 1)]));
    }
    let price = body_int(&b, "price")?;
    if kind == Kind::Market && price.is_some() {
        return Err(error::invalid(vec![error::field("price", "not_allowed", "市价委托不填写价格")]));
    }
    let ph = st.phase();
    if ph == CPhase::Closed {
        return Err(E::new(422, "market_closed", "休市中"));
    }
    if !st.cfg.futures.enabled {
        return Err(order_error("期货交易还没有开放"));
    }
    // the futures steps (docs/design/futures-ui.md §4); an order that only closes a position needs none of them
    let st2 = st.clone();
    let refusal = read(&st, move |c| super::assess::futures_steps(&st2, c, me)).await?;
    let refusal = super::assess::futures_refusal(&refusal);
    let (d, t) = (st.clock.today(), st.clock.ts());
    let out = engine(&st, move |ctx| -> R<FuturesOrderResponse> {
        let e = &mut ctx.engine;
        if let Some(err) = refusal {
            let held = e.st.futures.positions.get(&(me, contract)).map_or(0, |p| p.qty);
            let closes = held != 0 && (held > 0) == (side == Side::Sell) && qty <= held.abs();
            if !closes {
                return Err(err);
            }
        }
        if ph == CPhase::Continuous && e.state("auction_day") != Some(d.as_str()) {
            e.open_market(&d, &t);
        }
        let req = FutOrderReq {
            account_id: me,
            contract_id: contract,
            side,
            kind,
            qty,
            price,
            day: &d,
            ts: &t,
            match_: ph == CPhase::Continuous,
        };
        let placed = e.futures_place(&req).map_err(|err| order_error(&err.0))?;
        e.flush().map_err(error::internal)?;
        let order = e.st.futures.orders.get(&placed.order_id).map(order_view).unwrap_or_else(|| FuturesOrder {
            id: placed.order_id,
            contract_id: contract,
            side: side.as_str().into(),
            kind: kind.as_str().into(),
            price: price.map(IndexPrice),
            quantity: qty,
            filled: placed.filled,
            frozen: m(0),
            status: placed.status.as_str().into(),
            created_at: ts(t.clone()),
        });
        Ok(FuturesOrderResponse { order, filled: placed.filled, average_price: placed.avg_price.map(IndexPrice) })
    })
    .await??;
    Ok(super::created(out))
}

/// 期货撤单
#[utoipa::path(delete, path = "/api/v1/me/futures/orders/{id}", operation_id = "futures_cancel", tag = "account",
    params(("id" = i64, Path, description = "委托 id")),
    responses((status = 200, body = MyFutures), (status = "default", body = ErrorBody)))]
pub async fn cancel(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<MyFutures>> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    let (d, t) = (st.clock.today(), st.clock.ts());
    let out = engine(&st, move |ctx| -> R<MyFutures> {
        let e = &mut ctx.engine;
        e.futures_cancel(me, id, &t).map_err(|err| order_error(&err.0))?;
        e.flush().map_err(error::internal)?;
        Ok(my_futures(e, me, &d))
    })
    .await??;
    Ok(Json(out))
}
