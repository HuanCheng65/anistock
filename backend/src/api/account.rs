//! Accounts: register, login, logout, me, portfolio (v1 `app.py` "account"), and the session check.

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension};

use super::json::J;
use super::validate::{credentials, Body};
use super::{col, div, get_i64, http, internal, sql, yuan, ApiError, ApiResult, AppState, Reply, Req};
use crate::auth::{self, Session};
use crate::obj;

/// The signed-in player (`accounts` row, kind = 'player').
#[derive(Debug, Clone)]
pub struct Player {
    pub id: i64,
    pub username: J,
}

/// v1 `current_player`: 401 `请先登录` unless the cookie is valid and names a player account.
pub async fn current_player(st: &Arc<AppState>, r: &Req) -> Result<Player, ApiError> {
    optional_player(st, r).await?.ok_or_else(|| http(401, "请先登录"))
}

/// v1 `optional_player` (only the 401 is swallowed; a crash in `read_session` is still a 500).
pub async fn optional_player(st: &Arc<AppState>, r: &Req) -> Result<Option<Player>, ApiError> {
    let aid = match auth::read_session(&st.secret, r.session().as_deref(), st.unix_now()) {
        Session::None => return Ok(None),
        Session::Crash => return Err(internal("read_session raised")),
        Session::Player(0) => return Ok(None), // `if aid` in v1
        Session::Player(a) => a,
    };
    st.read(move |c| {
        c.query_row("SELECT id, username FROM accounts WHERE id=? AND kind='player'", [aid], |row| {
            Ok(Player { id: row.get("id")?, username: col(row, "username") })
        })
        .optional()
        .map_err(sql)
    })
    .await
}

/// `POST /api/register`
pub async fn register(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let body: Body = r.body()?;
    let cred = credentials(&body).map_err(|e| super::invalid(&body, e))?;
    if cred.invite.as_deref() != Some(st.cfg.invite_code.as_str()) {
        return Err(http(400, "邀请码不对，请向社团管理员索取"));
    }
    let name = py_strip(&cred.username).to_string();
    // scrypt is ~50 ms of CPU: on the blocking pool, before taking the engine's time
    let pw = cred.password.clone();
    let pw_hash = tokio::task::spawn_blocking(move || auth::hash_pw(&pw)).await.map_err(|e| internal(e.to_string()))?;
    let (day, ts) = (st.clock.today(), st.clock.ts());
    let grant = (st.cfg.game.initial_cash * 100.0).round_ties_even() as i64;
    let aid = st
        .actor
        .call(move |ctx| -> Result<i64, ApiError> {
            let e = &mut ctx.engine;
            if e.accounts().any(|a| a.username.as_deref() == Some(name.as_str())) {
                return Err(http(400, "这个用户名已经有人用了"));
            }
            let world = e.special("world").ok_or_else(|| internal("no world account"))?;
            let aid = e.create_account("player", &name, Some(&name), Some(&pw_hash), None, &ts);
            e.transfer(world, aid, grant, "grant", &day, &ts, None, None);
            e.flush().map_err(internal)?;
            Ok(aid)
        })
        .await
        .map_err(|_| internal("engine stopped"))??;
    let token = auth::make_session(&st.secret, aid, st.unix_now());
    Ok(Reply { status: 200, json: obj! {"ok" => true}, set_cookie: Some(auth::session_cookie(&token)) })
}

/// Python `str.strip()` (Unicode whitespace).
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}

