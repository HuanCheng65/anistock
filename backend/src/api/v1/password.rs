//! Changing a password, and resetting a forgotten one (docs/design/account.md §13).
//!
//! The one proof of who owns an account is 身份验证: the QQ bound to it. The bot asks for a reset code
//! for the QQ that messaged it in private (`POST /bot/password-resets`) and sends the code back there;
//! the player types username, code and a new password on the web (`POST /auth/password-reset`). An
//! account without a binding asks the operator, who issues a code with `anistock reset-code <username>`.
//!
//! Sessions are signed cookies with no server state; a password change records the time, and a cookie
//! issued before it (its expiry − 30 days) is refused (`issued_before_change`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use super::bind::{display_code, normalize_code, with_side, ALPHABET};
use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{body_str, json_body, need, Caller};
use crate::api::AppState;
use crate::auth;

pub const CODE_MINUTES: i64 = 10;
pub const MAX_ATTEMPTS: i64 = 5;

fn code_hash(code: &str) -> String {
    hex::encode(Sha256::digest(normalize_code(code).as_bytes()))
}

/// A new reset code for `account` (replacing any unused one). Returns (code, expires_at).
pub fn issue_code(c: &mut Connection, account: i64, source: &str, now: &chrono::DateTime<chrono_tz::Tz>) -> rusqlite::Result<(String, String)> {
    let code: String = (0..8).map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char).collect();
    let created = crate::config::format_ts(now);
    let expires = crate::config::format_ts(&(*now + chrono::Duration::minutes(CODE_MINUTES)));
    let tx = c.transaction()?;
    tx.execute("DELETE FROM password_resets WHERE account_id=? AND used_at IS NULL", [account])?;
    tx.execute(
        "INSERT INTO password_resets(code_hash, account_id, source, created_at, expires_at) VALUES (?,?,?,?,?)",
        params![code_hash(&code), account, source, created, expires],
    )?;
    tx.commit()?;
    Ok((display_code(&code), expires))
}

/// Password rules, as at registration.
fn check_new(pw: &str) -> R<()> {
    let n = pw.chars().count();
    if n < 6 {
        return Err(error::invalid(vec![error::field("new_password", "too_short", "密码至少 6 位")]));
    }
    if n > 128 {
        return Err(error::invalid(vec![error::field("new_password", "too_long", "密码最多 128 位")]));
    }
    Ok(())
}

/// The time of each account's last password change, read once and kept up to date.
pub fn changed_at(st: &AppState, account: i64) -> Option<i64> {
    {
        let g = st.v1.pw_changed.read().unwrap();
        if let Some(m) = g.as_ref() {
            return m.get(&account).copied();
        }
    }
    let loaded: HashMap<i64, i64> = with_side(st, |c| {
        let mut q = c.prepare("SELECT account_id, pw_changed_at FROM account_security").map_err(sql)?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?;
        rows.collect::<Result<_, _>>().map_err(sql)
    })
    .unwrap_or_default();
    let v = loaded.get(&account).copied();
    *st.v1.pw_changed.write().unwrap() = Some(loaded);
    v
}

/// A session cookie issued before the account's last password change.
pub fn issued_before_change(st: &AppState, account: i64, token: &str) -> bool {
    let Some(changed) = changed_at(st, account) else { return false };
    let exp: i64 = token.split('.').nth(1).and_then(|e| e.trim().parse().ok()).unwrap_or(0);
    exp - auth::SESSION_DAYS * 86400 < changed
}

/// Hash and store a new password, record the change (older sessions stop working), end live sessions.
async fn set_password(st: &Arc<AppState>, account: i64, pw: String) -> R<()> {
    let hash = super::account::hashing(st, move || auth::hash_pw(&pw)).await?;
    let now = st.unix_now() as i64;
    with_side(st, |c| {
        c.execute("UPDATE accounts SET pw_hash=? WHERE id=? AND kind='player'", params![hash, account]).map_err(sql)?;
        c.execute(
            "INSERT INTO account_security(account_id, pw_changed_at) VALUES (?,?)
             ON CONFLICT(account_id) DO UPDATE SET pw_changed_at=excluded.pw_changed_at",
            params![account, now],
        )
        .map_err(sql)?;
        Ok(())
    })?;
    changed_at(st, account);
    if let Some(m) = st.v1.pw_changed.write().unwrap().as_mut() {
        m.insert(account, now);
    }
    if let Some(live) = &st.live {
        let _ = live.send(crate::live::Ctl::Logout { account });
    }
    Ok(())
}

/// A fresh session cookie after a password change (issued now, so it is not before the change).
fn signed_in(st: &AppState, account: i64, me: Me) -> Response {
    let token = auth::make_session(&st.secret, account, st.unix_now());
    super::account::with_cookie(axum::response::IntoResponse::into_response(Json(me)), &auth::session_cookie(&token))
}

/// 修改密码
#[utoipa::path(post, path = "/api/v1/me/password", tag = "account",
    request_body = ChangePasswordRequest,
    responses((status = 200, body = Me), (status = "default", body = ErrorBody)))]
