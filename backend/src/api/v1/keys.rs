//! Personal API keys for scripts (docs/v2-api-access.md). A key is `anx_` + 40 base62 characters, shown
//! once; only its SHA-256 is kept, on the side connection like the QQ bindings (not game state).
//!
//! Phase 1 grants `read:account` only: a key reads `/me/*` and may subscribe `me` on the WebSocket; every
//! write stays with the browser session (`Caller::player`).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use rusqlite::{params, OptionalExtension};

use super::bind::{side_exec, with_side};
use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{body_int, body_str, json_body_opt, read, Caller};
use crate::api::AppState;

pub const PREFIX: &str = "anx_";
const BODY_LEN: usize = 40;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// keys in force (neither revoked nor expired) per account
pub const MAX_KEYS: i64 = 5;
pub const LIFETIMES: [i64; 3] = [30, 90, 365];
const DEFAULT_DAYS: i64 = 90;
/// `last_used_at` is rewritten at most this often (seconds), unless the address changed
const TOUCH_EVERY: i64 = 60;

pub fn hash(token: &str) -> Vec<u8> {
    use sha2::Digest;
    sha2::Sha256::digest(token.as_bytes()).to_vec()
}

fn unix(ts: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(ts).map(|d| d.timestamp()).unwrap_or(0)
}

/// A key that authenticated a request.
#[derive(Clone, Debug)]
pub struct KeyAuth {
    pub id: i64,
    pub account: i64,
    pub scopes: Vec<String>,
    /// Unix seconds
    pub expires: i64,
}

/// The key behind `token` (which starts with `anx_`): unknown, revoked, expired, or of an account that is
/// no longer a player -> 401.
pub async fn authenticate(st: &Arc<AppState>, token: &str, ip: &str) -> R<KeyAuth> {
    let h = hash(token);
    let found = read(st, move |c| {
        c.query_row(
            "SELECT k.id, k.account_id, k.scopes, k.expires_at, k.last_used_at, k.last_used_ip
             FROM api_keys k JOIN accounts a ON a.id=k.account_id
             WHERE k.key_hash=? AND k.revoked_at IS NULL AND a.kind='player'",
            [h],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(sql)
    })
    .await?;
    let Some((id, account, scopes, expires, used_at, used_ip)) = found else { return Err(error::bad_api_key()) };
    let now = st.clock.now().timestamp();
    let expires = unix(&expires);
    if expires <= now {
        return Err(error::bad_api_key());
    }
    let stale = used_at.as_deref().map(unix).is_none_or(|t| now - t >= TOUCH_EVERY);
    if stale || used_ip.as_deref() != Some(ip) {
        let (st2, ts, ip) = (st.clone(), st.clock.ts(), ip.to_string());
        tokio::task::spawn_blocking(move || {
            let _ = side_exec(&st2, "UPDATE api_keys SET last_used_at=?, last_used_ip=? WHERE id=?", params![ts, ip, id]);
        });
    }
    let scopes = scopes.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
    Ok(KeyAuth { id, account, scopes, expires })
}

/// Revoke every key of `account` (a password reset); returns their ids so open connections can be closed.
pub fn revoke_all(c: &rusqlite::Connection, account: i64, now: &str) -> R<Vec<i64>> {
    let mut s = c.prepare("SELECT id FROM api_keys WHERE account_id=? AND revoked_at IS NULL").map_err(sql)?;
    let ids: Vec<i64> = s.query_map([account], |r| r.get(0)).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    c.execute("UPDATE api_keys SET revoked_at=? WHERE account_id=? AND revoked_at IS NULL", params![now, account]).map_err(sql)?;
    Ok(ids)
}

/// Close the WebSocket connections opened with these keys.
pub fn disconnect(st: &AppState, keys: &[i64]) {
    if let Some(live) = &st.live {
        for &key in keys {
            let _ = live.send(crate::live::Ctl::Revoke { key });
        }
    }
}

fn key_from_row(r: &rusqlite::Row, now: i64) -> rusqlite::Result<ApiKey> {
    let scopes: String = r.get("scopes")?;
    let expires: String = r.get("expires_at")?;
    Ok(ApiKey {
        id: r.get("id")?,
        name: r.get("name")?,
        prefix: r.get("prefix")?,
        scopes: scopes.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect(),
        created_at: ts(r.get::<_, String>("created_at")?),
        expired: unix(&expires) <= now,
        expires_at: ts(expires),
        last_used_at: r.get::<_, Option<String>>("last_used_at")?.map(ts),
        last_used_ip: r.get("last_used_ip")?,
    })
}

/// 我的 API 密钥（仅网页）
#[utoipa::path(get, operation_id = "list_api_keys", path = "/api/v1/me/api-keys", tag = "account",
    responses((status = 200, body = ApiKeysResponse), (status = "default", body = ErrorBody)))]
