//! Corporate actions (docs/v2-corporate.md §3, docs/api-v1.md): a stock's splits, rights issues and buybacks,
//! and rights subscriptions.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde_json::json;

use super::error::{self, ErrorBody, E, R};
use super::types::*;
use super::{body_int, engine, json_body, need, Caller};
use crate::api::AppState;
use crate::corporate::Action;

fn view(a: &Action) -> CorporateAction {
    let p = &a.params;
    let (split, rights, buyback) = (a.kind == "split", a.kind == "rights", a.kind == "buyback");
    let done = a.status == "done";
    CorporateAction {
        id: a.id,
        kind: a.kind.clone(),
        status: a.status.clone(),
        announced: day(a.announced.clone()),
        effective: day(a.effective.clone()),
        split_ratio: split.then_some(p.k),
        rights_ratio: rights.then_some(r6(p.ratio)),
        rights_price: rights.then_some(m(p.price)),
        rights_budget: rights.then_some(m(p.budget)),
        rights_terp: (rights && done).then_some(m(p.terp)),
        rights_issued: (rights && done).then_some(p.issued),
        rights_subscribed: (rights && done).then_some(p.subscribed),
        buyback_budget: buyback.then_some(m(p.budget)),
        buyback_cap: buyback.then_some(m(p.cap)),
        buyback_spent: buyback.then_some(m(p.spent)),
        buyback_bought: buyback.then_some(p.bought),
    }
}

/// 公司资本政策：拆股、配股、回购（新的在前）
#[utoipa::path(get, path = "/api/v1/stocks/{id}/corporate-actions", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = CorporateActionsResponse), (status = "default", body = ErrorBody)))]
pub async fn actions(State(st): State<Arc<AppState>>, Path(raw): Path<String>) -> R<Json<CorporateActionsResponse>> {
    let id = super::path_id(&raw, "id")?;
    let items = engine(&st, move |ctx| -> R<Vec<CorporateAction>> {
        ctx.engine.company(id).ok_or_else(|| error::stock_not_found(id))?;
        Ok(ctx.engine.corporate_actions(id).into_iter().rev().map(view).collect())
    })
    .await??;
    Ok(Json(CorporateActionsResponse { items }))
}

fn offer(e: &crate::engine::Engine, id: i64, me: Option<i64>) -> R<RightsOffer> {
    e.company(id).ok_or_else(|| error::stock_not_found(id))?;
    let a = e.open_action(id, "rights").cloned();
    let (mut ent, mut sub, mut frozen) = (0, 0, 0);
    if let (Some(a), Some(me)) = (&a, me) {
        ent = e.rights_entitlement(a.id, me);
        if let Some(s) = e.rights_sub(a.id, me) {
            sub = s.qty;
            frozen = s.frozen;
        }
    }
    Ok(RightsOffer { action: a.as_ref().map(view), entitlement: ent, subscribed: sub, frozen: m(frozen) })
}

/// 正在认购期的配股（登录时带调用者的配股权和认购）
#[utoipa::path(get, path = "/api/v1/stocks/{id}/rights", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = RightsOffer), (status = "default", body = ErrorBody)))]
pub async fn rights(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Json<RightsOffer>> {
    let id = super::path_id(&raw, "id")?;
    let me = caller.viewer();
    let o = engine(&st, move |ctx| offer(&ctx.engine, id, me)).await??;
    Ok(Json(o))
}

/// 认购配股（设定数量，幂等；0 等于撤回）
#[utoipa::path(put, path = "/api/v1/stocks/{id}/rights/subscription", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    request_body = RightsRequest,
    responses((status = 200, body = RightsOffer), (status = "default", body = ErrorBody)))]
pub async fn subscribe(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<RightsOffer>> {
    let me = caller.trader(&st).await?.account;
    super::account::require_signed(&st, me).await?;
    let id = super::path_id(&raw, "id")?;
    let b = json_body(&headers, &body)?;
    let qty = need(body_int(&b, "quantity")?, "quantity")?;
    if qty < 0 {
        return Err(error::invalid(vec![error::too_small("quantity", 0)]));
    }
    set(&st, me, id, qty).await
}

/// 撤回配股认购（幂等）
#[utoipa::path(delete, path = "/api/v1/stocks/{id}/rights/subscription", tag = "stocks",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = RightsOffer), (status = "default", body = ErrorBody)))]
pub async fn unsubscribe(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Json<RightsOffer>> {
    let me = caller.trader(&st).await?.account;
    let id = super::path_id(&raw, "id")?;
    set(&st, me, id, 0).await
}

async fn set(st: &Arc<AppState>, me: i64, id: i64, qty: i64) -> R<Json<RightsOffer>> {
    let ts = st.clock.ts();
    let o = engine(st, move |ctx| -> R<RightsOffer> {
        let e = &mut ctx.engine;
        e.company(id).ok_or_else(|| error::stock_not_found(id))?;
        if e.open_action(id, "rights").is_none() {
            if qty == 0 {
                return offer(e, id, Some(me));
            }
            return Err(E::new(422, "rights_not_open", "这支股票没有在认购期的配股"));
        }
        let res = e.rights_subscribe(me, id, qty, &ts);
        e.flush().map_err(error::internal)?;
        res.map_err(|err| E::new(422, "rights_rejected", err.0).details(json!({"quantity": qty})))?;
        offer(e, id, Some(me))
    })
    .await??;
    Ok(Json(o))
}
