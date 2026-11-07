//! Loading and saving the futures tables (V17). Every change is one row, written by the persister in the
//! same transaction as the rest of the operation (`Change::Futures`).

use rusqlite::{params, Connection, OptionalExtension, Row};

use super::model::*;
use super::FuturesState;
use crate::model::{Kind, OrderStatus, Side};

#[derive(Debug, Clone, PartialEq)]
pub enum FuturesChange {
    Contract(Contract),
    Order(FutOrder),
    Trade(FutTrade),
    Position(Position),
    PositionDelete(i64, i64),
    Day(FutDay),
    Heat(HeatRow),
}

pub fn apply(conn: &Connection, c: &FuturesChange) -> rusqlite::Result<()> {
    match c {
        FuturesChange::Contract(k) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO futures_contracts(id, code, company_id, work_id, period, first_day, last_day, listed_on,
                     status, tick_value, forecast, prev_settle, open_ref, vcm_until, vcm_low, vcm_high, vcm_count, last_price,
                     final_price, settled_on)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                k.id,
                k.code,
                k.company_id,
                k.work_id,
                k.period,
                k.first_day,
                k.last_day,
                k.listed_on,
                k.status.as_str(),
                k.tick_value,
                k.forecast,
                k.prev_settle,
                k.open_ref,
                k.vcm_until,
                k.vcm_low,
                k.vcm_high,
                k.vcm_count,
                k.last_price,
                k.final_price,
                k.settled_on
            ])?;
        }
        FuturesChange::Order(o) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO futures_orders(id, account_id, contract_id, side, kind, price, qty, filled, frozen, status,
                     created_at, updated_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![
                o.id,
                o.account_id,
                o.contract_id,
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
        FuturesChange::Trade(t) => {
            conn.prepare_cached(
                "INSERT INTO futures_trades(id, contract_id, price, qty, buyer_id, seller_id, buy_order, sell_order, ts, day)
                 VALUES (?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![t.id, t.contract_id, t.price, t.qty, t.buyer_id, t.seller_id, t.buy_order, t.sell_order, t.ts, t.day])?;
        }
        FuturesChange::Position(p) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO futures_positions(account_id, contract_id, qty, basis, margin) VALUES (?,?,?,?,?)",
            )?
            .execute(params![p.account_id, p.contract_id, p.qty, p.basis, p.margin])?;
        }
        FuturesChange::PositionDelete(a, c) => {
            conn.prepare_cached("DELETE FROM futures_positions WHERE account_id=? AND contract_id=?")?
                .execute([a, c])?;
        }
        FuturesChange::Day(d) => {
            let (o, h, l, c) = match d.ohlc {
                Some((o, h, l, c)) => (Some(o), Some(h), Some(l), Some(c)),
                None => (None, None, None, None),
            };
            conn.prepare_cached(
                "INSERT OR REPLACE INTO futures_days(contract_id, day, open, high, low, close, volume, settle, settle_source,
                     open_interest)
                 VALUES (?,?,?,?,?,?,?,?,?,?)",
            )?
            .execute(params![d.contract_id, d.day, o, h, l, c, d.volume, d.settle, d.source.as_str(), d.open_interest])?;
        }
        FuturesChange::Heat(h) => {
            // published once, never revised
            conn.prepare_cached(
                "INSERT OR IGNORE INTO heat_index(work_id, day, idx, us, cn, jp, flags, version) VALUES (?,?,?,?,?,?,?,?)",
            )?
            .execute(params![h.work_id, h.day, h.idx, h.us, h.cn, h.jp, h.flags, h.version])?;
        }
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

fn bad(col: usize, what: &str, v: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, format!("bad {what}: {v}").into())
}