/// `POST /api/login`
pub async fn login(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let body = r.body()?;
    let cred = credentials(&body).map_err(|e| super::invalid(&body, e))?;
    let name = py_strip(&cred.username).to_string();
    let row: Option<(i64, Option<String>)> = st
        .read(move |c| {
            c.query_row("SELECT id, pw_hash FROM accounts WHERE username=? AND kind='player'", [&name], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(sql)
        })
        .await?;
    let Some((id, stored)) = row else { return Err(http(400, "用户名或密码不对")) };
    let pw = cred.password.clone();
    let ok = tokio::task::spawn_blocking(move || auth::check_pw(&pw, stored.as_deref()))
        .await
        .map_err(|e| internal(e.to_string()))?
        .map_err(|_| internal("check_pw raised (pw_hash is NULL or malformed)"))?;
    if !ok {
        return Err(http(400, "用户名或密码不对"));
    }
    let token = auth::make_session(&st.secret, id, st.unix_now());
    Ok(Reply { status: 200, json: obj! {"ok" => true}, set_cookie: Some(auth::session_cookie(&token)) })
}

/// `POST /api/logout`
pub fn logout() -> ApiResult {
    Ok(Reply {
        status: 200,
        json: obj! {"ok" => true},
        set_cookie: Some(auth::delete_cookie(std::time::SystemTime::now())),
    })
}

/// company_id, qty, qty_frozen, code, title, title_ja, last_price, prev_close
type HoldingRow = (i64, i64, i64, J, J, J, i64, i64);

/// v1 `portfolio_rows`
pub fn portfolio_rows(c: &Connection, account_id: i64) -> Result<Vec<J>, ApiError> {
    let mut st = c
        .prepare_cached(
            "SELECT h.company_id, h.qty, h.qty_frozen, co.code, co.title, co.title_ja, co.last_price, co.prev_close, co.status
             FROM holdings h JOIN companies co ON co.id = h.company_id
             WHERE h.account_id=? AND h.qty > 0 ORDER BY co.code",
        )
        .map_err(sql)?;
    let rows: Vec<HoldingRow> = st
        .query_map([account_id], |h| {
            Ok((
                get_i64(h, "company_id"),
                get_i64(h, "qty"),
                get_i64(h, "qty_frozen"),
                col(h, "code"),
                col(h, "title"),
                col(h, "title_ja"),
                get_i64(h, "last_price"),
                get_i64(h, "prev_close"),
            ))
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    let mut out = Vec::new();
    for (cid, qty, qty_frozen, code, title, title_ja, last, prev) in rows {
        let one = |q: &str| -> Result<J, ApiError> {
            c.query_row(q, [account_id, cid], |r| Ok(col(r, "v"))).map_err(sql)
        };
        let cost = one("SELECT COALESCE(SUM(qty*cost),0) AS v FROM lots WHERE account_id=? AND company_id=?")?;
        let divs = one(
            "SELECT COALESCE(SUM(amount),0) AS v FROM ledger WHERE to_id=? AND company_id=? AND reason='dividend'",
        )?;
        let oldest = one("SELECT MIN(acquired_on) AS v FROM lots WHERE account_id=? AND company_id=?")?;
        let J::Int(cost) = cost else { return Err(internal("lot cost is not an int")) };
        let value = qty * last;
        out.push(obj! {
            "company_id" => cid, "code" => code, "title" => title, "title_ja" => title_ja, "qty" => qty,
            "available" => qty - qty_frozen, "avg_cost" => yuan(crate::exchange::py_round_div(cost, qty)),
            "price" => yuan(last), "value" => yuan(value), "pnl" => yuan(value - cost),
            "pnl_pct" => if cost != 0 { J::Float(div(value - cost, cost)) } else { J::Null },
            "dividends" => yuan(divs),
            "day_change" => if prev != 0 { J::Float(div(last - prev, prev)) } else { J::Int(0) },
            "oldest_lot" => oldest,
        });
    }
    Ok(out)
}

/// v1 `totals`: cash, frozen, available, stock_value, total (as `(fields, cash + stock cents)`).
pub fn totals(c: &Connection, account_id: i64) -> Result<(Vec<(String, J)>, i64), ApiError> {
    let (cash, frozen): (i64, i64) = c
        .query_row("SELECT cash, cash_frozen FROM accounts WHERE id=?", [account_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(sql)?;
    let stock: i64 = c
        .query_row(
            "SELECT COALESCE(SUM(h.qty * co.last_price),0) FROM holdings h
             JOIN companies co ON co.id=h.company_id WHERE h.account_id=?",
            [account_id],
            |r| r.get(0),
        )
        .map_err(sql)?;
    let kv = vec![
        ("cash".to_string(), yuan(cash)),
        ("frozen".to_string(), yuan(frozen)),
        ("available".to_string(), yuan(cash - frozen)),
        ("stock_value".to_string(), yuan(stock)),
        ("total".to_string(), yuan(cash + stock)),
    ];
    Ok((kv, cash + stock))
}

/// `GET /api/me`
pub async fn me(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let p = current_player(st, r).await?;
    let st2 = st.clone();
    st.read(move |c| {
        let (t, total_cents) = totals(c, p.id)?;
        let initial = super::game(&st2.cfg, "initial_cash");
        let initial_f = match initial {
            J::Int(i) => i as f64,
            J::Float(f) => f,
            _ => return Err(internal("initial_cash")),
        };
        let rate = super::state_rate(c)?;
        let divs = c
            .query_row(
                "SELECT COALESCE(SUM(amount),0) AS v FROM ledger WHERE to_id=? AND reason='dividend'",
                [p.id],
                |r| Ok(col(r, "v")),
            )
            .map_err(sql)?;
        let total = total_cents as f64 / 100.0;
        let mut kv = vec![("id".to_string(), J::Int(p.id)), ("username".to_string(), p.username.clone())];
        kv.extend(t);
        kv.push(("initial".into(), initial));
        kv.push(("return_pct".into(), J::Float((total - initial_f) / initial_f)));
        kv.push(("dividends".into(), yuan(divs)));
        kv.push(("deposit_rate".into(), J::Float(super::deposit_rate(&st2.cfg, rate))));
        Ok(Reply::ok(J::Obj(kv)))
    })
    .await
}

/// `GET /api/portfolio`
pub async fn portfolio(st: &Arc<AppState>, r: &Req) -> ApiResult {
    let p = current_player(st, r).await?;
    st.read(move |c| {
        let mut kv = vec![("positions".to_string(), J::Arr(portfolio_rows(c, p.id)?))];
        kv.extend(totals(c, p.id)?.0);
        Ok(Reply::ok(J::Obj(kv)))
    })
    .await
}
