//! Loading and saving the funds (V24). A fund touched by an operation is rewritten whole at its end (`Engine::commit`
//! → `FundsChange::Fund`): its row, its holders with their lots, what it owes, its open requests. Confirmations,
//! cancellations, the daily net values and the month's choices are rows of their own.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension};

use super::{Confirmed, Fund, FundsState, NavDay, ReqKind, Request};

#[derive(Debug, Clone, PartialEq)]
pub struct RequestRow {
    pub id: i64,
    pub account: i64,
    /// "subscribe" | "redeem"
    pub kind: String,
    /// cents subscribed or units (hundredths) redeemed
    pub amount: i64,
    pub day: String,
    pub young: i64,
    /// "pending" | "executing"
    pub status: String,
    pub created_at: String,
    pub taken: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FundRow {
    pub key: String,
    pub account: i64,
    pub units: i64,
    pub pending_cash: i64,
    pub nav: f64,
    pub own_costs: i64,
    pub impact_charged: i64,
    pub fees_paid: i64,
    /// (account, units, lots as JSON)
    pub holders: Vec<(i64, i64, String)>,
    pub owed: Vec<(i64, i64)>,
    pub open: Vec<RequestRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FundsChange {
    Fund(FundRow),
    RequestDone { id: i64, status: String },
    Confirmed { fund: String, c: Confirmed },
    Nav { fund: String, d: NavDay },
    Mark { key: String, value: String },
}

fn row(r: &Request, status: &str) -> RequestRow {
    let (kind, amount) = match r.kind {
        ReqKind::Subscribe(a) => ("subscribe", a),
        ReqKind::Redeem(u) => ("redeem", u),
    };
    RequestRow {
        id: r.id,
        account: r.account,
        kind: kind.into(),
        amount,
        day: r.day.clone(),
        young: r.young,
        status: status.into(),
        created_at: r.created_at.clone(),
        taken: serde_json::to_string(&r.taken).unwrap_or_default(),
    }
}

/// The whole of a fund, as `FundsChange::Fund` writes it.
pub fn snapshot(f: &Fund) -> FundRow {
    FundRow {
        key: f.key.clone(),
        account: f.account,
        units: f.units,
        pending_cash: f.pending_cash,
        nav: f.nav,
        own_costs: f.own_costs,
        impact_charged: f.impact_charged,
        fees_paid: f.fees_paid,
        holders: f
            .holders
            .iter()
            .filter(|(_, u)| **u > 0)
            .map(|(a, u)| (*a, *u, serde_json::to_string(f.lots.get(a).map_or(&vec![], |v| v)).unwrap_or_default()))
            .collect(),
        owed: f.owed.iter().map(|(a, c)| (*a, *c)).collect(),
        open: f
            .pending
            .iter()
            .map(|r| row(r, "pending"))
            .chain(f.executing.iter().map(|r| row(r, "executing")))
            .collect(),
    }
}

pub fn apply(conn: &Connection, c: &FundsChange) -> rusqlite::Result<()> {
    match c {
        FundsChange::Fund(f) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO funds(key, account_id, units, pending_cash, nav, own_costs, impact_charged, fees_paid)
                 VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![f.key, f.account, f.units, f.pending_cash, f.nav, f.own_costs, f.impact_charged, f.fees_paid])?;
            conn.prepare_cached("DELETE FROM fund_holders WHERE fund=?")?.execute([&f.key])?;
            for (a, u, lots) in &f.holders {
                conn.prepare_cached("INSERT INTO fund_holders(fund, account_id, units, lots) VALUES (?,?,?,?)")?
                    .execute(params![f.key, a, u, lots])?;
            }
            conn.prepare_cached("DELETE FROM fund_owed WHERE fund=?")?.execute([&f.key])?;
            for (a, cents) in &f.owed {
                conn.prepare_cached("INSERT INTO fund_owed(fund, account_id, cents) VALUES (?,?,?)")?
                    .execute(params![f.key, a, cents])?;
            }
            for r in &f.open {
                conn.prepare_cached(
                    "INSERT INTO fund_requests(id, fund, account_id, kind, amount, day, young, status, created_at, taken)
                     VALUES (?,?,?,?,?,?,?,?,?,?)
                     ON CONFLICT(id) DO UPDATE SET status=excluded.status, young=excluded.young",
                )?
                .execute(params![r.id, f.key, r.account, r.kind, r.amount, r.day, r.young, r.status, r.created_at, r.taken])?;
            }
        }
        FundsChange::RequestDone { id, status } => {
            conn.prepare_cached("UPDATE fund_requests SET status=? WHERE id=?")?.execute(params![status, id])?;
        }
        FundsChange::Confirmed { fund: _, c } => {
            conn.prepare_cached(
                "UPDATE fund_requests SET status='confirmed', confirmed_on=?, nav=?, units=?, cash=?, cost=? WHERE id=?",
            )?
            .execute(params![c.day, c.nav, c.units, c.cash, c.cost, c.id])?;
        }
        FundsChange::Nav { fund, d } => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO fund_nav(fund, day, nav, units, aum, cash, fee) VALUES (?,?,?,?,?,?,?)",
            )?
            .execute(params![fund, d.day, d.nav, d.units, d.aum, d.cash, d.fee])?;
        }
        FundsChange::Mark { key, value } => {
            conn.prepare_cached("INSERT OR REPLACE INTO fund_marks(key, value) VALUES (?,?)")?
                .execute(params![key, value])?;
        }
    }
    Ok(())
}