pub async fn change(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    let id = caller.player()?;
    let b = json_body(&headers, &body)?;
    let current = need(body_str(&b, "current_password")?, "current_password")?;
    let new = need(body_str(&b, "new_password")?, "new_password")?;
    check_new(&new)?;
    let stored: Option<String> = super::read(&st, move |c| {
        c.query_row("SELECT pw_hash FROM accounts WHERE id=?", [id], |r| r.get(0)).map_err(sql)
    })
    .await?;
    let ok = super::account::hashing(&st, move || auth::check_pw(&current, stored.as_deref())).await?.unwrap_or(false);
    if !ok {
        return Err(error::invalid(vec![error::field("current_password", "mismatch", "当前密码不正确")]));
    }
    set_password(&st, id, new).await?;
    let me = super::account::load_me(&st, id).await?;
    Ok(signed_in(&st, id, me))
}

/// 用验证码重设密码（忘记密码）
#[utoipa::path(post, path = "/api/v1/auth/password-reset", tag = "auth",
    request_body = ResetPasswordRequest,
    responses((status = 200, body = Me), (status = "default", body = ErrorBody)))]
pub async fn reset(State(st): State<Arc<AppState>>, headers: HeaderMap, body: Bytes) -> R<Response> {
    let b = json_body(&headers, &body)?;
    let username = need(body_str(&b, "username")?, "username")?.trim().to_string();
    let code = need(body_str(&b, "code")?, "code")?;
    let new = need(body_str(&b, "new_password")?, "new_password")?;
    check_new(&new)?;
    let invalid = || E::new(422, "reset_code_invalid", "验证码不正确或已过期，重新获取后再试");
    let now = st.clock.ts();
    let hash = code_hash(&code);
    let account = with_side(&st, |c| {
        let id: Option<i64> =
            c.query_row("SELECT id FROM accounts WHERE username=? AND kind='player'", [&username], |r| r.get(0)).optional().map_err(sql)?;
        let Some(id) = id else { return Ok(None) };
        // the account's live code (at most one)
        let live: Option<(String, String, i64)> = c
            .query_row(
                "SELECT code_hash, expires_at, attempts FROM password_resets WHERE account_id=? AND used_at IS NULL",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(sql)?;
        let Some((stored, expires, attempts)) = live else { return Ok(None) };
        let fresh = chrono::DateTime::parse_from_rfc3339(&expires).ok() > chrono::DateTime::parse_from_rfc3339(&now).ok();
        if !fresh || attempts >= MAX_ATTEMPTS {
            return Ok(None);
        }
        if !bool::from(subtle::ConstantTimeEq::ct_eq(stored.as_bytes(), hash.as_bytes())) {
            c.execute("UPDATE password_resets SET attempts=attempts+1 WHERE code_hash=?", [&stored]).map_err(sql)?;
            return Ok(None);
        }
        c.execute("UPDATE password_resets SET used_at=? WHERE code_hash=?", params![now, stored]).map_err(sql)?;
        Ok(Some(id))
    })?;
    let Some(id) = account else { return Err(invalid()) };
    set_password(&st, id, new).await?;
    // someone else may have had the account: its API keys go too (docs/design/api-access.md §3)
    let revoked = with_side(&st, |c| super::keys::revoke_all(c, id, &now))?;
    super::keys::disconnect(&st, &revoked);
    let me = super::account::load_me(&st, id).await?;
    Ok(signed_in(&st, id, me))
}

/// 为绑定的账号生成找回密码的验证码（机器人）
#[utoipa::path(post, path = "/api/v1/bot/password-resets", tag = "bot",
    request_body = BotResetRequest,
    responses((status = 201, body = ResetCode), (status = "default", body = ErrorBody)))]
pub async fn bot_issue(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Response> {
    if !matches!(caller, Caller::Bot { .. }) {
        return Err(error::forbidden("bot"));
    }
    let b = json_body(&headers, &body)?;
    let platform = need(body_str(&b, "platform")?, "platform")?;
    let external = need(body_str(&b, "external_id")?, "external_id")?.trim().to_string();
    if platform != "qq" {
        return Err(error::invalid(vec![error::not_allowed("platform")]));
    }
    super::bind::check_external(&external)?;
    {
        let mut l = st.v1.limiter.lock().unwrap();
        let v = l.take(&st.v1.limits, super::ratelimit::Key::Bind(format!("reset:{platform}:{external}")));
        if !v.allowed {
            return Err(error::rate_limited(v.reset.max(1)));
        }
    }
    let now = st.clock.now();
    let (code, expires, id, username) = with_side(&st, |c| {
        let found: Option<(i64, String)> = c
            .query_row(
                "SELECT a.id, a.username FROM bindings b JOIN accounts a ON a.id=b.account_id
                 WHERE b.platform=? AND b.external_id=? AND a.kind='player'",
                [&platform, &external],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(sql)?;
        let Some((id, username)) = found else { return Err(error::not_bound()) };
        let (code, expires) = issue_code(c, id, "qq", &now).map_err(sql)?;
        Ok((code, expires, id, username))
    })?;
    Ok(super::created(ResetCode { code, expires_at: ts(expires), account: UserRef { id, username } }))
}
