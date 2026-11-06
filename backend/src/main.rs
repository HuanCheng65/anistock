//! AniStock (安易 AniEx) game server: the public API v1 (`/api/v1`, docs/api-v1.md), the real-time push
//! (`/api/v1/ws`) and the web app from `web/dist`, on the game database (passwords and session cookies
//! are compatible with every earlier release).
//!
//!     ANISTOCK_HOST=127.0.0.1 ANISTOCK_PORT=18620 anistock
//!     anistock replay-history ...     (operator command: the pre-launch history, src/sim/history.rs)
//!
//! Environment (besides the ones in `config.rs`: ANISTOCK_SECRET, ANISTOCK_INVITE, ANISTOCK_DB,
//! ANISTOCK_GAME_DB, ANISTOCK_ALWAYS_OPEN, ANISTOCK_NO_LOOPS, ANISTOCK_ROOT):
//!   ANISTOCK_HOST / ANISTOCK_PORT   listen address (default 127.0.0.1:18620, like the systemd unit)
//!   ANISTOCK_WORKERS                tokio worker threads (default 2)
//!   ANISTOCK_BOT_TOKENS             bot tokens for the QQ bot, `name:token[,name:token]` (docs/api-v1.md §3.2)
//!   ANISTOCK_FREEZE_TIME            development only: pin the game clock (and session times) to an ISO
//!                                   timestamp, e.g. 2026-11-05T10:00:00+08:00
//!   ANISTOCK_NO_RATE_LIMIT=1        development only: no API rate limits
//!
//! `anistock reset-code <username>` issues a password reset code for a player who cannot use the QQ
//! bot (no binding): check who is asking first. The code is valid 10 minutes.
//!
//! `anistock announce <title> <body-file> [--no-popup]` posts an announcement, shown once to every player who
//! opened an account before it as a popup after sign-in.
//!
//! `anistock surveillance [days]` prints the wash-trade report (对倒监控, src/surveil.rs) for the last
//! `days` trading days (default 7) and exits; it only reads the game database.
//!
//! Threads: the tokio workers (HTTP), tokio's blocking pool (SQLite reads, scrypt), the engine thread
//! (`actor`, with the push fan-out between jobs), the persistence thread. Behind the local reverse proxy
//! the client address comes from X-Real-IP (else the last X-Forwarded-For entry), for the per-IP limits.

use std::net::SocketAddr;
use std::sync::Arc;

use anistock::actor::{Actor, Ctx, EngineOps};
use anistock::api::{fallback, AppState, ReadPool};
use anistock::{auth, store, Clock, Config, Engine};

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn fail(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("{msg}");
    std::process::exit(code)
}

/// `anistock surveillance [days]`: trades between accounts that signed in from the same address, at
/// a price more than 5% off the day's VWAP or more than 20% of the stock's volume that day.
fn surveillance(cfg: &Config, days: i64) {
    let c = store::connect(&cfg.game_db).unwrap_or_else(|e| fail(format!("{}: {e}", cfg.game_db.display()), 1));
    let today = chrono::Utc::now().with_timezone(&cfg.tz).date_naive();
    let day = |n: i64| (today - chrono::Duration::days(n)).format("%Y-%m-%d").to_string();
    let flags = anistock::surveil::report(&c, &day(days), &day(0), &day(30), 0.05, 0.2)
        .unwrap_or_else(|e| fail(format!("report: {e}"), 1));
    if flags.is_empty() {
        println!("no flagged trades in the last {days} days");
    }
    for f in flags {
        println!(
            "{}  trade {}  stock {}  {} <- {}  {} x {:.2} (VWAP {:.2}, {:.0}% of the day's volume)  shared address {}",
            f.day,
            f.trade_id,
            f.company_id,
            f.buyer,
            f.seller,
            f.qty,
            f.price as f64 / 100.0,
            f.vwap / 100.0,
            f.volume_share * 100.0,
            f.ip
        );
    }
}

