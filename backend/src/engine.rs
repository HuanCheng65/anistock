//! The engine: all mutable game state in memory (accounts, holdings, FIFO lots, open orders and their
//! books, companies' market fields, state rows), and the exchange operations ported from
//! `server/exchange.py`. Single-threaded; every public mutating method is one atomic operation that
//! either fails before changing anything (returning the player-facing `OrderError`) or applies fully and
//! hands its row changes to the `ChangeSink` as one batch (one database transaction).
//!
//! Ids are assigned here exactly as SQLite would assign them to v1's inserts: `max(id) + 1` over the rows
//! currently in the table. Orders, trades, ledger, news and accounts are never deleted, so a counter from
//! the loaded maximum suffices; lots are deleted when consumed, so a new lot takes `max(live lot id) + 1`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::ts_add_minutes;
use crate::exchange::{cooling, pct0_str, py_round_div, sanity_band, vcm_anchor, yuan_str, Fees, OrderError, Rules};
use crate::model::*;
use crate::store::persist::{Change, ChangeSink, MemorySink};
use crate::store::{self, Persister, Snapshot, StoreError};

/// Result of `place_order`, same fields as v1's dict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Placed {
    pub order_id: i64,
    pub status: OrderStatus,
    pub filled: i64,
    pub avg_price: Option<i64>,
}

/// A new order as the API receives it. `side` / `kind` are strings so invalid values get v1's messages.
#[derive(Debug, Clone)]
pub struct NewOrder<'a> {
    pub account_id: i64,
    pub company_id: i64,
    pub side: &'a str,
    pub kind: &'a str,
    pub qty: i64,
    pub price: Option<i64>,
    pub day: &'a str,
    pub ts: &'a str,
    /// false during the opening auction: a limit order only rests on the book
    pub match_: bool,
}

/// Aggregated depth: (price, total remaining qty, number of orders).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Level {
    pub price: i64,
    pub qty: i64,
    pub orders: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BookView {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Book {
    /// (Reverse(price), id): best (highest) price first, then time
    bids: BTreeSet<(Reverse<i64>, i64)>,
    /// (price, id): best (lowest) price first, then time
    asks: BTreeSet<(i64, i64)>,
}

#[derive(Debug, Clone)]
pub struct TradeRef {
    pub day: String,
    pub ts: String,
    pub price: i64,
    /// trade id and quantity (the live push's recent trades)
    pub id: i64,
    pub qty: i64,
}

/// Per-day trade aggregate of one company (candles; v1 reads it from `trades`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DayAgg {
    pub open: i64,
    pub high: i64,
    pub low: i64,
    pub close: i64,
    pub volume: i64,
}

/// All mutable game state. Cloned before the daily close so a failed close can be rolled back.
#[derive(Clone, Default)]
pub(crate) struct State {
    pub accounts: BTreeMap<i64, Account>,
    pub companies: BTreeMap<i64, Company>,
    pub holdings: BTreeMap<(i64, i64), Holding>,
    /// hidden rowid of each holdings row (v1 iterates holders in rowid order), and the reverse map
    pub holding_rowid: HashMap<(i64, i64), i64>,
    pub holdings_by_rowid: BTreeMap<i64, (i64, i64)>,
    pub lots: BTreeMap<i64, Lot>,
    pub lots_by_owner: HashMap<(i64, i64), VecDeque<i64>>,
    /// open orders, plus orders closed during the current operation (removed at its end)
    pub orders: HashMap<i64, Order>,
    pub books: HashMap<i64, Book>,
    pub by_account: HashMap<i64, BTreeSet<i64>>,
    /// per company: trades of its latest trading day, in id order (VCM look-back)
    pub recent_trades: HashMap<i64, Vec<TradeRef>>,
    pub state: BTreeMap<String, String>,
    pub specials: HashMap<String, i64>,
    pub ipo_apps: BTreeMap<(i64, i64), IpoApplication>,
    pub company_days: BTreeMap<(i64, String), CompanyDay>,
    pub macro_days: BTreeMap<String, MacroDay>,
    /// (company, day) -> trade aggregate, for days not closed yet
    pub day_aggs: BTreeMap<(i64, String), DayAgg>,
    /// (seller, day) -> SUM(price*qty) of trades sold by 'company' accounts, for days not closed yet
    pub company_proceeds: HashMap<(i64, String), i64>,
    /// day -> ledger amounts received by gov as corp_tax / dividend_tax / stamp_duty
    pub gov_taxes: BTreeMap<String, i64>,
    /// trading days in a row each listed stock has closed under 1 币 (面值退市 countdown)
    pub penny_days: HashMap<i64, i64>,
    /// the valuation filter's state per (company, market index)
    pub jstate: HashMap<i64, crate::vclose::JState>,
    /// players who have restructured: their chain-linked return (crate::restructure)
    pub player_returns: BTreeMap<i64, PlayerReturn>,
    pub last_order_id: i64,
    pub last_trade_id: i64,
    pub last_ledger_id: i64,
    pub last_news_id: i64,
    pub last_account_id: i64,
}

pub struct Engine {
    pub fees: Fees,
    pub rules: Rules,
    pub(crate) st: State,

    pub(crate) out: Vec<Change>,
    dirty_accounts: BTreeSet<i64>,
    dirty_orders: BTreeSet<i64>,
    pub(crate) dirty_companies: BTreeSet<i64>,
    reap: Vec<i64>,
    sink: Box<dyn ChangeSink>,
    db_path: Option<PathBuf>,
    reader: Option<rusqlite::Connection>,
    /// > 0 inside `begin_batch` .. `end_batch`: operations accumulate into one database transaction
    batch_depth: u32,
    /// per company: notional (cents) traded with a player on either side since this engine started
    /// (in memory only; the NPC market reads it to see how active the humans are in each stock)
    player_flow: HashMap<i64, i64>,
}

impl Engine {
    /// Load the game database at `path` (migrations must already be applied), check the invariants, and
    /// start the persistence thread on it.
    pub fn open(path: &Path, fees: Fees, rules: Rules) -> Result<Engine, StoreError> {
        let snap = {
            let conn = store::connect(path)?;
            store::load(&conn)?
        };
        let p = Persister::start(path)?;
        let mut e = Engine::from_snapshot(snap, fees, rules, Box::new(p));
        e.db_path = Some(path.to_path_buf());
        Ok(e)
    }

