//! The single-writer engine thread: owns the Exchange, the WS client registry and the
//! NPC / market-maker / snapshot timers.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use axum::extract::ws::Utf8Bytes;
use crossbeam_channel::{Receiver, RecvTimeoutError};
use rand::Rng;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::exchange::{now_us, Exchange, OType, Placed, Side, Status, NSYM};

pub struct OrderIn {
    pub acct: i64,
    pub sym: i64,
    pub side: Side,
    pub typ: OType,
    pub qty: i64,
    pub px: Option<i64>,
}

#[derive(Serialize)]
pub struct Stats {
    pub orders: u64,
    pub npc: u64,
    pub trades: u64,
    pub ws: u64,
    pub dropped: u64,
}

pub enum Msg {
    Order(OrderIn, oneshot::Sender<Result<Placed, &'static str>>),
    Cancel(i64, u64, oneshot::Sender<bool>),
    Stats(oneshot::Sender<Stats>),
    WsAdd {
        id: u64,
        sym: usize,
        tx: mpsc::Sender<Utf8Bytes>,
        kill: oneshot::Sender<()>,
    },
    WsDel(u64),
}

pub struct Config {
    pub npc_rate: u64,
    pub mm: bool,
}

struct Client {
    sym: usize,
    tx: mpsc::Sender<Utf8Bytes>,
    kill: oneshot::Sender<()>,
}

#[derive(Serialize)]
struct BookEvt<'a> {
    t: &'static str,
    sym: usize,
    last: i64,
    vol: i64,
    bids: &'a [[i64; 2]],
    asks: &'a [[i64; 2]],
    ts: i64,
}

#[derive(Serialize)]
struct MktEvt<'a> {
    t: &'static str,
    ts: i64,
    s: &'a [[i64; 3]],
}

const NPC_PERIOD: Duration = Duration::from_millis(100);
const SEC: Duration = Duration::from_millis(1000);
const NPC_TTL: Duration = Duration::from_secs(30);

pub struct Engine {
    ex: Exchange,
    cfg: Config,
    http_orders: u64,
    npc: u64,
    dropped: u64,
    clients: HashMap<u64, Client>,
    subs: Vec<Vec<u64>>,
    npc_fifo: VecDeque<(Instant, u64, i64)>, // (placed_at, id, acct)
    mm_ids: Vec<[Option<u64>; 2]>,
    rm_buf: Vec<(u64, bool)>,
    npc_tick_no: u64, // k = 0, 1, 2, ...
}

impl Engine {
    pub fn new(ex: Exchange, cfg: Config) -> Self {
        Engine {
            ex,
            cfg,
            http_orders: 0,
            npc: 0,
            dropped: 0,
            clients: HashMap::new(),
            subs: vec![Vec::new(); NSYM],
            npc_fifo: VecDeque::new(),
            mm_ids: vec![[None, None]; NSYM],
            rm_buf: Vec::new(),
            npc_tick_no: 0,
        }
    }

    pub fn run(mut self, rx: Receiver<Msg>) {
        let start = Instant::now();
        let mut next_npc = start + NPC_PERIOD;
        let mut next_mkt = start + SEC;
        loop {
            let deadline = next_npc.min(next_mkt);
            match rx.recv_deadline(deadline) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if now >= next_npc {
                // NPC tick k: expiry, NPC orders, then staggered MM re-quote of sym % 10 == k % 10
                let k = self.npc_tick_no;
                self.npc_tick_no += 1;
                self.npc_tick(now);
                if self.cfg.mm {
                    self.mm_tick((k % 10) as usize);
                }
                next_npc = advance(next_npc, NPC_PERIOD, now);
            }
            if now >= next_mkt {
                self.mkt_snapshot();
                next_mkt = advance(next_mkt, SEC, now);
            }
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Order(o, reply) => {
                let r = self.ex.place(o.acct, o.sym, o.side, o.typ, o.qty, o.px);
                if r.is_ok() {
                    self.http_orders += 1;
                }
                let _ = reply.send(r);
                if r.is_ok() {
                    self.publish_book(o.sym as usize);
                }
            }
            Msg::Cancel(acct, id, reply) => {
                let r = self.ex.cancel(acct, id);
                let _ = reply.send(r.is_some());
                if let Some(sym) = r {
                    self.publish_book(sym);
                }
            }
            Msg::Stats(reply) => {
                let _ = reply.send(Stats {
                    orders: self.http_orders,
                    npc: self.npc,
                    trades: self.ex.trades,
                    ws: self.clients.len() as u64,
                    dropped: self.dropped,
                });
            }
            Msg::WsAdd { id, sym, tx, kill } => {
                self.subs[sym].push(id);
                self.clients.insert(id, Client { sym, tx, kill });
            }
            Msg::WsDel(id) => self.remove_client(id, false),
        }
    }

