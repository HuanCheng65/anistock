//! Accounts (docs/api-v1.md §3.4, §4.4.3–4, §4.5, §4.6): auth, me, portfolio, asset history, orders,
//! trades, ledger, restructuring, IPO subscription, leaderboard.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Datelike;
use rusqlite::{Connection, OptionalExtension};
use serde_json::json;

use super::error::{self, sql, ErrorBody, E, R};
use super::market::{date, deposit_rate, initial_cents, iso, policy_rate, quarter_start, state, stock_refs};
use super::types::*;
use super::{body_int, body_str, engine, json_body, json_body_opt, need, read, Caller, Page, Q};
use crate::api::AppState;
use crate::auth;
use crate::config::Phase as CPhase;
use crate::engine::NewOrder;

/// The trading day of a timestamp (the date of `ts − day_rollover`).
pub fn trading_day_of(st: &AppState, t: &str) -> String {
    let (h, mi) = st.cfg.game.day_rollover.split_once(':').unwrap_or(("0", "0"));
    let mins = h.trim().parse::<i64>().unwrap_or(0) * 60 + mi.trim().parse::<i64>().unwrap_or(0);
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|dt| (dt.naive_local() - chrono::Duration::minutes(mins)).date().format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| t.get(..10).unwrap_or("").to_string())
}

fn restructure_reason(r: &crate::restructure::Refusal) -> (&'static str, String) {
    use crate::restructure::Refusal::*;
    match r {
        NotPoor(c) => ("not_poor", format!("总资产低于 {} 币才能申请重整", error::coins(*c).trim_end_matches(".00"))),
        ThisSeason => ("this_season", "本季已经重整过一次".into()),
        MarketClosed => ("market_closed", "连续交易时段才能申请重整".into()),
        Margin => ("margin_debt", "先了结融资融券负债才能申请重整".into()),
        Broken(_) => ("broken", "暂时无法申请重整".into()),
    }
}

// ---------------------------------------------------------------- me

pub fn player_return(c: &Connection, id: i64, initial: i64) -> R<(f64, i64, i64, Option<String>)> {
    let row = c
        .query_row(
            "SELECT return_factor, segment_base, restructures, last_day FROM player_returns WHERE account_id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, Some(r.get(3)?))),
        )
        .optional()
        .map_err(sql)?;
    Ok(row.unwrap_or((1.0, initial, 0, None)))
}

pub fn stock_value(c: &Connection, id: i64) -> R<i64> {
    c.query_row(
        "SELECT COALESCE(SUM(h.qty * co.last_price),0) FROM holdings h JOIN companies co ON co.id=h.company_id WHERE h.account_id=?",
        [id],
        |r| r.get(0),
    )
    .map_err(sql)
}

/// The `Me` of an account; `unauthenticated` when it is not a player.
pub fn me_of(st: &AppState, c: &Connection, id: i64) -> R<Me> {
    let row: Option<(String, i64, i64)> = c
        .query_row("SELECT username, cash, cash_frozen FROM accounts WHERE id=? AND kind='player'", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .map_err(sql)?;
    let (username, cash, frozen) = row.ok_or_else(error::unauthenticated)?;
    let stock = stock_value(c, id)?;
    let total = cash + stock;
    let initial = initial_cents(&st.cfg);
    let (factor, base, count, last_day) = player_return(c, id, initial)?;
    let today = st.clock.today();
    let eligible = crate::restructure::check(total, initial, st.cfg.game.restructure_threshold, last_day.as_deref(), &today)
        .and_then(|()| match st.phase() {
            CPhase::Continuous => Ok(()),
            _ => Err(crate::restructure::Refusal::MarketClosed),
        });
    let divs: i64 = c
        .query_row("SELECT COALESCE(SUM(amount),0) FROM ledger WHERE to_id=? AND reason='dividend'", [id], |r| r.get(0))
        .map_err(sql)?;
    let rate = policy_rate(c)?;
    let (reason, message) = match &eligible {
        Ok(()) => (None, None),
        Err(e) => {
            let (r, m) = restructure_reason(e);
            (Some(r.to_string()), Some(m))
        }
    };
    Ok(Me {
        id,
        username,
        trading_day: day(today),
        cash: m(cash),
        frozen_cash: m(frozen),
        available_cash: m(cash - frozen),
        stock_value: m(stock),
        total_assets: m(total),
        initial_cash: m(initial),
        total_return: crate::restructure::linked_return(factor, base, total),
        dividends: m(divs),
        deposit_rate: r6(deposit_rate(&st.cfg, rate)),
        restructure: RestructureState { count, eligible: eligible.is_ok(), reason, message },
    })
}

async fn load_me(st: &Arc<AppState>, id: i64) -> R<Me> {
    let st2 = st.clone();
    read(st, move |c| me_of(&st2, c, id)).await
}

/// 我的账户
#[utoipa::path(get, path = "/api/v1/me", tag = "account",
    responses((status = 200, body = Me), (status = "default", body = ErrorBody)))]
