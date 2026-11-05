//! Orders and IPO subscriptions (writes, through the engine thread) and the player's own history
//! (reads): v1 `app.py` "orders" and "IPO subscription".

use std::sync::Arc;

use rusqlite::Row;

use super::account::current_player;
use super::json::J;
use super::validate::{order_in, param_int, subscribe_in, to_cents, Cents, PriceIn};
use super::{col, get_i64, http, internal, sql, sql_int, yuan, ApiError, ApiResult, AppState, Reply, Req};
use crate::config::Phase;
use crate::engine::NewOrder;
use crate::obj;

/// v1 `order_json(row)`
pub fn order_json(r: &Row) -> J {
    obj! {
        "id" => col(r, "id"), "company_id" => col(r, "company_id"), "side" => col(r, "side"), "kind" => col(r, "kind"),
        "price" => yuan(col(r, "price")), "qty" => col(r, "qty"), "filled" => col(r, "filled"), "status" => col(r, "status"),
        "created_at" => col(r, "created_at"), "updated_at" => col(r, "updated_at"),
    }
}

fn gone<T>(_: crate::actor::ActorGone) -> Result<T, ApiError> {
    Err(internal("engine stopped"))
}

/// `POST /api/orders`
pub async fn place(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let body = r.body()?;
    let player = current_player(st, r).await?;
    let o = order_in(&body).map_err(|e| super::invalid(&body, e))?;
    let ph = st.phase();
    let g = &st.cfg.game;
    if ph == Phase::Closed {
        return Err(http(
            400,
            format!("现在休市（集合竞价 {}–{}，连续交易 {}）", g.auction_start, g.trading_open, g.session_text()),
        ));
    }
    let price = if o.kind == "limit" {
        match to_cents(&o.price) {
            Cents::Ok(c) => Some(c),
            Cents::BadFormat => return Err(http(400, "价格格式不对")),
            Cents::BadValue => return Err(http(400, "价格最多两位小数，且必须大于 0")),
            Cents::Crash => return Err(internal(format!("decimal.InvalidOperation on price {:?}", o.price))),
        }
    } else {
        let _: &PriceIn = &o.price;
        None
    };
    let company_id = sql_int(o.company_id)?;
    let (day, ts) = (st.clock.today(), st.clock.ts());
    let (side, kind, qty, account_id) = (o.side, o.kind, o.qty, player.id);
    let placed = st
        .actor
        .call(move |ctx| -> Result<J, ApiError> {
            let e = &mut ctx.engine;
            if e.company(company_id).is_none() {
                return Err(http(404, "没有这支股票"));
            }
            let req = NewOrder {
                account_id,
                company_id,
                side: &side,
                kind: &kind,
                qty,
                price,
                day: &day,
                ts: &ts,
                match_: ph == Phase::Continuous,
            };
            // v1: ensure_opening_auction and the order share one transaction (a rejected order rolls the
            // opening auction back too)
            let res = if ph == Phase::Continuous { e.open_and_place(&req) } else { e.place_order(&req) };
            e.flush().map_err(internal)?;
            let p = res.map_err(|err| http(400, err.0))?;
            Ok(obj! {"order_id" => p.order_id, "status" => p.status.as_str(), "filled" => p.filled,
            "avg_price" => yuan(p.avg_price)})
        })
        .await
        .or_else(gone)??;
    Ok(Reply::ok(placed))
}

/// `DELETE /api/orders/{order_id}`
pub async fn cancel(st: &Arc<AppState>, r: &Req, raw_id: &str) -> ApiResult {
    let player = current_player(st, r).await?;
    let id = param_int("path", "order_id", raw_id).map_err(|e| ApiError::Validation(vec![e]))?;
    let id = sql_int(id)?;
    let ts = st.clock.ts();
    let aid = player.id;
    st.actor
        .call(move |ctx| -> Result<(), ApiError> {
            let res = ctx.engine.cancel_order(aid, id, &ts);
            ctx.engine.flush().map_err(internal)?;
            res.map_err(|err| http(400, err.0))
        })
        .await
        .or_else(gone)??;
    Ok(Reply::ok(obj! {"ok" => true}))
}