    fn remove_client(&mut self, id: u64, slow: bool) {
        if let Some(c) = self.clients.remove(&id) {
            self.subs[c.sym].retain(|x| *x != id);
            if slow {
                self.dropped += 1;
                let _ = c.kill.send(());
            }
        }
    }

    fn send_to(&mut self, id: u64, msg: &Utf8Bytes) {
        if let Some(c) = self.clients.get(&id) {
            match c.tx.try_send(msg.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => self.rm_buf.push((id, true)),
                Err(mpsc::error::TrySendError::Closed(_)) => self.rm_buf.push((id, false)),
            }
        }
    }

    fn flush_removals(&mut self) {
        let mut buf = std::mem::take(&mut self.rm_buf);
        for (id, slow) in buf.drain(..) {
            self.remove_client(id, slow);
        }
        self.rm_buf = buf;
    }

    /// Book event after any processed order/cancel on `sym`; encoded once, shared by all
    /// subscribers.
    fn publish_book(&mut self, sym: usize) {
        let (bids, asks) = self.ex.depth(sym);
        let evt = BookEvt {
            t: "book",
            sym,
            last: self.ex.last[sym],
            vol: self.ex.vol[sym],
            bids: &bids,
            asks: &asks,
            ts: now_us(),
        };
        let text = Utf8Bytes::from(serde_json::to_string(&evt).expect("encode book"));
        let subs = std::mem::take(&mut self.subs[sym]);
        for &id in &subs {
            self.send_to(id, &text);
        }
        self.subs[sym] = subs;
        self.flush_removals();
    }

    fn mkt_snapshot(&mut self) {
        let s: Vec<[i64; 3]> = (0..NSYM)
            .map(|i| [i as i64, self.ex.last[i], self.ex.vol[i]])
            .collect();
        let evt = MktEvt {
            t: "mkt",
            ts: now_us(),
            s: &s,
        };
        let text = Utf8Bytes::from(serde_json::to_string(&evt).expect("encode mkt"));
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            self.send_to(id, &text);
        }
        self.flush_removals();
    }

    /// Internal (NPC / MM) order: same validation, matching and book events.
    fn place_internal(&mut self, acct: i64, sym: usize, side: Side, typ: OType, qty: i64, px: Option<i64>) -> Option<Placed> {
        match self.ex.place(acct, sym as i64, side, typ, qty, px) {
            Ok(p) => {
                self.npc += 1;
                self.publish_book(sym);
                Some(p)
            }
            Err(_) => None,
        }
    }

    fn cancel_internal(&mut self, acct: i64, id: u64) {
        if let Some(sym) = self.ex.cancel(acct, id) {
            self.publish_book(sym);
        }
    }

    fn npc_tick(&mut self, now: Instant) {
        // expire NPC limit orders older than 30 s
        while let Some(&(t, id, acct)) = self.npc_fifo.front() {
            if now.duration_since(t) < NPC_TTL {
                break;
            }
            self.npc_fifo.pop_front();
            if self.ex.is_open(id) {
                self.cancel_internal(acct, id);
            }
        }
        let n = self.cfg.npc_rate / 10;
        let mut rng = rand::rng();
        for _ in 0..n {
            let acct: i64 = rng.random_range(10000..11000);
            let sym: usize = rng.random_range(0..NSYM);
            let side = if rng.random_bool(0.5) { Side::B } else { Side::S };
            let r = self.ex.ref_px(sym);
            if rng.random_bool(0.8) {
                let u: f64 = rng.random_range(-0.02..0.02);
                let px = (r as f64 * (1.0 + u)).round() as i64;
                let qty: i64 = rng.random_range(1..=100);
                if let Some(p) = self.place_internal(acct, sym, side, OType::L, qty, Some(px)) {
                    if p.status == Status::Open {
                        self.npc_fifo.push_back((Instant::now(), p.id, acct));
                    }
                }
            } else {
                let qty: i64 = rng.random_range(1..=50);
                self.place_internal(acct, sym, side, OType::M, qty, None);
            }
        }
    }

    /// Staggered market makers: re-quote every symbol with sym % 10 == slot.
    fn mm_tick(&mut self, slot: usize) {
        for sym in (slot..NSYM).step_by(10) {
            let acct = 11000 + sym as i64;
            let prev = std::mem::take(&mut self.mm_ids[sym]);
            for id in prev.into_iter().flatten() {
                self.cancel_internal(acct, id);
            }
            let r = self.ex.ref_px(sym);
            let bid = self.place_internal(acct, sym, Side::B, OType::L, 200, Some(r * 99 / 100));
            let ask = self.place_internal(acct, sym, Side::S, OType::L, 200, Some(r * 101 / 100));
            let open_id = |p: Option<Placed>| p.filter(|p| p.status == Status::Open).map(|p| p.id);
            self.mm_ids[sym] = [open_id(bid), open_id(ask)];
        }
    }
}

fn advance(next: Instant, period: Duration, now: Instant) -> Instant {
    let n = next + period;
    if n <= now {
        now + period
    } else {
        n
    }
}