pub async fn me(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<Me>> {
    load_me(&st, caller.reader()?).await.map(Json)
}

// ---------------------------------------------------------------- portfolio

pub fn positions(c: &Connection, id: i64) -> R<Vec<Position>> {
    let mut s = c
        .prepare_cached(
            "SELECT h.company_id, h.qty, h.qty_frozen, co.code, co.title, co.last_price, co.prev_close,
               (SELECT COALESCE(SUM(qty*cost),0) FROM lots l WHERE l.account_id=h.account_id AND l.company_id=h.company_id),
               (SELECT COALESCE(SUM(amount),0) FROM ledger WHERE to_id=h.account_id AND company_id=h.company_id AND reason='dividend'),
               (SELECT MIN(acquired_on) FROM lots l WHERE l.account_id=h.account_id AND l.company_id=h.company_id)
             FROM holdings h JOIN companies co ON co.id = h.company_id
             WHERE h.account_id=? AND h.qty > 0 ORDER BY co.code",
        )
        .map_err(sql)?;
    let rows = s
        .query_map([id], |r| {
            let (qty, frozen, last, prev, cost): (i64, i64, i64, i64, i64) =
                (r.get(1)?, r.get(2)?, r.get(5)?, r.get(6)?, r.get(7)?);
            let value = qty * last;
            Ok(Position {
                stock: StockRef { id: r.get(0)?, code: r.get(3)?, title: r.get(4)? },
                quantity: qty,
                available_quantity: qty - frozen,
                average_cost: m(crate::exchange::py_round_div(cost, qty)),
                price: m(last),
                value: m(value),
                pnl: m(value - cost),
                pnl_ratio: (cost != 0).then(|| r6((value - cost) as f64 / cost as f64)),
                day_change: super::market::ratio(last, prev),
                dividends: m(r.get(8)?),
                oldest_lot_on: r.get(9)?,
            })
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// 持仓
#[utoipa::path(get, path = "/api/v1/me/portfolio", tag = "account",
    responses((status = 200, body = Portfolio), (status = "default", body = ErrorBody)))]
pub async fn portfolio(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<Portfolio>> {
    let id = caller.reader()?;
    let st2 = st.clone();
    read(&st, move |c| {
        let me = me_of(&st2, c, id)?;
        Ok(Portfolio {
            trading_day: me.trading_day.clone(),
            summary: PortfolioSummary {
                cash: me.cash,
                frozen_cash: me.frozen_cash,
                available_cash: me.available_cash,
                stock_value: me.stock_value,
                total_assets: me.total_assets,
            },
            items: positions(c, id)?,
        })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- asset history

/// 资产曲线
#[utoipa::path(get, path = "/api/v1/me/asset-history", tag = "account",
    params(("from" = Option<String>, Query, description = "交易日，含"), ("to" = Option<String>, Query, description = "交易日，含")),
    responses((status = 200, body = AssetHistory), (status = "default", body = ErrorBody)))]
pub async fn asset_history(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<AssetHistory>> {
    let id = caller.reader()?;
    let q = Q::parse(q.as_deref());
    let from = q.day("from")?.unwrap_or_else(|| "0000-01-01".into());
    let to = q.day("to")?.unwrap_or_else(|| "9999-12-31".into());
    let st2 = st.clone();
    read(&st, move |c| {
        let me = me_of(&st2, c, id)?;
        let mut items: Vec<AssetDay> = {
            let mut s = c
                .prepare_cached(
                    "SELECT day, cash, stock_value, return_index, restructured FROM player_days
                     WHERE account_id=? AND day>=? AND day<=? ORDER BY day",
                )
                .map_err(sql)?;
            let rows = s
                .query_map(rusqlite::params![id, from, to], |r| {
                    let (cash, sv, idx): (i64, i64, f64) = (r.get(1)?, r.get(2)?, r.get(3)?);
                    Ok(AssetDay {
                        trading_day: day(r.get::<_, String>(0)?),
                        cash: m(cash),
                        stock_value: m(sv),
                        total_assets: m(cash + sv),
                        return_index: r6(idx),
                        total_return: r6(idx - 1.0),
                        restructured: r.get::<_, i64>(4)? != 0,
                        is_partial: false,
                    })
                })
                .map_err(sql)?;
            rows.collect::<Result<_, _>>().map_err(sql)?
        };
        let today = st2.clock.today();
        let closed = state(c, "last_closed")?.unwrap_or_default();
        if today > closed && today >= from && today <= to && items.last().is_none_or(|l| l.trading_day.0 < today) {
            let (_, _, _, last_day) = player_return(c, id, initial_cents(&st2.cfg))?;
            items.push(AssetDay {
                trading_day: day(today.clone()),
                cash: me.cash,
                stock_value: me.stock_value,
                total_assets: me.total_assets,
                return_index: r6(me.total_return + 1.0),
                total_return: r6(me.total_return),
                restructured: last_day.as_deref() == Some(today.as_str()),
                is_partial: true,
            });
        }
        Ok(AssetHistory { items })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- orders

const ORDER_COLS: &str = "o.id, o.company_id, o.side, o.kind, o.price, o.qty, o.filled, o.frozen, o.status, o.created_at, o.updated_at,
    (SELECT SUM(t.price*t.qty) FROM trades t WHERE (t.buyer_id=o.account_id AND t.buy_order=o.id) OR (t.seller_id=o.account_id AND t.sell_order=o.id)),
    (SELECT SUM(t.qty) FROM trades t WHERE (t.buyer_id=o.account_id AND t.buy_order=o.id) OR (t.seller_id=o.account_id AND t.sell_order=o.id)),
    c.code, c.title";

pub fn order_from_row(st: &AppState, r: &rusqlite::Row) -> rusqlite::Result<Order> {
    let notional: Option<i64> = r.get(11)?;
    let fq: Option<i64> = r.get(12)?;
    let created: String = r.get(9)?;
    let side: String = r.get(2)?;
    Ok(Order {
        id: r.get(0)?,
        stock: StockRef { id: r.get(1)?, code: r.get(13)?, title: r.get(14)? },
        side: if side == "sell" { OrderSide::Sell } else { OrderSide::Buy },
        r#type: r.get(3)?,
        price: om(r.get(4)?),
        quantity: r.get(5)?,
        filled_quantity: r.get(6)?,
        average_fill_price: match (notional, fq) {
            (Some(n), Some(q)) if q > 0 => Some(m(crate::exchange::py_round_div(n, q))),
            _ => None,
        },
        frozen_cash: m(if side == "buy" { r.get(7)? } else { 0 }),
        status: r.get(8)?,
        trading_day: day(trading_day_of(st, &created)),
        created_at: ts(created),
        updated_at: ts(r.get::<_, String>(10)?),
    })
}

pub fn order_of(st: &AppState, c: &Connection, account: i64, id: i64) -> R<Option<Order>> {
    c.query_row(
        &format!("SELECT {ORDER_COLS} FROM orders o JOIN companies c ON c.id=o.company_id WHERE o.id=? AND o.account_id=?"),
        [id, account],
        |r| order_from_row(st, r),
    )
    .optional()
    .map_err(sql)
}

/// 我的委托
#[utoipa::path(get, path = "/api/v1/me/orders", tag = "account",
    params(("status" = Option<String>, Query, description = "open（默认）、closed、all"),
        ("stock_id" = Option<i64>, Query, description = "某支股票"),
        ("limit" = Option<i64>, Query, description = "默认 50，最大 200"),
        ("before" = Option<String>, Query, description = "游标"), ("after" = Option<String>, Query, description = "游标")),
    responses((status = 200, body = OrderPage), (status = "default", body = ErrorBody)))]
pub async fn orders(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<OrderPage>> {
    let id = caller.reader()?;
    let q = Q::parse(q.as_deref());
    let page = Page::parse(&q)?;
    let status = match q.get("status").unwrap_or("open") {
        "open" => "AND o.status='open'",
        "closed" => "AND o.status!='open'",
        "all" => "",
        _ => return Err(error::invalid(vec![error::not_allowed("status")])),
    };
    let stock = match q.int("stock_id", i64::MIN, i64::MAX)? {
        Some(s) => format!("AND o.company_id={s}"),
        None => String::new(),
    };
    let (cond, tail) = page.sql("o.id");
    let sqlq = format!(
        "SELECT {ORDER_COLS} FROM orders o JOIN companies c ON c.id=o.company_id WHERE o.account_id=? {status} {stock} AND {cond} {tail}"
    );
    let st2 = st.clone();
    read(&st, move |c| {
        let mut s = c.prepare(&sqlq).map_err(sql)?;
        let rows = s.query_map([id], |r| order_from_row(&st2, r)).map_err(sql)?;
        let rows: Vec<Order> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let (items, next_cursor, has_more) = page.finish(rows, |o| o.id);
        Ok(OrderPage { items, next_cursor, has_more })
    })
    .await
    .map(Json)
}

/// 一笔委托
#[utoipa::path(get, path = "/api/v1/me/orders/{id}", tag = "account",
    params(("id" = i64, Path, description = "委托 id")),
    responses((status = 200, body = Order), (status = "default", body = ErrorBody)))]
pub async fn order_one(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<Order>> {
    let me = caller.reader()?;
    let id = super::path_id(&raw, "id")?;
    let st2 = st.clone();
    read(&st, move |c| order_of(&st2, c, me, id)?.ok_or_else(|| error::order_not_found(id))).await.map(Json)
}

fn market_closed(st: &AppState) -> E {
    let g = &st.cfg.game;
    let now = st.clock.now();
    let next = crate::config::format_ts(&crate::config::next_daily_step(&now, &g.auction_start));
    E::new(
        422,
        "market_closed",
        format!("休市中（集合竞价 {}–{}，连续交易 {}）", g.auction_start, g.trading_open, g.session_text()),
    )
    .details(json!({"phase": "closed", "next_open_at": next}))
}

/// The engine's remaining rejections, by message (the checks before the engine call cover the usual ones).
fn order_error(msg: &str) -> E {
    match msg {
        "可用资金不足" => E::new(422, "insufficient_cash", "可用资金不足"),
        "可卖股数不足" => E::new(422, "insufficient_shares", "可卖数量不足"),
        "集合竞价阶段只接受限价单" => E::new(422, "auction_limit_only", "集合竞价时段只接受限价委托"),
        "限价单需要价格" => E::new(422, "price_required", "限价委托需要填写价格"),
        "这支股票不存在或已退市" => E::new(422, "stock_not_tradable", "这支股票已退市"),
        m if m.starts_with("价格太离谱了") => E::new(422, "price_out_of_range", m.replace("价格太离谱了", "价格超出范围")),
        m => E::new(422, "order_rejected", m.to_string()),
    }
}

/// 下单
#[utoipa::path(post, path = "/api/v1/me/orders", tag = "account",
    request_body = PlaceOrderRequest,
    params(("Idempotency-Key" = Option<String>, Header, description = "同一账号 24 小时内重复的 key 返回第一次的结果")),
    responses((status = 201, body = PlaceOrderResponse), (status = "default", body = ErrorBody)))]
pub async fn place(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    let me = caller.player()?;
    let key = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_string);
    let Some(k) = key else { return place_new(&st, me, &headers, &body).await.map(super::created) };
    if k.is_empty() || k.len() > 64 {
        return Err(error::invalid(vec![error::bad_format("idempotency-key")]));
    }
    // the first request with a key leaves an in-flight marker (status 0); a concurrent repeat waits for
    // its result instead of placing a second order
    let id = (me, k);
    for _ in 0..200 {
        {
            let mut idem = st.v1.idem.lock().unwrap();
            idem.retain(|_, v| v.0.elapsed().as_secs() < 86400);
            match idem.get(&id) {
                Some((_, 0, _)) => {}
                Some((_, status, bytes)) => {
                    let mut r = Response::new(axum::body::Body::from(bytes.clone()));
                    *r.status_mut() = StatusCode::from_u16(*status).unwrap_or(StatusCode::OK);
                    r.headers_mut().insert("content-type", HeaderValue::from_static("application/json"));
                    return Ok(r);
                }
                None => {
                    idem.insert(id.clone(), (std::time::Instant::now(), 0, Vec::new()));
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let res = place_new(&st, me, &headers, &body).await;
    let mut idem = st.v1.idem.lock().unwrap();
    match res {
        Ok(out) => {
            idem.insert(id, (std::time::Instant::now(), 201, serde_json::to_vec(&out).unwrap_or_default()));
            Ok(super::created(out))
        }
        Err(e) => {
            // a rejected order is not remembered: the same key may try again
            idem.remove(&id);
            Err(e)
        }
    }
}

async fn place_new(st: &Arc<AppState>, me: i64, headers: &HeaderMap, body: &[u8]) -> R<PlaceOrderResponse> {
    let b = json_body(headers, body)?;
    let stock_id = need(body_int(&b, "stock_id")?, "stock_id")?;
    let side = need(body_str(&b, "side")?, "side")?;
    if side != "buy" && side != "sell" {
        return Err(error::invalid(vec![error::not_allowed("side")]));
    }
    let kind = need(body_str(&b, "type")?, "type")?;
    if kind != "limit" && kind != "market" {
        return Err(error::invalid(vec![error::not_allowed("type")]));
    }
    let qty = need(body_int(&b, "quantity")?, "quantity")?;
    if qty < 1 {
        return Err(error::invalid(vec![error::too_small("quantity", 1)]));
    }
    if qty > super::market::ORDER_QTY_MAX {
        return Err(error::invalid(vec![error::too_large("quantity", super::market::ORDER_QTY_MAX)]));
    }
    let price = body_int(&b, "price")?;
    if kind == "market" && price.is_some() {
        return Err(error::invalid(vec![error::field("price", "not_allowed", "市价委托不填写价格")]));
    }
    if let Some(p) = price {
        if p < 1 {
            return Err(error::invalid(vec![error::field("price", "too_small", "价格需大于 0")]));
        }
    }
    let ph = st.phase();
    if ph == CPhase::Closed {
        return Err(market_closed(&st));
    }
    let (d, t) = (st.clock.today(), st.clock.ts());
    let order_id = engine(&st, move |ctx| -> R<i64> {
        let e = &mut ctx.engine;
        let co = e.company(stock_id).cloned().ok_or_else(|| error::stock_not_found(stock_id))?;
        if co.status != "listed" {
            return Err(E::new(422, "stock_not_tradable", if co.status == "delisted" { "这支股票已退市" } else { "这支股票还没有上市" })
                .details(json!({"status": co.status})));
        }
        if kind == "market" && ph == CPhase::Auction {
            return Err(E::new(422, "auction_limit_only", "集合竞价时段只接受限价委托"));
        }
        if kind == "limit" {
            let Some(p) = price else { return Err(E::new(422, "price_required", "限价委托需要填写价格")) };
            let (lo, hi) = crate::exchange::sanity_band(&co, &e.rules);
            if p > hi {
                return Err(E::new(
                    422,
                    "price_out_of_range",
                    format!("价格超出范围：不能超过昨收的 {} 倍（{} 币）", e.rules.sanity_mult, error::coins(hi)),
                )
                .details(json!({"max": hi, "min": lo})));
            }
            if side == "buy" {
                let need_cash = e.fees.buy_reserve(p, qty);
                let avail = e.available_cash(me);
                if avail < need_cash {
                    return Err(E::new(422, "insufficient_cash", format!("可用资金不足：需要 {} 币", error::coins(need_cash)))
                        .details(json!({"required": need_cash, "available": avail})));
                }
            }
        }
        if side == "sell" {
            let avail = e.available_shares(me, stock_id);
            if avail < qty {
                return Err(E::new(422, "insufficient_shares", format!("可卖数量不足：可卖 {} 股", error::group(avail)))
                    .details(json!({"requested": qty, "available": avail})));
            }
        }
        let req = NewOrder {
            account_id: me,
            company_id: stock_id,
            side: &side,
            kind: &kind,
            qty,
            price,
            day: &d,
            ts: &t,
            match_: ph == CPhase::Continuous,
        };
        let res = if ph == CPhase::Continuous { e.open_and_place(&req) } else { e.place_order(&req) };
        e.flush().map_err(error::internal)?;
        res.map(|p| p.order_id).map_err(|err| order_error(&err.0))
    })
    .await??;
    let st2 = st.clone();
    read(st, move |c| {
        let order = order_of(&st2, c, me, order_id)?.ok_or_else(|| error::internal("placed order missing"))?;
        let col = if order.side == OrderSide::Buy { "buy_order" } else { "sell_order" };
        let mut s = c.prepare(&format!("SELECT id, price, qty FROM trades WHERE {col}=? ORDER BY id")).map_err(sql)?;
        let fills = s
            .query_map([order_id], |r| Ok(Fill { trade_id: r.get(0)?, price: m(r.get(1)?), quantity: r.get(2)? }))
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)?;
        Ok(PlaceOrderResponse { order, fills })
    })
    .await
}

/// 撤单
#[utoipa::path(delete, path = "/api/v1/me/orders/{id}", tag = "account",
    params(("id" = i64, Path, description = "委托 id")),
    responses((status = 200, body = OrderEnvelope), (status = "default", body = ErrorBody)))]
pub async fn cancel(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<OrderEnvelope>> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    let st2 = st.clone();
    let current = read(&st, move |c| order_of(&st2, c, me, id)).await?.ok_or_else(|| error::order_not_found(id))?;
    if current.status != "open" {
        return Err(error::order_finished(&current.status));
    }
    let t = st.clock.ts();
    engine(&st, move |ctx| -> R<()> {
        let res = ctx.engine.cancel_order(me, id, &t);
        ctx.engine.flush().map_err(error::internal)?;
        res.map_err(|e| {
            if e.0.contains("已经结束") {
                error::order_finished("filled")
            } else {
                error::order_not_found(id)
            }
        })
    })
    .await??;
    let st2 = st.clone();
    let order = read(&st, move |c| order_of(&st2, c, me, id)).await?.ok_or_else(|| error::order_not_found(id))?;
    Ok(Json(OrderEnvelope { order }))
}

// ---------------------------------------------------------------- trades and ledger

pub fn my_trade_from_row(st: &AppState, me: i64, r: &rusqlite::Row, refs: &std::collections::HashMap<i64, StockRef>) -> rusqlite::Result<MyTrade> {
    let buyer: i64 = r.get("buyer_id")?;
    let side = if buyer == me { OrderSide::Buy } else { OrderSide::Sell };
    let (price, qty): (i64, i64) = (r.get("price")?, r.get("qty")?);
    let cid: i64 = r.get("company_id")?;
    let t: String = r.get("ts")?;
    let _ = st;
    Ok(MyTrade {
        id: r.get("id")?,
        order_id: if side == OrderSide::Buy { r.get("buy_order")? } else { r.get("sell_order")? },
        stock: refs.get(&cid).cloned().unwrap_or(StockRef { id: cid, code: String::new(), title: String::new() }),
        side,
        price: m(price),
        quantity: qty,
        amount: m(price * qty),
        commission: m(r.get::<_, Option<i64>>("commission")?.unwrap_or(0)),
        stamp_duty: m(r.get::<_, Option<i64>>("stamp")?.unwrap_or(0)),
        at: ts(t),
        trading_day: day(r.get::<_, String>("day")?),
    })
}

pub const MY_TRADE_COLS: &str = "t.*,
    (SELECT SUM(amount) FROM ledger l WHERE l.ref=t.id AND l.from_id=?1 AND l.reason='commission' AND l.company_id=t.company_id) commission,
    (SELECT SUM(amount) FROM ledger l WHERE l.ref=t.id AND l.from_id=?1 AND l.reason='stamp_duty' AND l.company_id=t.company_id) stamp";

/// 我的成交
#[utoipa::path(get, path = "/api/v1/me/trades", tag = "account",
    params(("stock_id" = Option<i64>, Query, description = "某支股票"),
        ("limit" = Option<i64>, Query, description = "默认 50，最大 200"),
        ("before" = Option<String>, Query, description = "游标"), ("after" = Option<String>, Query, description = "游标")),
    responses((status = 200, body = MyTradePage), (status = "default", body = ErrorBody)))]
pub async fn my_trades(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<MyTradePage>> {
    let me = caller.reader()?;
    let q = Q::parse(q.as_deref());
    let page = Page::parse(&q)?;
    let stock = match q.int("stock_id", i64::MIN, i64::MAX)? {
        Some(s) => format!("AND t.company_id={s}"),
        None => String::new(),
    };
    let (cond, tail) = page.sql("t.id");
    let sqlq = format!("SELECT {MY_TRADE_COLS} FROM trades t WHERE (t.buyer_id=?1 OR t.seller_id=?1) {stock} AND {cond} {tail}");
    let st2 = st.clone();
    read(&st, move |c| {
        let refs = stock_refs(c)?;
        let mut s = c.prepare(&sqlq).map_err(sql)?;
        let rows = s.query_map([me], |r| my_trade_from_row(&st2, me, r, &refs)).map_err(sql)?;
        let rows: Vec<MyTrade> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let (items, next_cursor, has_more) = page.finish(rows, |t| t.id);
        Ok(MyTradePage { items, next_cursor, has_more })
    })
    .await
    .map(Json)
}

/// 流水原因的显示标签（docs/api-v1.md §4.5.8）
pub fn reason_label(reason: &str) -> String {
    match reason {
        "grant" => "初始资金",
        "dividend" => "分红",
        "dividend_tax" => "股息税",
        "commission" => "佣金",
        "stamp_duty" => "印花税",
        "interest" => "存款利息",
        "transfer" => "财政补贴",
        "delisting" => "退市返还",
        "ipo" => "新股认购缴款",
        "ipo_refund" => "认购退款",
        "restructure" => "破产重整",
        "trade" => "成交",
        _ => "其他",
    }
    .to_string()
}

/// 资金流水
#[utoipa::path(get, path = "/api/v1/me/ledger", tag = "account",
    params(("reason" = Option<String>, Query, description = "可逗号分隔"),
        ("stock_id" = Option<i64>, Query, description = "某支股票"),
        ("trading_day" = Option<String>, Query, description = "某个交易日"),
        ("limit" = Option<i64>, Query, description = "默认 50，最大 200"),
        ("before" = Option<String>, Query, description = "游标"), ("after" = Option<String>, Query, description = "游标")),
    responses((status = 200, body = LedgerPage), (status = "default", body = ErrorBody)))]
pub async fn ledger(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<LedgerPage>> {
    let me = caller.reader()?;
    let q = Q::parse(q.as_deref());
    let page = Page::parse(&q)?;
    let mut conds = vec!["(l.to_id=?1 OR l.from_id=?1)".to_string(), "l.reason != 'trade'".to_string()];
    let mut args: Vec<String> = Vec::new();
    if let Some(rs) = q.list("reason").filter(|v| !v.is_empty()) {
        let base = 2;
        conds.push(format!("l.reason IN ({})", (0..rs.len()).map(|i| format!("?{}", base + i)).collect::<Vec<_>>().join(",")));
        args.extend(rs);
    }
    if let Some(s) = q.int("stock_id", i64::MIN, i64::MAX)? {
        conds.push(format!("l.company_id={s}"));
    }
    if let Some(d) = q.day("trading_day")? {
        conds.push(format!("l.day='{d}'"));
    }
    let (cond, tail) = page.sql("l.id");
    conds.push(cond);
    let sqlq = format!("SELECT l.* FROM ledger l WHERE {} {tail}", conds.join(" AND "));
    read(&st, move |c| {
        let refs = stock_refs(c)?;
        let mut s = c.prepare(&sqlq).map_err(sql)?;
        let mut params: Vec<rusqlite::types::Value> = vec![me.into()];
        params.extend(args.into_iter().map(rusqlite::types::Value::from));
        let rows = s
            .query_map(rusqlite::params_from_iter(params), |r| {
                let amount: i64 = r.get("amount")?;
                let to: i64 = r.get("to_id")?;
                let reason: String = r.get("reason")?;
                let cid: Option<i64> = r.get("company_id")?;
                Ok(LedgerEntry {
                    id: r.get("id")?,
                    at: ts(r.get::<_, String>("ts")?),
                    trading_day: day(r.get::<_, String>("day")?),
                    reason_label: reason_label(&reason),
                    reason,
                    amount: m(if to == me { amount } else { -amount }),
                    stock: cid.and_then(|id| refs.get(&id).cloned()),
                    r#ref: r.get("ref")?,
                })
            })
            .map_err(sql)?;
        let rows: Vec<LedgerEntry> = rows.collect::<Result<_, _>>().map_err(sql)?;
        let (items, next_cursor, has_more) = page.finish(rows, |e| e.id);
        Ok(LedgerPage { items, next_cursor, has_more })
    })
    .await
    .map(Json)
}

// ---------------------------------------------------------------- restructure

/// 破产重整
#[utoipa::path(post, path = "/api/v1/me/restructure", tag = "account",
    request_body = RestructureRequest,
    responses((status = 200, body = MeEnvelope), (status = "default", body = ErrorBody)))]