    /// An engine over a snapshot, writing to any sink (e.g. `MemorySink` for offline use).
    pub fn from_snapshot(s: Snapshot, fees: Fees, rules: Rules, sink: Box<dyn ChangeSink>) -> Engine {
        let mut e = Engine {
            fees,
            rules,
            st: State::from_snapshot_parts(&s),
            out: Vec::new(),
            dirty_accounts: BTreeSet::new(),
            dirty_orders: BTreeSet::new(),
            dirty_companies: BTreeSet::new(),
            reap: Vec::new(),
            sink,
            db_path: None,
            reader: None,
            batch_depth: 0,
            player_flow: HashMap::new(),
        };
        for o in s.open_orders.values() {
            e.st.by_account.entry(o.account_id).or_default().insert(o.id);
            e.book_insert(o);
            e.st.orders.insert(o.id, o.clone());
        }
        e
    }

    /// An empty in-memory engine (no database), mostly for tests.
    pub fn in_memory(fees: Fees, rules: Rules) -> (Engine, MemorySink) {
        let sink = MemorySink::default();
        (Engine::from_snapshot(Snapshot::default(), fees, rules, Box::new(sink.clone())), sink)
    }

    /// Wrap the change sink (the real-time fan-out tees the change stream; see `live.rs`).
    pub fn wrap_sink(&mut self, f: impl FnOnce(Box<dyn ChangeSink>) -> Box<dyn ChangeSink>) {
        self.commit();
        let inner = std::mem::replace(&mut self.sink, Box::new(MemorySink::default()));
        self.sink = f(inner);
    }

    /// Block until every change so far is committed to the database.
    pub fn flush(&mut self) -> Result<(), String> {
        self.commit();
        let r = self.sink.flush();
        if let Some(e) = self.sink.failure() {
            panic!("persistence failed, stopping the engine: {e}");
        }
        r
    }

    // ------------------------------------------------------------ read access

    pub fn account(&self, id: i64) -> Option<&Account> {
        self.st.accounts.get(&id)
    }
    pub fn accounts(&self) -> impl Iterator<Item = &Account> {
        self.st.accounts.values()
    }
    pub fn company(&self, id: i64) -> Option<&Company> {
        self.st.companies.get(&id)
    }
    pub fn companies(&self) -> impl Iterator<Item = &Company> {
        self.st.companies.values()
    }
    pub fn holding(&self, account_id: i64, company_id: i64) -> Option<&Holding> {
        self.st.holdings.get(&(account_id, company_id))
    }
    pub fn holdings_of(&self, account_id: i64) -> impl Iterator<Item = &Holding> {
        self.st.holdings.range((account_id, i64::MIN)..=(account_id, i64::MAX)).map(|(_, h)| h)
    }
    /// An owner's lots in FIFO order.
    pub fn lots_of(&self, account_id: i64, company_id: i64) -> Vec<&Lot> {
        self.st
            .lots_by_owner
            .get(&(account_id, company_id))
            .map(|ids| ids.iter().map(|i| &self.st.lots[i]).collect())
            .unwrap_or_default()
    }
    /// An open order.
    pub fn order(&self, id: i64) -> Option<&Order> {
        self.st.orders.get(&id).filter(|o| o.status == OrderStatus::Open)
    }
    /// An account's open orders, by id.
    pub fn open_orders_of(&self, account_id: i64) -> Vec<&Order> {
        self.st
            .by_account
            .get(&account_id)
            .map(|ids| ids.iter().map(|i| &self.st.orders[i]).collect())
            .unwrap_or_default()
    }
    pub fn state(&self, key: &str) -> Option<&str> {
        self.st.state.get(key).map(String::as_str)
    }
    /// Id of a singleton account (world, cb, gov).
    pub fn special(&self, kind: &str) -> Option<i64> {
        self.st.specials.get(kind).copied()
    }

    pub fn available_cash(&self, account_id: i64) -> i64 {
        self.st.accounts.get(&account_id).map(|a| a.cash - a.cash_frozen).unwrap_or(0)
    }

    pub fn available_shares(&self, account_id: i64, company_id: i64) -> i64 {
        self.st.holdings.get(&(account_id, company_id)).map(|h| h.qty - h.qty_frozen).unwrap_or(0)
    }

    /// Notional (cents) traded with a player in `company_id` since the engine started.
    pub fn player_flow(&self, company_id: i64) -> i64 {
        self.player_flow.get(&company_id).copied().unwrap_or(0)
    }

    /// Players' resting limit orders in `company_id`, best first per side: (is_buy, price, shares left,
    /// created_at).
    pub fn player_orders(&self, company_id: i64) -> Vec<(bool, i64, i64, &str)> {
        let Some(b) = self.st.books.get(&company_id) else { return vec![] };
        let mut out = Vec::new();
        let player = |o: &Order| self.st.accounts.get(&o.account_id).is_some_and(|a| a.kind == "player");
        for (Reverse(p), id) in &b.bids {
            let o = &self.st.orders[id];
            if player(o) {
                out.push((true, *p, o.left(), o.created_at.as_str()));
            }
        }
        for (p, id) in &b.asks {
            let o = &self.st.orders[id];
            if player(o) {
                out.push((false, *p, o.left(), o.created_at.as_str()));
            }
        }
        out
    }

    /// Price levels of the book, best first (v1 `book`).
    pub fn book(&self, company_id: i64, depth: usize) -> BookView {
        let mut view = BookView { bids: vec![], asks: vec![] };
        let Some(b) = self.st.books.get(&company_id) else { return view };
        let push = |levels: &mut Vec<Level>, price: i64, left: i64| {
            if let Some(l) = levels.last_mut() {
                if l.price == price {
                    l.qty += left;
                    l.orders += 1;
                    return true;
                }
            }
            if levels.len() == depth {
                return false;
            }
            levels.push(Level { price, qty: left, orders: 1 });
            true
        };
        for (Reverse(p), id) in &b.bids {
            if !push(&mut view.bids, *p, self.st.orders[id].left()) {
                break;
            }
        }
        for (p, id) in &b.asks {
            if !push(&mut view.asks, *p, self.st.orders[id].left()) {
                break;
            }
        }
        view
    }

