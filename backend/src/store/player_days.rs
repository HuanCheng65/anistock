//! The one-time backfill of `player_days` (migration V6, docs/api-v1.md §4.5.10).
//!
//! Each player's history is replayed day by day from their registration to the last closed day: cash from
//! the ledger (every cash movement is a ledger row with its trading day), holdings from trades, IPO
//! allotments (the day of the `ipo` payment), restructurings and delistings (the day of the
//! `restructure` / `delisting` payment for that stock: the holding is gone after it), valued at each
//! day's close (`candles`, else the last close before, else the IPO price). The return index is chain-
//! linked at every restructuring (`restructures`).
//!
//! The replay must reproduce every player's current cash and holdings (once the movements after the last
//! close are taken back out). If any player does not match, nothing is written and the snapshots start at
//! the next close; the log says which account and why.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension};

/// What the backfill did, for the startup log.
#[derive(Debug)]
pub enum Backfill {
    Written { days: usize, players: usize, rows: usize },
    Skipped(String),
}

fn trading_day_of(ts: &str, rollover_min: i64) -> Option<String> {
    let dt = chrono::DateTime::parse_from_rfc3339(ts).ok()?;
    Some((dt.naive_local() - chrono::Duration::minutes(rollover_min)).date().format("%Y-%m-%d").to_string())
}

fn next_day(d: &str) -> String {
    let x = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap() + chrono::Duration::days(1);
    x.format("%Y-%m-%d").to_string()
}

