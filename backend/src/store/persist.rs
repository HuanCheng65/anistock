//! Write-behind persistence. The engine describes every row it changes as a `Change`; a dedicated
//! thread applies them in order, one transaction per batch, committing at most 50 ms after the first
//! change of the batch arrives (WAL, synchronous=NORMAL). `flush()` blocks until everything sent
//! before it is committed.
//!
//! Changes are applied in emission order, which reproduces v1's SQL statement order where it matters:
//! `holdings` rows are deleted and re-inserted exactly when v1 does (so even hidden rowids agree), and a
//! consumed lot is deleted before the next lot is inserted with the id SQLite would have given it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use rusqlite::{params, Connection};

use crate::model::*;

pub const BATCH_WINDOW: Duration = Duration::from_millis(50);

/// One row-level change. Inserts carry engine-assigned ids.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    AccountInsert(Account),
    /// `UPDATE accounts SET cash, cash_frozen`
    /// `INSERT OR REPLACE INTO valuation_joint` (the joint filter's state of one show, JSON)
    ValuationJoint { company_id: i64, state: String, day: String, updated_at: String },
    ValuationJointDelete { company_id: i64 },
    /// `INSERT INTO company_works` (a new season joins its series' company; V9)
    CompanyWork { work_id: i64, company_id: i64, joined_on: String },
    /// `UPDATE accounts SET kind`
    AccountKind { id: i64, kind: String },
    /// `UPDATE accounts SET name, username, pw_hash` (a closed account loses its identity)
    AccountIdentity { id: i64, name: String, username: Option<String>, pw_hash: Option<String> },
    AccountBalance {
        id: i64,
        cash: i64,
        cash_frozen: i64,
    },
    /// Full row, inserted or replacing the row with the same id.
    OrderUpsert(Order),
    TradeInsert(Trade),
    LedgerInsert(LedgerEntry),
    /// Insert the holding, or update qty/qty_frozen of the existing row (keeps its rowid).
    HoldingUpsert(Holding),
    HoldingDelete {
        account_id: i64,
        company_id: i64,
    },
    LotInsert(Lot),
    LotQty {
        id: i64,
        qty: i64,
    },
    LotDelete {
        id: i64,
    },
    /// The exchange-owned columns of `companies`; other columns are never touched by this change.
    CompanyMarket {
        id: i64,
        last_price: i64,
        open_ref: Option<i64>,
        vcm_until: Option<String>,
        vcm_low: Option<i64>,
        vcm_high: Option<i64>,
        vcm_count: i64,
    },
    /// A new company row (every column).
    CompanyInsert(Company),
    /// Every column of an existing company (engine-owned row).
    CompanyUpdate(Company),
    /// `INSERT OR REPLACE INTO company_days` (the row is deleted and re-inserted, like v1).
    CompanyDayReplace(CompanyDay),
    /// `INSERT OR IGNORE INTO company_days(...) VALUES (c, day, 0,0,0,0,0)`
    CompanyDayInsertIgnore {
        company_id: i64,
        day: String,
    },
    /// `UPDATE company_days SET dividend_ps`
    CompanyDayDividend {
        company_id: i64,
        day: String,
        dividend_ps: i64,
    },
    /// `UPDATE company_days SET float_shares` (V23)
    CompanyDayFloat {
        company_id: i64,
        day: String,
        float_shares: i64,
    },
    /// `INSERT OR REPLACE INTO candles`
    CandleReplace(Candle),
    /// `INSERT OR REPLACE INTO macro_days`
    MacroDayReplace(MacroDay),
    IpoAppInsert(IpoApplication),
    IpoAppDelete {
        company_id: i64,
        account_id: i64,
    },
    /// `UPDATE ipo_applications SET allotted=?, frozen=0`
    IpoAppAllot {
        company_id: i64,
        account_id: i64,
        allotted: i64,
    },
    NewsInsert(News),
    /// `INSERT INTO restructures` (id assigned by the database)
    RestructureInsert(Restructure),
    /// `INSERT OR REPLACE INTO player_returns`
    PlayerReturnSet(PlayerReturn),
    /// `INSERT OR REPLACE INTO player_days` (the daily asset snapshot, written by the close)
    PlayerDayReplace(PlayerDay),
    /// 融资融券 rows (crate::margin)
    Margin(crate::margin::MarginChange),
    /// corporate actions (crate::corporate, V10)
    Corporate(crate::corporate::CorporateChange),
    /// 热度期货 rows (crate::futures, V19)
    Futures(crate::futures::FuturesChange),
    /// 指数基金 rows (crate::funds, V24)
    Funds(crate::funds::store::FundsChange),
    /// `INSERT OR REPLACE INTO lots` (a split rescales a lot's quantity and cost)
    LotReplace(Lot),
    StateSet {
        key: String,
        value: String,
    },
}

