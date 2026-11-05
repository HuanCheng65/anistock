//! Real-time push: the market-data fan-out behind `/api/ws` (`api/ws.rs`), after the load-tested design in
//! `bench/SPEC-v2.md` / `bench/rust-v2`:
//!
//! - **Source.** The engine's change stream: a `Tee` sink copies the row changes the engine hands to the
//!   persister (trades, orders, balances, company market fields, news) into a channel. The engine itself
//!   is unchanged and stays the single writer; everything here runs on the engine thread between jobs.
//! - **Conflated stock events** (spec §1). A change on stock K only marks K dirty. Every 100 ms, each dirty
//!   stock with subscribers is encoded once (top 5 levels, last/open/high/low, volume, turnover, VCM, and
//!   the trades since the previous flush) and enqueued to its subscribers.
//! - **Spread quotes snapshot** (spec §2). Every second the quotes of the stocks whose price, volume or
//!   prev close changed are encoded once; delivery is spread over the next 10 ticks by `client id % 10`.
//!   A new `quotes` subscriber gets the full snapshot at once.
//! - **Initial stock snapshots are spread**: a `stock:<id>` subscription is answered on the next ticks, at
//!   most `INIT_PER_TICK` per tick, so a burst of page loads does not land on one tick.
//! - **Batched writes** (spec §3): the per-client writer drains its queue into one NDJSON text frame.
//! - **Slow clients**: per-client queues hold `QUEUE` messages; a client whose queue is full is dropped
//!   (spec), and the browser reconnects and gets a fresh snapshot. The engine never waits on a socket.
//! - `news` and `me` (a player's own orders, fills and cash) are rare and go out as they happen.
//!
//! Messages (one JSON object per line):
//!   {"t":"quotes","full":bool,"s":[[id,price,prev_close,volume,turnover,cooling], ...]}
//!   {"t":"stock","id",...,"bids":[[p,q,n]],"asks":[...],"vcm":{...}|null,"trades":[[id,p,q,ts]]}
//!   {"t":"news","item":{...}}      {"t":"order","order":{...}}      {"t":"fill",...}      {"t":"cash",...}

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use axum::extract::ws::Utf8Bytes;
use crossbeam_channel::{Receiver, Sender};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::config::Clock;
use crate::engine::Engine;
use crate::model::OrderStatus;
use crate::store::persist::{Change, ChangeSink};

/// Per-client queue (messages, not frames), as in the spec.
pub const QUEUE: usize = 256;
pub const TICK: Duration = Duration::from_millis(100);
/// Initial stock snapshots answered per tick.
const INIT_PER_TICK: usize = 64;
/// Stock topics one connection may hold.
pub const MAX_STOCKS: usize = 16;

/// Copies the changes the fan-out cares about, then hands every change to the real sink.
pub struct Tee {
    pub inner: Box<dyn ChangeSink>,
    pub tx: Sender<Vec<Change>>,
}

fn relevant(c: &Change) -> bool {
    matches!(
        c,
        Change::TradeInsert(_)
            | Change::OrderUpsert(_)
            | Change::AccountBalance { .. }
            | Change::CompanyMarket { .. }
            | Change::CompanyUpdate(_)
            | Change::CompanyInsert(_)
            | Change::NewsInsert(_)
    )
}

impl ChangeSink for Tee {
    fn send(&mut self, changes: Vec<Change>) {
        let mine: Vec<Change> = changes.iter().filter(|c| relevant(c)).cloned().collect();
        if !mine.is_empty() {
            let _ = self.tx.send(mine);
        }
        self.inner.send(changes)
    }
    fn flush(&mut self) -> Result<(), String> {
        self.inner.flush()
    }
    fn failure(&self) -> Option<String> {
        self.inner.failure()
    }
}

/// Control messages from the WebSocket tasks to the engine thread.
pub enum Ctl {
    Add { id: u64, account: Option<i64>, tx: mpsc::Sender<Utf8Bytes>, kill: oneshot::Sender<()> },
    Sub { id: u64, topics: Vec<String> },
    Unsub { id: u64, topics: Vec<String> },
    Del(u64),
}