pub async fn list(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<ApiKeysResponse>> {
    let me = caller.player()?;
    let now = st.clock.now().timestamp();
    read(&st, move |c| {
        let mut s = c.prepare("SELECT * FROM api_keys WHERE account_id=? AND revoked_at IS NULL ORDER BY id DESC").map_err(sql)?;
        let rows = s.query_map([me], |r| key_from_row(r, now)).map_err(sql)?;
        Ok(ApiKeysResponse { items: rows.collect::<Result<_, _>>().map_err(sql)?, max: MAX_KEYS })
    })
    .await
    .map(Json)
}

/// 生成 API 密钥（仅网页）。明文只在这次返回
#[utoipa::path(post, operation_id = "create_api_key", path = "/api/v1/me/api-keys", tag = "account",
    request_body = ApiKeyRequest,
    responses((status = 201, body = ApiKeyCreated), (status = "default", body = ErrorBody)))]
pub async fn create(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    let me = caller.player()?;
    let b = json_body_opt(&headers, &body)?;
    let name = body_str(&b, "name")?.unwrap_or_default().trim().to_string();
    let n = name.chars().count();
    if n == 0 {
        return Err(error::invalid(vec![error::missing("name")]));
    }
    if n > 20 {
        return Err(error::invalid(vec![error::field("name", "too_long", "名称最多 20 字")]));
    }
    let scopes: Vec<String> = match b.get("scopes") {
        None | Some(serde_json::Value::Null) => vec!["read:account".into()],
        Some(serde_json::Value::Array(a)) => {
            let v: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            if v.len() != a.len() || v.is_empty() || v.iter().any(|s| s != "read:account") {
                return Err(error::invalid(vec![error::field("scopes", "not_allowed", "目前只能授予 read:account")]));
            }
            v
        }
        Some(_) => return Err(error::invalid(vec![error::wrong_type("scopes")])),
    };
    let days = body_int(&b, "expires_in_days")?.unwrap_or(DEFAULT_DAYS);
    if !LIFETIMES.contains(&days) {
        return Err(error::invalid(vec![error::field("expires_in_days", "not_allowed", "有效期只能是 30、90 或 365 天")]));
    }
    let now = st.clock.now();
    let created = crate::config::format_ts(&now);
    let expires = crate::config::format_ts(&(now + chrono::Duration::days(days)));
    let body: String = (0..BODY_LEN).map(|_| BASE62[rand::random_range(0..BASE62.len())] as char).collect();
    let token = format!("{PREFIX}{body}");
    let prefix = format!("{PREFIX}{}", &body[..4]);
    let (h, scopes_s, now_unix) = (hash(&token), scopes.join(","), now.timestamp());
    let (name2, prefix2, created2, expires2) = (name.clone(), prefix.clone(), created.clone(), expires.clone());
    let st2 = st.clone();
    let id = tokio::task::spawn_blocking(move || {
        with_side(&st2, |c| {
            let tx = c.transaction().map_err(sql)?;
            let live: Vec<String> = {
                let mut s = tx.prepare("SELECT expires_at FROM api_keys WHERE account_id=? AND revoked_at IS NULL").map_err(sql)?;
                let rows = s.query_map([me], |r| r.get(0)).map_err(sql)?;
                rows.collect::<Result<_, _>>().map_err(sql)?
            };
            if live.iter().filter(|e| unix(e) > now_unix).count() as i64 >= MAX_KEYS {
                return Err(error::too_many_api_keys(MAX_KEYS));
            }
            tx.execute(
                "INSERT INTO api_keys(account_id, name, prefix, key_hash, scopes, created_at, expires_at) VALUES (?,?,?,?,?,?,?)",
                params![me, name2, prefix2, h, scopes_s, created2, expires2],
            )
            .map_err(sql)?;
            let id = tx.last_insert_rowid();
            tx.commit().map_err(sql)?;
            Ok(id)
        })
    })
    .await
    .map_err(error::internal)??;
    let key = ApiKey {
        id,
        name,
        prefix,
        scopes,
        created_at: ts(created),
        expires_at: ts(expires),
        expired: false,
        last_used_at: None,
        last_used_ip: None,
    };
    Ok(super::created(ApiKeyCreated { key, token }))
}

/// 吊销 API 密钥（仅网页）
#[utoipa::path(delete, operation_id = "revoke_api_key", path = "/api/v1/me/api-keys/{id}", tag = "account",
    params(("id" = i64, Path, description = "密钥 id")),
    responses((status = 204, description = "已吊销"), (status = "default", body = ErrorBody)))]
pub async fn revoke(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Response> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    let (st2, now) = (st.clone(), st.clock.ts());
    let n = tokio::task::spawn_blocking(move || {
        side_exec(&st2, "UPDATE api_keys SET revoked_at=? WHERE id=? AND account_id=? AND revoked_at IS NULL", params![now, id, me])
    })
    .await
    .map_err(error::internal)??;
    if n == 0 {
        return Err(E::new(404, "api_key_not_found", "API 密钥不存在"));
    }
    disconnect(&st, &[id]);
    Ok(super::no_content())
}