pub fn apply(conn: &Connection, c: &Change) -> rusqlite::Result<()> {
    match c {
        Change::AccountInsert(a) => {
            conn.prepare_cached(
                "INSERT INTO accounts(id, kind, name, username, pw_hash, company_id, cash, cash_frozen, created_at)
                 VALUES (?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                a.id,
                a.kind,
                a.name,
                a.username,
                a.pw_hash,
                a.company_id,
                a.cash,
                a.cash_frozen,
                a.created_at
            ])?;
        }
        Change::ValuationJoint { company_id, state, day, updated_at } => {
            conn.prepare_cached(
                "INSERT INTO valuation_joint(company_id, state, day, updated_at) VALUES (?,?,?,?)
                 ON CONFLICT(company_id) DO UPDATE SET state=excluded.state, day=excluded.day, updated_at=excluded.updated_at",
            )?
            .execute(params![company_id, state, day, updated_at])?;
        }
        Change::CompanyWork { work_id, company_id, joined_on } => {
            conn.prepare_cached("INSERT OR REPLACE INTO company_works(work_id, company_id, joined_on) VALUES (?,?,?)")?
                .execute(params![work_id, company_id, joined_on])?;
        }
        Change::ValuationJointDelete { company_id } => {
            conn.prepare_cached("DELETE FROM valuation_joint WHERE company_id=?")?.execute(params![company_id])?;
        }
        Change::AccountKind { id, kind } => {
            conn.prepare_cached("UPDATE accounts SET kind=? WHERE id=?")?.execute(params![kind, id])?;
        }
        Change::AccountIdentity { id, name, username, pw_hash } => {
            conn.prepare_cached("UPDATE accounts SET name=?, username=?, pw_hash=? WHERE id=?")?
                .execute(params![name, username, pw_hash, id])?;
        }
        Change::AccountBalance { id, cash, cash_frozen } => {
            conn.prepare_cached("UPDATE accounts SET cash=?, cash_frozen=? WHERE id=?")?.execute(params![
                cash,
                cash_frozen,
                id
            ])?;
        }
        Change::OrderUpsert(o) => {
            conn.prepare_cached(
                "INSERT INTO orders(id, account_id, company_id, side, kind, price, qty, filled, frozen, status,
                                    created_at, updated_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id, company_id=excluded.company_id,
                   side=excluded.side, kind=excluded.kind, price=excluded.price, qty=excluded.qty,
                   filled=excluded.filled, frozen=excluded.frozen, status=excluded.status,
                   created_at=excluded.created_at, updated_at=excluded.updated_at",
            )?
            .execute(params![
                o.id,
                o.account_id,
                o.company_id,
                o.side.as_str(),
                o.kind.as_str(),
                o.price,
                o.qty,
                o.filled,
                o.frozen,
                o.status.as_str(),
                o.created_at,
                o.updated_at
            ])?;
        }
        Change::TradeInsert(t) => {
            conn.prepare_cached(
                "INSERT INTO trades(id, company_id, price, qty, buyer_id, seller_id, buy_order, sell_order, ts, day)
                 VALUES (?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                t.id,
                t.company_id,
                t.price,
                t.qty,
                t.buyer_id,
                t.seller_id,
                t.buy_order,
                t.sell_order,
                t.ts,
                t.day
            ])?;
        }
        Change::LedgerInsert(l) => {
            conn.prepare_cached(
                "INSERT INTO ledger(id, ts, day, from_id, to_id, amount, reason, company_id, ref)
                 VALUES (?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                l.id,
                l.ts,
                l.day,
                l.from_id,
                l.to_id,
                l.amount,
                l.reason,
                l.company_id,
                l.r#ref
            ])?;
        }
        Change::HoldingUpsert(h) => {
            conn.prepare_cached(
                "INSERT INTO holdings(account_id, company_id, qty, qty_frozen) VALUES (?,?,?,?)
                 ON CONFLICT(account_id, company_id) DO UPDATE SET qty=excluded.qty, qty_frozen=excluded.qty_frozen",
            )?
            .execute(params![h.account_id, h.company_id, h.qty, h.qty_frozen])?;
        }
        Change::HoldingDelete { account_id, company_id } => {
            conn.prepare_cached("DELETE FROM holdings WHERE account_id=? AND company_id=?")?
                .execute(params![account_id, company_id])?;
        }
        Change::LotInsert(l) => {
            conn.prepare_cached(
                "INSERT INTO lots(id, account_id, company_id, qty, cost, acquired_on) VALUES (?,?,?,?,?,?)",
            )?
            .execute(params![l.id, l.account_id, l.company_id, l.qty, l.cost, l.acquired_on])?;
        }
        Change::LotQty { id, qty } => {
            conn.prepare_cached("UPDATE lots SET qty=? WHERE id=?")?.execute(params![qty, id])?;
        }
        Change::LotDelete { id } => {
            conn.prepare_cached("DELETE FROM lots WHERE id=?")?.execute(params![id])?;
        }
        Change::CompanyMarket { id, last_price, open_ref, vcm_until, vcm_low, vcm_high, vcm_count } => {
            conn.prepare_cached(
                "UPDATE companies SET last_price=?, open_ref=?, vcm_until=?, vcm_low=?, vcm_high=?, vcm_count=?
                 WHERE id=?",
            )?
            .execute(params![last_price, open_ref, vcm_until, vcm_low, vcm_high, vcm_count, id])?;
        }
        Change::CompanyInsert(c) => {
            conn.prepare_cached(
                "INSERT INTO companies(id, code, title, title_ja, title_en, studio, episodes, long_runner, first_air,
                   listed_on, status, sub_start, sub_end, shares, ipo_price, prev_close, open_ref, vcm_until, vcm_low,
                   vcm_high, vcm_count, last_price, fair_value, profit_week, low_weeks, delisted_on, source, genres)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                c.id,
                c.code,
                c.title,
                c.title_ja,
                c.title_en,
                c.studio,
                c.episodes,
                c.long_runner,
                c.first_air,
                c.listed_on,
                c.status,
                c.sub_start,
                c.sub_end,
                c.shares,
                c.ipo_price,
                c.prev_close,
                c.open_ref,
                c.vcm_until,
                c.vcm_low,
                c.vcm_high,
                c.vcm_count,
                c.last_price,
                c.fair_value,
                c.profit_week,
                c.low_weeks,
                c.delisted_on,
                c.source,
                c.genres
            ])?;
        }
        Change::CompanyUpdate(c) => {
            conn.prepare_cached(
                "UPDATE companies SET code=?, title=?, title_ja=?, title_en=?, studio=?, episodes=?, long_runner=?,
                   first_air=?, listed_on=?, status=?, sub_start=?, sub_end=?, shares=?, ipo_price=?, prev_close=?,
                   open_ref=?, vcm_until=?, vcm_low=?, vcm_high=?, vcm_count=?, last_price=?, fair_value=?,
                   profit_week=?, low_weeks=?, delisted_on=?, source=?, genres=? WHERE id=?",
            )?
            .execute(params![
                c.code,
                c.title,
                c.title_ja,
                c.title_en,
                c.studio,
                c.episodes,
                c.long_runner,
                c.first_air,
                c.listed_on,
                c.status,
                c.sub_start,
                c.sub_end,
                c.shares,
                c.ipo_price,
                c.prev_close,
                c.open_ref,
                c.vcm_until,
                c.vcm_low,
                c.vcm_high,
                c.vcm_count,
                c.last_price,
                c.fair_value,
                c.profit_week,
                c.low_weeks,
                c.delisted_on,
                c.source,
                c.genres,
                c.id
            ])?;
        }
        Change::CompanyDayReplace(d) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired, dividend_ps)
                 VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![d.company_id, d.day, d.revenue, d.rev_us, d.rev_cn, d.rev_jp, d.aired, d.dividend_ps])?;
        }
        Change::CompanyDayInsertIgnore { company_id, day } => {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired)
                 VALUES (?,?,0,0,0,0,0)",
            )?
            .execute(params![company_id, day])?;
        }
        Change::CompanyDayDividend { company_id, day, dividend_ps } => {
            conn.prepare_cached("UPDATE company_days SET dividend_ps=? WHERE company_id=? AND day=?")?
                .execute(params![dividend_ps, company_id, day])?;
        }
        Change::CompanyDayFloat { company_id, day, float_shares } => {
            conn.prepare_cached("UPDATE company_days SET float_shares=? WHERE company_id=? AND day=?")?
                .execute(params![float_shares, company_id, day])?;
        }
        Change::CandleReplace(c) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO candles(company_id, day, open, high, low, close, volume, source)
                 VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![c.company_id, c.day, c.open, c.high, c.low, c.close, c.volume, c.source])?;
        }
        Change::MacroDayReplace(m) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO macro_days(day, rate, m2, m2_growth, injected, exported, taxes, interest)
                 VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                m.day,
                m.rate,
                m.m2,
                m.m2_growth,
                m.injected,
                m.exported,
                m.taxes,
                m.interest
            ])?;
        }
        Change::IpoAppInsert(a) => {
            conn.prepare_cached(
                "INSERT INTO ipo_applications(company_id, account_id, qty, frozen, allotted, created_at)
                 VALUES (?,?,?,?,?,?)",
            )?
            .execute(params![a.company_id, a.account_id, a.qty, a.frozen, a.allotted, a.created_at])?;
        }
        Change::RestructureInsert(r) => {
            conn.prepare_cached(
                "INSERT INTO restructures(account_id, day, ts, assets_before, sold, injected) VALUES (?,?,?,?,?,?)",
            )?
            .execute(params![r.account_id, r.day, r.ts, r.assets_before, r.sold, r.injected])?;
        }
        Change::PlayerReturnSet(p) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO player_returns(account_id, return_factor, segment_base, restructures, last_day)
                 VALUES (?,?,?,?,?)",
            )?
            .execute(params![p.account_id, p.return_factor, p.segment_base, p.restructures, p.last_day])?;
        }
        Change::PlayerDayReplace(p) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO player_days(account_id, day, cash, stock_value, liabilities, return_index, restructured)
                 VALUES (?,?,?,?,?,?,?)",
            )?
            .execute(params![p.account_id, p.day, p.cash, p.stock_value, p.liabilities, p.return_index, p.restructured as i64])?;
        }
        Change::IpoAppDelete { company_id, account_id } => {
            conn.prepare_cached("DELETE FROM ipo_applications WHERE company_id=? AND account_id=?")?
                .execute(params![company_id, account_id])?;
        }
        Change::IpoAppAllot { company_id, account_id, allotted } => {
            conn.prepare_cached(
                "UPDATE ipo_applications SET allotted=?, frozen=0 WHERE company_id=? AND account_id=?",
            )?
            .execute(params![allotted, company_id, account_id])?;
        }
        Change::NewsInsert(n) => {
            conn.prepare_cached("INSERT INTO news(id, ts, day, kind, company_id, title, body) VALUES (?,?,?,?,?,?,?)")?
                .execute(params![n.id, n.ts, n.day, n.kind, n.company_id, n.title, n.body])?;
        }
        Change::Margin(m) => crate::margin::apply(conn, m)?,
        Change::Corporate(m) => crate::corporate::apply(conn, m)?,
        Change::Futures(m) => crate::futures::store::apply(conn, m)?,
        Change::Funds(m) => crate::funds::store::apply(conn, m)?,
        Change::LotReplace(l) => {
            conn.prepare_cached("INSERT OR REPLACE INTO lots(id, account_id, company_id, qty, cost, acquired_on) VALUES (?,?,?,?,?,?)")?
                .execute(params![l.id, l.account_id, l.company_id, l.qty, l.cost, l.acquired_on])?;
        }
        Change::StateSet { key, value } => {
            conn.prepare_cached(
                "INSERT INTO state(key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )?
            .execute(params![key, value])?;
        }
    }
    Ok(())
}