struct Client {
    account: Option<i64>,
    tx: mpsc::Sender<Utf8Bytes>,
    kill: Option<oneshot::Sender<()>>,
    quotes: bool,
    news: bool,
    me: bool,
    stocks: Vec<i64>,
}

#[derive(PartialEq, Clone, Copy)]
struct QuoteKey {
    price: i64,
    prev: i64,
    vol: i64,
    cooling: bool,
}

/// The fan-out state, owned by the engine thread.
pub struct Fanout {
    clock: Clock,
    changes: Receiver<Vec<Change>>,
    pub ctl: Receiver<Ctl>,
    clients: HashMap<u64, Client>,
    stock_subs: HashMap<i64, Vec<u64>>,
    slots: [Vec<u64>; 10],
    dirty: HashSet<i64>,
    /// trades per stock since the previous flush: (id, price, qty, ts)
    new_trades: HashMap<i64, Vec<(i64, i64, i64, String)>>,
    /// initial stock snapshots to answer: (client, stock)
    init: Vec<(u64, i64)>,
    /// today's turnover (cents) per stock, and the day it belongs to
    turnover: HashMap<i64, i64>,
    turnover_day: String,
    snap_last: HashMap<i64, QuoteKey>,
    snapshot: Option<Utf8Bytes>,
    tick_no: u64,
    rm: Vec<(u64, bool)>,
    pub dropped: u64,
}

fn yuan(c: i64) -> f64 {
    c as f64 / 100.0
}

impl Fanout {
    /// `turnover`: today's turnover per stock (cents) from the database at startup.
    pub fn new(clock: Clock, changes: Receiver<Vec<Change>>, ctl: Receiver<Ctl>, turnover: HashMap<i64, i64>) -> Fanout {
        let day = clock.today();
        Fanout {
            clock,
            changes,
            ctl,
            clients: HashMap::new(),
            stock_subs: HashMap::new(),
            slots: Default::default(),
            dirty: HashSet::new(),
            new_trades: HashMap::new(),
            init: Vec::new(),
            turnover,
            turnover_day: day,
            snap_last: HashMap::new(),
            snapshot: None,
            tick_no: 0,
            rm: Vec::new(),
            dropped: 0,
        }
    }

    pub fn clients(&self) -> usize {
        self.clients.len()
    }

    // ------------------------------------------------------------ clients

    pub fn control(&mut self, c: Ctl, e: &Engine) {
        match c {
            Ctl::Add { id, account, tx, kill } => {
                self.slots[(id % 10) as usize].push(id);
                self.clients.insert(
                    id,
                    Client { account, tx, kill: Some(kill), quotes: false, news: false, me: false, stocks: vec![] },
                );
                let hello = json!({"t": "hello", "me": account.is_some()});
                self.send_to(id, &Utf8Bytes::from(hello.to_string()));
            }
            Ctl::Sub { id, topics } => {
                for t in topics {
                    self.subscribe(id, &t, e);
                }
            }
            Ctl::Unsub { id, topics } => {
                for t in topics {
                    self.unsubscribe(id, &t);
                }
            }
            Ctl::Del(id) => self.remove(id, false),
        }
        self.flush_removals();
    }

