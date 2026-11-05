//! Background loops, as in v1 (`npc_loop`, `daily_loop`), disabled by `ANISTOCK_NO_LOOPS=1`.
//!
//! - NPCs: every `npc.tick_seconds`, unless the market is closed: when continuous trading has started,
//!   first the day's opening auction (`open_market`, once a day), then one NPC round (resting orders
//!   only during the auction). One engine job, like v1's one transaction.
//! - Daily close: when `last_closed` is before yesterday and the time is past `daily_step_at`: refresh
//!   the derived market data (Python subprocesses, run outside the engine thread so trading is not held
//!   up, unlike v1 which held its lock), then close every missing day and back up (engine job). Sleeps
//!   until the next `daily_step_at`, between 60 s and 1 h at a time.

use std::sync::Arc;
use std::time::Duration;

use crate::api::AppState;
use crate::config::{hm, next_daily_step, prev_day, Phase};

pub fn spawn(st: &Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    vec![tokio::spawn(npc_loop(st.clone())), tokio::spawn(daily_loop(st.clone()))]
}

async fn npc_loop(st: Arc<AppState>) {
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

async fn daily_loop(st: Arc<AppState>) {
    loop {
        let last = st.actor.call(|ctx| ctx.engine.state("last_closed").map(str::to_string)).await;
        let Ok(last) = last else { return };
        let yesterday = prev_day(&st.clock.today());
        let due = st.clock.now().time() >= hm(&st.cfg.game.daily_step_at);
        if last.as_deref().unwrap_or("") < yesterday.as_str() && due {
            let cfg = st.cfg.clone();
            let _ = tokio::task::spawn_blocking(move || crate::inputs::refresh_market_data(&cfg)).await;
            let (today, ts) = (st.clock.today(), st.clock.ts());
            let r = st
                .actor
                .call(move |ctx| {
                    let cfg = ctx.cfg.clone();
                    ctx.ops.catch_up(&mut ctx.engine, &cfg, &today, &ts)
                })
                .await;
            match r {
                Ok(Ok(n)) => eprintln!("daily close: {n} day(s) closed"),
                Ok(Err(e)) => eprintln!("daily close failed: {e}"),
                Err(_) => return,
            }
        }
        let now = st.clock.now();
        let wait = (next_daily_step(&now, &st.cfg.game.daily_step_at) - now).num_milliseconds() as f64 / 1000.0;
        tokio::time::sleep(Duration::from_secs_f64(wait.clamp(60.0, 3600.0))).await;
    }
}