pub fn load(conn: &Connection) -> rusqlite::Result<FundsState> {
    let mut st = FundsState::default();
    let has = conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='funds'", [], |_| Ok(()))
        .optional()?
        .is_some();
    if !has {
        return Ok(st);
    }
    let mut s = conn
        .prepare("SELECT key, account_id, units, pending_cash, nav, own_costs, impact_charged, fees_paid FROM funds")?;
    let rows = s.query_map([], |r| {
        Ok(Fund {
            key: r.get(0)?,
            account: r.get(1)?,
            units: r.get(2)?,
            pending_cash: r.get(3)?,
            nav: r.get(4)?,
            own_costs: r.get(5)?,
            impact_charged: r.get(6)?,
            fees_paid: r.get(7)?,
            ..Default::default()
        })
    })?;
    for f in rows {
        let f = f?;
        st.funds.insert(f.key.clone(), f);
    }
    let mut s = conn.prepare("SELECT fund, account_id, units, lots FROM fund_holders")?;
    let rows = s.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?))
    })?;
    for x in rows {
        let (fund, a, u, lots) = x?;
        if let Some(f) = st.funds.get_mut(&fund) {
            f.holders.insert(a, u);
            f.lots.insert(a, serde_json::from_str(&lots).unwrap_or_default());
        }
    }
    let mut s = conn.prepare("SELECT fund, account_id, cents FROM fund_owed")?;
    let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?;
    for x in rows {
        let (fund, a, c) = x?;
        if let Some(f) = st.funds.get_mut(&fund) {
            f.owed.insert(a, c);
        }
    }
    let mut s = conn.prepare(
        "SELECT fund, id, account_id, kind, amount, day, young, status, created_at, taken FROM fund_requests
         WHERE status IN ('pending', 'executing') ORDER BY id",
    )?;
    let rows = s.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
        ))
    })?;
    for x in rows {
        let (fund, id, account, kind, amount, day, young, status, created_at, taken) = x?;
        let Some(f) = st.funds.get_mut(&fund) else { continue };
        let kind = if kind == "subscribe" { ReqKind::Subscribe(amount) } else { ReqKind::Redeem(amount) };
        let r = Request {
            id,
            account,
            kind,
            day,
            young,
            created_at,
            taken: serde_json::from_str(&taken).unwrap_or_default(),
        };
        if status == "executing" {
            f.executing.push(r);
        } else {
            f.pending.push(r);
        }
    }
    st.last_request_id = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM fund_requests", [], |r| r.get(0))?;
    // the last net value of each fund, from its history
    let mut s = conn.prepare("SELECT fund, day, nav, units, aum, cash, fee FROM fund_nav WHERE (fund, day) IN (SELECT fund, MAX(day) FROM fund_nav GROUP BY fund)")?;
    let rows = s.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            NavDay {
                day: r.get(1)?,
                nav: r.get(2)?,
                units: r.get(3)?,
                aum: r.get(4)?,
                cash: r.get(5)?,
                fee: r.get(6)?,
            },
        ))
    })?;
    for x in rows {
        let (fund, d) = x?;
        if let Some(f) = st.funds.get_mut(&fund) {
            f.history.push(d);
        }
    }
    let marks: BTreeMap<String, String> = conn
        .prepare("SELECT key, value FROM fund_marks")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    if let Some(v) = marks.get("floats") {
        st.floats = serde_json::from_str(v).ok();
    }
    for (k, v) in &marks {
        if let Some(key) = k.strip_prefix("top:") {
            if let Ok(x) = serde_json::from_str(v) {
                st.top.insert(key.to_string(), x);
            }
        }
    }
    st.auction_day = marks.get("auction_day").cloned().unwrap_or_default();
    session(conn, &mut st)?;
    Ok(st)
}

