//! Background loops, as in v1 (`npc_loop`, `daily_loop`), disabled by `ANISTOCK_NO_LOOPS=1`.
//!
//! - NPCs: every `npc.tick_seconds`, unless the market is closed: when continuous trading has started,
//!   first the day's opening auction (`open_market`, once a day), then one NPC round (resting orders
//!   only during the auction). One engine job, like v1's one transaction.
//! - Daily close: when `last_closed` is before yesterday and the time is past `daily_step_at`: refresh
//!   the derived market data and build each day's inputs (Python subprocesses, run outside the engine
//!   thread so trading is not held up, unlike v1 which held its lock), then close each missing day (one
//!   engine job per day) and back up. Sleeps until the next `daily_step_at`, between 60 s and 1 h at a
//!   time.

use std::sync::Arc;
use std::time::Duration;

use crate::api::AppState;
use crate::config::{hm, next_daily_step, prev_day, Phase};

pub fn spawn(st: &Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    vec![tokio::spawn(npc_loop(st.clone())), tokio::spawn(daily_loop(st.clone())), tokio::spawn(margin_loop(st.clone()))]
}

/// 融资融券 (crate::margin): every 30 seconds of continuous trading, lift covered margin calls and run one
/// round of every forced liquidation.
async fn margin_loop(st: Arc<AppState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        if st.phase() != Phase::Continuous {
            continue;
        }
        let (day, ts) = (st.clock.today(), st.clock.ts());
        let secs = chrono::Timelike::num_seconds_from_midnight(&st.clock.now().time()) as i64;
        if st
            .actor
            .call(move |ctx| {
                ctx.engine.margin_sweep(&day, &ts);
                // the buyback desk (crate::corporate)
                ctx.engine.corporate_minute(&day, &ts, secs);
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn npc_loop(st: Arc<AppState>) {
    let market = st.actor.call(|ctx| ctx.ops.market_enabled()).await.unwrap_or(false);
    if market {
        return market_loop(st).await;
    }
    let every = Duration::from_secs_f64(st.cfg.npc.tick_seconds.max(0.05));
    loop {
        tokio::time::sleep(every).await;
        let ph = st.phase();
        if ph == Phase::Closed {
            continue;
        }
        let (day, ts) = (st.clock.today(), st.clock.ts());
        let r = st
            .actor
            .call(move |ctx| {
                if ph == Phase::Continuous {
                    ctx.engine.open_market(&day, &ts);
                }
                let cfg = ctx.cfg.clone();
                ctx.ops.npc_tick(&mut ctx.engine, &cfg, &day, &ts, ph == Phase::Auction);
            })
            .await;
        if r.is_err() {
            return; // shutting down
        }
    }
}

/// The v2 NPC market: one engine job a second (the agents act at their own Poisson times inside it), and
/// the hourly 热度快讯 fetch (a Python subprocess, outside the engine thread) while shows air.
async fn market_loop(st: Arc<AppState>) {
    let mut last_fetch_hour = -1i64;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let now = st.clock.now().timestamp();
        if st.actor.call(move |ctx| ctx.ops.market_tick(&mut ctx.engine, now)).await.is_err() {
            return;
        }
        // at five past every hour from 08:05 to 03:05 (late-night anime; the session runs to 04:00), fetch
        // the episode comments of the last 26 hours
        let (_, s) = crate::market::local(now);
        let hour = now.div_euclid(3600);
        let h = s / 3600;
        if (s % 3600) >= 300 && hour != last_fetch_hour && (h >= 8 || h < 4) {
            last_fetch_hour = hour;
            let cfg = st.cfg.clone();
            tokio::task::spawn_blocking(move || crate::inputs::fetch_flashes(&cfg));
        }
    }
}

async fn daily_loop(st: Arc<AppState>) {
    loop {
        let last = st.actor.call(|ctx| ctx.engine.state("last_closed").map(str::to_string)).await;
        let Ok(last) = last else { return };
        let yesterday = prev_day(&st.clock.today());
        let due = st.clock.now().time() >= hm(&st.cfg.game.daily_step_at);
        if last.as_deref().unwrap_or("") < yesterday.as_str() && due {
            let cfg = st.cfg.clone();
            let _ = tokio::task::spawn_blocking(move || crate::inputs::refresh_market_data(&cfg)).await;
            match catch_up(&st).await {
                Ok(Some(n)) => eprintln!("daily close: {n} day(s) closed"),
                Ok(None) => {}
                Err(e) => eprintln!("daily close failed: {e}"),
            }
        }
        let now = st.clock.now();
        let wait = (next_daily_step(&now, &st.cfg.game.daily_step_at) - now).num_milliseconds() as f64 / 1000.0;
        tokio::time::sleep(Duration::from_secs_f64(wait.clamp(60.0, 3600.0))).await;
    }
}

/// v1 `app.catch_up`: close every day from the last closed one up to yesterday, then back up the
/// database. Each day's inputs come from the Python model (`inputs::fetch`, a subprocess of up to
/// minutes) on the blocking pool, so trading goes on meanwhile; the close itself is one engine job, stamped
/// with the clock when it runs (like v1, which reads `clock.ts()` per day). Stops at the first failure
/// without a backup, like v1's exception. Returns the number of days closed (None: the engine stopped).
pub async fn catch_up(st: &Arc<AppState>) -> Result<Option<usize>, String> {
    let Ok((last, window_start)) = st
        .actor
        .call(|ctx| {
            (ctx.engine.state("last_closed").map(str::to_string), ctx.engine.state("window_start").map(str::to_string))
        })
        .await
    else {
        return Ok(None);
    };
    let last = last.ok_or("state 'last_closed' missing")?;
    let parse = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|e| format!("{d}: {e}"));
    let mut d = parse(&last)? + chrono::Duration::days(1);
    let yesterday_s = prev_day(&st.clock.today());
    let yesterday = parse(&yesterday_s)?;
    let (mut closed, mut any) = (0, false);
    while d <= yesterday {
        let day = d.format("%Y-%m-%d").to_string();
        let (cfg, ws, day2) = (st.cfg.clone(), window_start.clone(), day.clone());
        let inputs = tokio::task::spawn_blocking(move || crate::inputs::fetch(&cfg, &day2, ws.as_deref()))
            .await
            .map_err(|e| format!("inputs for {day}: {e}"))??;
        let clock = st.clock.clone();
        let r = st
            .actor
            .call(move |ctx| {
                let cfg = ctx.cfg.clone();
                let low: std::collections::HashMap<i64, i64> = ctx.engine.companies().map(|c| (c.id, c.low_weeks)).collect();
                let now = clock.now();
                let out = ctx.engine.run_day(&cfg, &day, &crate::config::format_ts(&now), &inputs)?;
                if matches!(out, crate::daily::CloseOutcome::Closed(_)) {
                    ctx.ops.market_after_close(&mut ctx.engine, &cfg, &inputs, now.timestamp(), &low)?;
                }
                Ok::<_, String>(out)
            })
            .await;
        match r {
            Ok(r) => {
                if matches!(r?, crate::daily::CloseOutcome::Closed(_)) {
                    closed += 1;
                    after_close(st, &d.format("%Y-%m-%d").to_string()).await;
                }
            }
            Err(_) => return Ok(None),
        }
        any = true;
        d += chrono::Duration::days(1);
    }
    if any {
        // v1 backs up whenever catch_up went through at least one day
        let r = st
            .actor
            .call(move |ctx| {
                let db = ctx.cfg.game_db.clone();
                ctx.engine.daily_backup(&db, &yesterday_s)
            })
            .await;
        if let Ok(Err(e)) = r {
            return Err(format!("backup: {e}"));
        }
    }
    Ok(Some(closed))
}

/// After a close: the `daily_close` event (and `leaderboard_weekly` after a Sunday), `time` to every
/// connection, and the search names rebuilt (the collector refreshed its snapshots before the close).
async fn after_close(st: &Arc<AppState>, day: &str) {
    let _ = st.actor.call(|ctx| ctx.engine.flush()).await;
    let (st2, d) = (st.clone(), day.to_string());
    let events = tokio::task::spawn_blocking(move || {
        crate::api::v1::search::refresh(&st2);
        st2.reads.run(|c| {
            let mut out = Vec::new();
            let at = st2.clock.ts();
            if let Ok(v) = crate::api::v1::events::daily_close(c, &d) {
                out.push(("daily_close", v));
            }
            let sunday = chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                .is_ok_and(|x| chrono::Datelike::weekday(&x) == chrono::Weekday::Sun);
            if sunday {
                if let Ok(v) = crate::api::v1::events::leaderboard_weekly(c, &d) {
                    out.push(("leaderboard_weekly", v));
                }
            }
            (out, at)
        })
    })
    .await;
    let Some(live) = &st.live else { return };
    if let Ok(Ok((evs, at))) = events {
        for (kind, data) in evs {
            let _ = live.send(crate::live::Ctl::Emit(crate::api::v1::events::Pending {
                kind: kind.into(),
                at: at.clone(),
                trading_day: day.to_string(),
                stock: None,
                data,
                news_id: None,
            }));
        }
    }
    let _ = live.send(crate::live::Ctl::Time);
}
