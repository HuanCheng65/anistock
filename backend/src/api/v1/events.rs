//! The `events` topic (docs/api-v1.md §5.3.7): bot-facing events derived from news, trades and the daily
//! close. Thresholds live in `[api.events]` of game.toml (`big_move_daily`, `big_move_window`,
//! `big_move_window_min`).
//!
//! - From news (on the engine thread, `live.rs`): `flash`, `vcm`, `ipo_open`, `ipo_allotted`,
//!   `delisting_warning`, `delisted`.
//! - From trades (`live.rs` via `Moves`): `big_move` (both rules), `listed` (the first trade on the
//!   listing day).
//! - From the daily close (`loops.rs`, after a close succeeds; built from the database here): `daily_close`,
//!   and `leaderboard_weekly` after the close of a Sunday (the market trades every day, so the week of
//!   the 周榜 ends with Sunday's close).

use std::collections::{HashMap, VecDeque};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

use super::error::{sql, R};
use super::types::*;

#[derive(Clone, Debug)]
pub struct EventRules {
    pub daily: f64,
    pub window: f64,
    pub window_min: i64,
}

impl EventRules {
    pub fn from_config(cfg: &crate::config::Config) -> EventRules {
        EventRules {
            daily: cfg.api_f64("events.big_move_daily", 0.10),
            window: cfg.api_f64("events.big_move_window", 0.05),
            window_min: cfg.api_f64("events.big_move_window_min", 30.0) as i64,
        }
    }
}

/// An `event` message (§5.3.7).
#[derive(Serialize, ToSchema, Clone, Debug)]
pub struct Event {
    pub seq: u64,
    /// 开放枚举：flash、big_move、vcm、ipo_open、ipo_allotted、listed、delisting_warning、delisted、daily_close、leaderboard_weekly
    pub kind: String,
    pub at: Timestamp,
    pub trading_day: TradingDay,
    pub stock: Option<StockRef>,
    #[schema(value_type = Object)]
    pub data: Value,
    pub news_id: Option<i64>,
}

/// An event before it gets its `seq`.
#[derive(Clone, Debug)]
pub struct Pending {
    pub kind: String,
    pub at: String,
    pub trading_day: String,
    pub stock: Option<StockRef>,
    pub data: Value,
    pub news_id: Option<i64>,
}

/// `热度快讯：《…》第 1180 集讨论量高于预期 35%` -> (episode, signed surprise ratio).
pub fn parse_flash(title: &str) -> (Option<f64>, Option<f64>) {
    let ep = title.split("第 ").nth(1).and_then(|s| s.split(" 集").next()).and_then(|s| s.trim().parse::<f64>().ok());
    let sign = if title.contains("低于预期") { -1.0 } else { 1.0 };
    let pct = title
        .rsplit("预期 ")
        .next()
        .and_then(|s| s.trim_end_matches('%').trim().parse::<f64>().ok())
        .map(|p| r6(sign * p / 100.0));
    (ep, pct)
}

/// The `big_move` detector: per stock, recent (unix second, price) and what already fired.
#[derive(Default)]
pub struct Moves {
    recent: HashMap<i64, VecDeque<(i64, i64)>>,
    /// (stock, up?) -> highest daily band fired, per trading day
    daily: HashMap<(i64, bool), (String, i64)>,
    /// (stock, up?) -> unix second of the last window event
    window: HashMap<(i64, bool), i64>,
}

impl Moves {
    /// A trade at `price` (unix `t`): the `big_move` payloads it triggers.
    pub fn trade(&mut self, rules: &EventRules, stock: i64, t: i64, price: i64, prev_close: i64, day: &str) -> Vec<Value> {
        let mut out = Vec::new();
        let span = rules.window_min * 60;
        let ring = self.recent.entry(stock).or_default();
        // reference: the last trade at or before t - window
        let reference = ring.iter().rev().find(|(at, _)| *at <= t - span).map(|(_, p)| *p);
        ring.push_back((t, price));
        while ring.len() > 2 && ring[1].0 <= t - span {
            ring.pop_front();
        }
        if prev_close > 0 && rules.daily > 0.0 {
            let change = price as f64 / prev_close as f64 - 1.0;
            let up = change > 0.0;
            let band = (change.abs() / rules.daily + 1e-9).floor() as i64;
            let fired = self.daily.get(&(stock, up)).filter(|(d, _)| d == day).map_or(0, |(_, b)| *b);
            if band >= 1 && band > fired {
                self.daily.insert((stock, up), (day.to_string(), band));
                out.push(json!({"rule": "daily", "change": r6(change), "threshold": r6(band as f64 * rules.daily),
                    "window_minutes": null, "price": price, "reference_price": prev_close, "prev_close": prev_close}));
            }
        }
        if let Some(r) = reference.filter(|r| *r > 0) {
            let change = price as f64 / r as f64 - 1.0;
            let up = change > 0.0;
            let last = self.window.get(&(stock, up)).copied();
            if change.abs() >= rules.window && last.is_none_or(|l| t - l >= span) {
                self.window.insert((stock, up), t);
                out.push(json!({"rule": "window", "change": r6(change), "threshold": rules.window,
                    "window_minutes": rules.window_min, "price": price, "reference_price": r, "prev_close": prev_close}));
            }
        }
        out
    }
}

// ---------------------------------------------------------------- events built after a close

fn stock_ref(c: &Connection, id: i64) -> R<Option<StockRef>> {
    super::market::stock_ref(c, id)
}