pub async fn restructure(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<MeEnvelope>> {
    let me = caller.player()?;
    let b = json_body(&headers, &body)?;
    match b.get("confirm") {
        Some(serde_json::Value::Bool(true)) => {}
        None => return Err(error::invalid(vec![error::missing("confirm")])),
        _ => return Err(error::invalid(vec![error::field("confirm", "not_allowed", "确认后才能申请重整")])),
    }
    let initial = initial_cents(&st.cfg);
    let threshold = st.cfg.game.restructure_threshold;
    let limit = crate::restructure::threshold_cents(initial, threshold);
    let refuse = |r: &crate::restructure::Refusal| {
        let (code, msg) = restructure_reason(r);
        E::new(422, "restructure_not_eligible", msg).details(json!({"reason": code, "threshold": limit}))
    };
    if st.phase() != CPhase::Continuous {
        return Err(refuse(&crate::restructure::Refusal::MarketClosed));
    }
    let (d, t) = (st.clock.today(), st.clock.ts());
    let res = engine(&st, move |ctx| {
        let e = &mut ctx.engine;
        let r = e.restructure(me, initial, threshold, &d, &t);
        let f = e.flush();
        (r.map(|_| ()), f)
    })
    .await?;
    res.1.map_err(error::internal)?;
    if let Err(r) = res.0 {
        if let crate::restructure::Refusal::Broken(why) = &r {
            return Err(error::internal(why));
        }
        return Err(refuse(&r));
    }
    Ok(Json(MeEnvelope { me: load_me(&st, me).await? }))
}

// ---------------------------------------------------------------- IPO subscription

/// 认购新股（设定数量，幂等）
#[utoipa::path(put, path = "/api/v1/ipos/{id}/subscription", tag = "ipos",
    params(("id" = i64, Path, description = "股票 id")),
    request_body = SubscriptionRequest,
    responses((status = 200, body = SubscriptionResponse), (status = "default", body = ErrorBody)))]
