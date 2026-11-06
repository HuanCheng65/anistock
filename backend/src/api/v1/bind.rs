//! QQ bindings (docs/api-v1.md §3.5). Codes and bindings are not game state: they are written on a small
//! connection of their own (`V1State::side`), not through the engine.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};

use super::error::{self, sql, ErrorBody, R};
use super::types::*;
use super::{body_str, json_body, json_body_opt, need, read, Caller};
use crate::api::AppState;

/// Crockford base32 without I L O U.
pub const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const CODE_MINUTES: i64 = 10;

/// Run one write on the side connection (opened on first use).
pub fn side_exec(st: &AppState, q: &str, p: impl rusqlite::Params) -> R<usize> {
    with_side(st, |c| c.execute(q, p).map_err(sql))
}

pub fn with_side<T>(st: &AppState, f: impl FnOnce(&mut Connection) -> R<T>) -> R<T> {
    let mut g = st.v1.side.lock().unwrap();
    if g.is_none() {
        let c = Connection::open(&st.cfg.game_db).map_err(sql)?;
        c.busy_timeout(std::time::Duration::from_millis(5000)).map_err(sql)?;
        *g = Some(c);
    }
    f(g.as_mut().unwrap())
}

/// A QQ identity from the official bot: its openid (letters, digits, `_`, `-`; up to 64 characters).
/// A plain QQ number also passes (the bot of v1 sent those).
pub fn check_external(external: &str) -> R<()> {
    if external.is_empty() || external.len() > 64 || !external.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(error::invalid(vec![error::bad_format("external_id")]));
    }
    Ok(())
}

/// `K7Q39MZD` -> `K7Q3-9MZD`
pub fn display_code(c: &str) -> String {
    format!("{}-{}", &c[..4], &c[4..])
}

/// What the player typed: hyphens and spaces dropped, upper case, I/L -> 1, O -> 0.
pub fn normalize_code(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| match c.to_ascii_uppercase() {
            'I' | 'L' => '1',
            'O' => '0',
            x => x,
        })
        .collect()
}

pub fn mask(external: &str) -> String {
    let ch: Vec<char> = external.chars().collect();
    if ch.len() <= 4 {
        return "*".repeat(ch.len());
    }
    let head: String = ch[..3].iter().collect();
    let tail: String = ch[ch.len() - 3..].iter().collect();
    format!("{head}****{tail}")
}

fn binding_from_row(r: &rusqlite::Row) -> rusqlite::Result<Binding> {
    let ext: String = r.get("external_id")?;
    let scopes: String = r.get("scopes")?;
    Ok(Binding {
        id: r.get("id")?,
        platform: r.get("platform")?,
        display_name: mask(&ext),
        external_id: ext,
        scopes: scopes.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect(),
        created_at: ts(r.get::<_, String>("created_at")?),
        last_used_at: r.get::<_, Option<String>>("last_used_at")?.map(ts),
    })
}

/// 生成绑定码（仅网页）
#[utoipa::path(post, path = "/api/v1/me/bind-codes", tag = "bindings",
    request_body = BindCodeRequest,
    responses((status = 201, body = BindCode), (status = "default", body = ErrorBody)))]
pub async fn create_code(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    let me = caller.player()?;
    let b = json_body_opt(&headers, &body)?;
    let scopes: Vec<String> = match b.get("scopes") {
        None | Some(serde_json::Value::Null) => vec!["read:account".into()],
        Some(serde_json::Value::Array(a)) => {
            let v: Vec<String> = a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            if v.len() != a.len() || v.is_empty() || v.iter().any(|s| s != "read:account") {
                return Err(error::invalid(vec![error::field("scopes", "not_allowed", "只能授予 read:account")]));
            }
            v
        }
        Some(_) => return Err(error::invalid(vec![error::wrong_type("scopes")])),
    };
    let now = st.clock.now();
    let created = crate::config::format_ts(&now);
    let expires = crate::config::format_ts(&(now + chrono::Duration::minutes(CODE_MINUTES)));
    let code: String = (0..8).map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char).collect();
    let (code2, scopes2, exp2) = (code.clone(), scopes.join(","), expires.clone());
    let st2 = st.clone();
    tokio::task::spawn_blocking(move || {
        with_side(&st2, |c| {
            let bound: Option<i64> = c
                .query_row("SELECT id FROM bindings WHERE account_id=? AND platform='qq'", [me], |r| r.get(0))
                .optional()
                .map_err(sql)?;
            if bound.is_some() {
                return Err(error::already_bound("account"));
            }
            let tx = c.transaction().map_err(sql)?;
            tx.execute("DELETE FROM bind_codes WHERE account_id=? AND used_at IS NULL", [me]).map_err(sql)?;
            tx.execute(
                "INSERT INTO bind_codes(code, account_id, scopes, created_at, expires_at) VALUES (?,?,?,?,?)",
                params![code2, me, scopes2, created, exp2],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)
        })
    })
    .await
    .map_err(error::internal)??;
    Ok(super::created(BindCode { code: display_code(&code), expires_at: ts(expires), scopes }))
}

/// 我的绑定（仅网页）
#[utoipa::path(get, path = "/api/v1/me/bindings", tag = "bindings",
    responses((status = 200, body = BindingsResponse), (status = "default", body = ErrorBody)))]
pub async fn list(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>) -> R<Json<BindingsResponse>> {
    let me = caller.player()?;
    read(&st, move |c| {
        let mut s = c.prepare("SELECT * FROM bindings WHERE account_id=? ORDER BY id").map_err(sql)?;
        let rows = s.query_map([me], binding_from_row).map_err(sql)?;
        Ok(BindingsResponse { items: rows.collect::<Result<_, _>>().map_err(sql)? })
    })
    .await
    .map(Json)
}

