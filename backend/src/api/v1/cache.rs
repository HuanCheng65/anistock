//! Shared answers for the endpoints every client polls (`/market/quotes`, `/market/indices`, `/leaderboard`):
//! built once and handed to every caller while they are current, instead of once per request.
//!
//! An entry is current while its fingerprint matches and it is younger than the TTL. The market fingerprint
//! ([`market_fingerprint`]) moves with every trade, ledger entry, close and listing, so quotes and indices are
//! exact up to the persister's batch; the TTL bounds what moves with the clock alone (冷静期, `is_new`).
//! The leaderboard has no fingerprint (every trade moves it): it is at most `leaderboard_secs` old.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::error::{sql, R};

pub struct Memo<K, T> {
    ttl: Duration,
    map: Mutex<HashMap<K, (Instant, String, Arc<T>)>>,
}

impl<K: Eq + Hash, T> Memo<K, T> {
    pub fn new(ttl: Duration) -> Self {
        Memo { ttl, map: Mutex::new(HashMap::new()) }
    }

    /// The entry for `key`, if it was built under `fingerprint` less than the TTL ago.
    pub fn get(&self, key: &K, fingerprint: &str) -> Option<Arc<T>> {
        let map = self.map.lock().unwrap();
        map.get(key).filter(|(at, fp, _)| fp == fingerprint && at.elapsed() < self.ttl).map(|(_, _, v)| v.clone())
    }

    pub fn put(&self, key: K, fingerprint: String, value: Arc<T>) {
        self.map.lock().unwrap().insert(key, (Instant::now(), fingerprint, value));
    }
}

/// What the market answers depend on, besides the clock: the trading day, the last close, the newest trade
/// and ledger entry, and the listed companies' shares and reference prices (listings, splits, a close).
/// All primary-key or small-table reads; run it in the same read transaction as the answer it guards.
pub fn market_fingerprint(c: &Connection, today: &str) -> R<String> {
    c.query_row(
        "SELECT (SELECT MAX(id) FROM trades), (SELECT MAX(id) FROM ledger),
           (SELECT value FROM state WHERE key='last_closed'),
           (SELECT COUNT(*) || ':' || TOTAL(shares) || ':' || TOTAL(prev_close) || ':' || TOTAL(last_price)
              FROM companies WHERE status='listed')",
        [],
        |r| {
            let (t, l, d, co): (Option<i64>, Option<i64>, Option<String>, Option<String>) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            Ok(format!(
                "{today}|{}|{}|{}|{}",
                t.unwrap_or(0),
                l.unwrap_or(0),
                d.unwrap_or_default(),
                co.unwrap_or_default()
            ))
        },
    )
    .map_err(sql)
}

pub struct Caches {
    pub quotes: Memo<(), super::types::QuotesResponse>,
    /// (with_series, total_return)
    pub indices: Memo<(bool, bool), super::types::IndicesResponse>,
    /// period
    pub leaderboard: Memo<String, super::types::Leaderboard>,
}

impl Caches {
    pub fn from_config(cfg: &crate::config::Config) -> Caches {
        let secs = |k: &str, d: f64| Duration::from_secs_f64(cfg.api_f64(&format!("cache.{k}"), d).max(0.0));
        Caches {
            quotes: Memo::new(secs("market_secs", 3.0)),
            indices: Memo::new(secs("market_secs", 3.0)),
            leaderboard: Memo::new(secs("leaderboard_secs", 30.0)),
        }
    }
}