pub async fn subscribe(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<SubscriptionResponse>> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    let b = json_body(&headers, &body)?;
    let qty = need(body_int(&b, "quantity")?, "quantity")?;
    if qty < 1 {
        return Err(error::invalid(vec![error::too_small("quantity", 1)]));
    }
    if qty > super::market::ORDER_QTY_MAX {
        return Err(error::invalid(vec![error::too_large("quantity", super::market::ORDER_QTY_MAX)]));
    }
    let (d, t) = (st.clock.today(), st.clock.ts());
    let frozen = engine(&st, move |ctx| -> R<i64> {
        let e = &mut ctx.engine;
        let co = e.company(id).cloned().filter(|c| c.sub_start.as_deref().is_some_and(|s| !s.is_empty()));
        let co = co.ok_or_else(|| error::ipo_not_found(id))?;
        let (start, end) = (co.sub_start.clone().unwrap_or_default(), co.sub_end.clone().unwrap_or_default());
        if co.status != "subscribing" || !(start.as_str() <= d.as_str() && d.as_str() <= end.as_str()) {
            return Err(E::new(422, "ipo_not_open", format!("不在认购期（认购时间是 {start} 至 {end}）"))
                .details(json!({"sub_start": start, "sub_end": end})));
        }
        let need_cash = e.fees.buy_reserve(co.ipo_price, qty);
        let released = e.ipo_application(id, me).map_or(0, |a| a.frozen);
        let avail = e.available_cash(me) + released;
        if avail < need_cash {
            return Err(E::new(
                422,
                "insufficient_cash",
                format!("可用资金不足：认购 {} 股需要冻结 {} 币", error::group(qty), error::coins(need_cash)),
            )
            .details(json!({"required": need_cash, "available": avail})));
        }
        let res = ctx.ops.subscribe(&mut ctx.engine, me, id, qty, &d, &t);
        ctx.engine.flush().map_err(error::internal)?;
        res.map_err(|err| E::new(422, "ipo_not_open", err.0))
    })
    .await??;
    let ipo = super::stocks::load_ipo(&st, id, Some(me)).await?;
    Ok(Json(SubscriptionResponse { ipo, frozen: m(frozen) }))
}

