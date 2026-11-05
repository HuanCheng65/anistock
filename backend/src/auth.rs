//! Passwords and sessions, compatible with v1 (`server/app.py`), so players keep their passwords and
//! their login cookies when v2 replaces v1 (and a v2 cookie works on v1).
//!
//! - password: scrypt(N=2^14, r=8, p=1, dklen=64) stored as `"<salt hex>:<hash hex>"`, 16-byte salt
//!   (`hash_pw` / `check_pw`)
//! - session cookie `session` = `"{account_id}.{exp}.{hmac_sha256_hex(SECRET, f"{account_id}.{exp}")}"`,
//!   valid for 30 days (`make_session` / `read_session`)
//! - SECRET: `ANISTOCK_SECRET`, else (v1 `lifespan`) a random 32-byte key kept hex-encoded in state
//!   `session_secret`, created on first start. v1 compares against the literal default
//!   `dev-only-secret-change-me`, so setting that value explicitly also selects the stored key.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub const SESSION_DAYS: i64 = 30;
pub const DEV_SECRET: &[u8] = b"dev-only-secret-change-me";

fn scrypt_hex(pw: &str, salt: &[u8]) -> String {
    let params = scrypt::Params::new(14, 8, 1, 64).expect("scrypt params");
    let mut out = [0u8; 64];
    scrypt::scrypt(pw.as_bytes(), salt, &params, &mut out).expect("scrypt output length");
    hex::encode(out)
}

/// v1 `hash_pw` with a fresh random salt. CPU-heavy (~50 ms): call from a blocking thread.
pub fn hash_pw(pw: &str) -> String {
    let salt: [u8; 16] = rand::random();
    hash_pw_with_salt(pw, &salt)
}

pub fn hash_pw_with_salt(pw: &str, salt: &[u8]) -> String {
    format!("{}:{}", hex::encode(salt), scrypt_hex(pw, salt))
}

/// Why `check_pw` could not even compare: v1 raises (an uncaught exception, so a 500) on these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PwError {
    /// `pw_hash` is NULL (`None.split` -> AttributeError), or not `salt:hash` (unpacking ValueError), or
    /// the salt is not hex (`bytes.fromhex` ValueError)
    Malformed,
}

/// v1 `check_pw(pw, stored)`.
pub fn check_pw(pw: &str, stored: Option<&str>) -> Result<bool, PwError> {
    let stored = stored.ok_or(PwError::Malformed)?;
    let parts: Vec<&str> = stored.split(':').collect();
    let [salt, h] = parts[..] else { return Err(PwError::Malformed) };
    let salt = py_fromhex(salt).ok_or(PwError::Malformed)?;
    let mine = scrypt_hex(pw, &salt);
    // hmac.compare_digest(str, str): v1 raises TypeError if `h` is not ASCII
    if !h.is_ascii() {
        return Err(PwError::Malformed);
    }
    Ok(mine.as_bytes().ct_eq(h.as_bytes()).into())
}

/// Python `bytes.fromhex`: pairs of hex digits, ASCII whitespace allowed between pairs.
pub fn py_fromhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let hi = (b[i] as char).to_digit(16)?;
        let lo = (*b.get(i + 1)? as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
        i += 2;
    }
    Some(out)
}

fn sign(secret: &[u8], payload: &str) -> String {
    let mut m = <Hmac<Sha256>>::new_from_slice(secret).expect("hmac accepts any key length");
    m.update(payload.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

/// v1 `make_session`: `now` is `time.time()` (seconds since the epoch).
pub fn make_session(secret: &[u8], account_id: i64, now: f64) -> String {
    let exp = now.trunc() as i64 + SESSION_DAYS * 86400;
    let payload = format!("{account_id}.{exp}");
    let sig = sign(secret, &payload);
    format!("{payload}.{sig}")
}

/// Outcome of v1 `read_session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// no cookie, wrong shape, bad signature or expired: 401 for `current_player`
    None,
    Player(i64),
    /// v1 raises (500): a validly signed cookie whose fields are not ints (never issued by either
    /// version), or a non-ASCII signature (`compare_digest` TypeError)
    Crash,
}

/// v1 `read_session(token)`.
pub fn read_session(secret: &[u8], token: Option<&str>, now: f64) -> Session {
    let Some(token) = token.filter(|t| !t.is_empty()) else { return Session::None };
    let parts: Vec<&str> = token.split('.').collect();
    let [aid, exp, sig] = parts[..] else { return Session::None };
    let good = sign(secret, &format!("{aid}.{exp}"));
    if !sig.is_ascii() {
        return Session::Crash;
    }
    if !bool::from(sig.as_bytes().ct_eq(good.as_bytes())) {
        return Session::None;
    }
    let Some(exp) = py_int(exp) else { return Session::Crash };
    if (exp as f64) < now {
        return Session::None;
    }
    match py_int(aid) {
        Some(a) => Session::Player(a),
        None => Session::Crash,
    }
}

/// An integer field of a signed cookie (an optional sign and digits).
fn py_int(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}

/// `Set-Cookie` of v1 `set_cookie` (Starlette `Response.set_cookie(..., max_age, httponly, samesite="lax")`).
pub fn session_cookie(token: &str) -> String {
    format!("session={token}; HttpOnly; Max-Age={}; Path=/; SameSite=lax", SESSION_DAYS * 86400)
}

/// `Set-Cookie` of Starlette `delete_cookie("session")` (expires = now, in the real wall clock).
pub fn delete_cookie(now: std::time::SystemTime) -> String {
    format!("session=\"\"; expires={}; Max-Age=0; Path=/; SameSite=lax", httpdate::fmt_http_date(now))
}

/// Starlette's cookie parser (`cookie_parser`): `;`-separated, `key=value` (a chunk without `=` is a
/// value with an empty key), both stripped, value unquoted the `http.cookies` way. Last one wins.
pub fn cookie_value(header_values: &[&str], name: &str) -> Option<String> {
    let mut found = None;
    for h in header_values {
        for chunk in h.split(';') {
            let (k, v) = match chunk.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => ("", chunk.trim()),
            };
            if (!k.is_empty() || !v.is_empty()) && k == name {
                found = Some(unquote_cookie(v));
            }
        }
    }
    found
}