/// `anistock reset-code <username>`
fn reset_code(cfg: &Config, username: &str) {
    let mut c = store::connect(&cfg.game_db).unwrap_or_else(|e| fail(format!("{}: {e}", cfg.game_db.display()), 1));
    let id: i64 = c
        .query_row("SELECT id FROM accounts WHERE username=? AND kind='player'", [username], |r| r.get(0))
        .unwrap_or_else(|_| fail(format!("no player named {username:?}"), 1));
    let now = chrono::Utc::now().with_timezone(&cfg.tz);
    let (code, expires) = anistock::api::v1::password::issue_code(&mut c, id, "operator", &now)
        .unwrap_or_else(|e| fail(format!("reset code: {e}"), 1));
    println!("{username}: {code} (valid until {expires}); enter it at 忘记密码 on the login page");
}

/// `anistock announce <title> <body-file> [--no-popup]`: post an announcement (shown once to each player as a
/// popup after sign-in, docs/api-v1.md `Me.announcement`). Works while the server runs (a side table).
fn announce(cfg: &Config, args: &[String]) {
    let usage = || fail("usage: anistock announce <title> <body-file> [--no-popup]", 2);
    let (Some(title), Some(file)) = (args.first(), args.get(1)) else { usage() };
    let popup = !args.iter().any(|a| a == "--no-popup");
    let body = std::fs::read_to_string(file).unwrap_or_else(|e| fail(format!("{file}: {e}"), 1));
    let body = body.trim();
    if title.trim().is_empty() || body.is_empty() {
        usage();
    }
    store::migrate(&cfg.game_db).unwrap_or_else(|e| fail(format!("{}: {e}", cfg.game_db.display()), 1));
    let c = store::connect(&cfg.game_db).unwrap_or_else(|e| fail(format!("{}: {e}", cfg.game_db.display()), 1));
    let now = anistock::config::format_ts(&chrono::Utc::now().with_timezone(&cfg.tz));
    c.execute("INSERT INTO announcements(created_at, title, body, popup) VALUES (?,?,?,?)", rusqlite::params![now, title.trim(), body, popup as i64])
        .unwrap_or_else(|e| fail(format!("announce: {e}"), 1));
    println!("announcement {} posted at {now}{}", c.last_insert_rowid(), if popup { " (popup)" } else { "" });
}

fn main() {
    // operator command (docs/ops-rebuild-replay.md): run with the server stopped
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("replay-history") {
        anistock::sim::history::cli(&args[1..]);
        return;
    }
    let mut cfg = Config::load().unwrap_or_else(|e| fail(format!("config: {e}"), 2));
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("reset-code") {
        let Some(name) = args.get(2) else { fail("usage: anistock reset-code <username>", 2) };
        reset_code(&cfg, name);
        return;
    }
    if args.get(1).map(String::as_str) == Some("announce") {
        announce(&cfg, &args[2..]);
        return;
    }
    if args.get(1).map(String::as_str) == Some("surveillance") {
        let days = args.get(2).map_or(7, |d| d.parse().unwrap_or_else(|_| fail("usage: anistock surveillance [days]", 2)));
        surveillance(&cfg, days);
        return;
    }
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
    engine.margin = cfg.margin.clone();
    engine.corporate = cfg.corporate.clone();

    // the session key: ANISTOCK_SECRET, else the random one kept in the game database
    let stored = engine.state("session_secret").map(str::to_string);
    let secret = auth::resolve_secret(&cfg.secret, stored, |k| engine.set_state("session_secret", k))
        .unwrap_or_else(|e| fail(e, 1));
    // the market's sentiment draws are salted with the same key: without ANISTOCK_SECRET the public dev key
    // would let anyone compute them from the repository
    cfg.secret = secret.clone();
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
    // real-time push (/api/v1/ws): tee the engine's change stream into the fan-out on the engine thread
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
        let app = anistock::api::v1::router(st.clone()).fallback(move |req| fallback(st2.clone(), req));
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