/// 解除绑定（仅网页）
#[utoipa::path(delete, path = "/api/v1/me/bindings/{id}", tag = "bindings",
    params(("id" = i64, Path, description = "绑定 id")),
    responses((status = 204, description = "已解除"), (status = "default", body = ErrorBody)))]
pub async fn remove(State(st): State<Arc<AppState>>, Extension(caller): Extension<Caller>, Path(raw): Path<String>) -> R<Response> {
    let me = caller.player()?;
    let id = super::path_id(&raw, "id")?;
    let st2 = st.clone();
    let n = tokio::task::spawn_blocking(move || side_exec(&st2, "DELETE FROM bindings WHERE id=? AND account_id=?", params![id, me]))
        .await
        .map_err(error::internal)??;
    if n == 0 {
        return Err(error::binding_not_found());
    }
    Ok(super::no_content())
}

/// 机器人兑换绑定码
#[utoipa::path(post, path = "/api/v1/bot/bindings", tag = "bot",
    request_body = RedeemRequest,
    responses((status = 201, body = RedeemResponse), (status = "default", body = ErrorBody)),
    security(("bot" = [])))]
pub async fn redeem(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    if !matches!(caller, Caller::Bot { .. }) {
        return Err(error::forbidden("bot"));
    }
    let b = json_body(&headers, &body)?;
    let code = normalize_code(&need(body_str(&b, "code")?, "code")?);
    let platform = need(body_str(&b, "platform")?, "platform")?;
    let external = need(body_str(&b, "external_id")?, "external_id")?.trim().to_string();
    if platform != "qq" {
        return Err(error::invalid(vec![error::not_allowed("platform")]));
    }
    check_external(&external)?;
    {
        let mut l = st.v1.limiter.lock().unwrap();
        let v = l.take(&st.v1.limits, super::ratelimit::Key::Bind(format!("{platform}:{external}")));
        if !v.allowed {
            return Err(error::rate_limited(v.reset.max(1)));
        }
    }
    let now = st.clock.ts();
    let st2 = st.clone();
    let (binding, account) = tokio::task::spawn_blocking(move || {
        with_side(&st2, |c| {
            let tx = c.transaction().map_err(sql)?;
            let row: Option<(i64, String, String, Option<String>)> = tx
                .query_row("SELECT account_id, scopes, expires_at, used_at FROM bind_codes WHERE code=?", [&code], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .optional()
                .map_err(sql)?;
            let Some((account, scopes, expires, used)) = row else { return Err(error::bind_code_invalid()) };
            let exp = chrono::DateTime::parse_from_rfc3339(&expires).map_err(error::internal)?;
            let now_dt = chrono::DateTime::parse_from_rfc3339(&now).map_err(error::internal)?;
            if used.is_some() || exp <= now_dt {
                return Err(error::bind_code_invalid());
            }
            let qq_taken: Option<i64> = tx
                .query_row("SELECT account_id FROM bindings WHERE platform=? AND external_id=?", [&platform, &external], |r| r.get(0))
                .optional()
                .map_err(sql)?;
            if qq_taken.is_some() {
                return Err(error::already_bound("qq"));
            }
            let acct_taken: Option<i64> = tx
                .query_row("SELECT id FROM bindings WHERE account_id=? AND platform=?", params![account, platform], |r| r.get(0))
                .optional()
                .map_err(sql)?;
            if acct_taken.is_some() {
                return Err(error::already_bound("account"));
            }
            tx.execute(
                "INSERT INTO bindings(account_id, platform, external_id, scopes, created_at) VALUES (?,?,?,?,?)",
                params![account, platform, external, scopes, now],
            )
            .map_err(sql)?;
            let id = tx.last_insert_rowid();
            tx.execute("UPDATE bind_codes SET used_at=? WHERE code=?", params![now, code]).map_err(sql)?;
            let binding = tx.query_row("SELECT * FROM bindings WHERE id=?", [id], binding_from_row).map_err(sql)?;
            let username: String =
                tx.query_row("SELECT username FROM accounts WHERE id=?", [account], |r| r.get(0)).map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok((binding, UserRef { id: account, username }))
        })
    })
    .await
    .map_err(error::internal)??;
    Ok(super::created(RedeemResponse { binding, account }))
}

/// 机器人解除绑定（群里 /安易 解绑）
#[utoipa::path(delete, path = "/api/v1/bot/bindings/{platform}/{external_id}", tag = "bot",
    params(("platform" = String, Path, description = "qq"), ("external_id" = String, Path, description = "私聊消息里发送者的 openid")),
    responses((status = 204, description = "已解除"), (status = "default", body = ErrorBody)),
    security(("bot" = [])))]
pub async fn bot_remove(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path((platform, external)): Path<(String, String)>,
) -> R<Response> {
    if !matches!(caller, Caller::Bot { .. }) {
        return Err(error::forbidden("bot"));
    }
    let st2 = st.clone();
    let n = tokio::task::spawn_blocking(move || {
        side_exec(&st2, "DELETE FROM bindings WHERE platform=? AND external_id=?", params![platform, external])
    })
    .await
    .map_err(error::internal)??;
    if n == 0 {
        return Err(error::binding_not_found());
    }
    Ok(super::no_content())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes() {
        assert_eq!(normalize_code("k7q3-9mzd"), "K7Q39MZD");
        assert_eq!(display_code("K7Q39MZD"), "K7Q3-9MZD");
        assert_eq!(mask("123456789"), "123****789");
    }
}