/// `http.cookies._unquote`: strip surrounding double quotes and undo `\"`, `\\` and `\ooo` escapes.
fn unquote_cookie(v: &str) -> String {
    if v.len() < 2 || !v.starts_with('"') || !v.ends_with('"') {
        return v.to_string();
    }
    let inner = &v[1..v.len() - 1];
    let b = inner.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push(u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 4]).unwrap(), 8).unwrap_or(0));
            i += 4;
        } else if b[i] == b'\\' && i + 1 < b.len() {
            out.push(b[i + 1]);
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The signing key: `ANISTOCK_SECRET` if set (and not the dev default), else state `session_secret`
/// (hex), creating it when missing. `stored` reads the state row; `store` writes a new one.
pub fn resolve_secret(
    configured: &[u8],
    stored: Option<String>,
    store: impl FnOnce(&str),
) -> Result<Vec<u8>, String> {
    if configured != DEV_SECRET {
        return Ok(configured.to_vec());
    }
    let key = match stored.filter(|k| !k.is_empty()) {
        Some(k) => k,
        None => {
            let k = hex::encode(rand::random::<[u8; 32]>());
            store(&k);
            k
        }
    };
    eprintln!("ANISTOCK_SECRET is not set: using a random key stored in the game database");
    py_fromhex(&key).ok_or_else(|| "state 'session_secret' is not hex".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Produced by v1's code:
    //   app.hash_pw("secret0", bytes([1]) * 16)
    //   hmac.new(b"difftest-secret-0123456789", b"4.1796436000", hashlib.sha256).hexdigest()
    const PY_HASH: &str = "01010101010101010101010101010101:fc39547353e98b250f08737cbbc7c67261ffd827cbad302530135011c589666dfe8fab7faaedbc8e37eff130e408d520efd07333bcac0e39f835733a6f13d407";
    /// the cookie v1 set for account 4 in the API difftest (`ANISTOCK_SECRET=difftest-secret-0123456789`)
    const PY_COOKIE: &str = "4.1796436000.8a08b49b3ed04e2f74c886bb55fa06b1206c959cd1ed9056156e792b680f8d57";

    #[test]
    fn password_from_python() {
        let h = hash_pw_with_salt("secret0", &[1u8; 16]);
        assert_eq!(h, PY_HASH);
        assert_eq!(check_pw("secret0", Some(&h)), Ok(true));
        assert_eq!(check_pw("secret1", Some(&h)), Ok(false));
        assert_eq!(check_pw("x", None), Err(PwError::Malformed));
        assert_eq!(check_pw("x", Some("abc")), Err(PwError::Malformed));
        assert_eq!(check_pw("x", Some("zz:00")), Err(PwError::Malformed));
        let fresh = hash_pw("长密码长密码");
        assert_eq!(check_pw("长密码长密码", Some(&fresh)), Ok(true));
    }

    #[test]
    fn sessions() {
        let secret = b"difftest-secret-0123456789";
        let t = make_session(secret, 4, 1793844000.25);
        assert_eq!(t, PY_COOKIE);
        assert_eq!(read_session(secret, Some(&t), 1793844001.0), Session::Player(4));
        assert_eq!(read_session(secret, Some(&t), 1796436000.5), Session::None);
        assert_eq!(read_session(b"other", Some(&t), 1793844001.0), Session::None);
        assert_eq!(read_session(secret, Some("garbage"), 0.0), Session::None);
        assert_eq!(read_session(secret, None, 0.0), Session::None);
        assert_eq!(cookie_value(&["a=1; session=abc.d"], "session").as_deref(), Some("abc.d"));
        assert_eq!(cookie_value(&["session=\"x\\\"y\""], "session").as_deref(), Some("x\"y"));
    }
}