/// 撤回认购（幂等）
#[utoipa::path(delete, path = "/api/v1/ipos/{id}/subscription", tag = "ipos",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = IpoEnvelope), (status = "default", body = ErrorBody)))]
pub async fn unsubscribe(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
) -> R<Json<IpoEnvelope>> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    engine(&st, move |ctx| -> R<()> {
        let co = ctx.engine.company(id).cloned().filter(|c| c.sub_start.as_deref().is_some_and(|s| !s.is_empty()));
        let co = co.ok_or_else(|| error::ipo_not_found(id))?;
        if co.status != "subscribing" {
            return Err(E::new(422, "ipo_closed", "认购已截止，无法撤回"));
        }
        let res = ctx.ops.unsubscribe(&mut ctx.engine, me, id);
        ctx.engine.flush().map_err(error::internal)?;
        res.map_err(|_| E::new(422, "ipo_closed", "认购已截止，无法撤回"))
    })
    .await??;
    let ipo = super::stocks::load_ipo(&st, id, Some(me)).await?;
    Ok(Json(IpoEnvelope { ipo }))
}

// ---------------------------------------------------------------- leaderboard

/// Every player's (id, username, total assets, current return index, restructures).
fn players_now(c: &Connection, initial: i64) -> R<Vec<(i64, String, i64, f64, i64)>> {
    let mut s = c
        .prepare_cached(
            "SELECT a.id, a.username, a.cash + COALESCE((SELECT SUM(h.qty*co.last_price) FROM holdings h
               JOIN companies co ON co.id=h.company_id WHERE h.account_id=a.id), 0),
               COALESCE(p.return_factor, 1.0), COALESCE(p.segment_base, ?), COALESCE(p.restructures, 0)
             FROM accounts a LEFT JOIN player_returns p ON p.account_id=a.id WHERE a.kind='player' ORDER BY a.id",
        )
        .map_err(sql)?;
    let rows = s
        .query_map([initial], |r| {
            let (total, factor, base): (i64, f64, i64) = (r.get(2)?, r.get(3)?, r.get(4)?);
            Ok((r.get(0)?, r.get(1)?, total, crate::restructure::linked_return(factor, base, total) + 1.0, r.get(5)?))
        })
        .map_err(sql)?;
    rows.collect::<Result<_, _>>().map_err(sql)
}