/// `daily_close` for `d` (after its close), from the database.
pub fn daily_close(c: &Connection, d: &str) -> R<Value> {
    let idx = super::market::compute_indices(c, d, false, super::market::IndexBasis::TotalReturn)?;
    // closes of d and the day before, per stock listed on d
    let mut s = c
        .prepare(
            "SELECT k.company_id, k.close, (SELECT close FROM candles p WHERE p.company_id=k.company_id AND p.day<k.day ORDER BY p.day DESC LIMIT 1)
             FROM candles k WHERE k.day=?",
        )
        .map_err(sql)?;
    let rows: Vec<(i64, i64, Option<i64>)> =
        s.query_map([d], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    // the day before in today's share terms (a split or ex-rights day is not a fall)
    let adj = super::market::price_factors(c)?;
    let mut ch: Vec<(i64, f64)> = rows
        .iter()
        .filter_map(|(id, cl, p)| {
            p.filter(|p| *p > 0).map(|p| (*id, *cl as f64 / (p as f64 * super::market::factor_on(adj.get(id), d)) - 1.0))
        })
        .collect();
    let (mut up, mut flat, mut down) = (0, 0, 0);
    for (_, x) in &ch {
        if *x > 1e-9 {
            up += 1
        } else if *x < -1e-9 {
            down += 1
        } else {
            flat += 1
        }
    }
    ch.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let pick = |v: &[(i64, f64)]| -> R<Vec<Value>> {
        let mut out = Vec::new();
        for (id, x) in v {
            if let Some(r) = stock_ref(c, *id)? {
                out.push(json!({"stock": r, "change": r6(*x)}));
            }
        }
        Ok(out)
    };
    let gainers: Vec<(i64, f64)> = ch.iter().filter(|x| x.1 > 0.0).take(3).cloned().collect();
    let losers: Vec<(i64, f64)> = ch.iter().rev().filter(|x| x.1 < 0.0).take(3).cloned().collect();
    let divs: i64 = c
        .query_row(
            "SELECT COALESCE(SUM(l.amount),0) FROM ledger l JOIN accounts a ON a.id=l.to_id WHERE l.day=? AND l.reason='dividend' AND a.kind='player'",
            [d],
            |r| r.get(0),
        )
        .map_err(sql)?;
    let rate: Option<f64> = c.query_row("SELECT rate FROM macro_days WHERE day=?", [d], |r| r.get(0)).optional().map_err(sql)?;
    Ok(json!({
        "trading_day": d,
        "indices": idx.items.iter().map(|i| json!({"key": i.key, "value": i.value, "change": i.change})).collect::<Vec<_>>(),
        "breadth": {"up": up, "flat": flat, "down": down},
        "top_gainers": pick(&gainers)?,
        "top_losers": pick(&losers)?,
        "dividends_paid": divs,
        "policy_rate": rate,
    }))
}

/// `leaderboard_weekly` for the week ending with `d` (a Sunday), from `player_days`.
pub fn leaderboard_weekly(c: &Connection, d: &str) -> R<Value> {
    let end = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(super::error::internal)?;
    let monday = (end - chrono::Duration::days(6)).format("%Y-%m-%d").to_string();
    let mut s = c
        .prepare(
            "SELECT a.id, a.username, e.cash + e.stock_value - e.liabilities, e.return_index,
               (SELECT return_index FROM player_days b WHERE b.account_id=a.id AND b.day<?1 ORDER BY b.day DESC LIMIT 1),
               COALESCE((SELECT restructures FROM player_returns p WHERE p.account_id=a.id), 0)
             FROM accounts a JOIN player_days e ON e.account_id=a.id AND e.day=?2 WHERE a.kind='player'",
        )
        .map_err(sql)?;
    let mut rows: Vec<(f64, Value)> = s
        .query_map([&monday, &d.to_string()], |r| {
            let (id, name, total, idx, base, n): (i64, String, i64, f64, Option<f64>, i64) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
            let ret = idx / base.filter(|b| *b > 0.0).unwrap_or(1.0) - 1.0;
            Ok((ret, json!({"user": {"id": id, "username": name}, "total_assets": total, "return": r6(ret), "restructures": n})))
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)?;
    rows.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1["user"]["id"].as_i64().cmp(&b.1["user"]["id"].as_i64()))
    });
    let top: Vec<Value> = rows
        .into_iter()
        .take(10)
        .enumerate()
        .map(|(i, (_, mut v))| {
            v["rank"] = json!(i + 1);
            v
        })
        .collect();
    Ok(json!({"from_day": monday, "to_day": d, "top": top}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_titles() {
        let (ep, s) = parse_flash("热度快讯：《航海王》第 1180 集讨论量高于预期 35%");
        assert_eq!(ep, Some(1180.0));
        assert_eq!(s, Some(0.35));
        let (_, s) = parse_flash("热度快讯：《X》第 3 集讨论量低于预期 12%");
        assert_eq!(s, Some(-0.12));
    }

    #[test]
    fn big_moves() {
        let rules = EventRules { daily: 0.10, window: 0.05, window_min: 30 };
        let mut m = Moves::default();
        assert!(m.trade(&rules, 1, 0, 1000, 1000, "d").is_empty());
        let e = m.trade(&rules, 1, 60, 1105, 1000, "d");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0]["rule"], "daily");
        assert!(m.trade(&rules, 1, 120, 1150, 1000, "d").is_empty(), "same band fires once");
        assert_eq!(m.trade(&rules, 1, 180, 1201, 1000, "d")[0]["threshold"], 0.2);
        // window: 31 minutes later the reference is the 1105 print (the last one at least 30 minutes old)
        let e = m.trade(&rules, 1, 31 * 60, 1210, 1210, "d");
        assert!(e.iter().any(|x| x["rule"] == "window" && x["reference_price"] == 1105), "{e:?}");
        assert!(m.trade(&rules, 1, 32 * 60, 1300, 1300, "d").iter().all(|x| x["rule"] != "window"));
    }
}
