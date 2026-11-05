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
        Ok(applied) if !applied.is_empty() => {
            eprintln!("applied database migrations: {applied:?}");
            if applied.iter().any(|m| m.starts_with("V6__")) {
                let initial = (cfg.game.initial_cash * 100.0).round_ties_even() as i64;
                let res = store::connect(&cfg.game_db)
                    .and_then(|mut c| Ok(store::player_days::backfill(&mut c, initial, &cfg.game.day_rollover)?));
                match res {
                    Ok(store::player_days::Backfill::Written { days, players, rows }) => {
                        eprintln!("player_days: backfilled {rows} rows ({players} players, {days} days)")
                    }
                    Ok(store::player_days::Backfill::Skipped(why)) => {
                        eprintln!("player_days: no backfill ({why}); snapshots start at the next close")
                    }
                    Err(e) => eprintln!("player_days: backfill failed ({e}); snapshots start at the next close"),
                }
            }
        }
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
    }
    .with_rollover(&cfg.game.day_rollover);
    // the NPC market keeps its own (UTC+8, 04:00) day arithmetic
    if cfg.market.enabled
        && (anistock::config::hm(&cfg.game.day_rollover) - chrono::NaiveTime::MIN).num_seconds() != anistock::market::ROLLOVER_S
    {
        fail(format!("game.day_rollover {} must be 04:00 while [market] is enabled", cfg.game.day_rollover), 1);
    }
    // go-live of the filtered valuation (docs/v2-valuation.md): start it from the inputs history once
    match engine.valuation_go_live(&cfg, &clock.ts()) {
        Ok(Some(n)) => eprintln!("valuation: started the filter from the inputs history, {n} companies valued"),
        Ok(None) => {}
        Err(e) => eprintln!("valuation bootstrap failed (the filter starts at the next close): {e}"),
    }
    engine.flush().unwrap_or_else(|e| fail(format!("persist: {e}"), 1));
    // v1: DIST = ROOT / "web" / "dist" with ROOT resolved; mounted only if it exists at startup
    let root = std::fs::canonicalize(&cfg.root).unwrap_or_else(|_| cfg.root.clone());
    let dist = Some(root.join("web").join("dist")).filter(|d| d.exists());

    let cfg = Arc::new(cfg);
    let ops = EngineOps::new(&cfg);
    // real-time push (/api/ws): tee the engine's change stream into the fan-out on the engine thread
    let (change_tx, change_rx) = crossbeam_channel::unbounded();
    let (live_tx, live_rx) = crossbeam_channel::unbounded();
    engine.wrap_sink(|inner| Box::new(anistock::live::Tee { inner, tx: change_tx }));
    let turnover = today_turnover(&cfg.game_db, &clock.today());
    let fanout = anistock::live::Fanout::new(clock.clone(), cfg.clone(), change_rx, live_rx, turnover);
    let (actor, engine_thread) =
        Actor::start_live(Ctx { engine, cfg: cfg.clone(), ops: Box::new(ops) }, Some(fanout));
    // the v2 NPC market ([market].enabled): first start migrates the database (after a backup)
    let now = clock.now().timestamp();
    match actor.call_blocking(move |ctx| {
        let cfg = ctx.cfg.clone();
        ctx.ops.market_startup(&mut ctx.engine, &cfg, now)
    }) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => fail(format!("npc market: {e}"), 1),
        Err(_) => fail("engine thread gone", 1),
    }
    let st = Arc::new(AppState {
        actor: actor.clone(),
        cfg: cfg.clone(),
        clock,
        secret,
        reads: ReadPool::new(cfg.game_db.clone(), 8),
        dist,
        live: Some(live_tx),
        v1: anistock::api::v1::V1State::new(&cfg),
    });
    anistock::api::v1::search::refresh(&st);
    if st.v1.bots.is_empty() {
        eprintln!("ANISTOCK_BOT_TOKENS is not set: bot endpoints refuse every token");
    }

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
        let app = anistock::api::v1::router(st.clone()).fallback(move |req| dispatch(st2.clone(), req));
        let app = app.into_make_service_with_connect_info::<SocketAddr>();
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

/// Today's turnover per stock (cents), the starting point of the pushed quotes.
fn today_turnover(db: &std::path::Path, today: &str) -> std::collections::HashMap<i64, i64> {
    let q = || -> rusqlite::Result<std::collections::HashMap<i64, i64>> {
        let c = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut s = c.prepare("SELECT company_id, SUM(qty*price) FROM trades WHERE day=? GROUP BY company_id")?;
        let rows = s.query_map([today], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    };
    q().unwrap_or_else(|e| {
        eprintln!("live: today's turnover: {e}");
        Default::default()
    })
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
