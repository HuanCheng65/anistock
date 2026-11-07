//! Real-time push: the fan-out behind `/api/v1/ws` (`api/v1/ws.rs`, docs/api-v1.md §5), after the
//! load-tested design in `bench/SPEC-v2.md` / `bench/rust-v2`:
//!
//! - **Source.** The engine's change stream: a `Tee` sink copies the row changes the engine hands to the
//!   persister (trades, orders, balances, company market fields, news) into a channel. The engine itself
//!   is unchanged and stays the single writer; everything here runs on the engine thread between jobs.
//! - **Conflated stock events.** A change on stock K only marks K dirty. Every 100 ms, each dirty stock
//!   with subscribers is encoded once and enqueued to its subscribers (`seq` per stock).
//! - **Spread quotes snapshot.** Every second the rows that changed are encoded once (`seq` global);
//!   delivery is spread over the next 10 ticks by `client id % 10`. A new or resyncing `quotes`
//!   subscriber gets a full snapshot at once, carrying the last snapshot's `seq` (the next delta it gets
//!   is `seq + 1`). The first build after the trading day changes is full for everyone.
//! - **Initial stock snapshots are spread**: at most `INIT_PER_TICK` per tick. A client joins the stock's
//!   subscriber list only when its full snapshot goes out, so it never sees a delta before the full one.
//! - **Batched writes**: the per-client writer drains its queue into one NDJSON text frame.
//! - **Slow clients**: per-client queues hold `QUEUE` messages; a full queue closes the connection (4008).
//! - **Futures** (`futures:{id}`): a change on contract K marks it dirty like a stock; every 100 ms each dirty
//!   contract with subscribers gets one snapshot of the contract and its book (a few dozen contracts, small
//!   messages: no deltas). A player's futures orders and positions send `futures_mine` on `me`.
//! - `news`, `events` and `me` go out as they happen. `news` and `events` keep the last 24 hours (at most
//!   2000 each) for `since` on reconnect; outside that window the client gets `reset`.
//! - `time` every 20 s and at once on a phase change, the 04:00 day change and a finished close.
//! - Sessions: logout (`Ctl::Logout`), a revoked API key (`Ctl::Revoke`) and cookie or key expiry
//!   (checked with `time`) close with 4001.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::Utf8Bytes;
use crossbeam_channel::{Receiver, Sender};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::api::v1::events::{self, Event, EventRules, Moves, Pending};
use crate::api::v1::types::{self as t, Money, OrderSide, StockRef};
use crate::api::v1::ws::{WsLimits, WsRejected, WsServerMessage as M};
use crate::config::{Clock, Config};
use crate::engine::Engine;
use crate::futures::FuturesChange;
use crate::model::OrderStatus;
use crate::store::persist::{Change, ChangeSink};

/// Per-client queue (messages, not frames).
pub const QUEUE: usize = 256;
pub const TICK: Duration = Duration::from_millis(100);
/// Initial stock snapshots answered per tick.
const INIT_PER_TICK: usize = 64;
/// Stock topics one connection may hold.
pub const MAX_STOCKS: usize = 16;
/// `time` every this many ticks (20 s).
const TIME_TICKS: u64 = 200;
const REPLAY_MAX: usize = 2000;
const REPLAY_AGE: Duration = Duration::from_secs(24 * 3600);
pub const QUOTE_COLS: [&str; 6] = ["id", "price", "prev_close", "volume", "turnover", "vcm_active"];

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
            | Change::Futures(
                FuturesChange::Contract(_)
                    | FuturesChange::Order(_)
                    | FuturesChange::Trade(_)
                    | FuturesChange::Position(_)
                    | FuturesChange::PositionDelete(..)
            )
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

/// Control messages from the WebSocket tasks (and the loops) to the engine thread.
pub enum Ctl {
    /// `key`: the API key the connection authenticated with (docs/v2-api-access.md), if any
    Add { id: u64, account: Option<i64>, bot: bool, key: Option<i64>, exp: Option<i64>, tx: mpsc::Sender<Utf8Bytes>, kill: oneshot::Sender<u16> },
    Sub { id: u64, topics: Vec<String>, since: HashMap<String, i64> },
    Unsub { id: u64, topics: Vec<String> },
    Resync { id: u64, topics: Vec<String> },
    Ping { id: u64, ping: Value },
    Del(u64),
    /// the session of this account ended (logout): close its connections with 4001
    Logout { account: i64 },
    /// this API key was revoked: close the connections opened with it (4001)
    Revoke { key: i64 },
    /// an event built elsewhere (the daily close's `daily_close`, `leaderboard_weekly`)
    Emit(Pending),
    /// send `time` now (after a close)
    Time,
}

struct Client {
    account: Option<i64>,
    bot: bool,
    key: Option<i64>,
    exp: Option<i64>,
    tx: mpsc::Sender<Utf8Bytes>,
    kill: Option<oneshot::Sender<u16>>,
    quotes: bool,
    /// skip quote deltas with seq <= this (it got a newer full snapshot)
    quotes_after: u64,
    news: bool,
    events_all: bool,
    event_kinds: HashSet<String>,
    me: bool,
    stocks: Vec<i64>,
    contracts: Vec<i64>,
}