    /// Equilibrium price of the resting limit orders: most volume, then least imbalance, then closest to
    /// `reference`. `(None, 0)` if nothing crosses.
    pub fn auction_price(&self, company_id: i64, reference: i64) -> (Option<i64>, i64) {
        let Some(b) = self.st.books.get(&company_id) else { return (None, 0) };
        let mut buys: BTreeMap<i64, i64> = BTreeMap::new();
        let mut sells: BTreeMap<i64, i64> = BTreeMap::new();
        for (Reverse(p), id) in &b.bids {
            *buys.entry(*p).or_default() += self.st.orders[id].left();
        }
        for (p, id) in &b.asks {
            *sells.entry(*p).or_default() += self.st.orders[id].left();
        }
        if buys.is_empty() || sells.is_empty() {
            return (None, 0);
        }
        let prices: BTreeSet<i64> = buys.keys().chain(sells.keys()).copied().collect();
        let mut best: Option<((i64, i64, i64), i64, i64)> = None;
        for p in prices {
            let demand: i64 = buys.range(p..).map(|(_, q)| q).sum();
            let supply: i64 = sells.range(..=p).map(|(_, q)| q).sum();
            let vol = demand.min(supply);
            if vol <= 0 {
                continue;
            }
            let key = (vol, -(demand - supply).abs(), -(p - reference).abs());
            if best.as_ref().is_none_or(|b| key > b.0) {
                best = Some((key, p, vol));
            }
        }
        match best {
            Some((_, p, v)) => (Some(p), v),
            None => (None, 0),
        }
    }