/// What the funds keep of the session in memory, rebuilt from the trades and the ledger (a restart mid-session):
/// the volumes of the last 5 closed sessions and of this one, the funds' fills and costs since the last close.
fn session(conn: &Connection, st: &mut FundsState) -> rusqlite::Result<()> {
    let Some(closed): Option<String> =
        conn.query_row("SELECT value FROM state WHERE key='last_closed'", [], |r| r.get(0)).optional()?
    else {
        return Ok(());
    };
    let from = chrono::NaiveDate::parse_from_str(&closed, "%Y-%m-%d")
        .map(|d| (d - chrono::Duration::days(14)).format("%Y-%m-%d").to_string())
        .unwrap_or_default();
    // (company, day) -> volume
    let mut vols: BTreeMap<(i64, String), i64> = BTreeMap::new();
    let mut s = conn.prepare("SELECT company_id, day, SUM(qty) FROM trades WHERE day >= ? AND day <= ? GROUP BY company_id, day")?;
    for x in s.query_map(params![from, closed], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))? {
        let (c, d, q) = x?;
        vols.insert((c, d), q);
    }
    let mut days: Vec<&String> = vols.keys().map(|k| &k.1).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    days = days.split_off(days.len().saturating_sub(5));
    let mut s = conn.prepare("SELECT id, listed_on FROM companies WHERE status='listed'")?;
    for x in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (c, listed_on) = x?;
        let d: std::collections::VecDeque<i64> = days
            .iter()
            .filter(|d| d.as_str() >= listed_on.as_str())
            .map(|d| vols.get(&(c, (*d).clone())).copied().unwrap_or(0))
            .collect();
        if !d.is_empty() {
            st.vol_days.insert(c, d);
        }
    }
    let accounts: std::collections::BTreeSet<i64> = st.funds.values().map(|f| f.account).collect();
    let mut s = conn.prepare("SELECT company_id, qty, price, buyer_id, seller_id FROM trades WHERE day > ? ORDER BY id")?;
    let rows = s.query_map([&closed], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
    })?;
    for x in rows {
        let (c, q, p, buyer, seller) = x?;
        *st.vol_today.entry(c).or_default() += q;
        for (a, sign) in [(buyer, 1), (seller, -1)] {
            if accounts.contains(&a) {
                *st.fund_today.entry((a, c)).or_default() += q;
                st.fills.push((a, c, sign * q, p));
            }
        }
    }
    if !accounts.is_empty() {
        let mut s = conn.prepare(
            "SELECT from_id, SUM(amount) FROM ledger WHERE day > ? AND reason IN ('commission', 'stamp_duty')
             AND from_id IN (SELECT id FROM accounts WHERE kind='fund') GROUP BY from_id",
        )?;
        for x in s.query_map([&closed], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (a, c) = x?;
            st.costs.insert(a, c);
        }
    }
    Ok(())
}
