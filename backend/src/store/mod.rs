//! SQLite storage: connection setup, schema migrations (refinery), backups, the invariant check, and
//! loading the full game state at startup. Same file and schema as v1 (`server/db.py`).

pub mod persist;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};

use crate::model::*;

pub use persist::{Change, Persister};

mod embedded {
    refinery::embed_migrations!("migrations");
}

pub type DbResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Migration(String),
    Invariant(String),
    Io(std::io::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sqlite(e) => write!(f, "sqlite: {e}"),
            StoreError::Migration(e) => write!(f, "migration: {e}"),
            StoreError::Invariant(e) => write!(f, "invariant violated: {e}"),
            StoreError::Io(e) => write!(f, "io: {e}"),
        }
    }
}
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}
impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// Open a connection like v1 `db.connect`: WAL, no foreign keys, 5 s busy timeout, synchronous=NORMAL.
pub fn connect(path: &Path) -> DbResult<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "OFF")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    Ok(conn)
}

fn table_exists(conn: &Connection, name: &str) -> DbResult<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Apply pending migrations (`backend/migrations`, `V1__initial.sql` is v1's `001_initial.sql`).
///
/// A database created by v1 has yoyo's `_yoyo_migration` table: if it records `001_initial` and refinery
/// has no history yet, V1 is marked applied without running it. If anything is still pending and the
/// database already has an `accounts` table, it is backed up first to
/// `<dir>/backups/<stem>-before-migration-<YYYYmmdd-HHMMSS>.db`. Returns the names of the migrations
/// applied (e.g. `["V1__initial"]`; the baseline does not count).
pub fn migrate(db_path: &Path) -> DbResult<Vec<String>> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let existed = db_path.exists();
    let mut conn = connect(db_path)?;
    let runner = embedded::migrations::runner();
    let mig = |e: refinery::Error| StoreError::Migration(e.to_string());

    let history = |conn: &mut Connection| -> DbResult<Vec<i64>> {
        if !table_exists(conn, "refinery_schema_history")? {
            return Ok(vec![]);
        }
        Ok(runner.get_applied_migrations(conn).map_err(mig)?.iter().map(|m| m.version() as i64).collect())
    };

    if table_exists(&conn, "_yoyo_migration")? {
        if history(&mut conn)?.is_empty() {
            let has_001 = conn
                .query_row("SELECT 1 FROM _yoyo_migration WHERE migration_id='001_initial'", [], |_| Ok(()))
                .optional()?
                .is_some();
            if !has_001 {
                return Err(StoreError::Migration(
                    "_yoyo_migration exists but does not record 001_initial; refusing to guess".into(),
                ));
            }
            embedded::migrations::runner().set_target(refinery::Target::FakeVersion(1)).run(&mut conn).map_err(mig)?;
        }
    }

    let applied = history(&mut conn)?;
    let pending: Vec<String> = runner
        .get_migrations()
        .iter()
        .filter(|m| !applied.contains(&(m.version() as i64)))
        .map(|m| m.to_string())
        .collect();
    if pending.is_empty() {
        return Ok(vec![]);
    }
    if existed && table_exists(&conn, "accounts")? {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let stem = db_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dir = db_path.parent().map(Path::to_path_buf).unwrap_or_default();
        backup(&conn, &dir.join("backups").join(format!("{stem}-before-migration-{stamp}.db")))?;
    }
    let report = runner.run(&mut conn).map_err(mig)?;
    Ok(report.applied_migrations().iter().map(|m| m.to_string()).collect())
}

/// Consistent online copy (SQLite backup API); safe while the game is running.
pub fn backup(conn: &Connection, dest: &Path) -> DbResult<PathBuf> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut out = Connection::open(dest)?;
    {
        let b = rusqlite::backup::Backup::new(conn, &mut out)?;
        b.run_to_completion(256, std::time::Duration::from_millis(0), None)?;
    }
    Ok(dest.to_path_buf())
}