    fn subscribe(&mut self, id: u64, topic: &str, e: &Engine) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        match topic {
            "quotes" => {
                if !c.quotes {
                    c.quotes = true;
                    let full = self.encode_quotes(e, true);
                    self.send_to(id, &full);
                }
            }
            "news" => c.news = true,
            "me" => c.me = c.account.is_some(),
            _ => {
                let Some(cid) = topic.strip_prefix("stock:").and_then(|s| s.parse::<i64>().ok()) else { return };
                if c.stocks.contains(&cid) || c.stocks.len() >= MAX_STOCKS || e.company(cid).is_none() {
                    return;
                }
                c.stocks.push(cid);
                self.stock_subs.entry(cid).or_default().push(id);
                self.init.push((id, cid));
            }
        }
    }

    fn unsubscribe(&mut self, id: u64, topic: &str) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        match topic {
            "quotes" => c.quotes = false,
            "news" => c.news = false,
            "me" => c.me = false,
            _ => {
                let Some(cid) = topic.strip_prefix("stock:").and_then(|s| s.parse::<i64>().ok()) else { return };
                c.stocks.retain(|x| *x != cid);
                if let Some(v) = self.stock_subs.get_mut(&cid) {
                    v.retain(|x| *x != id);
                }
            }
        }
    }

    fn remove(&mut self, id: u64, slow: bool) {
        if let Some(mut c) = self.clients.remove(&id) {
            for cid in &c.stocks {
                if let Some(v) = self.stock_subs.get_mut(cid) {
                    v.retain(|x| *x != id);
                }
            }
            self.slots[(id % 10) as usize].retain(|x| *x != id);
            if slow {
                self.dropped += 1;
                if let Some(k) = c.kill.take() {
                    let _ = k.send(());
                }
            }
        }
    }

    fn send_to(&mut self, id: u64, msg: &Utf8Bytes) {
        if let Some(c) = self.clients.get(&id) {
            match c.tx.try_send(msg.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => self.rm.push((id, true)),
                Err(mpsc::error::TrySendError::Closed(_)) => self.rm.push((id, false)),
            }
        }
    }

    fn flush_removals(&mut self) {
        for (id, slow) in std::mem::take(&mut self.rm) {
            self.remove(id, slow);
        }
    }

    // ------------------------------------------------------------ engine changes

    /// Fold the engine's latest changes in (after every job). Stocks only get marked dirty; `news` and `me`
    /// go out now.
    pub fn absorb(&mut self, e: &Engine) {
        while let Ok(batch) = self.changes.try_recv() {
            for c in batch {
                self.change(c, e);
            }
        }
        self.flush_removals();
    }

    fn is_player(e: &Engine, account: i64) -> bool {
        e.account(account).is_some_and(|a| a.kind == "player")
    }

    fn me_ids(&self, account: i64) -> Vec<u64> {
        self.clients.iter().filter(|(_, c)| c.me && c.account == Some(account)).map(|(id, _)| *id).collect()
    }

    fn to_account(&mut self, account: i64, msg: Value) {
        let ids = self.me_ids(account);
        if ids.is_empty() {
            return;
        }
        let text = Utf8Bytes::from(msg.to_string());
        for id in ids {
            self.send_to(id, &text);
        }
    }

    fn change(&mut self, c: Change, e: &Engine) {
        match c {
            Change::TradeInsert(t) => {
                self.dirty.insert(t.company_id);
                if t.day != self.turnover_day {
                    self.turnover.clear();
                    self.turnover_day = t.day.clone();
                }
                *self.turnover.entry(t.company_id).or_default() += t.price * t.qty;
                if self.stock_subs.get(&t.company_id).is_some_and(|v| !v.is_empty()) {
                    self.new_trades.entry(t.company_id).or_default().push((t.id, t.price, t.qty, t.ts.clone()));
                }
                let co = e.company(t.company_id);
                for (acct, side, order) in [(t.buyer_id, "buy", t.buy_order), (t.seller_id, "sell", t.sell_order)] {
                    if Self::is_player(e, acct) {
                        self.to_account(
                            acct,
                            json!({"t": "fill", "side": side, "company_id": t.company_id,
                                   "title": co.map(|c| c.title.as_str()), "code": co.map(|c| c.code.as_str()),
                                   "price": yuan(t.price), "qty": t.qty, "order_id": order, "ts": t.ts}),
                        );
                    }
                }
            }
            Change::OrderUpsert(o) => {
                self.dirty.insert(o.company_id);
                if Self::is_player(e, o.account_id) {
                    let co = e.company(o.company_id);
                    self.to_account(
                        o.account_id,
                        json!({"t": "order", "order": {
                            "id": o.id, "company_id": o.company_id, "side": o.side, "kind": o.kind,
                            "price": o.price.map(yuan), "qty": o.qty, "filled": o.filled, "status": o.status,
                            "created_at": o.created_at, "updated_at": o.updated_at,
                            "title": co.map(|c| c.title.as_str()), "code": co.map(|c| c.code.as_str()),
                        }, "open": o.status == OrderStatus::Open}),
                    );
                }
            }
            Change::AccountBalance { id, cash, cash_frozen } => {
                if Self::is_player(e, id) {
                    self.to_account(
                        id,
                        json!({"t": "cash", "cash": yuan(cash), "frozen": yuan(cash_frozen), "available": yuan(cash - cash_frozen)}),
                    );
                }
            }
            Change::CompanyMarket { id, .. } => {
                self.dirty.insert(id);
            }
            Change::CompanyUpdate(c) | Change::CompanyInsert(c) => {
                self.dirty.insert(c.id);
            }
            Change::NewsInsert(n) => {
                let code = n.company_id.and_then(|id| e.company(id)).map(|c| c.code.clone());
                let text = Utf8Bytes::from(
                    json!({"t": "news", "item": {"id": n.id, "day": n.day, "ts": n.ts, "kind": n.kind, "title": n.title,
                           "body": n.body, "company_id": n.company_id, "code": code}})
                    .to_string(),
                );
                let ids: Vec<u64> = self.clients.iter().filter(|(_, c)| c.news).map(|(id, _)| *id).collect();
                for id in ids {
                    self.send_to(id, &text);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ the 100 ms tick

    /// Tick k: quotes snapshot build (every 10th tick) and delivery to slot k % 10, initial stock
    /// snapshots, then the conflated stock flush.
    pub fn tick(&mut self, e: &Engine) {
        self.absorb(e);
        let k = self.tick_no;
        self.tick_no += 1;
        let slot = (k % 10) as usize;
        if slot == 0 {
            self.snapshot = Some(self.encode_quotes(e, false));
        }
        if let Some(text) = self.snapshot.clone() {
            let ids: Vec<u64> = self.slots[slot].iter().copied().filter(|id| self.clients[id].quotes).collect();
            for id in ids {
                self.send_to(id, &text);
            }
            self.flush_removals();
        }
        let ts = self.clock.ts();
        let today = self.clock.today();
        // initial snapshots, spread
        if !self.init.is_empty() {
            let n = self.init.len().min(INIT_PER_TICK);
            let batch: Vec<(u64, i64)> = self.init.drain(..n).collect();
            let mut cache: HashMap<i64, Utf8Bytes> = HashMap::new();
            for (id, cid) in batch {
                if !self.clients.get(&id).is_some_and(|c| c.stocks.contains(&cid)) {
                    continue;
                }
                let text = cache.entry(cid).or_insert_with(|| self.encode_stock(e, cid, &ts, &today, &[], true)).clone();
                self.send_to(id, &text);
            }
            self.flush_removals();
        }
        // conflated stock events
        let dirty: Vec<i64> = self.dirty.drain().collect();
        for cid in dirty {
            let trades = self.new_trades.remove(&cid).unwrap_or_default();
            let Some(subs) = self.stock_subs.get(&cid).filter(|v| !v.is_empty()).cloned() else { continue };
            let text = self.encode_stock(e, cid, &ts, &today, &trades, false);
            for id in subs {
                self.send_to(id, &text);
            }
        }
        self.new_trades.clear();
        self.flush_removals();
    }

    fn day_vol(e: &Engine, cid: i64, today: &str) -> Option<crate::engine::DayAgg> {
        e.st.day_aggs.get(&(cid, today.to_string())).copied()
    }

    fn turnover_of(&self, cid: i64, today: &str) -> i64 {
        if self.turnover_day == today {
            self.turnover.get(&cid).copied().unwrap_or(0)
        } else {
            0
        }
    }

    fn encode_quotes(&mut self, e: &Engine, full: bool) -> Utf8Bytes {
        let today = self.clock.today();
        let ts = self.clock.ts();
        let mut rows = Vec::new();
        for c in e.companies().filter(|c| c.status == "listed") {
            let vol = Self::day_vol(e, c.id, &today).map_or(0, |a| a.volume);
            let key = QuoteKey { price: c.last_price, prev: c.prev_close, vol, cooling: crate::exchange::cooling(c, &ts).is_some() };
            let changed = self.snap_last.get(&c.id) != Some(&key);
            if !full && !changed {
                continue;
            }
            if !full {
                self.snap_last.insert(c.id, key);
            }
            rows.push(json!([c.id, yuan(c.last_price), yuan(c.prev_close), vol, yuan(self.turnover_of(c.id, &today)), key.cooling]));
        }
        Utf8Bytes::from(json!({"t": "quotes", "full": full, "s": rows}).to_string())
    }

    fn encode_stock(&self, e: &Engine, cid: i64, ts: &str, today: &str, trades: &[(i64, i64, i64, String)], init: bool) -> Utf8Bytes {
        let Some(c) = e.company(cid) else { return Utf8Bytes::from_static("{}") };
        let agg = Self::day_vol(e, cid, today);
        let book = e.book(cid, 5);
        let lv = |l: &Vec<crate::engine::Level>| l.iter().map(|x| json!([yuan(x.price), x.qty, x.orders])).collect::<Vec<_>>();
        let vcm = crate::exchange::cooling(c, ts).map(|(lo, hi)| {
            json!({"active": true, "until": c.vcm_until, "low": yuan(lo), "high": yuan(hi), "count": c.vcm_count})
        });
        let tr: Vec<Value> = trades.iter().rev().map(|(id, p, q, ts)| json!([id, yuan(*p), q, ts])).collect();
        Utf8Bytes::from(
            json!({
                "t": "stock", "id": cid, "init": init,
                "price": yuan(c.last_price), "prev_close": yuan(c.prev_close),
                "open": agg.map(|a| yuan(a.open)), "high": agg.map(|a| yuan(a.high)), "low": agg.map(|a| yuan(a.low)),
                "volume": agg.map_or(0, |a| a.volume), "turnover": yuan(self.turnover_of(cid, today)),
                "open_ref": c.open_ref.map(yuan),
                "bids": lv(&book.bids), "asks": lv(&book.asks), "vcm": vcm, "trades": tr,
            })
            .to_string(),
        )
    }
}

/// Next tick deadline after `prev`, skipping missed ticks.
pub fn advance(prev: Instant, now: Instant) -> Instant {
    let mut n = prev + TICK;
    if n <= now {
        n = now + TICK;
    }
    n
}

/// Join `first` with whatever is already queued (no waiting), newline-separated, at most `QUEUE` messages
/// (spec §3; from bench/rust-v2).
pub fn batch(first: Utf8Bytes, rx: &mut mpsc::Receiver<Utf8Bytes>) -> Utf8Bytes {
    let Ok(second) = rx.try_recv() else {
        return first;
    };
    let mut s = String::with_capacity(first.len() + second.len() + 1);
    s.push_str(first.as_str());
    s.push('\n');
    s.push_str(second.as_str());
    let mut n = 2;
    while n < QUEUE {
        match rx.try_recv() {
            Ok(m) => {
                s.push('\n');
                s.push_str(m.as_str());
                n += 1;
            }
            Err(_) => break,
        }
    }
    Utf8Bytes::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_joins_queued_messages_as_ndjson() {
        let (tx, mut rx) = mpsc::channel::<Utf8Bytes>(QUEUE);
        tx.try_send(Utf8Bytes::from_static("{\"a\":2}")).unwrap();
        tx.try_send(Utf8Bytes::from_static("{\"a\":3}")).unwrap();
        let out = batch(Utf8Bytes::from_static("{\"a\":1}"), &mut rx);
        assert_eq!(out.as_str(), "{\"a\":1}\n{\"a\":2}\n{\"a\":3}");
        let out = batch(Utf8Bytes::from_static("x"), &mut rx);
        assert_eq!(out.as_str(), "x");
        for _ in 0..QUEUE {
            tx.try_send(Utf8Bytes::from_static("m")).unwrap();
        }
        let first = rx.try_recv().unwrap();
        tx.try_send(Utf8Bytes::from_static("m")).unwrap();
        assert_eq!(batch(first, &mut rx).as_str().split('\n').count(), QUEUE);
    }
}