#[derive(PartialEq, Clone, Copy)]
struct QuoteKey {
    price: i64,
    prev: i64,
    vol: i64,
    cooling: bool,
}

struct Replay {
    seq: u64,
    kind: String,
    at: Instant,
    text: Utf8Bytes,
}

/// The fan-out state, owned by the engine thread.
pub struct Fanout {
    clock: Clock,
    cfg: Arc<Config>,
    rules: EventRules,
    rollover_min: i64,
    changes: Receiver<Vec<Change>>,
    pub ctl: Receiver<Ctl>,
    clients: HashMap<u64, Client>,
    stock_subs: HashMap<i64, Vec<u64>>,
    slots: [Vec<u64>; 10],
    dirty: HashSet<i64>,
    fut_subs: HashMap<i64, Vec<u64>>,
    fut_dirty: HashSet<i64>,
    fut_seq: HashMap<i64, u64>,
    /// trades per stock since the previous flush: (id, price, qty, ts)
    new_trades: HashMap<i64, Vec<(i64, i64, i64, String)>>,
    /// initial stock snapshots to answer: (client, stock)
    init: Vec<(u64, i64)>,
    turnover: HashMap<i64, i64>,
    turnover_day: String,
    snap_last: HashMap<i64, QuoteKey>,
    snapshot: Option<(u64, Utf8Bytes)>,
    snapshot_day: String,
    quotes_seq: u64,
    stock_seq: HashMap<i64, u64>,
    me_seq: HashMap<i64, u64>,
    events_seq: u64,
    news_replay: VecDeque<Replay>,
    event_replay: VecDeque<Replay>,
    /// (notional, qty) filled per open order since start (for `average_fill_price`)
    fills: HashMap<i64, (i64, i64)>,
    moves: Moves,
    tick_no: u64,
    last_phase: Option<crate::config::Phase>,
    last_day: String,
    rm: Vec<(u64, u16)>,
    pub dropped: u64,
}

fn text(m: &M) -> Utf8Bytes {
    Utf8Bytes::from(serde_json::to_string(m).unwrap_or_default())
}

impl Fanout {
    /// `turnover`: today's turnover per stock (cents) from the database at startup.
    pub fn new(clock: Clock, cfg: Arc<Config>, changes: Receiver<Vec<Change>>, ctl: Receiver<Ctl>, turnover: HashMap<i64, i64>) -> Fanout {
        let day = clock.today();
        let rollover_min = {
            let (h, m) = cfg.game.day_rollover.split_once(':').unwrap_or(("0", "0"));
            h.trim().parse::<i64>().unwrap_or(0) * 60 + m.trim().parse::<i64>().unwrap_or(0)
        };
        let now_s = clock.now().timestamp().max(0) as u64;
        Fanout {
            rules: EventRules::from_config(&cfg),
            cfg,
            clock,
            rollover_min,
            changes,
            ctl,
            clients: HashMap::new(),
            stock_subs: HashMap::new(),
            slots: Default::default(),
            dirty: HashSet::new(),
            fut_subs: HashMap::new(),
            fut_dirty: HashSet::new(),
            fut_seq: HashMap::new(),
            new_trades: HashMap::new(),
            init: Vec::new(),
            turnover,
            turnover_day: day.clone(),
            snap_last: HashMap::new(),
            snapshot: None,
            snapshot_day: day.clone(),
            quotes_seq: 0,
            stock_seq: HashMap::new(),
            me_seq: HashMap::new(),
            // monotonic across restarts (a restart's gap is outside every client's replay window anyway)
            events_seq: now_s * 1000,
            news_replay: VecDeque::new(),
            event_replay: VecDeque::new(),
            fills: HashMap::new(),
            moves: Moves::default(),
            tick_no: 0,
            last_phase: None,
            last_day: day,
            rm: Vec::new(),
            dropped: 0,
        }
    }

    pub fn clients(&self) -> usize {
        self.clients.len()
    }

