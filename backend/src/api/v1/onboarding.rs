//! 自选 on the server, and the one-off onboarding marks (page guides seen, the 入门步骤 card closed, the 首笔成交
//! note seen). Both are personal data outside the game books: written on the side connection, never through the
//! engine thread.

use std::collections::HashSet;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use rusqlite::{params, Connection};
use serde_json::Value;

use super::account::load_me;
use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{json_body, need, read, Caller};
use crate::api::AppState;

/// the most stocks one 自选 holds
pub const WATCH_MAX: i64 = 200;
/// the marks a player may set (`watch` is set by adding a stock)
pub const MARKS: [&str; 4] = ["guide.stock", "guide.order", "steps.closed", "first_fill"];

fn list(c: &Connection, id: i64) -> R<Vec<i64>> {
    let mut q = c.prepare_cached("SELECT company_id FROM watchlist WHERE account_id=? ORDER BY rowid DESC").map_err(sql)?;
    let ids = q.query_map([id], |r| r.get(0)).map_err(sql)?.collect::<Result<Vec<i64>, _>>().map_err(sql)?;
    Ok(ids)
}

/// The marks `id` has set, oldest first.
pub fn marks(c: &Connection, id: i64) -> R<Vec<String>> {
    let mut q = c.prepare_cached("SELECT key FROM account_marks WHERE account_id=? ORDER BY at, key").map_err(sql)?;
    let keys = q.query_map([id], |r| r.get(0)).map_err(sql)?.collect::<Result<Vec<String>, _>>().map_err(sql)?;
    Ok(keys)
}

fn full() -> E {
    E::new(422, "watchlist_full", format!("自选最多 {WATCH_MAX} 支"))
}

/// Adds `ids` (newest first, as the list reads) that are listed stocks and not in the list yet, oldest first so
/// the list keeps their order; `strict`: a single add, which reports an unknown stock or a full list.
fn add(c: &mut Connection, id: i64, ids: &[i64], at: &str, strict: bool) -> R<()> {
    let tx = c.transaction().map_err(sql)?;
    {
        let mut have: HashSet<i64> = list(&tx, id)?.into_iter().collect();
        let mut known = tx.prepare_cached("SELECT 1 FROM companies WHERE id=?").map_err(sql)?;
        let mut ins = tx.prepare_cached("INSERT INTO watchlist(account_id, company_id, added_at) VALUES (?,?,?)").map_err(sql)?;
        for &s in ids.iter().rev() {
            if have.contains(&s) {
                continue;
            }
            if !known.exists([s]).map_err(sql)? {
                if strict {
                    return Err(error::stock_not_found(s));
                }
                continue;
            }
            if have.len() as i64 >= WATCH_MAX {
                if strict {
                    return Err(full());
                }
                break;
            }
            ins.execute(params![id, s, at]).map_err(sql)?;
            have.insert(s);
        }
        if !have.is_empty() {
            tx.execute("INSERT OR IGNORE INTO account_marks(account_id, key, at) VALUES (?, 'watch', ?)", params![id, at]).map_err(sql)?;
        }
    }
    tx.commit().map_err(sql)
}

/// 自选
#[utoipa::path(get, path = "/api/v1/me/watchlist", tag = "account",
    responses((status = 200, body = Watchlist), (status = "default", body = ErrorBody)))]
pub async fn watchlist(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<Watchlist>> {
    let id = caller.player()?;
    read(&st, move |c| list(c, id)).await.map(|items| Json(Watchlist { items }))
}

/// 加入自选（已在自选中时不变）
#[utoipa::path(put, path = "/api/v1/me/watchlist/{id}", tag = "account",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = Watchlist), (status = "default", body = ErrorBody)))]
pub async fn watch(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Json<Watchlist>> {
    let id = caller.player()?;
    let stock = super::path_id(&raw, "id")?;
    let at = st.clock.ts();
    let items = super::bind::side(&st, move |c| {
        add(c, id, &[stock], &at, true)?;
        list(c, id)
    })
    .await?;
    Ok(Json(Watchlist { items }))
}

/// 移出自选（不在自选中时不变）
#[utoipa::path(delete, path = "/api/v1/me/watchlist/{id}", tag = "account",
    params(("id" = i64, Path, description = "股票 id")),
    responses((status = 200, body = Watchlist), (status = "default", body = ErrorBody)))]
pub async fn unwatch(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Json<Watchlist>> {
    let id = caller.player()?;
    let stock = super::path_id(&raw, "id")?;
    let items = super::bind::side(&st, move |c| {
        c.execute("DELETE FROM watchlist WHERE account_id=? AND company_id=?", params![id, stock]).map_err(sql)?;
        list(c, id)
    })
    .await?;
    Ok(Json(Watchlist { items }))
}

/// 合并自选：把另一份列表（例如浏览器里的旧自选，新加入的在前）中还没有的股票加到自选最前面，保持它们原来的顺序。
/// 不存在的股票和超出上限的部分忽略。
#[utoipa::path(post, path = "/api/v1/me/watchlist/import", tag = "account",
    request_body = Watchlist,
    responses((status = 200, body = Watchlist), (status = "default", body = ErrorBody)))]
pub async fn import(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<Watchlist>> {
    let id = caller.player()?;
    let b = json_body(&headers, &body)?;
    let items = match need(b.get("items"), "items")? {
        Value::Array(a) if a.len() <= 1000 => a.iter().map(Value::as_i64).collect::<Option<Vec<i64>>>(),
        _ => None,
    }
    .ok_or_else(|| error::invalid(vec![error::wrong_type("items")]))?;
    let at = st.clock.ts();
    // a stock listed twice keeps its first (newest) place
    let mut seen = HashSet::new();
    let items: Vec<i64> = items.into_iter().filter(|s| seen.insert(*s)).collect();
    let items = super::bind::side(&st, move |c| {
        add(c, id, &items, &at, false)?;
        list(c, id)
    })
    .await?;
    Ok(Json(Watchlist { items }))
}

/// 记录一次性的新手引导标记：`guide.stock`、`guide.order`（页面引导已看过）、`steps.closed`（入门步骤已关闭）、
/// `first_fill`（首笔成交说明已看过）
#[utoipa::path(post, path = "/api/v1/me/marks/{key}", tag = "account",
    params(("key" = String, Path, description = "标记")),
    responses((status = 200, body = Me), (status = "default", body = ErrorBody)))]
pub async fn mark(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(key): Path<String>) -> R<Json<Me>> {
    let id = caller.player()?;
    if !MARKS.contains(&key.as_str()) {
        return Err(error::invalid(vec![error::not_allowed("key")]));
    }
    let at = st.clock.ts();
    super::bind::side(&st, move |c| {
        c.execute("INSERT OR IGNORE INTO account_marks(account_id, key, at) VALUES (?,?,?)", params![id, key, at]).map_err(sql)
    })
    .await?;
    load_me(&st, id).await.map(Json)
}