enum Msg {
    Batch(Vec<Change>),
    Flush(Sender<Result<(), String>>),
    Stop,
}

/// Where the engine sends its changes.
pub trait ChangeSink: Send {
    fn send(&mut self, changes: Vec<Change>);
    /// Block until everything sent so far is durable (or report why it is not).
    fn flush(&mut self) -> Result<(), String>;
    /// A write that failed: the database is behind memory from here on.
    fn failure(&self) -> Option<String> {
        None
    }
}

/// Handle to the persistence thread.
pub struct Persister {
    tx: Sender<Msg>,
    handle: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<String>>>,
    path: PathBuf,
}

impl Persister {
    /// Start the writer thread on its own connection to `path`.
    pub fn start(path: &Path) -> Result<Persister, super::StoreError> {
        let conn = super::connect(path)?;
        let (tx, rx) = unbounded();
        let error = Arc::new(Mutex::new(None));
        let err2 = error.clone();
        let handle = std::thread::Builder::new()
            .name("persist".into())
            .spawn(move || writer(conn, rx, err2))
            .expect("spawn persist thread");
        Ok(Persister { tx, handle: Some(handle), error, path: path.to_path_buf() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn send(&self, changes: Vec<Change>) {
        if !changes.is_empty() {
            let _ = self.tx.send(Msg::Batch(changes));
        }
    }

    /// Block until every change sent before this call is committed.
    pub fn flush(&self) -> Result<(), String> {
        let (ack_tx, ack_rx) = bounded(1);
        self.tx.send(Msg::Flush(ack_tx)).map_err(|_| "persist thread is gone".to_string())?;
        ack_rx.recv().map_err(|_| "persist thread is gone".to_string())?
    }

    /// The first commit error, if any (the writer stops applying changes after one).
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// Flush and stop the thread.
    pub fn shutdown(mut self) -> Result<(), String> {
        let r = self.flush();
        let _ = self.tx.send(Msg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        r
    }
}

impl Drop for Persister {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = self.tx.send(Msg::Stop);
            let _ = h.join();
        }
    }
}

impl ChangeSink for Persister {
    fn send(&mut self, changes: Vec<Change>) {
        Persister::send(self, changes)
    }
    fn flush(&mut self) -> Result<(), String> {
        Persister::flush(self)
    }
    fn failure(&self) -> Option<String> {
        self.error()
    }
}

/// Collects changes in memory (no database), for tests and offline simulation.
#[derive(Default, Clone)]
pub struct MemorySink(pub Arc<Mutex<Vec<Change>>>);

impl ChangeSink for MemorySink {
    fn send(&mut self, changes: Vec<Change>) {
        self.0.lock().unwrap().extend(changes);
    }
    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

fn writer(mut conn: Connection, rx: Receiver<Msg>, error: Arc<Mutex<Option<String>>>) {
    let fail = |e: String| {
        let mut g = error.lock().unwrap();
        if g.is_none() {
            eprintln!("persist: {e}");
            *g = Some(e);
        }
    };
    let current_error = || error.lock().unwrap().clone();
    loop {
        let first = match rx.recv() {
            Ok(m) => m,
            Err(_) => return,
        };
        let mut acks: Vec<Sender<Result<(), String>>> = Vec::new();
        let mut stop = false;
        let mut batch: Vec<Change> = Vec::new();
        let handle = |m: Msg, batch: &mut Vec<Change>, acks: &mut Vec<_>, stop: &mut bool| -> bool {
            match m {
                Msg::Batch(cs) => {
                    batch.extend(cs);
                    false
                }
                Msg::Flush(a) => {
                    acks.push(a);
                    true
                }
                Msg::Stop => {
                    *stop = true;
                    true
                }
            }
        };
        let mut done = handle(first, &mut batch, &mut acks, &mut stop);
        let deadline = Instant::now() + BATCH_WINDOW;
        while !done {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(m) => done = handle(m, &mut batch, &mut acks, &mut stop),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    stop = true;
                    break;
                }
            }
        }
        if !batch.is_empty() && current_error().is_none() {
            let r = (|| -> rusqlite::Result<()> {
                let tx = conn.transaction()?;
                for c in &batch {
                    apply(&tx, c)?;
                }
                tx.commit()
            })();
            if let Err(e) = r {
                fail(format!("commit of {} changes failed: {e}", batch.len()));
            }
        }
        let res = match current_error() {
            Some(e) => Err(e),
            None => Ok(()),
        };
        for a in acks {
            let _ = a.send(res.clone());
        }
        if stop {
            return;
        }
    }
}
