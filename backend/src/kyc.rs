//! 身份验证 (KYC) and what it unlocks (docs/v2-account.md §2, §4).
//!
//! The eligibility gates (财政补贴, 新股认购, 排行榜) ask one question: is this account verified?
//! How an account gets verified is decided here and nowhere else. Today the one method is a QQ binding
//! (the official bot's openid; one openid per account, so one verified account per person). Another
//! method later is a change to `verified_ids` / `method`, not to the gates.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

/// `[account.eligibility]` of game.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Rules {
    /// the gates below need a verified account (off only for tests and simulations)
    pub require_kyc: bool,
    /// 财政补贴: days since registration
    pub transfer_min_days: i64,
    /// 财政补贴: at least one trade in this many days
    pub transfer_active_days: i64,
    /// 新股认购: the quota is the mean holdings value over this many closed trading days
    pub ipo_mv_days: i64,
    /// 新股认购: quota = mean holdings value × this
    pub ipo_quota_ratio: f64,
    /// 排行榜: days since registration
    pub rank_min_days: i64,
    /// 排行榜: trades so far
    pub rank_min_trades: i64,
    /// 融资融券: the lowest 风险测评 level (4 = C4 积极型)
    pub credit_min_level: i64,
    /// a 风险测评 counts this many days
    pub assessment_valid_days: i64,
    /// 风险测评 and knowledge tests a day (each)
    pub assessment_per_day: i64,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            require_kyc: true,
            transfer_min_days: 7,
            transfer_active_days: 28,
            ipo_mv_days: 5,
            ipo_quota_ratio: 1.0,
            rank_min_days: 0,
            rank_min_trades: 1,
            credit_min_level: 4,
            assessment_valid_days: 90,
            assessment_per_day: 3,
        }
    }
}

impl Rules {
    pub fn from_table(t: Option<&toml::Value>) -> Result<Rules, String> {
        match t {
            Some(v) => v.clone().try_into().map_err(|e| format!("[account.eligibility]: {e}")),
            None => Ok(Rules::default()),
        }
    }
}

/// Why a gate is closed.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    Open,
    NotVerified,
    TooNew { days: i64 },
    Inactive { days: i64 },
    FewTrades { need: i64, have: i64 },
    NoQuota { days: i64 },
}

/// Every verified account.
pub fn verified_ids(c: &Connection) -> rusqlite::Result<HashSet<i64>> {
    let mut q = c.prepare_cached("SELECT DISTINCT account_id FROM bindings")?;
    let ids = q.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(ids)
}

/// How `id` is verified (`qq`), or `None`.
pub fn method(c: &Connection, id: i64) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT platform FROM bindings WHERE account_id=? ORDER BY id LIMIT 1", [id], |r| r.get(0)).optional()
}

/// Whole days from the registration timestamp's date to `today` (both `YYYY-MM-DD…`).
pub fn days_since(created_at: &str, today: &str) -> i64 {
    let d = |s: &str| chrono::NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").ok();
    match (d(created_at), d(today)) {
        (Some(a), Some(b)) => (b - a).num_days(),
        _ => 0,
    }
}

fn day_minus(today: &str, n: i64) -> String {
    chrono::NaiveDate::parse_from_str(today.get(..10).unwrap_or(today), "%Y-%m-%d")
        .map(|d| (d - chrono::Duration::days(n)).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| today.to_string())
}

pub fn trade_count(c: &Connection, id: i64) -> rusqlite::Result<i64> {
    let n = |col: &str| -> rusqlite::Result<i64> {
        c.query_row(&format!("SELECT COUNT(*) FROM trades WHERE {col}=?"), [id], |r| r.get(0))
    };
    // a trade with oneself (never happens for players) would count twice: fine for a threshold
    Ok(n("buyer_id")? + n("seller_id")?)
}

/// A trade on or after `today - days`.
pub fn traded_since(c: &Connection, id: i64, today: &str, days: i64) -> rusqlite::Result<bool> {
    let from = day_minus(today, days);
    let q = |col: &str| -> rusqlite::Result<bool> {
        c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM trades WHERE {col}=? AND day>=?)"), params![id, from], |r| r.get(0))
    };
    Ok(q("buyer_id")? || q("seller_id")?)
}

/// 财政补贴
pub fn transfer_gate(c: &Connection, r: &Rules, id: i64, verified: bool, created_at: &str, today: &str) -> rusqlite::Result<Gate> {
    if r.require_kyc && !verified {
        return Ok(Gate::NotVerified);
    }
    if days_since(created_at, today) < r.transfer_min_days {
        return Ok(Gate::TooNew { days: r.transfer_min_days });
    }
    if !traded_since(c, id, today, r.transfer_active_days)? {
        return Ok(Gate::Inactive { days: r.transfer_active_days });
    }
    Ok(Gate::Open)
}

/// 排行榜
pub fn rank_gate(c: &Connection, r: &Rules, id: i64, verified: bool, created_at: &str, today: &str) -> rusqlite::Result<Gate> {
    if r.require_kyc && !verified {
        return Ok(Gate::NotVerified);
    }
    if days_since(created_at, today) < r.rank_min_days {
        return Ok(Gate::TooNew { days: r.rank_min_days });
    }
    let have = trade_count(c, id)?;
    if have < r.rank_min_trades {
        return Ok(Gate::FewTrades { need: r.rank_min_trades, have });
    }
    Ok(Gate::Open)
}