/// `GET /api/orders?status=open`
pub async fn my_orders(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let player = current_player(st, r).await?;
    let only_open = r.query("status").unwrap_or("open") == "open";
    st.read(move |c| {
        let mut q =
            "SELECT o.*, c.title, c.code FROM orders o JOIN companies c ON c.id=o.company_id WHERE o.account_id=?"
                .to_string();
        if only_open {
            q += " AND o.status='open'";
        }
        q += " ORDER BY o.id DESC LIMIT 200";
        let mut s = c.prepare(&q).map_err(sql)?;
        let rows = s
            .query_map([player.id], |r| {
                let mut j = order_json(r);
                j.set("title", col(r, "title"));
                j.set("code", col(r, "code"));
                Ok(j)
            })
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"orders" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

/// `GET /api/trades`
pub async fn my_trades(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let player = current_player(st, r).await?;
    let me = player.id;
    st.read(move |c| {
        let mut s = c
            .prepare(
                "SELECT t.*, c.title, c.code FROM trades t JOIN companies c ON c.id=t.company_id
                 WHERE t.buyer_id=? OR t.seller_id=? ORDER BY t.id DESC LIMIT 200",
            )
            .map_err(sql)?;
        let rows = s
            .query_map([me, me], |r| {
                Ok(obj! {"id" => col(r, "id"), "company_id" => col(r, "company_id"), "code" => col(r, "code"),
                "title" => col(r, "title"), "side" => if get_i64(r, "buyer_id") == me { "buy" } else { "sell" },
                "price" => yuan(col(r, "price")), "qty" => col(r, "qty"), "ts" => col(r, "ts")})
            })
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"trades" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

/// `GET /api/ledger`
pub async fn my_ledger(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let player = current_player(st, r).await?;
    let me = player.id;
    st.read(move |c| {
        let mut s = c
            .prepare(
                "SELECT l.*, c.title FROM ledger l LEFT JOIN companies c ON c.id=l.company_id
                 WHERE (l.to_id=? OR l.from_id=?) AND l.reason != 'trade' ORDER BY l.id DESC LIMIT 200",
            )
            .map_err(sql)?;
        let rows = s
            .query_map([me, me], |r| {
                let amount = get_i64(r, "amount");
                Ok(obj! {"ts" => col(r, "ts"), "reason" => col(r, "reason"), "title" => col(r, "title"),
                "amount" => yuan(if get_i64(r, "to_id") == me { amount } else { -amount })})
            })
            .map_err(sql)?;
        Ok(Reply::ok(obj! {"entries" => J::Arr(rows.collect::<Result<_, _>>().map_err(sql)?)}))
    })
    .await
}

/// `POST /api/ipos/{company_id}/subscribe`
pub async fn subscribe(st: &Arc<AppState>, r: &Req, raw_id: &str) -> ApiResult {
    let body = r.body()?;
    let player = current_player(st, r).await?;
    let id = param_int("path", "company_id", raw_id);
    let qty = subscribe_in(&body);
    let (id, qty) = match (id, qty) {
        (Ok(i), Ok(q)) => (i, q),
        (i, q) => {
            let mut errs = Vec::new();
            if let Err(e) = i {
                errs.push(e);
            }
            if let Err(es) = q {
                errs.extend(es);
            }
            return Err(super::invalid(&body, errs));
        }
    };
    let company_id = sql_int(id)?;
    let (today, ts) = (st.clock.today(), st.clock.ts());
    let aid = player.id;
    let frozen = st
        .actor
        .call(move |ctx| -> Result<i64, ApiError> {
            let res = ctx.ops.subscribe(&mut ctx.engine, aid, company_id, qty, &today, &ts);
            ctx.engine.flush().map_err(internal)?;
            res.map_err(|err| http(400, err.0))
        })
        .await
        .or_else(gone)??;
    Ok(Reply::ok(obj! {"ok" => true, "qty" => qty, "frozen" => yuan(frozen)}))
}

/// `DELETE /api/ipos/{company_id}/subscribe`
pub async fn unsubscribe(st: &Arc<AppState>, r: &Req, raw_id: &str) -> ApiResult {
    let player = current_player(st, r).await?;
    let id = param_int("path", "company_id", raw_id).map_err(|e| ApiError::Validation(vec![e]))?;
    let company_id = sql_int(id)?;
    let aid = player.id;
    st.actor
        .call(move |ctx| -> Result<(), ApiError> {
            let res = ctx.ops.unsubscribe(&mut ctx.engine, aid, company_id);
            ctx.engine.flush().map_err(internal)?;
            res.map_err(|err| http(400, err.0))
        })
        .await
        .or_else(gone)??;
    Ok(Reply::ok(obj! {"ok" => true}))
}
