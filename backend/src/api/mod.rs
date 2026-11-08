//! The HTTP layer: the public API v1 (`v1`, docs/api-v1.md) and the single-page app from `web/dist`
//! (`spa`). Everything else under `/api/` is 404 `not_found` in the v1 error format.
//!
//! Read paths run on pooled read-only SQLite connections inside one read transaction (a consistent
//! snapshot), on tokio's blocking pool. Writes go through the engine thread (`actor`), which flushes
//! before replying, so reads after a write see it.

pub mod spa;
pub mod v1;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, Method, Response};
use axum::response::IntoResponse;
use rusqlite::Connection;

use crate::actor::Actor;
use crate::config::{Clock, Config, Phase};

/// Shared by every handler.
pub struct AppState {
    pub actor: Actor,
    pub cfg: Arc<Config>,
    pub clock: Clock,
    pub secret: Vec<u8>,
    pub reads: ReadPool,
    /// `web/dist`, if it exists at startup
    pub dist: Option<PathBuf>,
    /// the real-time fan-out on the engine thread (`/api/v1/ws`); None in tools and tests
    pub live: Option<crossbeam_channel::Sender<crate::live::Ctl>>,
    /// API v1 state (rate limits, bot tokens, search names, side writer)
    pub v1: v1::V1State,
}

impl AppState {
    pub fn phase(&self) -> Phase {
        self.clock.phase(&self.cfg.game, self.cfg.always_open)
    }

    /// Seconds since the epoch (follows a frozen game clock, for tests).
    pub fn unix_now(&self) -> f64 {
        self.clock.now().timestamp_micros() as f64 / 1e6
    }
}

// ---------------------------------------------------------------- read pool

pub struct ReadPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    max_idle: usize,
}

impl ReadPool {
    pub fn new(path: PathBuf, max_idle: usize) -> ReadPool {
        ReadPool { path, idle: Mutex::new(Vec::new()), max_idle }
    }

    fn open(&self) -> rusqlite::Result<Connection> {
        let c = Connection::open(&self.path)?;
        c.busy_timeout(std::time::Duration::from_millis(5000))?;
        c.pragma_update(None, "query_only", "ON")?;
        Ok(c)
    }

    /// Run `f` on a pooled connection inside one read transaction.
    pub fn run<R>(&self, f: impl FnOnce(&Connection) -> R) -> rusqlite::Result<R> {
        let pooled = self.idle.lock().unwrap().pop();
        let conn = match pooled {
            Some(c) => c,
            None => self.open()?,
        };
        conn.execute_batch("BEGIN")?;
        let r = f(&conn);
        let _ = conn.execute_batch("COMMIT");
        let mut idle = self.idle.lock().unwrap();
        if idle.len() < self.max_idle {
            idle.push(conn);
        }
        Ok(r)
    }
}

// ---------------------------------------------------------------- everything outside the v1 router

/// Unknown `/api/*` paths: 404 `not_found` (v1 format); `/covers/{id}.jpg`: the cover images (static, no
/// rate limit); `/assets/*` and other GETs: the SPA.
pub async fn fallback(st: Arc<AppState>, req: Request) -> Response<Body> {
    let (parts, _) = req.into_parts();
    let path = parts.uri.path();
    if path == "/api" || path.starts_with("/api/") {
        let mut r = v1::error::not_found().into_response();
        r.headers_mut().insert("aniex-version", HeaderValue::from_static(v1::VERSION));
        return r;
    }
    if let Some(name) = path.strip_prefix("/covers/") {
        if parts.method != Method::GET && parts.method != Method::HEAD {
            return spa::plain(405, "Method Not Allowed");
        }
        return v1::stocks::cover_file(&st, name).await;
    }
    let Some(dist) = &st.dist else { return spa::plain(404, "Not Found") };
    if let Some(rest) = path.strip_prefix("/assets/") {
        return spa::asset(dist, rest, &parts.method, &parts.headers).await;
    }
    if parts.method == Method::GET || parts.method == Method::HEAD {
        let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned();
        return spa::catch_all(dist, &decoded).await;
    }
    spa::plain(405, "Method Not Allowed")
}