/// The leaderboard for `period` (`all`, `week`, `season`).
/// Every player ranked: `period_start: None` is the 总榜 (return = return index − 1); `Some(day)` divides
/// the current return index by the last `player_days` one before `day` (1.0 for players who joined since).
/// Sorted by return, ties by user id; ranks filled in.
pub fn ranked(c: &Connection, initial: i64, period_start: Option<&str>) -> R<Vec<LeaderRow>> {
    let mut rows: Vec<(f64, LeaderRow)> = Vec::new();
    for (id, username, total, idx, count) in players_now(c, initial)? {
        let ret = match period_start {
            None => idx - 1.0,
            Some(start) => {
                let base: Option<f64> = c
                    .query_row(
                        "SELECT return_index FROM player_days WHERE account_id=? AND day<? ORDER BY day DESC LIMIT 1",
                        rusqlite::params![id, start],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(sql)?;
                idx / base.filter(|b| *b > 0.0).unwrap_or(1.0) - 1.0
            }
        };
        rows.push((
            ret,
            LeaderRow { rank: 0, user: UserRef { id, username }, total_assets: m(total), r#return: r6(ret), restructures: count },
        ));
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.user.id.cmp(&b.1.user.id)));
    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut r))| {
            r.rank = i as i64 + 1;
            r
        })
        .collect())
}