/// Run the backfill on `conn` (one transaction). `initial` = starting cash in cents, `rollover` = `HH:MM`.
pub fn backfill(conn: &mut Connection, initial: i64, rollover: &str) -> rusqlite::Result<Backfill> {
    let rollover_min = {
        let (h, m) = rollover.split_once(':').unwrap_or(("4", "0"));
        h.trim().parse::<i64>().unwrap_or(4) * 60 + m.trim().parse::<i64>().unwrap_or(0)
    };
    let last: Option<String> =
        conn.query_row("SELECT value FROM state WHERE key='last_closed'", [], |r| r.get(0)).optional()?;
    let Some(last) = last.filter(|s| !s.is_empty()) else { return Ok(Backfill::Skipped("no closed day yet".into())) };
    let already: i64 = conn.query_row("SELECT COUNT(*) FROM player_days", [], |r| r.get(0))?;
    if already > 0 {
        return Ok(Backfill::Skipped("player_days is not empty".into()));
    }
    let players: Vec<(i64, i64, String)> = {
        let mut s = conn.prepare("SELECT id, cash, created_at FROM accounts WHERE kind='player' ORDER BY id")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    if players.is_empty() {
        return Ok(Backfill::Written { days: 0, players: 0, rows: 0 });
    }
    // closes per company and day
    let mut closes: HashMap<i64, BTreeMap<String, i64>> = HashMap::new();
    {
        let mut s = conn.prepare("SELECT company_id, day, close FROM candles")?;
        let mut rows = s.query([])?;
        while let Some(r) = rows.next()? {
            closes.entry(r.get(0)?).or_default().insert(r.get(1)?, r.get(2)?);
        }
    }
    let ipo_price: HashMap<i64, i64> = {
        let mut s = conn.prepare("SELECT id, ipo_price FROM companies")?;
        let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let price_on = |cid: i64, day: &str| -> i64 {
        closes
            .get(&cid)
            .and_then(|m| m.range(..=day.to_string()).next_back().map(|(_, p)| *p))
            .unwrap_or_else(|| ipo_price.get(&cid).copied().unwrap_or(0))
    };
    let restructures: HashMap<i64, Vec<(String, i64)>> = {
        let mut s = conn.prepare("SELECT account_id, day, assets_before FROM restructures ORDER BY id")?;
        let mut m: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
        let mut rows = s.query([])?;
        while let Some(r) = rows.next()? {
            m.entry(r.get(0)?).or_default().push((r.get(1)?, r.get(2)?));
        }
        m
    };

    let mut out: Vec<(i64, String, i64, i64, f64, bool)> = Vec::new();
    let mut all_days = std::collections::BTreeSet::new();
    for (pid, cash_now, created) in &players {
        let first = trading_day_of(created, rollover_min).unwrap_or_else(|| last.clone());
        // cash deltas per day
        let mut cash_d: BTreeMap<String, i64> = BTreeMap::new();
        {
            let mut s = conn.prepare(
                "SELECT day, SUM(CASE WHEN to_id=?1 THEN amount ELSE 0 END) - SUM(CASE WHEN from_id=?1 THEN amount ELSE 0 END)
                 FROM ledger WHERE to_id=?1 OR from_id=?1 GROUP BY day",
            )?;
            let mut rows = s.query([pid])?;
            while let Some(r) = rows.next()? {
                cash_d.insert(r.get(0)?, r.get(1)?);
            }
        }
        // share movements per day: (company, delta) or a "gone" marker (restructure / delisting)
        let mut moves: BTreeMap<String, Vec<(i64, Option<i64>)>> = BTreeMap::new();
        {
            let mut s = conn.prepare(
                "SELECT day, company_id, SUM(CASE WHEN buyer_id=?1 THEN qty ELSE 0 END) - SUM(CASE WHEN seller_id=?1 THEN qty ELSE 0 END)
                 FROM trades WHERE buyer_id=?1 OR seller_id=?1 GROUP BY day, company_id",
            )?;
            let mut rows = s.query([pid])?;
            while let Some(r) = rows.next()? {
                moves.entry(r.get(0)?).or_default().push((r.get(1)?, Some(r.get(2)?)));
            }
            let mut s = conn.prepare(
                "SELECT l.day, a.company_id, a.allotted FROM ipo_applications a
                 JOIN ledger l ON l.from_id=a.account_id AND l.company_id=a.company_id AND l.reason='ipo'
                 WHERE a.account_id=? AND a.allotted > 0",
            )?;
            let mut rows = s.query([pid])?;
            while let Some(r) = rows.next()? {
                moves.entry(r.get(0)?).or_default().push((r.get(1)?, Some(r.get(2)?)));
            }
            let mut s = conn.prepare(
                "SELECT DISTINCT day, company_id FROM ledger WHERE to_id=? AND reason IN ('restructure','delisting') AND company_id IS NOT NULL",
            )?;
            let mut rows = s.query([pid])?;
            while let Some(r) = rows.next()? {
                moves.entry(r.get(0)?).or_default().push((r.get(1)?, None));
            }
        }
        // replay
        let mut cash = 0i64;
        let mut hold: BTreeMap<i64, i64> = BTreeMap::new();
        let apply = |day: &str, cash: &mut i64, hold: &mut BTreeMap<i64, i64>| {
            *cash += cash_d.get(day).copied().unwrap_or(0);
            if let Some(ms) = moves.get(day) {
                // trades and allotments first, then whatever was sold off that day
                for (cid, d) in ms {
                    if let Some(d) = d {
                        *hold.entry(*cid).or_default() += d;
                    }
                }
                for (cid, d) in ms {
                    if d.is_none() {
                        hold.remove(cid);
                    }
                }
            }
            hold.retain(|_, q| *q != 0);
        };
        // everything before the first day counts on the first day
        let earliest = cash_d.keys().next().cloned().unwrap_or_else(|| first.clone()).min(first.clone());
        let mut d = earliest.clone();
        let (mut factor, mut base) = (1.0f64, initial);
        let segs = restructures.get(pid).cloned().unwrap_or_default();
        while d <= last {
            apply(&d, &mut cash, &mut hold);
            let stock: i64 = hold.iter().map(|(cid, q)| q * price_on(*cid, &d)).sum();
            let mut restructured = false;
            for (rd, before) in &segs {
                if *rd == d {
                    factor *= *before as f64 / base as f64;
                    base = initial;
                    restructured = true;
                }
            }
            if d >= first {
                let idx = if base > 0 { factor * (cash + stock) as f64 / base as f64 } else { factor };
                out.push((*pid, d.clone(), cash, stock, idx, restructured));
                all_days.insert(d.clone());
            }
            d = next_day(&d);
        }
        // the check: the replay + what happened after the last close = the account now
        let after_cash: i64 = cash_d.range(next_day(&last)..).map(|(_, v)| v).sum();
        if cash + after_cash != *cash_now {
            return Ok(Backfill::Skipped(format!(
                "account {pid}: replayed cash {} + later {after_cash} != {cash_now}",
                cash
            )));
        }
        let mut hold_now = hold.clone();
        for (_, ms) in moves.range(next_day(&last)..) {
            for (cid, dlt) in ms {
                match dlt {
                    Some(q) => *hold_now.entry(*cid).or_default() += q,
                    None => {
                        hold_now.remove(cid);
                    }
                }
            }
        }
        hold_now.retain(|_, q| *q != 0);
        let actual: BTreeMap<i64, i64> = {
            let mut s = conn.prepare("SELECT company_id, qty FROM holdings WHERE account_id=? AND qty != 0")?;
            let rows = s.query_map([pid], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        if hold_now != actual {
            return Ok(Backfill::Skipped(format!("account {pid}: replayed holdings {hold_now:?} != {actual:?}")));
        }
    }
    let tx = conn.transaction()?;
    {
        let mut s = tx.prepare(
            "INSERT OR REPLACE INTO player_days(account_id, day, cash, stock_value, return_index, restructured) VALUES (?,?,?,?,?,?)",
        )?;
        for (a, d, c, sv, idx, r) in &out {
            s.execute(params![a, d, c, sv, idx, *r as i64])?;
        }
    }
    tx.commit()?;
    Ok(Backfill::Written { days: all_days.len(), players: players.len(), rows: out.len() })
}