/// v1 `check_invariants`, as SQL on the database: cash sums to 0, each listed company's shares are all
/// held, lots match holdings, and no ordinary account is negative or over-frozen.
pub fn check_invariants(conn: &Connection) -> DbResult<()> {
    let total: i64 = conn.query_row("SELECT COALESCE(SUM(cash), 0) FROM accounts", [], |r| r.get(0))?;
    if total != 0 {
        return Err(StoreError::Invariant(format!("cash does not sum to zero: {total}")));
    }
    let bad = rows3(
        conn,
        "SELECT c.id, c.shares, COALESCE(SUM(h.qty), 0) AS held FROM companies c
         LEFT JOIN holdings h ON h.company_id = c.id
         WHERE c.status = 'listed' GROUP BY c.id HAVING held != c.shares",
    )?;
    if !bad.is_empty() {
        return Err(StoreError::Invariant(format!("share count mismatch: {:?}", &bad[..bad.len().min(5)])));
    }
    let lot_bad: Vec<(i64, i64, i64, i64)> = conn
        .prepare(
            "SELECT h.account_id, h.company_id, h.qty, COALESCE(SUM(l.qty), 0) AS lq FROM holdings h
             LEFT JOIN lots l ON l.account_id = h.account_id AND l.company_id = h.company_id
             GROUP BY h.account_id, h.company_id HAVING lq != h.qty",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    if !lot_bad.is_empty() {
        return Err(StoreError::Invariant(format!(
            "lots do not match holdings: {:?}",
            &lot_bad[..lot_bad.len().min(5)]
        )));
    }
    let neg: Vec<(i64, String, i64, i64)> = conn
        .prepare(
            "SELECT id, kind, cash, cash_frozen FROM accounts
             WHERE kind NOT IN ('world', 'cb') AND (cash < 0 OR cash_frozen < 0 OR cash_frozen > cash)",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<Result<_, _>>()?;
    if !neg.is_empty() {
        return Err(StoreError::Invariant(format!("negative or over-frozen balances: {:?}", &neg[..neg.len().min(5)])));
    }
    Ok(())
}

fn rows3(conn: &Connection, sql: &str) -> DbResult<Vec<(i64, i64, i64)>> {
    Ok(conn.prepare(sql)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?)
}

/// Everything the engine keeps in memory, as loaded from the database.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub accounts: BTreeMap<i64, Account>,
    pub companies: BTreeMap<i64, Company>,
    pub holdings: BTreeMap<(i64, i64), Holding>,
    /// all lots, by id (FIFO order within an owner is id order)
    pub lots: BTreeMap<i64, Lot>,
    /// open orders only, by id
    pub open_orders: BTreeMap<i64, Order>,
    /// per company, the trades of that company's latest trading day (for the VCM look-back), id order
    pub recent_trades: BTreeMap<i64, Vec<Trade>>,
    pub state: BTreeMap<String, String>,
    pub max_order_id: i64,
    pub max_trade_id: i64,
    pub max_ledger_id: i64,
    pub max_news_id: i64,
    pub max_account_id: i64,
}

fn max_id(conn: &Connection, table: &str) -> DbResult<i64> {
    Ok(conn.query_row(&format!("SELECT COALESCE(MAX(id), 0) FROM {table}"), [], |r| r.get(0))?)
}

pub fn order_from_row(r: &rusqlite::Row) -> rusqlite::Result<Order> {
    let side: String = r.get("side")?;
    let kind: String = r.get("kind")?;
    let status: String = r.get("status")?;
    let bad = |what: &str, v: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("bad order {what} {v:?}").into(),
        )
    };
    Ok(Order {
        id: r.get("id")?,
        account_id: r.get("account_id")?,
        company_id: r.get("company_id")?,
        side: Side::parse(&side).ok_or_else(|| bad("side", &side))?,
        kind: Kind::parse(&kind).ok_or_else(|| bad("kind", &kind))?,
        price: r.get("price")?,
        qty: r.get("qty")?,
        filled: r.get("filled")?,
        frozen: r.get("frozen")?,
        status: OrderStatus::parse(&status).ok_or_else(|| bad("status", &status))?,
        created_at: r.get("created_at")?,
        updated_at: r.get("updated_at")?,
    })
}

fn trade_from_row(r: &rusqlite::Row) -> rusqlite::Result<Trade> {
    Ok(Trade {
        id: r.get("id")?,
        company_id: r.get("company_id")?,
        price: r.get("price")?,
        qty: r.get("qty")?,
        buyer_id: r.get("buyer_id")?,
        seller_id: r.get("seller_id")?,
        buy_order: r.get("buy_order")?,
        sell_order: r.get("sell_order")?,
        ts: r.get("ts")?,
        day: r.get("day")?,
    })
}

