//! AniStock v2 game server: a drop-in replacement for the v1 Python server (same port, database,
//! frontend, cookies and passwords).
//!
//!     ANISTOCK_HOST=127.0.0.1 ANISTOCK_PORT=18620 anistock
//!
//! Environment (besides the ones in `config.rs`: ANISTOCK_SECRET, ANISTOCK_INVITE, ANISTOCK_DB,
//! ANISTOCK_GAME_DB, ANISTOCK_ALWAYS_OPEN, ANISTOCK_NO_LOOPS, ANISTOCK_ROOT):
//!   ANISTOCK_HOST / ANISTOCK_PORT   listen address (default 127.0.0.1:18620, like the systemd unit)
//!   ANISTOCK_WORKERS                tokio worker threads (default 2)
//!   ANISTOCK_FREEZE_TIME            development only: pin the game clock (and session times) to an ISO
//!                                   timestamp, e.g. 2026-11-05T10:00:00+08:00 (the API difftest uses it)
//!
//! Threads: the tokio workers (HTTP), tokio's blocking pool (SQLite reads, scrypt), the engine thread
//! (`actor`), the persistence thread. v1 ran behind uvicorn `--proxy-headers`; v1 never reads the client
//! address, so v2 ignores X-Forwarded-For (X-Forwarded-Proto is only used in the slash redirect, which
//! exists only when web/dist is missing).

use std::net::SocketAddr;
use std::sync::Arc;

use anistock::actor::{Actor, Ctx, EngineOps};
use anistock::api::{dispatch, AppState, ReadPool};
use anistock::{auth, store, Clock, Config, Engine};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn fail(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("{msg}");
    std::process::exit(code)
}

fn main() {
    let cfg = Config::load().unwrap_or_else(|e| fail(format!("config: {e}"), 2));
    let host = env("ANISTOCK_HOST").unwrap_or_else(|| "127.0.0.1".into());
    let port: u16 = env("ANISTOCK_PORT").map_or(18620, |p| p.trim().parse().unwrap_or_else(|_| fail("ANISTOCK_PORT", 2)));
    let workers: usize = env("ANISTOCK_WORKERS").map_or(2, |w| w.trim().parse().unwrap_or_else(|_| fail("ANISTOCK_WORKERS", 2)));

    if !cfg.game_db.exists() {
        fail(format!("{} missing: run `uv run python -m server.seed` first", cfg.game_db.display()), 1);
    }
    match store::migrate(&cfg.game_db) {
        Ok(applied) if !applied.is_empty() => eprintln!("applied database migrations: {applied:?}"),
        Ok(_) => {}
        Err(e) => fail(format!("{}: {e}", cfg.game_db.display()), 1),
    }
    let mut engine = Engine::open(&cfg.game_db, cfg.fees(), cfg.rules())
        .unwrap_or_else(|e| fail(format!("refusing to start: {e}"), 1));

    // the session key: ANISTOCK_SECRET, else the random one kept in the game database
    let stored = engine.state("session_secret").map(str::to_string);
    let secret = auth::resolve_secret(&cfg.secret, stored, |k| engine.set_state("session_secret", k))
        .unwrap_or_else(|e| fail(e, 1));
    engine.flush().unwrap_or_else(|e| fail(format!("persist: {e}"), 1));

    let clock = match env("ANISTOCK_FREEZE_TIME") {
        Some(t) => {
            eprintln!("ANISTOCK_FREEZE_TIME: the clock is frozen at {t} (development only)");
            Clock::fixed(cfg.tz, t.trim())
        }
        None => Clock::new(cfg.tz),
    };
    // v1: DIST = ROOT / "web" / "dist" with ROOT resolved; mounted only if it exists at startup
    let root = std::fs::canonicalize(&cfg.root).unwrap_or_else(|_| cfg.root.clone());
    let dist = Some(root.join("web").join("dist")).filter(|d| d.exists());

    let cfg = Arc::new(cfg);
    let (actor, engine_thread) = Actor::start(Ctx { engine, cfg: cfg.clone(), ops: Box::new(EngineOps::default()) });
    let st = Arc::new(AppState {
        actor: actor.clone(),
        cfg: cfg.clone(),
        clock,
        secret,
        reads: ReadPool::new(cfg.game_db.clone(), 8),
        dist,
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers.max(1))
        .max_blocking_threads(32)
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(format!("tokio: {e}"), 1));
    rt.block_on(async {
        let addr: SocketAddr = format!("{host}:{port}").parse().unwrap_or_else(|e| fail(format!("{host}:{port}: {e}"), 2));
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| fail(format!("bind {addr}: {e}"), 1));
        let loops = if cfg.no_loops { vec![] } else { anistock::loops::spawn(&st) };
        eprintln!("AniStock v2 listening on http://{addr} ({workers} workers, game db {})", cfg.game_db.display());
        let st2 = st.clone();
        let app = axum::Router::new().fallback(move |req| dispatch(st2.clone(), req));
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await {
            eprintln!("server: {e}");
        }
        for l in loops {
            l.abort();
        }
    });
    // everything the engine did is committed before the process ends
    match actor.call_blocking(|ctx| ctx.engine.flush()) {
        Ok(Err(e)) => eprintln!("final flush: {e}"),
        Err(_) => eprintln!("final flush: engine thread gone"),
        Ok(Ok(())) => {}
    }
    drop(engine_thread);
    eprintln!("stopped");
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