/// The leaderboard for `period` (`all`, `week`, `season`).
pub fn leaderboard_of(st: &AppState, c: &Connection, period: &str, viewer: Option<i64>) -> R<Leaderboard> {
    let today = st.clock.today();
    let t = date(&today)?;
    let from_day = match period {
        "week" => Some(iso(t - chrono::Duration::days(t.weekday().num_days_from_monday() as i64))),
        "season" => Some(quarter_start(&today)),
        _ => state(c, "launched_on")?.filter(|s| !s.is_empty()),
    };
    let start = if period == "all" { None } else { from_day.as_deref() };
    let items = ranked(c, initial_cents(&st.cfg), start)?;
    let viewer = viewer.and_then(|v| items.iter().find(|r| r.user.id == v)).map(|r| LeaderViewer { rank: r.rank, r#return: r.r#return });
    Ok(Leaderboard { period: period.into(), from_day: from_day.map(day), to_day: day(today), as_of: ts(st.clock.ts()), items, viewer })
}

/// 排行榜
#[utoipa::path(get, path = "/api/v1/leaderboard", tag = "leaderboard",
    params(("period" = Option<String>, Query, description = "all（默认）、week、season")),
    responses((status = 200, body = Leaderboard), (status = "default", body = ErrorBody)))]
pub async fn leaderboard(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    RawQuery(q): RawQuery,
) -> R<Json<Leaderboard>> {
    let q = Q::parse(q.as_deref());
    let period = q.get("period").unwrap_or("all").to_string();
    if !["all", "week", "season"].contains(&period.as_str()) {
        return Err(error::invalid(vec![error::not_allowed("period")]));
    }
    let viewer = caller.viewer();
    let st2 = st.clone();
    read(&st, move |c| leaderboard_of(&st2, c, &period, viewer)).await.map(Json)
}