    fn trading_day_of(&self, ts: &str) -> String {
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|dt| (dt.naive_local() - chrono::Duration::minutes(self.rollover_min)).date().format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| ts.get(..10).unwrap_or("").to_string())
    }

    fn phase(&self) -> crate::config::Phase {
        self.clock.phase(&self.cfg.game, self.cfg.always_open)
    }

    fn sref(e: &Engine, id: i64) -> StockRef {
        e.company(id).map_or(StockRef { id, code: String::new(), title: String::new() }, |c| StockRef {
            id,
            code: c.code.clone(),
            title: c.title.clone(),
        })
    }

    // ------------------------------------------------------------ clients

    pub fn control(&mut self, c: Ctl, e: &Engine) {
        match c {
            Ctl::Add { id, account, bot, key, exp, tx, kill } => {
                self.slots[(id % 10) as usize].push(id);
                self.clients.insert(
                    id,
                    Client {
                        account,
                        bot,
                        key,
                        exp,
                        tx,
                        kill: Some(kill),
                        quotes: false,
                        quotes_after: 0,
                        news: false,
                        events_all: false,
                        event_kinds: HashSet::new(),
                        me: false,
                        stocks: vec![],
                        contracts: vec![],
                    },
                );
                let hello = M::Hello {
                    v: 1,
                    server_version: crate::api::v1::VERSION.into(),
                    me: account.is_some(),
                    bot,
                    now: t::ts(self.clock.ts()),
                    trading_day: t::day(self.clock.today()),
                    phase: self.phase().into(),
                    limits: WsLimits { stocks: MAX_STOCKS as i64, queue: QUEUE as i64 },
                };
                self.send_to(id, &text(&hello));
                let tm = self.time_msg(e);
                self.send_to(id, &tm);
            }
            Ctl::Sub { id, topics, since } => {
                let mut ok = Vec::new();
                let mut rejected = Vec::new();
                for tp in topics {
                    match self.subscribe(id, &tp, &since, e) {
                        Ok(()) => ok.push(tp),
                        Err(code) => rejected.push(WsRejected { topic: tp, code: code.into() }),
                    }
                }
                if self.clients.contains_key(&id) {
                    let ack = text(&M::Subscribed { topics: ok, rejected });
                    self.send_to(id, &ack);
                }
            }
            Ctl::Unsub { id, topics } => {
                for tp in topics {
                    self.unsubscribe(id, &tp);
                }
            }
            Ctl::Resync { id, topics } => {
                for tp in topics {
                    if tp == "quotes" && self.clients.get(&id).is_some_and(|c| c.quotes) {
                        self.send_full_quotes(id, e);
                    } else if let Some(k) = tp.strip_prefix("futures:").and_then(|s| s.parse::<i64>().ok()) {
                        if self.clients.get(&id).is_some_and(|c| c.contracts.contains(&k)) {
                            self.send_contract(id, k, e);
                        }
                    } else if let Some(cid) = tp.strip_prefix("stock:").and_then(|s| s.parse::<i64>().ok()) {
                        if self.clients.get(&id).is_some_and(|c| c.stocks.contains(&cid)) {
                            if let Some(v) = self.stock_subs.get_mut(&cid) {
                                v.retain(|x| *x != id);
                            }
                            self.init.push((id, cid));
                        }
                    }
                }
            }
            Ctl::Ping { id, ping } => {
                let m = text(&M::Pong { ping });
                self.send_to(id, &m);
            }
            Ctl::Del(id) => self.remove(id, None),
            Ctl::Logout { account } => {
                // a connection opened with an API key lives as long as the key, not the browser session
                let ids: Vec<u64> =
                    self.clients.iter().filter(|(_, c)| c.account == Some(account) && c.key.is_none()).map(|(i, _)| *i).collect();
                for id in ids {
                    self.rm.push((id, 4001));
                }
            }
            Ctl::Revoke { key } => {
                let ids: Vec<u64> = self.clients.iter().filter(|(_, c)| c.key == Some(key)).map(|(i, _)| *i).collect();
                for id in ids {
                    self.rm.push((id, 4001));
                }
            }
            Ctl::Emit(p) => {
                self.emit(p);
                let tm = self.time_msg(e);
                self.broadcast(&tm);
            }
            Ctl::Time => {
                let tm = self.time_msg(e);
                self.broadcast(&tm);
            }
        }
        self.flush_removals();
    }

    fn subscribe(&mut self, id: u64, topic: &str, since: &HashMap<String, i64>, e: &Engine) -> Result<(), &'static str> {
        let Some(c) = self.clients.get_mut(&id) else { return Err("unknown_topic") };
        match topic {
            "quotes" => {
                if !c.quotes {
                    c.quotes = true;
                    self.send_full_quotes(id, e);
                }
                Ok(())
            }
            "news" => {
                if !c.news {
                    if let Some(s) = since.get("news") {
                        self.replay_news(id, *s, e);
                    }
                    if let Some(c) = self.clients.get_mut(&id) {
                        c.news = true;
                    }
                }
                Ok(())
            }
            "me" => {
                if c.bot {
                    return Err("forbidden");
                }
                if c.account.is_none() {
                    return Err("unauthenticated");
                }
                c.me = true;
                Ok(())
            }
            _ if topic == "events" || topic.starts_with("events:") => {
                let kind = topic.strip_prefix("events:").map(str::to_string);
                if kind.as_deref() == Some("") {
                    return Err("unknown_topic");
                }
                let first = !c.events_all && c.event_kinds.is_empty();
                if first {
                    if let Some(s) = since.get("events") {
                        self.replay_events(id, *s, kind.as_deref());
                    }
                }
                let Some(c) = self.clients.get_mut(&id) else { return Ok(()) };
                match kind {
                    None => c.events_all = true,
                    Some(k) => {
                        c.event_kinds.insert(k);
                    }
                }
                Ok(())
            }
            _ if topic.starts_with("futures:") => {
                let Some(k) = topic["futures:".len()..].parse::<i64>().ok() else { return Err("unknown_topic") };
                if c.contracts.contains(&k) {
                    return Ok(());
                }
                if !e.st.futures.contracts.contains_key(&k) {
                    return Err("contract_not_found");
                }
                if c.contracts.len() >= MAX_STOCKS {
                    return Err("too_many_contracts");
                }
                c.contracts.push(k);
                self.fut_subs.entry(k).or_default().push(id);
                self.send_contract(id, k, e);
                Ok(())
            }
            _ => {
                let Some(cid) = topic.strip_prefix("stock:").and_then(|s| s.parse::<i64>().ok()) else {
                    return Err("unknown_topic");
                };
                if c.stocks.contains(&cid) {
                    return Ok(());
                }
                if e.company(cid).is_none() {
                    return Err("stock_not_found");
                }
                if c.stocks.len() >= MAX_STOCKS {
                    return Err("too_many_stocks");
                }
                c.stocks.push(cid);
                self.init.push((id, cid));
                Ok(())
            }
        }
    }

    fn unsubscribe(&mut self, id: u64, topic: &str) {
        let Some(c) = self.clients.get_mut(&id) else { return };
        match topic {
            "quotes" => c.quotes = false,
            "news" => c.news = false,
            "me" => c.me = false,
            "events" => c.events_all = false,
            _ if topic.starts_with("events:") => {
                c.event_kinds.remove(&topic["events:".len()..]);
            }
            _ if topic.starts_with("futures:") => {
                let Ok(k) = topic["futures:".len()..].parse::<i64>() else { return };
                c.contracts.retain(|x| *x != k);
                if let Some(v) = self.fut_subs.get_mut(&k) {
                    v.retain(|x| *x != id);
                }
            }
            _ => {
                let Some(cid) = topic.strip_prefix("stock:").and_then(|s| s.parse::<i64>().ok()) else { return };
                c.stocks.retain(|x| *x != cid);
                if let Some(v) = self.stock_subs.get_mut(&cid) {
                    v.retain(|x| *x != id);
                }
            }
        }
    }

    fn remove(&mut self, id: u64, code: Option<u16>) {
        if let Some(mut c) = self.clients.remove(&id) {
            for cid in &c.stocks {
                if let Some(v) = self.stock_subs.get_mut(cid) {
                    v.retain(|x| *x != id);
                }
            }
            for k in &c.contracts {
                if let Some(v) = self.fut_subs.get_mut(k) {
                    v.retain(|x| *x != id);
                }
            }
            self.slots[(id % 10) as usize].retain(|x| *x != id);
            if let Some(code) = code {
                if code == 4008 {
                    self.dropped += 1;
                }
                if let Some(k) = c.kill.take() {
                    let _ = k.send(code);
                }
            }
        }
    }

    fn send_to(&mut self, id: u64, msg: &Utf8Bytes) {
        if let Some(c) = self.clients.get(&id) {
            match c.tx.try_send(msg.clone()) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => self.rm.push((id, 4008)),
                Err(mpsc::error::TrySendError::Closed(_)) => self.rm.push((id, 0)),
            }
        }
    }

    fn broadcast(&mut self, msg: &Utf8Bytes) {
        let ids: Vec<u64> = self.clients.keys().copied().collect();
        for id in ids {
            self.send_to(id, msg);
        }
    }

    fn flush_removals(&mut self) {
        for (id, code) in std::mem::take(&mut self.rm) {
            self.remove(id, (code != 0).then_some(code));
        }
    }

    // ------------------------------------------------------------ replay (news, events)

    fn trim(buf: &mut VecDeque<Replay>) {
        while buf.len() > REPLAY_MAX || buf.front().is_some_and(|r| r.at.elapsed() > REPLAY_AGE) {
            buf.pop_front();
        }
    }

    fn replay_news(&mut self, id: u64, since: i64, e: &Engine) {
        let last = e.st.last_news_id;
        if since >= last {
            return;
        }
        Self::trim(&mut self.news_replay);
        let covered = self.news_replay.front().is_some_and(|r| r.seq as i64 <= since + 1);
        if !covered {
            let m = text(&M::Reset { topic: "news".into() });
            self.send_to(id, &m);
            return;
        }
        let msgs: Vec<Utf8Bytes> = self.news_replay.iter().filter(|r| r.seq as i64 > since).map(|r| r.text.clone()).collect();
        for m in msgs {
            self.send_to(id, &m);
        }
    }

    fn replay_events(&mut self, id: u64, since: i64, kind: Option<&str>) {
        if since >= self.events_seq as i64 {
            return;
        }
        Self::trim(&mut self.event_replay);
        let covered = self.event_replay.front().is_some_and(|r| r.seq as i64 <= since + 1);
        if !covered {
            let m = text(&M::Reset { topic: "events".into() });
            self.send_to(id, &m);
            return;
        }
        let msgs: Vec<Utf8Bytes> = self
            .event_replay
            .iter()
            .filter(|r| r.seq as i64 > since && kind.is_none_or(|k| k == r.kind))
            .map(|r| r.text.clone())
            .collect();
        for m in msgs {
            self.send_to(id, &m);
        }
    }

    fn emit(&mut self, p: Pending) {
        self.events_seq += 1;
        let seq = self.events_seq;
        let ev = Event {
            seq,
            kind: p.kind.clone(),
            at: t::ts(p.at),
            trading_day: t::day(p.trading_day),
            stock: p.stock,
            data: p.data,
            news_id: p.news_id,
        };
        let msg = text(&M::Event(ev));
        self.event_replay.push_back(Replay { seq, kind: p.kind.clone(), at: Instant::now(), text: msg.clone() });
        Self::trim(&mut self.event_replay);
        let ids: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, c)| c.events_all || c.event_kinds.contains(&p.kind))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.send_to(id, &msg);
        }
    }

    // ------------------------------------------------------------ engine changes

    /// Fold the engine's latest changes in (after every job).
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

    fn to_account(&mut self, account: i64, make: impl FnOnce(u64) -> M) {
        let ids: Vec<u64> = self.clients.iter().filter(|(_, c)| c.me && c.account == Some(account)).map(|(id, _)| *id).collect();
        if ids.is_empty() {
            return; // nobody listening: no encoding (the seq only has to be consecutive per connection)
        }
        let seq = self.me_seq.entry(account).or_default();
        *seq += 1;
        let msg = text(&make(*seq));
        for id in ids {
            self.send_to(id, &msg);
        }
    }

    fn change(&mut self, c: Change, e: &Engine) {
        match c {
            Change::TradeInsert(tr) => {
                self.dirty.insert(tr.company_id);
                if tr.day != self.turnover_day {
                    self.turnover.clear();
                    self.turnover_day = tr.day.clone();
                }
                *self.turnover.entry(tr.company_id).or_default() += tr.price * tr.qty;
                if self.stock_subs.get(&tr.company_id).is_some_and(|v| !v.is_empty()) {
                    self.new_trades.entry(tr.company_id).or_default().push((tr.id, tr.price, tr.qty, tr.ts.clone()));
                }
                let notional = tr.price * tr.qty;
                for o in [tr.buy_order, tr.sell_order].into_iter().flatten() {
                    let f = self.fills.entry(o).or_default();
                    f.0 += notional;
                    f.1 += tr.qty;
                }
                let sref = Self::sref(e, tr.company_id);
                for (acct, side, order) in [(tr.buyer_id, OrderSide::Buy, tr.buy_order), (tr.seller_id, OrderSide::Sell, tr.sell_order)] {
                    if Self::is_player(e, acct) {
                        let mt = t::MyTrade {
                            id: tr.id,
                            order_id: order,
                            stock: sref.clone(),
                            side,
                            price: Money(tr.price),
                            quantity: tr.qty,
                            amount: Money(notional),
                            commission: Money(e.fees.commission_on(notional)),
                            stamp_duty: Money(if side == OrderSide::Sell { e.fees.stamp_on(notional) } else { 0 }),
                            at: t::ts(tr.ts.clone()),
                            trading_day: t::day(tr.day.clone()),
                        };
                        self.to_account(acct, |seq| M::Fill { seq, trade: mt });
                    }
                }
                self.trade_events(&tr, e);
            }
            Change::OrderUpsert(o) => {
                self.dirty.insert(o.company_id);
                if Self::is_player(e, o.account_id) {
                    let avg = self.fills.get(&o.id).filter(|f| f.1 > 0 && f.1 == o.filled).map(|f| Money(crate::exchange::py_round_div(f.0, f.1)));
                    let order = t::Order {
                        id: o.id,
                        stock: Self::sref(e, o.company_id),
                        side: if o.side == crate::model::Side::Sell { OrderSide::Sell } else { OrderSide::Buy },
                        r#type: o.kind.as_str().into(),
                        price: o.price.map(Money),
                        quantity: o.qty,
                        filled_quantity: o.filled,
                        average_fill_price: avg,
                        frozen_cash: Money(if o.side == crate::model::Side::Buy { o.frozen } else { 0 }),
                        status: o.status.as_str().into(),
                        trading_day: t::day(self.trading_day_of(&o.created_at)),
                        created_at: t::ts(o.created_at.clone()),
                        updated_at: t::ts(o.updated_at.clone()),
                        credit: e.margin_mode(o.id).map(|m| m.as_str().to_string()),
                    };
                    self.to_account(o.account_id, |seq| M::Order { seq, order });
                }
                if o.status != OrderStatus::Open {
                    self.fills.remove(&o.id);
                }
            }
            Change::AccountBalance { id, cash, cash_frozen } => {
                if Self::is_player(e, id) {
                    self.to_account(id, |seq| M::Cash {
                        seq,
                        cash: Money(cash),
                        frozen_cash: Money(cash_frozen),
                        available_cash: Money(cash - cash_frozen),
                    });
                }
            }
            Change::Margin(crate::margin::MarginChange::Credit(c)) => {
                if Self::is_player(e, c.account_id) {
                    self.to_account(c.account_id, |seq| M::Credit { seq, status: Some(c.status.clone()), call_day: c.call_day.clone().map(t::day) });
                }
            }
            Change::Margin(crate::margin::MarginChange::CreditDelete(id)) => {
                if Self::is_player(e, id) {
                    self.to_account(id, |seq| M::Credit { seq, status: None, call_day: None });
                }
            }
            Change::CompanyMarket { id, .. } => {
                self.dirty.insert(id);
            }
            Change::CompanyUpdate(c) | Change::CompanyInsert(c) => {
                self.dirty.insert(c.id);
            }
            Change::Futures(f) => {
                let (contract, account) = match &f {
                    FuturesChange::Contract(k) => (Some(k.id), None),
                    FuturesChange::Order(o) => (Some(o.contract_id), Some(o.account_id)),
                    FuturesChange::Trade(tr) => (Some(tr.contract_id), None),
                    FuturesChange::Position(p) => (None, Some(p.account_id)),
                    FuturesChange::PositionDelete(a, _) => (None, Some(*a)),
                    _ => (None, None),
                };
                if let Some(k) = contract {
                    self.fut_dirty.insert(k);
                }
                if let Some(a) = account.filter(|a| Self::is_player(e, *a)) {
                    self.to_account(a, |seq| M::FuturesMine { seq });
                }
            }
            Change::NewsInsert(n) => {
                let item = t::NewsItem {
                    id: n.id,
                    at: t::ts(n.ts.clone()),
                    trading_day: t::day(n.day.clone()),
                    kind: n.kind.clone(),
                    category: t::NewsCategory::of(&n.kind),
                    title: n.title.clone(),
                    body: n.body.clone().filter(|b| !b.is_empty()),
                    stock: n.company_id.map(|id| Self::sref(e, id)),
                };
                let msg = text(&M::News { seq: n.id as u64, item });
                self.news_replay.push_back(Replay { seq: n.id as u64, kind: n.kind.clone(), at: Instant::now(), text: msg.clone() });
                Self::trim(&mut self.news_replay);
                let ids: Vec<u64> = self.clients.iter().filter(|(_, c)| c.news).map(|(id, _)| *id).collect();
                for id in ids {
                    self.send_to(id, &msg);
                }
                self.news_events(&n, e);
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ derived events

    fn pending(kind: &str, at: &str, day: &str, stock: Option<StockRef>, data: Value, news_id: Option<i64>) -> Pending {
        Pending { kind: kind.into(), at: at.into(), trading_day: day.into(), stock, data, news_id }
    }

    fn news_events(&mut self, n: &crate::model::News, e: &Engine) {
        let stock = n.company_id.map(|id| Self::sref(e, id));
        let co = n.company_id.and_then(|id| e.company(id));
        let data = match n.kind.as_str() {
            "flash" => {
                let (ep, s) = events::parse_flash(&n.title);
                json!({"episode": ep, "surprise": s})
            }
            "vcm" => {
                let v = co.map(|c| crate::api::v1::stocks::vcm_of(c, &n.ts));
                json!({"low": v.as_ref().and_then(|v| v.low), "high": v.as_ref().and_then(|v| v.high),
                       "until": v.as_ref().and_then(|v| v.until.clone()), "count_today": v.as_ref().map_or(0, |v| v.count_today)})
            }
            "ipo" | "allotment" => match co.map(|c| self.ipo_of(e, c, &n.day)) {
                Some(ipo) => json!({"ipo": ipo}),
                None => return,
            },
            "warning" => json!({"weeks_left": 1}),
            "delisting" => json!({"last_price": co.map(|c| c.last_price)}),
            _ => return,
        };
        let kind = match n.kind.as_str() {
            "ipo" => "ipo_open",
            "allotment" => "ipo_allotted",
            "warning" => "delisting_warning",
            "delisting" => "delisted",
            k => k,
        };
        self.emit(Self::pending(kind, &n.ts, &n.day, stock, data, Some(n.id)));
    }

    /// The `Ipo` object from engine memory (no viewer).
    fn ipo_of(&self, e: &Engine, co: &crate::model::Company, today: &str) -> t::Ipo {
        let float_share = crate::api::v1::market::param_f64(&self.cfg, "company.ipo_float", 0.25);
        let float_qty = (co.shares as f64 * float_share).round_ties_even() as i64;
        let base = (float_qty as f64 * self.cfg.game.ipo_public_base).round_ties_even() as i64;
        let apps: Vec<&crate::model::IpoApplication> = e.st.ipo_apps.values().filter(|a| a.company_id == co.id).collect();
        let q: i64 = apps.iter().map(|a| a.qty).sum();
        let a: i64 = apps.iter().filter_map(|a| a.allotted).sum();
        let status = crate::api::v1::stocks::ipo_status(co, today);
        let allotted = matches!(status, "allotted" | "listed");
        t::Ipo {
            stock: Self::sref(e, co.id),
            title_ja: co.title_ja.clone(),
            status: status.into(),
            price: Money(co.ipo_price),
            sub_start: co.sub_start.clone().unwrap_or_default(),
            sub_end: co.sub_end.clone().unwrap_or_default(),
            listed_on: co.listed_on.clone(),
            first_air: co.first_air.clone(),
            float_quantity: float_qty,
            base_public_quantity: base,
            applied_quantity: q,
            applicants: apps.len() as i64,
            result: allotted.then(|| t::IpoResult {
                oversubscribed: if base != 0 { t::r6(q as f64 / base as f64) } else { 0.0 },
                public_quantity: a,
                allotment_ratio: (q != 0).then(|| t::r6(a as f64 / q as f64)),
            }),
            first_day: None,
            viewer: None,
        }
    }

    fn trade_events(&mut self, tr: &crate::model::Trade, e: &Engine) {
        let Some(co) = e.company(tr.company_id) else { return };
        let unix = chrono::DateTime::parse_from_rfc3339(&tr.ts).map(|d| d.timestamp()).unwrap_or(0);
        let stock = Self::sref(e, tr.company_id);
        // the first trade of a listing day
        if co.listed_on == tr.day && e.st.day_aggs.get(&(tr.company_id, tr.day.clone())).is_some_and(|a| a.volume == tr.qty) {
            self.emit(Self::pending("listed", &tr.ts, &tr.day, Some(stock.clone()), json!({"price": tr.price, "ipo_price": co.ipo_price}), None));
        }
        for data in self.moves.trade(&self.rules, tr.company_id, unix, tr.price, co.prev_close, &tr.day) {
            self.emit(Self::pending("big_move", &tr.ts, &tr.day, Some(stock.clone()), data, None));
        }
    }

    // ------------------------------------------------------------ time

    fn time_msg(&self, e: &Engine) -> Utf8Bytes {
        let ph = self.phase();
        let ends = if self.cfg.always_open {
            None
        } else {
            let g = &self.cfg.game;
            let at = match ph {
                crate::config::Phase::Auction => &g.trading_open,
                crate::config::Phase::Continuous => &g.trading_close,
                crate::config::Phase::Closed => &g.auction_start,
            };
            Some(t::ts(crate::config::format_ts(&crate::config::next_daily_step(&self.clock.now(), at))))
        };
        text(&M::Time {
            now: t::ts(self.clock.ts()),
            trading_day: t::day(self.clock.today()),
            phase: ph.into(),
            phase_ends_at: ends,
            last_closed_day: e.state("last_closed").filter(|s| !s.is_empty()).map(t::day),
        })
    }

    // ------------------------------------------------------------ the 100 ms tick

    /// Tick k: quotes snapshot build (every 10th tick) and delivery to slot k % 10, initial stock
    /// snapshots, the conflated stock flush, and `time`.
    pub fn tick(&mut self, e: &Engine) {
        self.absorb(e);
        let k = self.tick_no;
        self.tick_no += 1;
        let slot = (k % 10) as usize;
        let today = self.clock.today();
        // time: every 20 s, and at once on a phase or day change
        let ph = self.phase();
        if k % TIME_TICKS == 0 || self.last_phase != Some(ph) || self.last_day != today {
            self.last_phase = Some(ph);
            self.last_day = today.clone();
            let tm = self.time_msg(e);
            self.broadcast(&tm);
            let now = self.clock.now().timestamp();
            let expired: Vec<u64> = self.clients.iter().filter(|(_, c)| c.exp.is_some_and(|x| x < now)).map(|(id, _)| *id).collect();
            for id in expired {
                self.rm.push((id, 4001));
            }
            self.flush_removals();
        }
        if slot == 0 {
            let full = self.snapshot_day != today;
            self.snapshot_day = today.clone();
            let (rows, _) = self.quote_rows(e, full);
            self.quotes_seq += 1;
            let msg = text(&M::Quotes {
                seq: self.quotes_seq,
                full,
                trading_day: t::day(today.clone()),
                cols: full.then(|| QUOTE_COLS.iter().map(|s| s.to_string()).collect()),
                rows,
            });
            self.snapshot = Some((self.quotes_seq, msg));
        }
        if let Some((seq, msg)) = self.snapshot.clone() {
            let ids: Vec<u64> =
                self.slots[slot].iter().copied().filter(|id| self.clients[id].quotes && self.clients[id].quotes_after < seq).collect();
            for id in ids {
                self.send_to(id, &msg);
            }
            self.flush_removals();
        }
        let ts = self.clock.ts();
        if !self.init.is_empty() {
            let n = self.init.len().min(INIT_PER_TICK);
            let batch: Vec<(u64, i64)> = self.init.drain(..n).collect();
            let mut cache: HashMap<i64, Utf8Bytes> = HashMap::new();
            for (id, cid) in batch {
                if !self.clients.get(&id).is_some_and(|c| c.stocks.contains(&cid)) {
                    continue;
                }
                let msg = match cache.get(&cid) {
                    Some(m) => m.clone(),
                    None => {
                        let seq = *self.stock_seq.get(&cid).unwrap_or(&0);
                        let recent: Vec<(i64, i64, i64, String)> = e
                            .st
                            .recent_trades
                            .get(&cid)
                            .map(|v| v.iter().rev().take(30).rev().map(|r| (r.id, r.price, r.qty, r.ts.clone())).collect())
                            .unwrap_or_default();
                        let m = self.encode_stock(e, cid, &ts, &today, &recent, true, seq);
                        cache.insert(cid, m.clone());
                        m
                    }
                };
                self.send_to(id, &msg);
                let subs = self.stock_subs.entry(cid).or_default();
                if !subs.contains(&id) {
                    subs.push(id);
                }
            }
            self.flush_removals();
        }
        let dirty: Vec<i64> = self.dirty.drain().collect();
        for cid in dirty {
            let trades = self.new_trades.remove(&cid).unwrap_or_default();
            let Some(subs) = self.stock_subs.get(&cid).filter(|v| !v.is_empty()).cloned() else { continue };
            let seq = {
                let s = self.stock_seq.entry(cid).or_default();
                *s += 1;
                *s
            };
            let msg = self.encode_stock(e, cid, &ts, &today, &trades, false, seq);
            for id in subs {
                self.send_to(id, &msg);
            }
        }
        self.new_trades.clear();
        let dirty: Vec<i64> = self.fut_dirty.drain().collect();
        for k in dirty {
            let Some(subs) = self.fut_subs.get(&k).filter(|v| !v.is_empty()).cloned() else { continue };
            let Some(msg) = self.encode_contract(e, k, &today) else { continue };
            for id in subs {
                self.send_to(id, &msg);
            }
        }
        self.flush_removals();
    }

    /// One contract's snapshot (`seq` per contract, advanced by every message).
    fn encode_contract(&mut self, e: &Engine, k: i64, today: &str) -> Option<Utf8Bytes> {
        let (contract, bids, asks) = crate::api::v1::futures::contract_market(e, k, today)?;
        let seq = {
            let s = self.fut_seq.entry(k).or_default();
            *s += 1;
            *s
        };
        Some(text(&M::Futures { seq, contract, bids, asks }))
    }

    /// A contract's snapshot to one subscriber at once (subscribe, resync).
    fn send_contract(&mut self, id: u64, k: i64, e: &Engine) {
        let today = self.clock.today();
        if let Some(msg) = self.encode_contract(e, k, &today) {
            self.send_to(id, &msg);
        }
    }

    fn turnover_of(&self, cid: i64, today: &str) -> i64 {
        if self.turnover_day == today {
            self.turnover.get(&cid).copied().unwrap_or(0)
        } else {
            0
        }
    }

    /// Rows of the listed stocks; `full` = all of them (and the change baseline is reset).
    fn quote_rows(&mut self, e: &Engine, full: bool) -> (Vec<Value>, usize) {
        let today = self.clock.today();
        let ts = self.clock.ts();
        let mut rows = Vec::new();
        for c in e.companies().filter(|c| c.status == "listed") {
            let vol = e.st.day_aggs.get(&(c.id, today.clone())).map_or(0, |a| a.volume);
            let key = QuoteKey { price: c.last_price, prev: c.prev_close, vol, cooling: crate::exchange::cooling(c, &ts).is_some() };
            let changed = self.snap_last.get(&c.id) != Some(&key);
            self.snap_last.insert(c.id, key);
            if !full && !changed {
                continue;
            }
            rows.push(json!([c.id, c.last_price, c.prev_close, vol, self.turnover_of(c.id, &today), key.cooling]));
        }
        let n = rows.len();
        (rows, n)
    }

    /// A full quotes snapshot for one client (subscribe, resync): current values, the last delta's seq.
    fn send_full_quotes(&mut self, id: u64, e: &Engine) {
        let today = self.clock.today();
        let ts = self.clock.ts();
        let rows: Vec<Value> = e
            .companies()
            .filter(|c| c.status == "listed")
            .map(|c| {
                let vol = e.st.day_aggs.get(&(c.id, today.clone())).map_or(0, |a| a.volume);
                json!([c.id, c.last_price, c.prev_close, vol, self.turnover_of(c.id, &today), crate::exchange::cooling(c, &ts).is_some()])
            })
            .collect();
        let seq = self.quotes_seq;
        let msg = text(&M::Quotes {
            seq,
            full: true,
            trading_day: t::day(today),
            cols: Some(QUOTE_COLS.iter().map(|s| s.to_string()).collect()),
            rows,
        });
        if let Some(c) = self.clients.get_mut(&id) {
            c.quotes_after = seq;
        }
        self.send_to(id, &msg);
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_stock(&self, e: &Engine, cid: i64, ts: &str, today: &str, trades: &[(i64, i64, i64, String)], full: bool, seq: u64) -> Utf8Bytes {
        let Some(c) = e.company(cid) else { return Utf8Bytes::from_static("{}") };
        let agg = e.st.day_aggs.get(&(cid, today.to_string())).copied();
        let book = e.book(cid, 5);
        let lv = |l: &Vec<crate::engine::Level>| l.iter().map(|x| json!([x.price, x.qty, x.orders])).collect::<Vec<_>>();
        let auction = (self.phase() == crate::config::Phase::Auction).then(|| {
            let (p, v) = e.auction_price(cid, c.prev_close);
            t::AuctionIndication { price: p.map(Money), quantity: v }
        });
        let tr: Vec<Value> = trades.iter().rev().map(|(id, p, q, ts)| json!([id, p, q, ts])).collect();
        text(&M::Stock {
            seq,
            id: cid,
            full,
            trading_day: t::day(today),
            as_of: t::ts(self.clock.now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()),
            price: Money(c.last_price),
            prev_close: Money(c.prev_close),
            open: agg.map(|a| Money(a.open)),
            high: agg.map(|a| Money(a.high)),
            low: agg.map(|a| Money(a.low)),
            volume: agg.map_or(0, |a| a.volume),
            turnover: Money(self.turnover_of(cid, today)),
            open_reference: c.open_ref.map(Money),
            bids: lv(&book.bids),
            asks: lv(&book.asks),
            vcm: crate::api::v1::stocks::vcm_of(c, ts),
            auction,
            trades: tr,
        })
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

/// Join `first` with whatever is already queued (no waiting), newline-separated, at most `QUEUE` messages.
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