/// 新股认购 quota (cents): the mean holdings value over the last `ipo_mv_days` closed trading days
/// before `today` (a day without a row counts as 0, like a new account in A-share 市值配售), × the ratio.
pub fn ipo_quota(c: &Connection, r: &Rules, id: i64, today: &str) -> rusqlite::Result<i64> {
    let n = r.ipo_mv_days.max(1);
    let sum: i64 = c.query_row(
        "SELECT COALESCE(SUM(stock_value),0) FROM (SELECT stock_value FROM player_days WHERE account_id=? AND day<? ORDER BY day DESC LIMIT ?)",
        params![id, today, n],
        |row| row.get(0),
    )?;
    Ok(((sum as f64 / n as f64) * r.ipo_quota_ratio).floor() as i64)
}

/// 新股认购 (the amount is checked against the quota when subscribing)
pub fn ipo_gate(r: &Rules, verified: bool, quota: i64) -> Gate {
    if r.require_kyc && !verified {
        Gate::NotVerified
    } else if quota <= 0 {
        Gate::NoQuota { days: r.ipo_mv_days }
    } else {
        Gate::Open
    }
}

/// The player-facing reason for a closed gate; `what` is 领取财政补贴 / 认购新股 / 进入排行.
pub fn reason(g: &Gate, what: &str) -> Option<String> {
    Some(match g {
        Gate::Open => return None,
        Gate::NotVerified => format!("完成身份验证后才能{what}"),
        Gate::TooNew { days } => format!("注册满 {days} 天后才能{what}"),
        Gate::Inactive { days } => format!("过去 {days} 天有成交才能{what}"),
        Gate::FewTrades { need: 1, .. } => format!("完成第一笔成交后才能{what}"),
        Gate::FewTrades { need, have } => format!("成交满 {need} 笔后才能{what}，还差 {} 笔", need - have),
        Gate::NoQuota { days } => format!("认购额度按前 {days} 个交易日的日均持仓市值计算，持有股票后才能{what}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE bindings (id INTEGER PRIMARY KEY, account_id INTEGER, platform TEXT);
             CREATE TABLE trades (id INTEGER PRIMARY KEY, buyer_id INTEGER, seller_id INTEGER, day TEXT);
             CREATE TABLE player_days (account_id INTEGER, day TEXT, stock_value INTEGER);
             INSERT INTO bindings(account_id, platform) VALUES (1, 'qq');",
        )
        .unwrap();
        c
    }

    #[test]
    fn verification_is_a_binding() {
        let c = db();
        assert_eq!(verified_ids(&c).unwrap(), HashSet::from([1]));
        assert_eq!(method(&c, 1).unwrap().as_deref(), Some("qq"));
        assert_eq!(method(&c, 2).unwrap(), None);
    }

    #[test]
    fn transfer_and_rank_gates() {
        let c = db();
        let r = Rules::default();
        let today = "2026-11-20";
        let old = "2026-11-01T10:00:00+08:00";
        assert_eq!(transfer_gate(&c, &r, 2, false, old, today).unwrap(), Gate::NotVerified);
        assert_eq!(transfer_gate(&c, &r, 1, true, "2026-11-15T10:00:00+08:00", today).unwrap(), Gate::TooNew { days: 7 });
        assert_eq!(transfer_gate(&c, &r, 1, true, old, today).unwrap(), Gate::Inactive { days: 28 });
        c.execute("INSERT INTO trades(buyer_id, seller_id, day) VALUES (9, 1, '2026-11-10')", []).unwrap();
        assert_eq!(transfer_gate(&c, &r, 1, true, old, today).unwrap(), Gate::Open);
        assert_eq!(rank_gate(&c, &r, 1, true, old, today).unwrap(), Gate::Open);
        assert_eq!(rank_gate(&c, &r, 2, true, old, today).unwrap(), Gate::FewTrades { need: 1, have: 0 });
        let r = Rules { rank_min_trades: 10, ..Rules::default() };
        assert_eq!(rank_gate(&c, &r, 1, true, old, today).unwrap(), Gate::FewTrades { need: 10, have: 1 });
        assert_eq!(reason(&Gate::FewTrades { need: 10, have: 1 }, "进入排行").unwrap(), "成交满 10 笔后才能进入排行，还差 9 笔");
        assert_eq!(reason(&Gate::FewTrades { need: 1, have: 0 }, "进入排行").unwrap(), "完成第一笔成交后才能进入排行");
        for _ in 0..9 {
            c.execute("INSERT INTO trades(buyer_id, seller_id, day) VALUES (1, 9, '2026-10-01')", []).unwrap();
        }
        assert_eq!(rank_gate(&c, &r, 1, true, old, today).unwrap(), Gate::Open);
        let off = Rules { require_kyc: false, ..Rules::default() };
        assert_eq!(rank_gate(&c, &off, 1, false, old, today).unwrap(), Gate::Open);
    }

    #[test]
    fn ipo_quota_is_the_mean_over_the_last_days() {
        let c = db();
        let r = Rules::default();
        assert_eq!(ipo_quota(&c, &r, 1, "2026-11-20").unwrap(), 0);
        assert_eq!(ipo_gate(&r, true, 0), Gate::NoQuota { days: 5 });
        for (d, v) in [("2026-11-12", 9_999_999), ("2026-11-13", 100), ("2026-11-14", 200), ("2026-11-17", 300), ("2026-11-18", 400), ("2026-11-19", 500), ("2026-11-20", 7_777)] {
            c.execute("INSERT INTO player_days VALUES (1, ?, ?)", params![d, v]).unwrap();
        }
        // 11-13 .. 11-19 (today's row is not closed yet; 11-12 is older than five days)
        assert_eq!(ipo_quota(&c, &r, 1, "2026-11-20").unwrap(), 300);
        assert_eq!(ipo_gate(&r, false, 300), Gate::NotVerified);
        assert_eq!(ipo_gate(&r, true, 300), Gate::Open);
    }
}
