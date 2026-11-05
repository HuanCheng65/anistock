//! The engine thread: one dedicated OS thread owns the `Engine` (all mutable game state) and runs jobs
//! sent by the HTTP handlers and the background loops, one at a time, in arrival order. A job is a
//! closure over `&mut Ctx`; its result comes back on a oneshot channel. No locks on game state.
//!
//! Write-then-read consistency: every player-facing mutation (register, order, cancel, subscribe) ends
//! with `engine.flush()` before it replies, so a request the player sends after seeing the reply also
//! sees the change in the database the read endpoints query. (Persistence commits in 50 ms batches
//! otherwise; `Msg::Flush` closes the batch at once, so the cost is one commit, not 50 ms.) NPC ticks do
//! not flush: their effects show up within 50 ms.
//!
//! A panic inside a job means the engine may hold a half-applied operation: the process exits (code 70)
//! instead of continuing; systemd restarts it from the database, which only ever holds whole operations.
//!
//! Engine features owned by other modules (IPO subscription, NPCs) are reached through `GameOps`, so the
//! HTTP layer does not depend on their exact signatures. The daily close is driven by `loops.rs`.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{unbounded, Receiver, Sender};

use crate::config::Config;
use crate::engine::Engine;
use crate::exchange::OrderError;
use crate::pyrandom::PyRandom;

/// Engine operations implemented outside the exchange core.
pub trait GameOps: Send {
    /// One round of NPC activity (v1 `npc.tick`); during the opening auction they only rest orders.
    fn npc_tick(&mut self, e: &mut Engine, cfg: &Config, day: &str, ts: &str, auction: bool);
    /// v1 `subscribe` after its pydantic checks: replace the application; returns the cash frozen.
    fn subscribe(
        &mut self,
        e: &mut Engine,
        account_id: i64,
        company_id: i64,
        qty: i64,
        today: &str,
        ts: &str,
    ) -> Result<i64, OrderError>;
    /// v1 `unsubscribe`.
    fn unsubscribe(&mut self, e: &mut Engine, account_id: i64, company_id: i64) -> Result<(), OrderError>;
}

/// The real implementations (`npc.rs`, `daily.rs`, `ipo.rs`).
pub struct EngineOps {
    rng: PyRandom,
}

impl Default for EngineOps {
    fn default() -> Self {
        // v1: random.Random() seeded from the OS
        EngineOps { rng: PyRandom::from_entropy() }
    }
}

impl GameOps for EngineOps {
    fn npc_tick(&mut self, e: &mut Engine, cfg: &Config, day: &str, ts: &str, auction: bool) {
        e.npc_tick(cfg, day, ts, &mut self.rng, auction);
    }

    fn subscribe(
        &mut self,
        e: &mut Engine,
        account_id: i64,
        company_id: i64,
        qty: i64,
        today: &str,
        ts: &str,
    ) -> Result<i64, OrderError> {
        e.subscribe(account_id, company_id, qty, today, ts)
    }

    fn unsubscribe(&mut self, e: &mut Engine, account_id: i64, company_id: i64) -> Result<(), OrderError> {
        e.unsubscribe(account_id, company_id)
    }
}

/// Everything the engine thread owns.
pub struct Ctx {
    pub engine: Engine,
    pub cfg: Arc<Config>,
    pub ops: Box<dyn GameOps>,
}

type Job = Box<dyn FnOnce(&mut Ctx) + Send>;

/// Handle to the engine thread; cheap to clone.
#[derive(Clone)]
pub struct Actor {
    tx: Sender<Job>,
}

/// The engine thread is gone (shutting down, or it died).
#[derive(Debug, Clone, Copy)]
pub struct ActorGone;

impl Actor {
    /// Start the engine thread. Dropping every `Actor` clone ends it; the handle then yields the context
    /// (after a final flush).
    pub fn start(ctx: Ctx) -> (Actor, JoinHandle<Ctx>) {
        let (tx, rx) = unbounded::<Job>();
        let h =
            std::thread::Builder::new().name("engine".into()).spawn(move || run(ctx, rx)).expect("spawn engine thread");
        (Actor { tx }, h)
    }

    /// Run `f` on the engine thread and wait for its result.
    pub async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Ctx) -> R + Send + 'static,
    ) -> Result<R, ActorGone> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |c: &mut Ctx| {
                let _ = tx.send(f(c));
            }))
            .map_err(|_| ActorGone)?;
        rx.await.map_err(|_| ActorGone)
    }

    /// Like `call`, from a non-async thread.
    pub fn call_blocking<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Ctx) -> R + Send + 'static,
    ) -> Result<R, ActorGone> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.tx
            .send(Box::new(move |c: &mut Ctx| {
                let _ = tx.send(f(c));
            }))
            .map_err(|_| ActorGone)?;
        rx.recv().map_err(|_| ActorGone)
    }
}

fn run(mut ctx: Ctx, rx: Receiver<Job>) -> Ctx {
    for job in rx {
        if catch_unwind(AssertUnwindSafe(|| job(&mut ctx))).is_err() {
            eprintln!("engine: a job panicked; exiting so the server restarts from the database");
            std::process::exit(70);
        }
    }
    if let Err(e) = ctx.engine.flush() {
        eprintln!("engine: final flush failed: {e}");
    }
    ctx
}