fn contract(r: &Row) -> rusqlite::Result<Contract> {
    let status: String = r.get(8)?;
    Ok(Contract {
        id: r.get(0)?,
        code: r.get(1)?,
        company_id: r.get(2)?,
        work_id: r.get(3)?,
        period: r.get(4)?,
        first_day: r.get(5)?,
        last_day: r.get(6)?,
        listed_on: r.get(7)?,
        status: ContractStatus::parse(&status).ok_or_else(|| bad(8, "contract status", &status))?,
        tick_value: r.get(9)?,
        forecast: r.get(10)?,
        prev_settle: r.get(11)?,
        open_ref: r.get(12)?,
        vcm_until: r.get(13)?,
        vcm_low: r.get(14)?,
        vcm_high: r.get(15)?,
        vcm_count: r.get(16)?,
        last_price: r.get(17)?,
        final_price: r.get(18)?,
        settled_on: r.get(19)?,
    })
}

fn order(r: &Row) -> rusqlite::Result<FutOrder> {
    let (side, kind, status): (String, String, String) = (r.get(3)?, r.get(4)?, r.get(9)?);
    Ok(FutOrder {
        id: r.get(0)?,
        account_id: r.get(1)?,
        contract_id: r.get(2)?,
        side: Side::parse(&side).ok_or_else(|| bad(3, "side", &side))?,
        kind: Kind::parse(&kind).ok_or_else(|| bad(4, "kind", &kind))?,
        price: r.get(5)?,
        qty: r.get(6)?,
        filled: r.get(7)?,
        frozen: r.get(8)?,
        status: OrderStatus::parse(&status).ok_or_else(|| bad(9, "order status", &status))?,
        created_at: r.get(10)?,
        updated_at: r.get(11)?,
    })
}

/// Load the futures state: contracts not settled, open orders, positions, and the id counters (empty if the
/// database predates V17, e.g. an old simulation world).
pub fn load(conn: &Connection) -> rusqlite::Result<FuturesState> {
    let mut s = FuturesState::default();
    if !table_exists(conn, "futures_contracts")? {
        return Ok(s);
    }
    let cols = "id, code, company_id, work_id, period, first_day, last_day, listed_on, status, tick_value, forecast, prev_settle,
                open_ref, vcm_until, vcm_low, vcm_high, vcm_count, last_price, final_price, settled_on";
    for k in conn
        .prepare(&format!("SELECT {cols} FROM futures_contracts WHERE status != 'settled'"))?
        .query_map([], contract)?
    {
        let k = k?;
        s.contracts.insert(k.id, k);
    }
    let cols = "id, account_id, contract_id, side, kind, price, qty, filled, frozen, status, created_at, updated_at";
    for o in conn
        .prepare(&format!("SELECT {cols} FROM futures_orders WHERE status='open' ORDER BY id"))?
        .query_map([], order)?
    {
        let o = o?;
        let id = o.id;
        s.insert_order(o);
        s.rest(id);
    }
    for p in conn.prepare("SELECT account_id, contract_id, qty, basis, margin FROM futures_positions")?.query_map(
        [],
        |r| {
            Ok(Position {
                account_id: r.get(0)?,
                contract_id: r.get(1)?,
                qty: r.get(2)?,
                basis: r.get(3)?,
                margin: r.get(4)?,
            })
        },
    )? {
        let p = p?;
        s.positions.insert((p.account_id, p.contract_id), p);
    }
    // the published index of the works still settling, from the first day of their earliest period
    if let Some(from) = s.contracts.values().map(|c| c.first_day.clone()).min() {
        for r in conn
            .prepare("SELECT work_id, day, idx FROM heat_index WHERE day >= ?")?
            .query_map([from], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?)))?
        {
            let (w, d, idx) = r?;
            if s.contracts.values().any(|c| c.work_id == w) {
                s.heat.insert((w, d), idx);
            }
        }
    }
    let max = |t: &str| -> rusqlite::Result<i64> {
        conn.query_row(&format!("SELECT COALESCE(MAX(id), 0) FROM {t}"), [], |r| r.get(0))
    };
    s.last_contract_id = max("futures_contracts")?;
    s.last_order_id = max("futures_orders")?;
    s.last_trade_id = max("futures_trades")?;
    Ok(s)
}