    /// v1 `check_invariants` on the in-memory state.
    pub fn check_invariants(&self) -> Result<(), String> {
        let total: i64 = self.st.accounts.values().map(|a| a.cash).sum();
        if total != 0 {
            return Err(format!("cash does not sum to zero: {total}"));
        }
        let mut held: HashMap<i64, i64> = HashMap::new();
        for h in self.st.holdings.values() {
            *held.entry(h.company_id).or_default() += h.qty;
        }
        for c in self.st.companies.values().filter(|c| c.status == "listed") {
            let h = held.get(&c.id).copied().unwrap_or(0);
            if h != c.shares {
                return Err(format!("share count mismatch: company {} has {} shares, {} held", c.id, c.shares, h));
            }
        }
        for (k, h) in &self.st.holdings {
            let lq: i64 = self.lots_of(k.0, k.1).iter().map(|l| l.qty).sum();
            if lq != h.qty {
                return Err(format!("lots do not match holdings: {k:?} holds {} lots {}", h.qty, lq));
            }
        }
        for a in self.st.accounts.values() {
            if a.kind != "world" && a.kind != "cb" && (a.cash < 0 || a.cash_frozen < 0 || a.cash_frozen > a.cash) {
                return Err(format!("negative or over-frozen balance: account {} {} {}", a.id, a.cash, a.cash_frozen));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ change tracking

    /// End of an operation: emit the final rows of everything touched and hand the batch to the sink.
    pub(crate) fn commit(&mut self) {
        for id in std::mem::take(&mut self.dirty_accounts) {
            if let Some(a) = self.st.accounts.get(&id) {
                self.out.push(Change::AccountBalance { id, cash: a.cash, cash_frozen: a.cash_frozen });
            }
        }
        for id in std::mem::take(&mut self.dirty_orders) {
            if let Some(o) = self.st.orders.get(&id) {
                self.out.push(Change::OrderUpsert(o.clone()));
            }
        }
        for id in std::mem::take(&mut self.dirty_companies) {
            if let Some(c) = self.st.companies.get(&id) {
                self.out.push(Change::CompanyUpdate(c.clone()));
            }
        }
        for id in std::mem::take(&mut self.reap) {
            if self.st.orders.get(&id).is_some_and(|o| o.status != OrderStatus::Open) {
                self.st.orders.remove(&id);
            }
        }
        if !self.out.is_empty() && self.batch_depth == 0 {
            let batch = std::mem::take(&mut self.out);
            self.sink.send(batch);
        }
        // memory must never run ahead of a database that stopped accepting writes: stop the engine (the
        // process exits and restarts from the database)
        if let Some(e) = self.sink.failure() {
            panic!("persistence failed, stopping the engine: {e}");
        }
    }

    pub(crate) fn emit_holding(&mut self, key: (i64, i64)) {
        if let Some(h) = self.st.holdings.get(&key) {
            self.out.push(Change::HoldingUpsert(*h));
        }
    }

    /// `DELETE FROM holdings WHERE account_id=? AND company_id=? AND qty=0 AND qty_frozen=0`
    pub(crate) fn drop_empty_holding(&mut self, key: (i64, i64)) {
        if self.st.holdings.get(&key).is_some_and(|h| h.qty == 0 && h.qty_frozen == 0) {
            self.st.holdings.remove(&key);
            if let Some(r) = self.st.holding_rowid.remove(&key) {
                self.st.holdings_by_rowid.remove(&r);
            }
            self.out.push(Change::HoldingDelete { account_id: key.0, company_id: key.1 });
        }
    }

    pub(crate) fn add_cash(&mut self, id: i64, cash: i64, frozen: i64) {
        if let Some(a) = self.st.accounts.get_mut(&id) {
            a.cash += cash;
            a.cash_frozen += frozen;
            self.dirty_accounts.insert(id);
        }
    }

    fn book_insert(&mut self, o: &Order) {
        let (Some(p), OrderStatus::Open) = (o.price, o.status) else { return };
        let b = self.st.books.entry(o.company_id).or_default();
        match o.side {
            Side::Buy => b.bids.insert((Reverse(p), o.id)),
            Side::Sell => b.asks.insert((p, o.id)),
        };
    }

    fn book_remove(&mut self, o: &Order) {
        let Some(p) = o.price else { return };
        if let Some(b) = self.st.books.get_mut(&o.company_id) {
            match o.side {
                Side::Buy => b.bids.remove(&(Reverse(p), o.id)),
                Side::Sell => b.asks.remove(&(p, o.id)),
            };
        }
    }

    // ------------------------------------------------------------ cash and shares

    /// Move cash between accounts with a ledger row (no-op for amount <= 0).
    #[allow(clippy::too_many_arguments)]
    pub fn transfer(
        &mut self,
        from_id: i64,
        to_id: i64,
        amount: i64,
        reason: &str,
        day: &str,
        ts: &str,
        company_id: Option<i64>,
        r#ref: Option<i64>,
    ) {
        self.do_transfer(from_id, to_id, amount, reason, day, ts, company_id, r#ref);
        self.commit();
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn do_transfer(
        &mut self,
        from_id: i64,
        to_id: i64,
        amount: i64,
        reason: &str,
        day: &str,
        ts: &str,
        company_id: Option<i64>,
        r#ref: Option<i64>,
    ) {
        if amount <= 0 {
            return;
        }
        self.add_cash(from_id, -amount, 0);
        self.add_cash(to_id, amount, 0);
        if matches!(reason, "corp_tax" | "dividend_tax" | "stamp_duty") && Some(to_id) == self.special("gov") {
            *self.st.gov_taxes.entry(day.to_string()).or_default() += amount;
        }
        self.st.last_ledger_id += 1;
        self.out.push(Change::LedgerInsert(LedgerEntry {
            id: self.st.last_ledger_id,
            ts: ts.to_string(),
            day: day.to_string(),
            from_id,
            to_id,
            amount,
            reason: reason.to_string(),
            company_id,
            r#ref,
        }));
    }

    /// Credit shares as a new lot acquired on `day` at `cost` per share.
    pub fn add_shares(&mut self, account_id: i64, company_id: i64, qty: i64, cost: i64, day: &str) {
        self.do_add_shares(account_id, company_id, qty, cost, day);
        self.commit();
    }

    pub(crate) fn do_add_shares(&mut self, account_id: i64, company_id: i64, qty: i64, cost: i64, day: &str) {
        if qty <= 0 {
            return;
        }
        let key = (account_id, company_id);
        if !self.st.holdings.contains_key(&key) {
            // a new row gets rowid max(rowid)+1, like SQLite
            let rowid = self.st.holdings_by_rowid.keys().next_back().map_or(1, |m| m + 1);
            self.st.holding_rowid.insert(key, rowid);
            self.st.holdings_by_rowid.insert(rowid, key);
        }
        self.st.holdings.entry(key).or_insert(Holding { account_id, company_id, qty: 0, qty_frozen: 0 }).qty += qty;
        self.emit_holding(key);
        let id = self.st.lots.keys().next_back().map_or(1, |m| m + 1);
        let lot = Lot { id, account_id, company_id, qty, cost, acquired_on: day.to_string() };
        self.out.push(Change::LotInsert(lot.clone()));
        self.st.lots.insert(id, lot);
        self.st.lots_by_owner.entry(key).or_default().push_back(id);
    }

    /// Take `qty` shares out FIFO; `frozen` of them were reserved by a sell order. Returns the consumed
    /// lots as (qty, cost, acquired_on). Fails without changing anything if the lots are short.
    pub fn remove_shares(
        &mut self,
        account_id: i64,
        company_id: i64,
        qty: i64,
        frozen: i64,
    ) -> Result<Vec<(i64, i64, String)>, String> {
        let r = self.do_remove_shares(account_id, company_id, qty, frozen);
        self.commit();
        r
    }

    pub(crate) fn do_remove_shares(
        &mut self,
        account_id: i64,
        company_id: i64,
        qty: i64,
        frozen: i64,
    ) -> Result<Vec<(i64, i64, String)>, String> {
        let key = (account_id, company_id);
        let have: i64 = self.lots_of(account_id, company_id).iter().map(|l| l.qty).sum();
        if have < qty {
            return Err(format!("lots short by {} for account {account_id} company {company_id}", qty - have));
        }
        if let Some(h) = self.st.holdings.get_mut(&key) {
            h.qty -= qty;
            h.qty_frozen -= frozen;
            self.emit_holding(key);
        }
        let mut taken = Vec::new();
        let mut left = qty;
        while left > 0 {
            let ids = self.st.lots_by_owner.get_mut(&key).expect("lots checked above");
            let id = *ids.front().expect("lots checked above");
            let lot = self.st.lots.get_mut(&id).unwrap();
            let q = left.min(lot.qty);
            taken.push((q, lot.cost, lot.acquired_on.clone()));
            if q == lot.qty {
                ids.pop_front();
                if ids.is_empty() {
                    self.st.lots_by_owner.remove(&key);
                }
                self.st.lots.remove(&id);
                self.out.push(Change::LotDelete { id });
            } else {
                lot.qty -= q;
                let nq = lot.qty;
                self.out.push(Change::LotQty { id, qty: nq });
            }
            left -= q;
        }
        self.drop_empty_holding(key);
        Ok(taken)
    }

    /// Open an account (player registration, NPCs). Returns its id.
    pub fn create_account(
        &mut self,
        kind: &str,
        name: &str,
        username: Option<&str>,
        pw_hash: Option<&str>,
        company_id: Option<i64>,
        ts: &str,
    ) -> i64 {
        let id = self.do_create_account(kind, name, username, pw_hash, company_id, ts);
        self.commit();
        id
    }

    /// Register a player (v1 `app.register`): the account and its starting grant from `world` in one
    /// operation (one database transaction). The caller checks that the username is free. Returns the id.
    pub fn register_player(&mut self, username: &str, pw_hash: &str, grant: i64, day: &str, ts: &str) -> i64 {
        let id = self.do_create_account("player", username, Some(username), Some(pw_hash), None, ts);
        let world = self.special("world").expect("no world account");
        self.do_transfer(world, id, grant, "grant", day, ts, None, None);
        self.commit();
        id
    }

    pub(crate) fn do_create_account(
        &mut self,
        kind: &str,
        name: &str,
        username: Option<&str>,
        pw_hash: Option<&str>,
        company_id: Option<i64>,
        ts: &str,
    ) -> i64 {
        self.st.last_account_id += 1;
        let a = Account {
            id: self.st.last_account_id,
            kind: kind.to_string(),
            name: name.to_string(),
            username: username.map(str::to_string),
            pw_hash: pw_hash.map(str::to_string),
            company_id,
            cash: 0,
            cash_frozen: 0,
            created_at: ts.to_string(),
        };
        self.st.specials.entry(a.kind.clone()).or_insert(a.id);
        self.out.push(Change::AccountInsert(a.clone()));
        let id = a.id;
        self.st.accounts.insert(id, a);
        id
    }

    /// Insert a news row (v1 `daily.news` / the VCM notice).
    pub(crate) fn push_news(
        &mut self,
        ts: &str,
        day: &str,
        kind: &str,
        company_id: Option<i64>,
        title: String,
        body: String,
    ) {
        self.st.last_news_id += 1;
        self.out.push(Change::NewsInsert(News {
            id: self.st.last_news_id,
            ts: ts.to_string(),
            day: day.to_string(),
            kind: kind.to_string(),
            company_id,
            title,
            body: Some(body),
        }));
    }

    /// Group the following operations into one database transaction (until `end_batch`); for multi-step
    /// maintenance such as the NPC-market migration. Each operation still validates on its own.
    pub fn begin_batch(&mut self) {
        self.batch_depth += 1;
    }

    pub fn end_batch(&mut self) {
        self.batch_depth = self.batch_depth.saturating_sub(1);
        self.commit();
    }

    /// Change an account's kind (retiring the v1 NPC accounts).
    pub fn set_account_kind(&mut self, id: i64, kind: &str) {
        if let Some(a) = self.st.accounts.get_mut(&id) {
            a.kind = kind.to_string();
            self.out.push(Change::AccountKind { id, kind: kind.to_string() });
        }
        self.commit();
    }

    /// Publish a news row (events, announcements).
    pub fn post_news(&mut self, ts: &str, day: &str, kind: &str, company_id: Option<i64>, title: String, body: String) {
        self.push_news(ts, day, kind, company_id, title, body);
        self.commit();
    }

    /// Restore the state saved before a failed multi-step operation (the daily close) and drop its changes.
    pub(crate) fn rollback(&mut self, saved: State) {
        self.st = saved;
        self.out.clear();
        self.dirty_accounts.clear();
        self.dirty_orders.clear();
        self.dirty_companies.clear();
        self.reap.clear();
    }

    pub(crate) fn mark_company(&mut self, id: i64) {
        self.dirty_companies.insert(id);
    }

    /// The game database this engine writes to (None for an in-memory engine).
    pub fn db_path(&self) -> Option<&Path> {
        self.db_path.as_deref()
    }

    /// A consistent copy of the database (flushes first; needs a database-backed engine).
    pub fn backup_to(&mut self, dest: &Path) -> Result<PathBuf, String> {
        self.flush()?;
        let path = self.db_path.clone().ok_or("no database")?;
        let conn = store::connect(&path).map_err(|e| e.to_string())?;
        store::backup(&conn, dest).map_err(|e| e.to_string())
    }

    /// Add a company (listing). The row is written with every column as given.
    pub fn insert_company(&mut self, c: Company) {
        self.out.push(Change::CompanyInsert(c.clone()));
        self.st.companies.insert(c.id, c);
        self.commit();
    }

    pub fn set_state(&mut self, key: &str, value: &str) {
        self.do_set_state(key, value);
        self.commit();
    }

    pub(crate) fn do_set_state(&mut self, key: &str, value: &str) {
        self.st.state.insert(key.to_string(), value.to_string());
        self.out.push(Change::StateSet { key: key.to_string(), value: value.to_string() });
    }

    // ------------------------------------------------------------ volatility control

    fn vcm_reference(&self, c: &Company, day: &str, ts: &str) -> (i64, f64) {
        let anchor = vcm_anchor(c);
        if c.vcm_count == 0 {
            let before = ts_add_minutes(ts, -self.rules.vcm_window_min);
            let price = self
                .st
                .recent_trades
                .get(&c.id)
                .and_then(|ts_| ts_.iter().rev().find(|t| t.day == day && t.ts.as_str() <= before.as_str()))
                .map(|t| t.price);
            return (price.unwrap_or(anchor), self.rules.vcm_threshold);
        }
        (anchor, self.rules.vcm_threshold * (c.vcm_count + 1) as f64)
    }

    /// May a trade print at `tp` now? Starts a cooling-off period (and posts a 'vcm' news item) when it
    /// may not, then answers whether `tp` is inside the new band.
    fn allow_fill(&mut self, company_id: i64, tp: i64, day: &str, ts: &str) -> bool {
        let c = &self.st.companies[&company_id];
        if let Some((lo, hi)) = cooling(c, ts) {
            return lo <= tp && tp <= hi;
        }
        let (r, thr) = self.vcm_reference(c, day, ts);
        if r <= 0 || ((tp as f64) / (r as f64) - 1.0).abs() <= thr + 1e-12 {
            return true;
        }
        if self.rules.vcm_min_tick && (tp - r).abs() <= 1 {
            return true;
        }
        let rf = r as f64;
        let mut lo = ((rf * (1.0 - thr) - 1e-9).ceil() as i64).max(1);
        let mut hi = (rf * (1.0 + thr) + 1e-9).floor() as i64;
        if self.rules.vcm_min_tick {
            lo = lo.min((r - 1).max(1));
            hi = hi.max(r + 1);
        }
        let until = ts_add_minutes(ts, self.rules.vcm_cool_min);
        let title = format!("《{}》触发冷静期", c.title);
        let body = format!(
            "成交价将偏离参考价 {} 超过 {}，{} 分钟内只能在 {} – {} 之间成交。",
            yuan_str(r),
            pct0_str(thr),
            self.rules.vcm_cool_min,
            yuan_str(lo),
            yuan_str(hi)
        );
        let c = self.st.companies.get_mut(&company_id).unwrap();
        c.vcm_until = Some(until);
        c.vcm_low = Some(lo);
        c.vcm_high = Some(hi);
        c.vcm_count += 1;
        self.dirty_companies.insert(company_id);
        self.st.last_news_id += 1;
        self.out.push(Change::NewsInsert(News {
            id: self.st.last_news_id,
            ts: ts.to_string(),
            day: day.to_string(),
            kind: "vcm".into(),
            company_id: Some(company_id),
            title,
            body: Some(body),
        }));
        lo <= tp && tp <= hi
    }

    // ------------------------------------------------------------ orders

    /// Validate, freeze, match (v1 `place_order`). With `match_ = false` (the opening auction) a limit
    /// order only rests on the book.
    pub fn place_order(&mut self, req: &NewOrder) -> Result<Placed, OrderError> {
        let r = self.do_place_order(req);
        self.commit();
        r
    }

    /// A player's order in the continuous phase (v1 `app.place`): opens the market for `req.day` first if
    /// that has not happened yet (`open_market`), then places the order, as one operation. If the order is
    /// rejected the opening auction is rolled back too, like v1's single transaction (it then runs with the
    /// next request, at that request's timestamp).
    pub fn open_and_place(&mut self, req: &NewOrder) -> Result<Placed, OrderError> {
        if self.state("auction_day") == Some(req.day) {
            return self.place_order(req);
        }
        self.commit();
        let saved = self.st.clone();
        self.do_open_market(req.day, req.ts);
        match self.do_place_order(req) {
            Ok(p) => {
                self.commit();
                Ok(p)
            }
            Err(e) => {
                self.rollback(saved);
                Err(e)
            }
        }
    }

    fn do_place_order(&mut self, req: &NewOrder) -> Result<Placed, OrderError> {
        let side = Side::parse(req.side).ok_or_else(|| OrderError::new("买卖方向不对"))?;
        let kind = Kind::parse(req.kind).ok_or_else(|| OrderError::new("委托类型不对"))?;
        let qty = req.qty;
        if qty <= 0 {
            return Err(OrderError::new("数量必须是正整数"));
        }
        let company = match self.st.companies.get(&req.company_id) {
            Some(c) if c.status == "listed" => c,
            _ => return Err(OrderError::new("这支股票不存在或已退市")),
        };
        let (_lo, hi) = sanity_band(company, &self.rules);
        let price = match kind {
            Kind::Limit => {
                let p = match req.price {
                    Some(p) if p > 0 => p,
                    _ => return Err(OrderError::new("限价单需要价格")),
                };
                if p > hi {
                    return Err(OrderError(format!(
                        "价格太离谱了：不能超过昨收的 {} 倍（{}）",
                        self.rules.sanity_mult,
                        yuan_str(hi)
                    )));
                }
                Some(p)
            }
            Kind::Market => {
                if !req.match_ {
                    return Err(OrderError::new("集合竞价阶段只接受限价单"));
                }
                None
            }
        };

        // freeze
        let mut frozen = 0;
        match side {
            Side::Buy => {
                if let Some(p) = price {
                    frozen = self.fees.buy_reserve(p, qty);
                    if self.available_cash(req.account_id) < frozen {
                        return Err(OrderError::new("可用资金不足"));
                    }
                    self.add_cash(req.account_id, 0, frozen);
                }
            }
            Side::Sell => {
                if self.available_shares(req.account_id, req.company_id) < qty {
                    return Err(OrderError::new("可卖股数不足"));
                }
                let key = (req.account_id, req.company_id);
                self.st.holdings.get_mut(&key).unwrap().qty_frozen += qty;
                self.emit_holding(key);
            }
        }

        self.st.last_order_id += 1;
        let id = self.st.last_order_id;
        let o = Order {
            id,
            account_id: req.account_id,
            company_id: req.company_id,
            side,
            kind,
            price,
            qty,
            filled: 0,
            frozen,
            status: OrderStatus::Open,
            created_at: req.ts.to_string(),
            updated_at: req.ts.to_string(),
        };
        self.st.orders.insert(id, o);
        self.st.by_account.entry(req.account_id).or_default().insert(id);
        self.dirty_orders.insert(id);

        let (filled, notional) = if req.match_ { self.do_match(id, req.day, req.ts) } else { (0, 0) };

        let o = &self.st.orders[&id];
        if o.filled == o.qty {
            self.close(id, OrderStatus::Filled, req.ts);
        } else if kind == Kind::Market {
            self.close(id, OrderStatus::Cancelled, req.ts); // immediate-or-cancel remainder
        } else {
            let o = o.clone();
            self.book_insert(&o);
        }
        Ok(Placed {
            order_id: id,
            status: self.st.orders[&id].status,
            filled,
            avg_price: if filled > 0 { Some(py_round_div(notional, filled)) } else { None },
        })
    }

    /// Cancel an open order of `account_id` (v1 `cancel_order`).
    pub fn cancel_order(&mut self, account_id: i64, order_id: i64, ts: &str) -> Result<(), OrderError> {
        let open_mine = self.st.orders.get(&order_id).is_some_and(|o| o.account_id == account_id);
        if !open_mine {
            // closed orders are not in memory: ask the database which message applies
            return Err(OrderError::new(match self.closed_order_owner(order_id) {
                Some(owner) if owner == account_id => "这笔委托已经结束，不能撤单",
                _ => "找不到这笔委托",
            }));
        }
        self.close(order_id, OrderStatus::Cancelled, ts);
        self.commit();
        Ok(())
    }

    fn closed_order_owner(&mut self, order_id: i64) -> Option<i64> {
        if order_id <= 0 || order_id > self.st.last_order_id {
            return None;
        }
        let path = self.db_path.clone()?;
        if self.flush().is_err() {
            return None;
        }
        if self.reader.is_none() {
            self.reader = store::connect(&path).ok();
        }
        let conn = self.reader.as_ref()?;
        store::order_owner_status(conn, order_id).ok().flatten().map(|(a, _)| a)
    }

    /// Cancel every open order of an account in one stock (the NPCs' requote).
    pub fn cancel_all(&mut self, account_id: i64, company_id: i64, ts: &str) {
        let ids: Vec<i64> = self
            .st
            .by_account
            .get(&account_id)
            .map(|s| s.iter().copied().filter(|i| self.st.orders[i].company_id == company_id).collect())
            .unwrap_or_default();
        for id in ids {
            self.close(id, OrderStatus::Cancelled, ts);
        }
        self.commit();
    }

    /// End an order and release whatever it still has frozen (v1 `_close`). Idempotent on closed orders
    /// still in memory (v1 closes a filled order a second time in `place_order`).
    pub(crate) fn close(&mut self, id: i64, status: OrderStatus, ts: &str) {
        let o = self.st.orders[&id].clone();
        if o.side == Side::Buy && o.frozen > 0 {
            self.add_cash(o.account_id, 0, -o.frozen);
        }
        if o.side == Side::Sell {
            let left = o.qty - o.filled;
            if left > 0 {
                let key = (o.account_id, o.company_id);
                if let Some(h) = self.st.holdings.get_mut(&key) {
                    h.qty_frozen -= left;
                    self.emit_holding(key);
                }
                self.drop_empty_holding(key);
            }
        }
        if o.status == OrderStatus::Open {
            self.book_remove(&o);
            if let Some(s) = self.st.by_account.get_mut(&o.account_id) {
                s.remove(&id);
                if s.is_empty() {
                    self.st.by_account.remove(&o.account_id);
                }
            }
            self.reap.push(id);
        }
        let o = self.st.orders.get_mut(&id).unwrap();
        o.status = status;
        o.frozen = 0;
        o.updated_at = ts.to_string();
        self.dirty_orders.insert(id);
    }

    /// Next resting order after `cursor` on the side opposite to `side`, if its price still crosses `limit`.
    fn next_resting(&self, company_id: i64, side: Side, limit: i64, cursor: Option<(i64, i64)>) -> Option<(i64, i64)> {
        let b = self.st.books.get(&company_id)?;
        match side {
            Side::Buy => {
                let mut it = match cursor {
                    None => b.asks.range(..),
                    Some(k) => b.asks.range((Excluded(k), Unbounded)),
                };
                it.next().filter(|(p, _)| *p <= limit).copied()
            }
            Side::Sell => {
                let mut it = match cursor {
                    None => b.bids.range(..),
                    Some((p, id)) => b.bids.range((Excluded((Reverse(p), id)), Unbounded)),
                };
                it.next().filter(|(Reverse(p), _)| *p >= limit).map(|(Reverse(p), id)| (*p, *id))
            }
        }
    }

    /// Match an incoming order against the book (v1 `_match`). Returns (shares filled, notional). Stops at
    /// the first price the volatility control will not let print.
    fn do_match(&mut self, order_id: i64, day: &str, ts: &str) -> (i64, i64) {
        let o = self.st.orders[&order_id].clone();
        let (lo, hi) = sanity_band(&self.st.companies[&o.company_id], &self.rules);
        let limit = match (o.side, o.price) {
            (Side::Buy, Some(p)) => p,
            (Side::Buy, None) => hi,
            (Side::Sell, Some(p)) => p,
            (Side::Sell, None) => lo,
        };
        let mut remaining = o.qty - o.filled;
        let (mut filled, mut notional) = (0, 0);
        let mut cursor = None;
        while let Some((tp, rid)) = self.next_resting(o.company_id, o.side, limit, cursor) {
            cursor = Some((tp, rid));
            if remaining <= 0 {
                break;
            }
            let r = &self.st.orders[&rid];
            if r.account_id == o.account_id {
                continue; // no self-trades
            }
            if r.status != OrderStatus::Open {
                continue;
            }
            let r_left = r.qty - r.filled;
            if !self.allow_fill(o.company_id, tp, day, ts) {
                break;
            }
            let mut q = remaining.min(r_left);
            if o.side == Side::Buy && o.kind == Kind::Market {
                // pay as you go: never spend more than is free right now
                let per = tp + (tp as f64 * self.fees.commission).ceil() as i64 + 1;
                q = q.min(self.available_cash(o.account_id).div_euclid(per));
                if q <= 0 {
                    break;
                }
            }
            let (buy, sell) = if o.side == Side::Buy { (order_id, rid) } else { (rid, order_id) };
            self.fill(buy, sell, q, tp, day, ts);
            remaining -= q;
            filled += q;
            notional += q * tp;
        }
        (filled, notional)
    }

    /// Settle one trade of `q` shares at `tp` (v1 `_fill`).
    fn fill(&mut self, buy_id: i64, sell_id: i64, q: i64, tp: i64, day: &str, ts: &str) {
        let buy = self.st.orders[&buy_id].clone();
        let sell = self.st.orders[&sell_id].clone();
        let company_id = buy.company_id;
        let notional = q * tp;
        let is_player = |a: i64| self.st.accounts.get(&a).is_some_and(|x| x.kind == "player");
        if is_player(buy.account_id) || is_player(sell.account_id) {
            *self.player_flow.entry(company_id).or_default() += notional;
        }
        let c_buy = self.fees.commission_on(notional);
        let c_sell = self.fees.commission_on(notional);
        let stamp = self.fees.stamp_on(notional);
        let world = self.special("world").expect("no world account");
        let gov = self.special("gov").expect("no gov account");

        self.st.last_trade_id += 1;
        let trade_id = self.st.last_trade_id;
        self.out.push(Change::TradeInsert(Trade {
            id: trade_id,
            company_id,
            price: tp,
            qty: q,
            buyer_id: buy.account_id,
            seller_id: sell.account_id,
            buy_order: Some(buy.id),
            sell_order: Some(sell.id),
            ts: ts.to_string(),
            day: day.to_string(),
        }));
        let recent = self.st.recent_trades.entry(company_id).or_default();
        if recent.last().is_some_and(|t| t.day.as_str() < day) {
            recent.retain(|t| t.day.as_str() >= day);
        }
        recent.push(TradeRef { day: day.to_string(), ts: ts.to_string(), price: tp, id: trade_id, qty: q });
        self.st
            .day_aggs
            .entry((company_id, day.to_string()))
            .and_modify(|a| {
                a.high = a.high.max(tp);
                a.low = a.low.min(tp);
                a.close = tp;
                a.volume += q;
            })
            .or_insert(DayAgg { open: tp, high: tp, low: tp, close: tp, volume: q });
        if self.st.accounts.get(&sell.account_id).is_some_and(|a| a.kind == "company") {
            *self.st.company_proceeds.entry((sell.account_id, day.to_string())).or_default() += notional;
        }

        // buyer pays; a resting/limit buy pays out of its frozen cash
        let spend = notional + c_buy;
        if buy.frozen > 0 {
            let used = spend.min(buy.frozen);
            self.st.orders.get_mut(&buy_id).unwrap().frozen -= used;
            self.add_cash(buy.account_id, 0, -used);
        }
        let cid = Some(company_id);
        let tref = Some(trade_id);
        self.do_transfer(buy.account_id, sell.account_id, notional, "trade", day, ts, cid, tref);
        self.do_transfer(buy.account_id, world, c_buy, "commission", day, ts, cid, tref);
        self.do_transfer(sell.account_id, world, c_sell, "commission", day, ts, cid, tref);
        self.do_transfer(sell.account_id, gov, stamp, "stamp_duty", day, ts, cid, tref);

        self.do_remove_shares(sell.account_id, company_id, q, q)
            .unwrap_or_else(|e| panic!("settlement of trade {trade_id}: {e}"));
        self.do_add_shares(buy.account_id, company_id, q, tp, day);

        for id in [buy_id, sell_id] {
            let o = self.st.orders.get_mut(&id).unwrap();
            o.filled += q;
            o.updated_at = ts.to_string();
            let full = o.filled == o.qty;
            self.dirty_orders.insert(id);
            if full {
                self.close(id, OrderStatus::Filled, ts);
            }
        }
        self.st.companies.get_mut(&company_id).unwrap().last_price = tp;
        self.dirty_companies.insert(company_id);
    }

    // ------------------------------------------------------------ opening auction (集合竞价)

    /// Clear one stock's opening auction at one price (v1 `run_auction`). Returns the opening price, or
    /// None if nothing crossed. Does not touch `open_ref` / VCM state; see `open_market`.
    pub fn run_auction(&mut self, company_id: i64, day: &str, ts: &str) -> Option<i64> {
        let r = self.do_run_auction(company_id, day, ts);
        self.commit();
        r
    }

    fn do_run_auction(&mut self, company_id: i64, day: &str, ts: &str) -> Option<i64> {
        let prev_close = self.st.companies.get(&company_id)?.prev_close;
        let (p, vol) = self.auction_price(company_id, prev_close);
        let p = p?;
        let b = &self.st.books[&company_id];
        let buys: Vec<i64> = b.bids.iter().take_while(|(Reverse(bp), _)| *bp >= p).map(|(_, id)| *id).collect();
        let sells: Vec<i64> = b.asks.iter().take_while(|(sp, _)| *sp <= p).map(|(_, id)| *id).collect();
        let mut left = vol;
        'buys: for b in &buys {
            if left <= 0 {
                break;
            }
            for s in &sells {
                if left <= 0 {
                    break;
                }
                let buy = &self.st.orders[b];
                let sell = &self.st.orders[s];
                if buy.status != OrderStatus::Open {
                    continue 'buys;
                }
                if sell.status != OrderStatus::Open || sell.account_id == buy.account_id {
                    continue;
                }
                let q = buy.left().min(sell.left()).min(left);
                if q > 0 {
                    self.fill(*b, *s, q, p, day, ts);
                    left -= q;
                }
            }
        }
        if left < vol {
            Some(p)
        } else {
            None
        }
    }

    /// Once a day when continuous trading starts (v1 `app.ensure_opening_auction`): clear every listed
    /// stock's opening auction, set the day's VCM anchor (`open_ref` = auction price or previous close)
    /// and reset the cooling-off state; recorded in state `auction_day`. No-op if already done for `day`.
    pub fn open_market(&mut self, day: &str, ts: &str) {
        self.do_open_market(day, ts);
        self.commit();
    }

    fn do_open_market(&mut self, day: &str, ts: &str) {
        if self.state("auction_day") == Some(day) {
            return;
        }
        let ids: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
        for id in ids {
            let prev_close = self.st.companies[&id].prev_close;
            let price = self.do_run_auction(id, day, ts);
            let c = self.st.companies.get_mut(&id).unwrap();
            c.open_ref = Some(match price {
                Some(p) if p != 0 => p,
                _ => prev_close,
            });
            c.vcm_until = None;
            c.vcm_count = 0;
            self.dirty_companies.insert(id);
        }
        self.do_set_state("auction_day", day);
    }
}

impl State {
    fn from_snapshot_parts(s: &Snapshot) -> State {
        let mut st = State {
            accounts: s.accounts.clone(),
            companies: s.companies.clone(),
            holdings: s.holdings.clone(),
            lots: s.lots.clone(),
            state: s.state.clone(),
            ipo_apps: s.ipo_apps.clone(),
            company_days: s.company_days.clone(),
            macro_days: s.macro_days.clone(),
            company_proceeds: s.company_proceeds.clone(),
            gov_taxes: s.gov_taxes.clone(),
            penny_days: s.penny_days.clone(),
            jstate: s.jstate.clone(),
            player_returns: s.player_returns.clone(),
            last_order_id: s.max_order_id,
            last_trade_id: s.max_trade_id,
            last_ledger_id: s.max_ledger_id,
            last_news_id: s.max_news_id,
            last_account_id: s.max_account_id,
            ..State::default()
        };
        for a in s.accounts.values() {
            st.specials.entry(a.kind.clone()).or_insert(a.id);
        }
        for l in s.lots.values() {
            st.lots_by_owner.entry((l.account_id, l.company_id)).or_default().push_back(l.id);
        }
        for (k, r) in &s.holding_rowids {
            st.holding_rowid.insert(*k, *r);
            st.holdings_by_rowid.insert(*r, *k);
        }
        for (c, ts) in &s.recent_trades {
            st.recent_trades.insert(
                *c,
                ts.iter().map(|t| TradeRef { day: t.day.clone(), ts: t.ts.clone(), price: t.price, id: t.id, qty: t.qty }).collect(),
            );
        }
        for (k, a) in &s.day_aggs {
            st.day_aggs.insert(k.clone(), DayAgg { open: a.0, high: a.1, low: a.2, close: a.3, volume: a.4 });
        }
        st
    }
}
