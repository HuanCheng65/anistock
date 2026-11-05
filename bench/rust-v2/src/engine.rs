//! The single-writer engine thread: owns the Exchange, the WS client registry and the
//! 100 ms tick (NPC expiry, NPC orders, staggered market makers, spread market snapshot,
//! conflated book flush).

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
    // v2 §1: conflated book events
    dirty: Vec<bool>,
    dirty_list: Vec<usize>,
    // v2 §2: spread market snapshot
    slots: Vec<Vec<u64>>, // client ids by id % 10
    snap_last: Vec<i64>,  // last/vol as of the previous snapshot
    snap_vol: Vec<i64>,
    snapshot: Option<Utf8Bytes>,
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
            dirty: vec![false; NSYM],
            dirty_list: Vec::new(),
            slots: vec![Vec::new(); 10],
            snap_last: (0..NSYM as i64).map(|s| 1000 + 100 * s).collect(),
            snap_vol: vec![0; NSYM],
            snapshot: None,
        }
    }

    pub fn run(mut self, rx: Receiver<Msg>) {
        let mut next_tick = Instant::now() + NPC_PERIOD;
        loop {
            match rx.recv_deadline(next_tick) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let now = Instant::now();
            if now >= next_tick {
                self.tick(now);
                next_tick = advance(next_tick, NPC_PERIOD, now);
            }
        }
    }

    /// NPC tick k: NPC expiry, NPC orders, market makers for sym % 10 == k % 10,
    /// snapshot build (every 10th tick) and delivery to client slot k % 10, then book flush.
    fn tick(&mut self, now: Instant) {
        let k = self.npc_tick_no;
        self.npc_tick_no += 1;
        let slot = (k % 10) as usize;
        self.npc_tick(now);
        if self.cfg.mm {
            self.mm_tick(slot);
        }
        if slot == 0 {
            self.build_snapshot();
        }
        if let Some(text) = self.snapshot.clone() {
            let ids = std::mem::take(&mut self.slots[slot]);
            for &id in &ids {
                self.send_to(id, &text);
            }
            self.slots[slot] = ids;
            self.flush_removals();
        }
        self.flush_books();
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
                    self.mark_dirty(o.sym as usize);
                }
            }
            Msg::Cancel(acct, id, reply) => {
                let r = self.ex.cancel(acct, id);
                let _ = reply.send(r.is_some());
                if let Some(sym) = r {
                    self.mark_dirty(sym);
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
                self.slots[(id % 10) as usize].push(id);
                self.clients.insert(id, Client { sym, tx, kill });
                // A new client gets one full snapshot immediately.
                let full = self.encode_mkt((0..NSYM).collect());
                self.send_to(id, &full);
                self.flush_removals();
            }
            Msg::WsDel(id) => self.remove_client(id, false),
        }
    }

    fn remove_client(&mut self, id: u64, slow: bool) {
        if let Some(c) = self.clients.remove(&id) {
            self.subs[c.sym].retain(|x| *x != id);
            self.slots[(id % 10) as usize].retain(|x| *x != id);
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

    /// v2 §1: processing an order/cancel only marks the symbol dirty.
    fn mark_dirty(&mut self, sym: usize) {
        if !self.dirty[sym] {
            self.dirty[sym] = true;
            self.dirty_list.push(sym);
        }
    }

    /// Encode one book message per dirty symbol with subscribers, enqueue it to each
    /// subscriber, then clear all dirty flags.
    fn flush_books(&mut self) {
        let list = std::mem::take(&mut self.dirty_list);
        for &sym in &list {
            self.dirty[sym] = false;
            if self.subs[sym].is_empty() {
                continue;
            }
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
        let mut list = list;
        list.clear();
        self.dirty_list = list;
    }

    fn encode_mkt(&self, syms: Vec<usize>) -> Utf8Bytes {
        let s: Vec<[i64; 3]> = syms
            .into_iter()
            .map(|i| [i as i64, self.ex.last[i], self.ex.vol[i]])
            .collect();
        let evt = MktEvt {
            t: "mkt",
            ts: now_us(),
            s: &s,
        };
        Utf8Bytes::from(serde_json::to_string(&evt).expect("encode mkt"))
    }

    /// v2 §2: build the snapshot once per second with only the symbols whose last or vol
    /// changed since the previous snapshot.
    fn build_snapshot(&mut self) {
        let changed: Vec<usize> = (0..NSYM)
            .filter(|&i| self.ex.last[i] != self.snap_last[i] || self.ex.vol[i] != self.snap_vol[i])
            .collect();
        for &i in &changed {
            self.snap_last[i] = self.ex.last[i];
            self.snap_vol[i] = self.ex.vol[i];
        }
        self.snapshot = Some(self.encode_mkt(changed));
    }

    /// Internal (NPC / MM) order: same validation, matching and book events.
    fn place_internal(&mut self, acct: i64, sym: usize, side: Side, typ: OType, qty: i64, px: Option<i64>) -> Option<Placed> {
        match self.ex.place(acct, sym as i64, side, typ, qty, px) {
            Ok(p) => {
                self.npc += 1;
                self.mark_dirty(sym);
                Some(p)
            }
            Err(_) => None,
        }
    }

    fn cancel_internal(&mut self, acct: i64, id: u64) {
        if let Some(sym) = self.ex.cancel(acct, id) {
            self.mark_dirty(sym);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(Exchange::new(None), Config { npc_rate: 0, mm: false })
    }

    fn add_client(e: &mut Engine, id: u64, sym: usize) -> mpsc::Receiver<Utf8Bytes> {
        let (tx, rx) = mpsc::channel(256);
        let (kill, _k) = oneshot::channel();
        std::mem::forget(_k);
        e.handle(Msg::WsAdd { id, sym, tx, kill });
        rx
    }

    fn order(e: &mut Engine, acct: i64, sym: i64, side: Side, qty: i64, px: i64) -> Placed {
        let (tx, mut rx) = oneshot::channel();
        e.handle(Msg::Order(
            OrderIn { acct, sym, side, typ: OType::L, qty, px: Some(px) },
            tx,
        ));
        rx.try_recv().unwrap().unwrap()
    }

    fn drain(rx: &mut mpsc::Receiver<Utf8Bytes>) -> Vec<serde_json::Value> {
        let mut v = Vec::new();
        while let Ok(m) = rx.try_recv() {
            v.push(serde_json::from_str(m.as_str()).unwrap());
        }
        v
    }

    #[test]
    fn new_client_gets_full_snapshot() {
        let mut e = engine();
        let mut rx = add_client(&mut e, 1, 5);
        let msgs = drain(&mut rx);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["t"], "mkt");
        assert_eq!(msgs[0]["s"].as_array().unwrap().len(), NSYM);
        assert_eq!(msgs[0]["s"][5], serde_json::json!([5, 1500, 0]));
    }

    #[test]
    fn book_events_are_conflated_per_tick() {
        let mut e = engine();
        let mut rx5 = add_client(&mut e, 1, 5); // slot 1
        let mut rx6 = add_client(&mut e, 2, 6); // slot 2
        drain(&mut rx5);
        drain(&mut rx6);
        // many orders on sym 5 within one tick: nothing sent yet
        for i in 0..20 {
            order(&mut e, 100 + i, 5, Side::S, 10, 1500 + i);
        }
        let b = order(&mut e, 1, 5, Side::B, 15, 1501);
        assert_eq!(b.filled, 15);
        let c = order(&mut e, 2, 5, Side::B, 1, 1400);
        let (ctx, _crx) = oneshot::channel();
        e.handle(Msg::Cancel(2, c.id, ctx));
        assert!(drain(&mut rx5).is_empty());

        e.tick(Instant::now()); // k = 0: builds snapshot, slot 0 (nobody), book flush
        let m5 = drain(&mut rx5);
        assert_eq!(m5.len(), 1, "one conflated book message");
        assert_eq!(m5[0]["t"], "book");
        assert_eq!(m5[0]["sym"], 5);
        assert_eq!(m5[0]["last"], 1501);
        assert_eq!(m5[0]["vol"], 15);
        assert_eq!(m5[0]["asks"], serde_json::json!([[1501, 5], [1502, 10], [1503, 10], [1504, 10], [1505, 10]]));
        assert_eq!(m5[0]["bids"], serde_json::json!([]));
        assert!(drain(&mut rx6).is_empty(), "sym 6 not dirty");

        e.tick(Instant::now()); // k = 1: snapshot to slot 1, nothing dirty
        let m5 = drain(&mut rx5);
        assert_eq!(m5.len(), 1);
        assert_eq!(m5[0]["t"], "mkt");
        // only changed symbols
        assert_eq!(m5[0]["s"], serde_json::json!([[5, 1501, 15]]));
        assert!(drain(&mut rx6).is_empty());
        e.tick(Instant::now()); // k = 2: slot 2 gets the same snapshot
        let m6 = drain(&mut rx6);
        assert_eq!(m6.len(), 1);
        assert_eq!(m6[0]["s"], serde_json::json!([[5, 1501, 15]]));
        for _ in 3..10 {
            e.tick(Instant::now());
        }
        assert!(drain(&mut rx5).is_empty() && drain(&mut rx6).is_empty());
        e.tick(Instant::now()); // k = 10: new snapshot, nothing changed
        e.tick(Instant::now()); // k = 11: slot 1
        let m5 = drain(&mut rx5);
        assert_eq!(m5.len(), 1);
        assert_eq!(m5[0]["s"], serde_json::json!([]));
    }

    #[test]
    fn slow_client_dropped_at_256() {
        let mut e = engine();
        let _rx = add_client(&mut e, 3, 7); // full snapshot = 1 queued
        for i in 0..300 {
            order(&mut e, 1, 7, Side::B, 1, 1000 + i);
            e.tick(Instant::now());
        }
        assert_eq!(e.dropped, 1);
        assert!(e.clients.is_empty());
    }
}
