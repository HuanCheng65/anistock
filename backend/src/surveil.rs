//! 对倒监控 (docs/v2-account.md §4.1): trades between related accounts that look like moving assets
//! from one account to another, which the user agreement forbids (风险揭示书 §6.3). A report for a
//! human to read (`anistock surveillance [days]`); nothing is punished automatically.
//!
//! Related: two players who signed in from the same address (IPv4, or IPv6 /64) within the window.
//! Suspicious: a trade between them at a price far from that stock's volume-weighted price of the day,
//! or one that is a large share of the stock's volume that day.

use rusqlite::{params, Connection};

pub struct Flag {
    pub day: String,
    pub trade_id: i64,
    pub company_id: i64,
    pub buyer: String,
    pub seller: String,
    pub price: i64,
    pub qty: i64,
    /// the stock's VWAP that day (cents)
    pub vwap: f64,
    /// this trade's share of the stock's volume that day
    pub volume_share: f64,
    /// the address the two shared
    pub ip: String,
}

/// Flags for the trading days `from..=to` (`YYYY-MM-DD`), relating accounts through sign-ins since
/// `logins_from`.
pub fn report(
    c: &Connection,
    from: &str,
    to: &str,
    logins_from: &str,
    price_dev: f64,
    volume_share: f64,
) -> rusqlite::Result<Vec<Flag>> {
    let mut q = c.prepare(
        "WITH pairs AS (
             SELECT DISTINCT a.account_id AS x, b.account_id AS y, a.ip AS ip
             FROM account_logins a JOIN account_logins b ON a.ip = b.ip AND a.account_id < b.account_id
             WHERE a.at >= ?3 AND b.at >= ?3
         ),
         day_stats AS (
             SELECT company_id, day, SUM(price * qty) * 1.0 / SUM(qty) AS vwap, SUM(qty) AS vol
             FROM trades WHERE day BETWEEN ?1 AND ?2 GROUP BY company_id, day
         )
         SELECT t.day, t.id, t.company_id, ub.username, us.username, t.price, t.qty, s.vwap, t.qty * 1.0 / s.vol, p.ip
         FROM trades t
         JOIN pairs p ON (p.x = t.buyer_id AND p.y = t.seller_id) OR (p.y = t.buyer_id AND p.x = t.seller_id)
         JOIN day_stats s ON s.company_id = t.company_id AND s.day = t.day
         JOIN accounts ub ON ub.id = t.buyer_id
         JOIN accounts us ON us.id = t.seller_id
         WHERE t.day BETWEEN ?1 AND ?2
           AND (ABS(t.price - s.vwap) > ?4 * s.vwap OR t.qty * 1.0 / s.vol > ?5)
         GROUP BY t.id
         ORDER BY t.day, t.id",
    )?;
    let rows = q.query_map(params![from, to, logins_from, price_dev, volume_share], |r| {
        Ok(Flag {
            day: r.get(0)?,
            trade_id: r.get(1)?,
            company_id: r.get(2)?,
            buyer: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            seller: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            price: r.get(5)?,
            qty: r.get(6)?,
            vwap: r.get(7)?,
            volume_share: r.get(8)?,
            ip: r.get(9)?,
        })
    })?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_off_market_trades_between_shared_addresses() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE accounts (id INTEGER PRIMARY KEY, username TEXT);
             CREATE TABLE account_logins (account_id INTEGER, ip TEXT, at TEXT);
             CREATE TABLE trades (id INTEGER PRIMARY KEY, company_id INTEGER, price INTEGER, qty INTEGER,
                                  buyer_id INTEGER, seller_id INTEGER, day TEXT);
             INSERT INTO accounts VALUES (1, 'main'), (2, 'alt'), (3, 'stranger'), (9, NULL);
             INSERT INTO account_logins VALUES (1, '10.0.0.1', '2026-11-01'), (2, '10.0.0.1', '2026-11-02'),
                                               (3, '10.0.0.9', '2026-11-02');
             -- the market: 1,000 shares around 5.00
             INSERT INTO trades VALUES (1, 7, 500, 900, 9, 3, '2026-11-05');
             -- alt sells to main at 9.00: off market
             INSERT INTO trades VALUES (2, 7, 900, 10, 1, 2, '2026-11-05');
             -- alt sells to main at 5.00, small: fine
             INSERT INTO trades VALUES (3, 7, 500, 10, 1, 2, '2026-11-05');
             -- a stranger sells to main at 9.00: not related
             INSERT INTO trades VALUES (4, 7, 900, 10, 1, 3, '2026-11-05');",
        )
        .unwrap();
        let f = report(&c, "2026-11-01", "2026-11-06", "2026-10-01", 0.05, 0.2).unwrap();
        assert_eq!(f.iter().map(|x| x.trade_id).collect::<Vec<_>>(), vec![2]);
        assert_eq!((f[0].buyer.as_str(), f[0].seller.as_str(), f[0].ip.as_str()), ("main", "alt", "10.0.0.1"));
    }
}