pub fn company_from_row(r: &rusqlite::Row) -> rusqlite::Result<Company> {
    Ok(Company {
        id: r.get("id")?,
        code: r.get("code")?,
        title: r.get("title")?,
        title_ja: r.get("title_ja")?,
        title_en: r.get("title_en")?,
        studio: r.get("studio")?,
        episodes: r.get("episodes")?,
        long_runner: r.get("long_runner")?,
        first_air: r.get("first_air")?,
        listed_on: r.get("listed_on")?,
        status: r.get("status")?,
        sub_start: r.get("sub_start")?,
        sub_end: r.get("sub_end")?,
        shares: r.get("shares")?,
        ipo_price: r.get("ipo_price")?,
        prev_close: r.get("prev_close")?,
        open_ref: r.get("open_ref")?,
        vcm_until: r.get("vcm_until")?,
        vcm_low: r.get("vcm_low")?,
        vcm_high: r.get("vcm_high")?,
        vcm_count: r.get("vcm_count")?,
        last_price: r.get("last_price")?,
        fair_value: r.get("fair_value")?,
        profit_week: r.get("profit_week")?,
        low_weeks: r.get("low_weeks")?,
        delisted_on: r.get("delisted_on")?,
    })
}

/// Read the whole game state. Runs `check_invariants` first and fails if it does not hold.
pub fn load(conn: &Connection) -> DbResult<Snapshot> {
    check_invariants(conn)?;
    let mut s = Snapshot::default();
    for a in conn
        .prepare("SELECT id, kind, name, username, pw_hash, company_id, cash, cash_frozen, created_at FROM accounts")?
        .query_map([], |r| {
            Ok(Account {
                id: r.get(0)?,
                kind: r.get(1)?,
                name: r.get(2)?,
                username: r.get(3)?,
                pw_hash: r.get(4)?,
                company_id: r.get(5)?,
                cash: r.get(6)?,
                cash_frozen: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?
    {
        let a = a?;
        s.accounts.insert(a.id, a);
    }
    for c in conn.prepare("SELECT * FROM companies")?.query_map([], company_from_row)? {
        let c = c?;
        s.companies.insert(c.id, c);
    }
    for h in conn.prepare("SELECT account_id, company_id, qty, qty_frozen FROM holdings")?.query_map([], |r| {
        Ok(Holding { account_id: r.get(0)?, company_id: r.get(1)?, qty: r.get(2)?, qty_frozen: r.get(3)? })
    })? {
        let h = h?;
        s.holdings.insert((h.account_id, h.company_id), h);
    }
    for l in
        conn.prepare("SELECT id, account_id, company_id, qty, cost, acquired_on FROM lots")?.query_map([], |r| {
            Ok(Lot {
                id: r.get(0)?,
                account_id: r.get(1)?,
                company_id: r.get(2)?,
                qty: r.get(3)?,
                cost: r.get(4)?,
                acquired_on: r.get(5)?,
            })
        })?
    {
        let l = l?;
        s.lots.insert(l.id, l);
    }
    for o in conn.prepare("SELECT * FROM orders WHERE status='open' ORDER BY id")?.query_map([], order_from_row)? {
        let o = o?;
        s.open_orders.insert(o.id, o);
    }
    for t in conn
        .prepare(
            "SELECT t.* FROM trades t JOIN (SELECT company_id, MAX(day) AS d FROM trades GROUP BY company_id) m
             ON m.company_id = t.company_id AND m.d = t.day ORDER BY t.id",
        )?
        .query_map([], trade_from_row)?
    {
        let t = t?;
        s.recent_trades.entry(t.company_id).or_default().push(t);
    }
    for kv in conn
        .prepare("SELECT key, value FROM state")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
    {
        let (k, v) = kv?;
        s.state.insert(k, v);
    }
    s.max_order_id = max_id(conn, "orders")?;
    s.max_trade_id = max_id(conn, "trades")?;
    s.max_ledger_id = max_id(conn, "ledger")?;
    s.max_news_id = max_id(conn, "news")?;
    s.max_account_id = max_id(conn, "accounts")?;
    Ok(s)
}

/// Owner and status of one order, read from the database (closed orders are not kept in memory).
pub fn order_owner_status(conn: &Connection, order_id: i64) -> DbResult<Option<(i64, String)>> {
    Ok(conn
        .query_row("SELECT account_id, status FROM orders WHERE id=?", params![order_id], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?)
}