// ---------------------------------------------------------------- auth

fn credentials(b: &serde_json::Map<String, serde_json::Value>) -> R<(String, String)> {
    let username = need(body_str(b, "username")?, "username")?;
    let password = need(body_str(b, "password")?, "password")?;
    let name = username.trim().to_string();
    let n = name.chars().count();
    if !(2..=20).contains(&n) {
        let reason = if n < 2 { "too_short" } else { "too_long" };
        return Err(error::invalid(vec![error::field("username", reason, "用户名需为 2–20 个字符")]));
    }
    if password.chars().count() < 6 {
        return Err(error::invalid(vec![error::field("password", "too_short", "密码至少 6 位")]));
    }
    if password.chars().count() > 128 {
        return Err(error::invalid(vec![error::field("password", "too_long", "密码最多 128 位")]));
    }
    Ok((name, password))
}

fn with_cookie(mut r: Response, cookie: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(cookie) {
        r.headers_mut().insert("set-cookie", v);
    }
    r
}

/// 注册
#[utoipa::path(post, path = "/api/v1/auth/register", tag = "auth",
    request_body = RegisterRequest,
    responses((status = 201, body = Me), (status = "default", body = ErrorBody)))]
pub async fn register(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> R<Response> {
    let b = json_body(&headers, &body)?;
    let (name, password) = credentials(&b)?;
    let invite = body_str(&b, "invite")?;
    if invite.as_deref().map(str::trim) != Some(st.cfg.invite_code.as_str()) {
        return Err(error::bad_invite());
    }
    let pw_hash = tokio::task::spawn_blocking(move || auth::hash_pw(&password)).await.map_err(error::internal)?;
    let (d, t) = (st.clock.today(), st.clock.ts());
    let grant = initial_cents(&st.cfg);
    let id = engine(&st, move |ctx| -> R<i64> {
        let e = &mut ctx.engine;
        if e.accounts().any(|a| a.username.as_deref() == Some(name.as_str())) {
            return Err(error::username_taken());
        }
        e.special("world").ok_or_else(|| error::internal("no world account"))?;
        let id = e.register_player(&name, &pw_hash, grant, &d, &t);
        e.flush().map_err(error::internal)?;
        Ok(id)
    })
    .await??;
    let me = load_me(&st, id).await?;
    let token = auth::make_session(&st.secret, id, st.unix_now());
    Ok(with_cookie(super::created(me), &auth::session_cookie(&token)))
}

/// 登录
#[utoipa::path(post, path = "/api/v1/auth/login", tag = "auth",
    request_body = LoginRequest,
    responses((status = 200, body = Me), (status = "default", body = ErrorBody)))]
pub async fn login(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> R<Response> {
    let b = json_body(&headers, &body)?;
    let username = need(body_str(&b, "username")?, "username")?.trim().to_string();
    let password = need(body_str(&b, "password")?, "password")?;
    let row: Option<(i64, Option<String>)> = read(&st, move |c| {
        c.query_row("SELECT id, pw_hash FROM accounts WHERE username=? AND kind='player'", [&username], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .map_err(sql)
    })
    .await?;
    let Some((id, stored)) = row else { return Err(error::bad_credentials()) };
    let ok = tokio::task::spawn_blocking(move || auth::check_pw(&password, stored.as_deref()))
        .await
        .map_err(error::internal)?
        .unwrap_or(false);
    if !ok {
        return Err(error::bad_credentials());
    }
    let me = load_me(&st, id).await?;
    let token = auth::make_session(&st.secret, id, st.unix_now());
    Ok(with_cookie(Json(me).into_response(), &auth::session_cookie(&token)))
}

/// 退出登录
#[utoipa::path(post, path = "/api/v1/auth/logout", tag = "auth",
    responses((status = 204, description = "已退出"), (status = "default", body = ErrorBody)))]
pub async fn logout(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, headers: HeaderMap, body: Bytes) -> R<Response> {
    let _ = json_body_opt(&headers, &body);
    if let Caller::Player { id } = caller {
        if let Some(live) = &st.live {
            let _ = live.send(crate::live::Ctl::Logout { account: id });
        }
    }
    Ok(with_cookie(super::no_content(), &auth::delete_cookie(std::time::SystemTime::now())))
}
