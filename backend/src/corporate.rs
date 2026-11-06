//! Company capital policy (docs/v2-corporate.md §3): stock splits (拆股), rights issues (配股) and buybacks (回购),
//! each run by the company's management by published rules, and the price adjustment factors they leave in the
//! price history (前复权 K-lines, the NPC market's price memory).
//!
//! - Split: a close above `split_above` for `split_days` trading days in a row announces a k-for-1 split, applied
//!   at the close before its first day `split_lead_days` later: every holding, lot, short, the share count × k;
//!   prices and the value ÷ k.
//! - Rights: when a sequel is announced (the sequel option on) its production cost (the value difference between
//!   the season counted net and in full) is paid from the company's cash (less this week's dividend) and the rest
//!   is raised by a rights issue at `rights_price` × the close, at most `rights_max_ratio` new shares per share.
//!   Players subscribe for `rights_days` days; the record, the payment and the issue happen at one close; untaken
//!   shares go to the rights holders (committee) at the theoretical ex-rights price, the difference paid to the
//!   holders who did not take them (HK Listing Rule 7.21 compensatory arrangement); short sellers pay the lender
//!   the same difference on the shares they owe.
//! - Buyback: on Sundays, idle cash (cash less the expected sequel production cost) of at least `buyback_min` of
//!   the market value, with the close under the value, starts a programme: at most the idle cash, at most the
//!   value at the announcement per share, for `buyback_days`; bought in continuous trading from
//!   `buyback_quiet_min` after the open to as long before the close, every `buyback_every_min` minutes at the
//!   best bid, at most `buyback_adv` of the trailing average daily volume a day (SEC Rule 10b-18); bought shares
//!   are cancelled at each close.

use std::collections::{BTreeMap, HashMap, VecDeque};

use chrono::{Duration, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::engine::{Engine, NewOrder};
use crate::exchange::OrderError;
use crate::model::{Company, Lot, OrderStatus};
use crate::store::persist::Change;
use crate::valuation::Params as VParams;
use crate::vfilter::FilterParams;

/// `[corporate]` of game.toml (docs/v2-corporate.md §3).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CorporateRules {
    pub enabled: bool,
    /// split when the close is above this (币) ...
    pub split_above: f64,
    /// ... this many trading days in a row
    pub split_days: i64,
    /// the split ratios a company chooses from
    pub split_ratios: Vec<i64>,
    /// days from the announcement to the first day in new units
    pub split_lead_days: i64,
    /// rights price as a share of the announcement day's close
    pub rights_price: f64,
    /// at most this many new shares per share
    pub rights_max_ratio: f64,
    /// subscription days (the last one is the record day)
    pub rights_days: i64,
    /// a buyback needs idle cash of at least this share of the market value
    pub buyback_min: f64,
    /// ... and the close under the value at this many closes in a row (persistent undervaluation)
    pub buyback_under_days: i64,
    /// programme length, days
    pub buyback_days: i64,
    /// at most this share of the trailing average daily volume a day
    pub buyback_adv: f64,
    /// trading days in the average daily volume
    pub buyback_adv_days: usize,
    pub buyback_every_min: i64,
    /// no buying this long after the open and before the close
    pub buyback_quiet_min: i64,
}

impl Default for CorporateRules {
    fn default() -> CorporateRules {
        CorporateRules {
            enabled: true,
            split_above: 100.0,
            split_days: 5,
            split_ratios: vec![2, 3, 4, 5, 10],
            split_lead_days: 7,
            rights_price: 0.8,
            rights_max_ratio: 0.3,
            rights_days: 3,
            buyback_min: 0.01,
            buyback_under_days: 20,
            buyback_days: 91,
            buyback_adv: 0.25,
            buyback_adv_days: 20,
            buyback_every_min: 10,
            buyback_quiet_min: 30,
        }
    }
}

impl CorporateRules {
    pub fn from_table(t: Option<&toml::Value>) -> Result<CorporateRules, String> {
        match t {
            None => Ok(CorporateRules::default()),
            Some(v) => v.clone().try_into().map_err(|e| format!("[corporate]: {e}")),
        }
    }
    fn split_above_cents(&self) -> i64 {
        (self.split_above * 100.0).round() as i64
    }
}

/// The parameters of one action (unused fields stay 0).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Params {
    /// split: k-for-1
    pub k: i64,
    /// rights: new shares per share (entitlement = floor(qty × ratio)), price (cents), the production cost to pay
    /// (cents), the sequel it funds, the work carrying it; on the record day: the theoretical ex-rights price,
    /// shares issued, shares subscribed by players, the cost actually paid
    pub ratio: f64,
    pub price: i64,
    pub budget: i64,
    pub sequel: i64,
    pub work: i64,
    pub terp: i64,
    pub issued: i64,
    pub subscribed: i64,
    pub paid: i64,
    /// buyback: the cash it may spend, the price cap (cents a share), spent and bought so far
    pub cap: i64,
    pub spent: i64,
    pub bought: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Action {
    pub id: i64,
    pub company_id: i64,
    /// split | rights | buyback
    pub kind: String,
    pub announced: String,
    pub effective: String,
    /// pending | active | done | cancelled
    pub status: String,
    pub params: Params,
}

/// A player's subscription to a rights issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RightsSub {
    pub action_id: i64,
    pub account_id: i64,
    pub qty: i64,
    pub frozen: i64,
    pub allotted: Option<i64>,
    pub created_at: String,
}

/// Prices before `day` × `price_factor`, volumes × `qty_factor`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Adjustment {
    pub company_id: i64,
    pub day: String,
    pub price_factor: f64,
    pub qty_factor: f64,
    pub action_id: i64,
}

#[derive(Debug, Clone, Default)]
pub struct CorporateState {
    pub actions: BTreeMap<i64, Action>,
    pub subs: BTreeMap<(i64, i64), RightsSub>,
    pub adjustments: Vec<Adjustment>,
    /// trading days in a row each listed stock has closed above the split threshold
    pub above_days: HashMap<i64, i64>,
    /// closes in a row each listed stock has closed under its value (memory only: 0 after a restart)
    pub under_days: HashMap<i64, i64>,
    /// each listed stock's daily volumes, latest last (the buyback's daily limit)
    pub volumes: HashMap<i64, VecDeque<i64>>,
    /// each listed stock's last 30 closes, latest last, in today's units (the buyback's published price cap)
    pub closes: HashMap<i64, VecDeque<i64>>,
    pub last_id: i64,
    /// buyback: the last 10-minute slot each programme acted in (memory only)
    slot: HashMap<i64, i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CorporateChange {
    Action(Action),
    Sub(RightsSub),
    SubDelete(i64, i64),
    Adjustment(Adjustment),
}

pub fn apply(conn: &Connection, c: &CorporateChange) -> rusqlite::Result<()> {
    match c {
        CorporateChange::Action(a) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO corporate_actions(id, company_id, kind, announced, effective, status, params)
                 VALUES (?,?,?,?,?,?,?)",
            )?
            .execute(params![
                a.id,
                a.company_id,
                a.kind,
                a.announced,
                a.effective,
                a.status,
                serde_json::to_string(&a.params).unwrap_or_default()
            ])?;
        }
        CorporateChange::Sub(s) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO rights_subscriptions(action_id, account_id, qty, frozen, allotted, created_at)
                 VALUES (?,?,?,?,?,?)",
            )?
            .execute(params![s.action_id, s.account_id, s.qty, s.frozen, s.allotted, s.created_at])?;
        }
        CorporateChange::SubDelete(a, b) => {
            conn.prepare_cached("DELETE FROM rights_subscriptions WHERE action_id=? AND account_id=?")?.execute([a, b])?;
        }
        CorporateChange::Adjustment(x) => {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO price_adjustments(company_id, day, price_factor, qty_factor, action_id) VALUES (?,?,?,?,?)",
            )?
            .execute(params![x.company_id, x.day, x.price_factor, x.qty_factor, x.action_id])?;
        }
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?", [name], |_| Ok(())).optional()?.is_some())
}

/// Load the corporate tables (empty before migration V10) and rebuild the split counters and volumes from the
/// live candles (prices adjusted for later splits and rights issues).
pub fn load(conn: &Connection, split_above_cents: i64, adv_days: usize) -> rusqlite::Result<CorporateState> {
    let mut s = CorporateState::default();
    if table_exists(conn, "corporate_actions")? {
        for r in conn
            .prepare("SELECT id, company_id, kind, announced, effective, status, params FROM corporate_actions")?
            .query_map([], |r| {
                Ok(Action {
                    id: r.get(0)?,
                    company_id: r.get(1)?,
                    kind: r.get(2)?,
                    announced: r.get(3)?,
                    effective: r.get(4)?,
                    status: r.get(5)?,
                    params: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                })
            })?
        {
            let a = r?;
            s.last_id = s.last_id.max(a.id);
            s.actions.insert(a.id, a);
        }
        for r in conn
            .prepare("SELECT action_id, account_id, qty, frozen, allotted, created_at FROM rights_subscriptions")?
            .query_map([], |r| {
                Ok(RightsSub { action_id: r.get(0)?, account_id: r.get(1)?, qty: r.get(2)?, frozen: r.get(3)?, allotted: r.get(4)?, created_at: r.get(5)? })
            })?
        {
            let x = r?;
            s.subs.insert((x.action_id, x.account_id), x);
        }
        for r in conn
            .prepare("SELECT company_id, day, price_factor, qty_factor, action_id FROM price_adjustments ORDER BY day, action_id")?
            .query_map([], |r| Ok(Adjustment { company_id: r.get(0)?, day: r.get(1)?, price_factor: r.get(2)?, qty_factor: r.get(3)?, action_id: r.get(4)? }))?
        {
            s.adjustments.push(r?);
        }
    }
    if table_exists(conn, "candles")? {
        let mut st = conn.prepare("SELECT company_id, day, close, volume FROM candles WHERE source='live' ORDER BY company_id, day DESC")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?;
        let mut done: std::collections::HashSet<i64> = Default::default();
        for row in rows {
            let (c, day, close, vol) = row?;
            let (pf, qf) = s.factor_after(c, &day);
            let v = s.volumes.entry(c).or_default();
            if v.len() < adv_days {
                v.push_front((vol as f64 * qf).round() as i64);
            }
            let cl = s.closes.entry(c).or_default();
            if cl.len() < CLOSE_DAYS {
                cl.push_front((close as f64 * pf).round() as i64);
            }
            if done.contains(&c) {
                continue;
            }
            if (close as f64 * pf) > split_above_cents as f64 {
                *s.above_days.entry(c).or_default() += 1;
            } else {
                done.insert(c);
            }
        }
    }
    Ok(s)
}

impl CorporateState {
    /// The cumulative (price, volume) factor of every adjustment of `company` after `day` (前复权).
    pub fn factor_after(&self, company: i64, day: &str) -> (f64, f64) {
        let mut f = (1.0, 1.0);
        for a in self.adjustments.iter().filter(|a| a.company_id == company && a.day.as_str() > day) {
            f.0 *= a.price_factor;
            f.1 *= a.qty_factor;
        }
        f
    }
}

/// Trading days in the average a buyback's price cap is set from.
const CLOSE_DAYS: usize = 30;

fn day_add(day: &str, n: i64) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| (d + Duration::days(n)).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn md(day: &str) -> String {
    NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| d.format("%-m 月 %-d 日").to_string()).unwrap_or_default()
}

fn yuan(c: i64) -> String {
    crate::pyfmt::fixed(c as f64 / 100.0, 2)
}

fn grouped(c: i64) -> String {
    crate::pyfmt::fixed_grouped(c as f64 / 100.0, 0)
}

/// Rights per 10 shares as shown (每 10 股配 x 股): one decimal, no trailing zero.
pub fn per_ten(ratio: f64) -> String {
    let x = (ratio * 100.0).round() / 10.0;
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.1}")
    }
}

/// What the close needs to value companies again (docs/v2-valuation.md).
pub struct ValueCtx<'a> {
    pub fp: &'a FilterParams,
    pub p: &'a VParams,
    pub totals: &'a [Vec<f64>; 3],
    pub today: i64,
    pub rate: f64,
    pub a_ref: f64,
}

/// Money the corporate step moved out of (or into) the economy, for the close's summary.
#[derive(Debug, Default, Clone, Copy)]
pub struct Flows {
    pub exported: i64,
    pub injected: i64,
}

impl Engine {
    fn corp_change(&mut self, c: CorporateChange) {
        self.out.push(Change::Corporate(c));
    }

    fn save_action(&mut self, a: Action) {
        self.corp_change(CorporateChange::Action(a.clone()));
        self.st.corp.actions.insert(a.id, a);
    }

    fn new_action(&mut self, company_id: i64, kind: &str, announced: &str, effective: &str, status: &str, params: Params) -> i64 {
        self.st.corp.last_id += 1;
        let id = self.st.corp.last_id;
        self.save_action(Action {
            id,
            company_id,
            kind: kind.into(),
            announced: announced.into(),
            effective: effective.into(),
            status: status.into(),
            params,
        });
        id
    }

    /// Every corporate action of a company, oldest first.
    pub fn corporate_actions(&self, company_id: i64) -> Vec<&Action> {
        self.st.corp.actions.values().filter(|a| a.company_id == company_id).collect()
    }

    /// The open action of `kind` of a company (pending or active).
    pub fn open_action(&self, company_id: i64, kind: &str) -> Option<&Action> {
        self.st.corp.actions.values().find(|a| a.company_id == company_id && a.kind == kind && (a.status == "pending" || a.status == "active"))
    }

    pub fn price_adjustments(&self) -> &[Adjustment] {
        &self.st.corp.adjustments
    }

    /// The cumulative (price, volume) adjustment of `company` for a bar on `day` (前复权).
    pub fn adjust_factor(&self, company: i64, day: &str) -> (f64, f64) {
        self.st.corp.factor_after(company, day)
    }

    pub fn rights_sub(&self, action_id: i64, account_id: i64) -> Option<&RightsSub> {
        self.st.corp.subs.get(&(action_id, account_id))
    }

    /// A holder's rights entitlement in an open rights issue at its current holding.
    pub fn rights_entitlement(&self, action_id: i64, account_id: i64) -> i64 {
        let Some(a) = self.st.corp.actions.get(&action_id) else { return 0 };
        let q = self.st.holdings.get(&(account_id, a.company_id)).map_or(0, |h| h.qty);
        (q as f64 * a.params.ratio + 1e-9).floor() as i64
    }

    fn holders_of(&self, company_id: i64) -> Vec<(i64, i64, String)> {
        self.st
            .holdings_by_rowid
            .values()
            .filter(|k| k.1 == company_id)
            .filter_map(|k| {
                let h = &self.st.holdings[k];
                let a = self.st.accounts.get(&k.0)?;
                (h.qty > 0).then(|| (k.0, h.qty, a.kind.clone()))
            })
            .collect()
    }

    fn cancel_company_orders(&mut self, company_id: i64, ts: &str, only: Option<i64>) {
        let mut ids: Vec<i64> = self
            .st
            .orders
            .values()
            .filter(|o| o.company_id == company_id && o.status == OrderStatus::Open && only.is_none_or(|a| o.account_id == a))
            .map(|o| o.id)
            .collect();
        ids.sort_unstable();
        for id in ids {
            self.close(id, OrderStatus::Cancelled, ts);
        }
    }

    fn record_adjustment(&mut self, company_id: i64, day: &str, price_factor: f64, qty_factor: f64, action_id: i64) {
        let a = Adjustment { company_id, day: day.to_string(), price_factor, qty_factor, action_id };
        self.corp_change(CorporateChange::Adjustment(a.clone()));
        self.st.corp.adjustments.push(a);
    }

    // ------------------------------------------------------------ the close

    /// Start of the daily close: buyback orders end, the shares bought back today are cancelled (注销).
    pub fn corporate_pre_close(&mut self, ts: &str) {
        let active: Vec<Action> = self.st.corp.actions.values().filter(|a| a.kind == "buyback" && a.status == "active").cloned().collect();
        for mut a in active {
            let c = a.company_id;
            let Some(acct) = self.account_of("company", c) else { continue };
            self.cancel_company_orders(c, ts, Some(acct));
            let q = self.st.holdings.get(&(acct, c)).map_or(0, |h| h.qty);
            if q > 0 {
                if let Ok(taken) = self.do_remove_shares(acct, c, q, 0) {
                    a.params.spent += taken.iter().map(|(n, cost, _)| n * cost).sum::<i64>();
                    a.params.bought += q;
                    if let Some(co) = self.st.companies.get_mut(&c) {
                        co.shares -= q;
                    }
                    self.mark_company(c);
                    self.save_action(a);
                }
            }
        }
    }

    /// The corporate step of the close, after the candles (docs/v2-corporate.md §3): settle rights issues on their
    /// record day, apply splits taking effect tomorrow, count closes over the split threshold and announce splits,
    /// end and (Sundays) start buybacks. Returns the money moved across the economy's border.
    pub fn corporate_close(&mut self, day: &str, ts: &str, sunday: bool, vc: Option<&ValueCtx>) -> Result<Flows, String> {
        let mut flows = Flows::default();
        if !self.corporate.enabled {
            return Ok(flows);
        }
        let tomorrow = day_add(day, 1);
        // rights issues recorded today
        let due: Vec<i64> = self.st.corp.actions.values().filter(|a| a.kind == "rights" && a.status == "pending" && a.effective.as_str() <= day).map(|a| a.id).collect();
        for id in due {
            let f = self.settle_rights(id, day, ts, vc)?;
            flows.exported += f.exported;
            flows.injected += f.injected;
        }
        // splits taking effect tomorrow
        let due: Vec<i64> = self.st.corp.actions.values().filter(|a| a.kind == "split" && a.status == "pending" && a.effective.as_str() <= tomorrow.as_str()).map(|a| a.id).collect();
        for id in due {
            self.apply_split(id, day, ts)?;
        }
        // the split rule and the volumes
        let listed: Vec<Company> = self.st.companies.values().filter(|c| c.status == "listed").cloned().collect();
        let mut closes: Vec<i64> = listed.iter().map(|c| c.prev_close).filter(|p| *p > 0).collect();
        closes.sort_unstable();
        let median = closes.get(closes.len() / 2).copied().unwrap_or(0);
        let above = self.corporate.split_above_cents();
        for c in &listed {
            let vol = self.st.day_aggs.get(&(c.id, day.to_string())).map_or(0, |a| a.volume);
            let v = self.st.corp.volumes.entry(c.id).or_default();
            v.push_back(vol);
            while v.len() > self.corporate.buyback_adv_days {
                v.pop_front();
            }
            let cl = self.st.corp.closes.entry(c.id).or_default();
            cl.push_back(c.prev_close);
            while cl.len() > CLOSE_DAYS {
                cl.pop_front();
            }
            let u = self.st.corp.under_days.entry(c.id).or_insert(0);
            *u = if c.prev_close > 0 && c.prev_close < c.fair_value { *u + 1 } else { 0 };
            let n = self.st.corp.above_days.entry(c.id).or_insert(0);
            *n = if c.prev_close > above { *n + 1 } else { 0 };
            let n = *n;
            if n >= self.corporate.split_days && median > 0 && self.open_action(c.id, "split").is_none() {
                let k = self.split_ratio(c.prev_close, median);
                let effective = day_add(day, self.corporate.split_lead_days);
                let id = self.new_action(c.id, "split", day, &effective, "pending", Params { k, ..Default::default() });
                let _ = id;
                self.push_news(
                    ts,
                    day,
                    "split",
                    Some(c.id),
                    format!("《{}》拆股：每 1 股拆为 {k} 股", c.title),
                    format!(
                        "收盘价已连续 {n} 个交易日高于 {} 星辉。{}起生效，持仓数量 × {k}，价格 ÷ {k}，市值不变。",
                        crate::pyfmt::fixed(self.corporate.split_above, 0),
                        md(&effective)
                    ),
                );
            }
        }
        // buybacks: end, then (Sundays) start
        let active: Vec<Action> = self.st.corp.actions.values().filter(|a| a.kind == "buyback" && a.status == "active").cloned().collect();
        for mut a in active {
            let Some(c) = self.st.companies.get(&a.company_id).cloned() else { continue };
            let spent_out = a.params.spent >= a.params.budget - c.prev_close.max(1);
            if a.effective.as_str() <= day || spent_out || c.status != "listed" {
                a.status = "done".into();
                let (bought, spent) = (a.params.bought, a.params.spent);
                self.save_action(a);
                let body = if bought > 0 {
                    format!("共回购并注销 {} 股，金额 {} 星辉，均价 {} 星辉。", crate::pyfmt::int_grouped(bought), grouped(spent), yuan(spent / bought.max(1)))
                } else {
                    "价格一直高于回购价格上限，没有买入。".to_string()
                };
                self.push_news(ts, day, "buyback", Some(c.id), format!("《{}》回购结束", c.title), body);
            }
        }
        if sunday {
            if let Some(vc) = vc {
                for c in &listed {
                    self.maybe_buyback(c.id, day, ts, vc);
                }
            }
        }
        Ok(flows)
    }

    /// k from the allowed ratios: the post-split price closest (in log) to the market's median close.
    fn split_ratio(&self, price: i64, median: i64) -> i64 {
        let want = (price as f64 / median.max(1) as f64).ln();
        let mut best = (f64::INFINITY, 2);
        for &k in &self.corporate.split_ratios {
            let d = ((k as f64).ln() - want).abs();
            if d < best.0 {
                best = (d, k);
            }
        }
        best.1
    }

    fn apply_split(&mut self, id: i64, day: &str, ts: &str) -> Result<(), String> {
        let mut a = self.st.corp.actions[&id].clone();
        let k = a.params.k.max(1);
        let c = a.company_id;
        let Some(co) = self.st.companies.get(&c).cloned() else { return Ok(()) };
        if co.status != "listed" {
            a.status = "cancelled".into();
            self.save_action(a);
            return Ok(());
        }
        // (a close running while the next day trades: orders still open would be in old units)
        self.cancel_company_orders(c, ts, None);
        let keys: Vec<(i64, i64)> = self.st.holdings.keys().filter(|k2| k2.1 == c).copied().collect();
        for key in keys {
            if let Some(h) = self.st.holdings.get_mut(&key) {
                h.qty *= k;
                h.qty_frozen *= k;
            }
            self.emit_holding(key);
            let ids: Vec<i64> = self.st.lots_by_owner.get(&key).map(|v| v.iter().copied().collect()).unwrap_or_default();
            for lid in ids {
                if let Some(l) = self.st.lots.get_mut(&lid) {
                    l.qty *= k;
                    l.cost = ((l.cost as f64) / k as f64).round() as i64;
                    let l: Lot = l.clone();
                    self.out.push(Change::LotReplace(l));
                }
            }
        }
        self.margin_on_split(c, k);
        let co = self.st.companies.get_mut(&c).unwrap();
        let div = |x: i64| ((x as f64) / k as f64).round().max(1.0) as i64;
        co.shares *= k;
        co.prev_close = div(co.prev_close);
        co.last_price = div(co.last_price);
        co.open_ref = co.open_ref.map(div);
        co.fair_value = div(co.fair_value);
        co.vcm_low = co.vcm_low.map(div);
        co.vcm_high = co.vcm_high.map(div);
        let title = co.title.clone();
        self.mark_company(c);
        let key = format!("mm_loan:{c}");
        if let Some(done) = self.state(&key).and_then(|v| v.parse::<i64>().ok()) {
            self.do_set_state(&key, &(done * k).to_string());
        }
        self.st.corp.above_days.insert(c, 0);
        if let Some(v) = self.st.corp.volumes.get_mut(&c) {
            for x in v.iter_mut() {
                *x *= k;
            }
        }
        if let Some(v) = self.st.corp.closes.get_mut(&c) {
            for x in v.iter_mut() {
                *x = div(*x);
            }
        }
        let eff = a.effective.clone();
        self.record_adjustment(c, &eff, 1.0 / k as f64, k as f64, id);
        a.status = "done".into();
        self.save_action(a);
        self.push_news(ts, day, "split", Some(c), format!("《{title}》拆股生效：每 1 股拆为 {k} 股"), "持仓数量和价格已按比例换算，市值不变。".into());
        Ok(())
    }

    // ------------------------------------------------------------ rights issues

    /// A sequel of `work` (company `c`) was announced at this close (docs/v2-corporate.md §3.3): its production
    /// cost is the value the season gains when it counts in full; paid from cash at once if the cash allows,
    /// else the rest by a rights issue.
    pub fn corporate_on_sequel(&mut self, c: i64, work: i64, sequel: i64, day: &str, ts: &str, vc: &ValueCtx) -> Flows {
        let mut flows = Flows::default();
        if !self.corporate.enabled || vc.fp.sequel_cost <= 0.0 || self.open_action(c, "rights").is_some() {
            return flows;
        }
        let Some(co) = self.st.companies.get(&c).cloned().filter(|x| x.status == "listed") else { return flows };
        let Some(unpaid) = self.company_value_full(vc, c) else { return flows };
        let Some(paid) = self.with_sequel_paid(work, Some(sequel), |e| e.company_value_full(vc, c)) else { return flows };
        let n = self.outstanding(&co);
        let budget = (paid - unpaid).max(0) * n;
        if budget <= 0 {
            return flows;
        }
        let Some(acct) = self.account_of("company", c) else { return flows };
        let cash = self.st.accounts[&acct].cash;
        // this Sunday's dividend stays (`profit_week` × payout, paid from cash)
        let pending = ((co.profit_week.max(0) as f64) * vc.p.payout_ratio).round() as i64;
        let usable = (cash - pending).max(0);
        if usable >= budget {
            self.do_transfer(acct, self.special("world").unwrap_or(acct), budget, "production_fee", day, ts, Some(c), None);
            flows.exported += budget;
            self.set_sequel_paid(work, Some(sequel), ts);
            self.push_news(
                ts,
                day,
                "rights",
                Some(c),
                format!("《{}》续作制作费由公司现金支付", co.title),
                format!("制作费 {} 星辉，全部来自公司留存的现金，不需要配股。", grouped(budget)),
            );
            return flows;
        }
        let short = budget - usable;
        let price = (((co.prev_close as f64) * self.corporate.rights_price).round() as i64).max(1);
        let ratio = ((short as f64 / price as f64 / n as f64 * 100.0).ceil() / 100.0).min(self.corporate.rights_max_ratio);
        let effective = day_add(day, self.corporate.rights_days);
        self.new_action(c, "rights", day, &effective, "pending", Params { ratio, price, budget, sequel, work, ..Default::default() });
        self.push_news(
            ts,
            day,
            "rights",
            Some(c),
            format!("《{}》配股：每 10 股配 {} 股，配股价 {} 星辉", co.title, per_ten(ratio), yuan(price)),
            format!(
                "为续作筹集制作费 {} 星辉（公司可用现金 {} 星辉）。{}前可以认购，{}收盘时按持仓登记、缴款并发行新股。未认购的部分由制作委员会按除权参考价接手，差价补给未认购的持有人。",
                grouped(budget),
                grouped(usable),
                md(&day_add(&effective, 0)),
                md(&effective)
            ),
        );
        flows
    }

    /// Subscribe to a rights issue (replacing an earlier subscription; 0 withdraws). Returns the cash frozen.
    pub fn rights_subscribe(&mut self, account_id: i64, company_id: i64, qty: i64, ts: &str) -> Result<i64, OrderError> {
        let a = self.open_action(company_id, "rights").cloned().ok_or_else(|| OrderError::new("这支股票没有在认购期的配股"))?;
        if qty < 0 {
            return Err(OrderError::new("数量必须是正整数"));
        }
        let max = self.rights_entitlement(a.id, account_id);
        if qty > max {
            return Err(OrderError(format!("最多可以认购 {} 股", crate::pyfmt::int_grouped(max))));
        }
        if let Some(old) = self.st.corp.subs.remove(&(a.id, account_id)) {
            self.add_cash(account_id, 0, -old.frozen);
            self.corp_change(CorporateChange::SubDelete(a.id, account_id));
        }
        if qty == 0 {
            self.commit();
            return Ok(0);
        }
        let frozen = qty * a.params.price;
        if self.available_cash(account_id) < frozen {
            self.commit();
            return Err(OrderError::new("可用资金不足"));
        }
        self.add_cash(account_id, 0, frozen);
        let s = RightsSub { action_id: a.id, account_id, qty, frozen, allotted: None, created_at: ts.to_string() };
        self.corp_change(CorporateChange::Sub(s.clone()));
        self.st.corp.subs.insert((a.id, account_id), s);
        self.commit();
        Ok(frozen)
    }

    /// Withdraw an account's open rights subscriptions (restructuring).
    pub(crate) fn rights_withdraw_all(&mut self, account_id: i64) {
        let keys: Vec<(i64, i64)> = self.st.corp.subs.iter().filter(|(k, s)| k.1 == account_id && s.allotted.is_none()).map(|(k, _)| *k).collect();
        for k in keys {
            if let Some(old) = self.st.corp.subs.remove(&k) {
                self.add_cash(account_id, 0, -old.frozen);
                self.corp_change(CorporateChange::SubDelete(k.0, k.1));
            }
        }
    }

    /// The record day: entitlements from today's holdings, players' subscriptions (capped at the entitlement),
    /// the committee's own take-up and the untaken shares at the theoretical ex-rights price with the difference
    /// paid to the holders who did not subscribe, the short sellers' compensation to the lender, the production
    /// cost, the new shares, tomorrow's reference price.
    fn settle_rights(&mut self, id: i64, day: &str, ts: &str, vc: Option<&ValueCtx>) -> Result<Flows, String> {
        let mut flows = Flows::default();
        let mut a = self.st.corp.actions[&id].clone();
        let c = a.company_id;
        let Some(co) = self.st.companies.get(&c).cloned() else { return Ok(flows) };
        if co.status != "listed" {
            self.cancel_rights(id, ts);
            return Ok(flows);
        }
        let world = self.special("world").ok_or("no world account")?;
        let acct = self.account_of("company", c).ok_or("no company account")?;
        let committee = self.account_of("committee", c);
        let (ratio, price) = (a.params.ratio, a.params.price);
        let ent = |q: i64| (q as f64 * ratio + 1e-9).floor() as i64;
        let holders: Vec<(i64, i64, String)> = self.holders_of(c).into_iter().filter(|h| h.0 != acct).collect();
        let total: i64 = holders.iter().map(|h| ent(h.1)).sum();
        let n = co.shares;
        let close = co.prev_close.max(1);
        let terp = ((close as f64 * n as f64 + price as f64 * total as f64) / (n + total).max(1) as f64).round() as i64;
        let premium = (terp - price).max(0);
        let mut subscribed = 0;
        let mut untaken: Vec<(i64, i64)> = Vec::new();
        for (h, q, kind) in &holders {
            let e = ent(*q);
            let take = if Some(*h) == committee {
                e
            } else if let Some(s) = self.st.corp.subs.get(&(id, *h)).cloned() {
                self.add_cash(*h, 0, -s.frozen);
                let t = s.qty.min(e);
                let mut s2 = s.clone();
                s2.allotted = Some(t);
                s2.frozen = 0;
                self.corp_change(CorporateChange::Sub(s2.clone()));
                self.st.corp.subs.insert((id, *h), s2);
                t
            } else {
                0
            };
            if take > 0 {
                if Some(*h) == committee {
                    // the rights holders fund their part from outside the economy
                    self.do_transfer(world, *h, take * price, "rights_funding", day, ts, Some(c), Some(id));
                    flows.injected += take * price;
                } else {
                    subscribed += take;
                }
                self.do_transfer(*h, acct, take * price, "rights", day, ts, Some(c), Some(id));
                self.do_add_shares(*h, c, take, price, day);
            }
            let _ = kind;
            if e > take {
                untaken.push((*h, e - take));
            }
        }
        // untaken shares: to the rights holders at the ex-rights price; the difference to the holders who lapsed
        let lapsed: i64 = untaken.iter().map(|u| u.1).sum();
        if lapsed > 0 {
            let to = committee.unwrap_or(world);
            self.do_transfer(world, acct, lapsed * price, "rights_funding", day, ts, Some(c), Some(id));
            flows.injected += lapsed * price;
            self.do_add_shares(to, c, lapsed, terp, day);
            // (the world account's stock — taken over in restructurings — lapses without a payment to itself)
            for (h, u) in untaken.iter().filter(|x| x.0 != world) {
                self.do_transfer(world, *h, u * premium, "rights_comp", day, ts, Some(c), Some(id));
                flows.injected += u * premium;
            }
        }
        // short sellers owe the lender the rights on the shares they borrowed
        let shorts: Vec<(i64, i64)> = self.st.margin.shorts.values().filter(|s| s.company_id == c).map(|s| (s.account_id, s.qty)).collect();
        for (s, q) in shorts {
            let owe = ent(q) * premium;
            if owe > 0 {
                let pay = owe.min(self.st.accounts[&s].cash - self.st.accounts[&s].cash_frozen).max(0);
                self.do_transfer(s, world, pay, "rights_short", day, ts, Some(c), Some(id));
                flows.exported += pay;
                if owe > pay {
                    self.margin_add_fee(s, c, owe - pay);
                }
            }
        }
        let issued = total;
        let cash = self.st.accounts[&acct].cash;
        let paid = a.params.budget.min(cash);
        self.do_transfer(acct, world, paid, "production_fee", day, ts, Some(c), Some(id));
        flows.exported += paid;
        let co2 = self.st.companies.get_mut(&c).unwrap();
        co2.shares += issued;
        co2.prev_close = terp;
        co2.last_price = terp;
        co2.open_ref = Some(terp);
        let title = co2.title.clone();
        self.mark_company(c);
        self.set_sequel_paid(a.params.work, Some(a.params.sequel), ts);
        if let Some(vc) = vc {
            if let Some(v) = self.company_value_full(vc, c) {
                self.st.companies.get_mut(&c).unwrap().fair_value = v;
            }
        }
        self.record_adjustment(c, &day_add(day, 1), terp as f64 / close as f64, 1.0, id);
        if let Some(v) = self.st.corp.closes.get_mut(&c) {
            for x in v.iter_mut() {
                *x = ((*x as f64) * terp as f64 / close as f64).round() as i64;
            }
        }
        a.status = "done".into();
        a.params.terp = terp;
        a.params.issued = issued;
        a.params.subscribed = subscribed;
        a.params.paid = paid;
        self.save_action(a);
        self.push_news(
            ts,
            day,
            "rights",
            Some(c),
            format!("《{title}》配股完成，除权参考价 {} 星辉", yuan(terp)),
            format!(
                "发行 {} 股，其中玩家认购 {} 股；未认购的 {} 股由制作委员会接手，每股差价 {} 星辉已补给未认购的持有人。制作费 {} 星辉已支付。",
                crate::pyfmt::int_grouped(issued),
                crate::pyfmt::int_grouped(subscribed),
                crate::pyfmt::int_grouped(lapsed),
                yuan(premium),
                grouped(paid)
            ),
        );
        Ok(flows)
    }

    fn cancel_rights(&mut self, id: i64, ts: &str) {
        let keys: Vec<(i64, i64)> = self.st.corp.subs.keys().filter(|k| k.0 == id).copied().collect();
        for k in keys {
            if let Some(s) = self.st.corp.subs.remove(&k) {
                if s.allotted.is_none() {
                    self.add_cash(k.1, 0, -s.frozen);
                }
                self.corp_change(CorporateChange::SubDelete(k.0, k.1));
            }
        }
        if let Some(mut a) = self.st.corp.actions.get(&id).cloned() {
            a.status = "cancelled".into();
            self.save_action(a);
        }
        let _ = ts;
    }

    /// A company leaves the market: its open actions end, rights subscriptions are refunded.
    pub(crate) fn corporate_on_delist(&mut self, company_id: i64, ts: &str) {
        let open: Vec<(i64, String)> = self
            .st
            .corp
            .actions
            .values()
            .filter(|a| a.company_id == company_id && (a.status == "pending" || a.status == "active"))
            .map(|a| (a.id, a.kind.clone()))
            .collect();
        for (id, kind) in open {
            if kind == "rights" {
                self.cancel_rights(id, ts);
            } else if let Some(mut a) = self.st.corp.actions.get(&id).cloned() {
                a.status = "cancelled".into();
                self.save_action(a);
            }
        }
    }

    // ------------------------------------------------------------ buybacks

    /// The expected production cost of the company's future sequels (the value its sequel option loses by counting
    /// seasons net of cost), cents: the cash management keeps for them (pecking order). 0 without the option.
    fn sequel_reserve(&self, c: i64, vc: &ValueCtx) -> i64 {
        if vc.fp.sequel_cost <= 0.0 {
            return 0;
        }
        let Some(net) = self.company_value_full(vc, c) else { return 0 };
        let mut gross_fp = vc.fp.clone();
        gross_fp.sequel_cost = 0.0;
        let g = ValueCtx { fp: &gross_fp, p: vc.p, totals: vc.totals, today: vc.today, rate: vc.rate, a_ref: vc.a_ref };
        let Some(gross) = self.company_value_full(&g, c) else { return 0 };
        let n = self.st.companies.get(&c).map_or(0, |co| self.outstanding(co));
        (gross - net).max(0) * n
    }

    fn maybe_buyback(&mut self, c: i64, day: &str, ts: &str, vc: &ValueCtx) {
        if self.open_action(c, "buyback").is_some() || self.open_action(c, "rights").is_some() || self.open_action(c, "split").is_some() {
            return;
        }
        let Some(co) = self.st.companies.get(&c).cloned().filter(|x| x.status == "listed") else { return };
        let Some(acct) = self.account_of("company", c) else { return };
        if co.prev_close <= 0 || co.prev_close >= co.fair_value || self.st.corp.under_days.get(&c).copied().unwrap_or(0) < self.corporate.buyback_under_days {
            return;
        }
        let cash = self.st.accounts[&acct].cash - self.st.accounts[&acct].cash_frozen;
        let idle = cash - self.sequel_reserve(c, vc);
        let mcap = co.prev_close * self.outstanding(&co);
        if (idle as f64) < self.corporate.buyback_min * mcap as f64 || idle <= 0 {
            return;
        }
        let end = day_add(day, self.corporate.buyback_days);
        // the published cap: 150% of the average close of the last 30 trading days (A-share announcements cap the price
        // against that average);
        // management also never pays more than its own estimate of the value (not published)
        let closes = self.st.corp.closes.get(&c).filter(|v| !v.is_empty());
        let avg = closes.map_or(co.prev_close, |v| v.iter().sum::<i64>() / v.len() as i64);
        let cap = avg * 3 / 2;
        self.new_action(c, "buyback", day, &end, "active", Params { budget: idle, cap, ..Default::default() });
        self.push_news(
            ts,
            day,
            "buyback",
            Some(c),
            format!("《{}》回购：不超过 {} 星辉", co.title, grouped(idle)),
            format!(
                "公司认为股价低于每股价值，用闲置现金在市场上回购股份并注销，价格不高于 {} 星辉，{}前完成。每天买入不超过过去 {} 个交易日日均成交量的 {}%，不在开盘后和收盘前 {} 分钟内买入。",
                yuan(cap),
                md(&end),
                self.corporate.buyback_adv_days,
                (self.corporate.buyback_adv * 100.0).round() as i64,
                self.corporate.buyback_quiet_min
            ),
        );
    }

    /// The buyback desk, every minute of continuous trading (`secs`: seconds since local midnight): each active
    /// programme bids at most once per `buyback_every_min` slot.
    pub fn corporate_minute(&mut self, day: &str, ts: &str, secs: i64) {
        if !self.corporate.enabled {
            return;
        }
        let quiet = self.corporate.buyback_quiet_min * 60;
        let (open, close) = (9 * 3600, 28 * 3600); // 09:00 to 04:00 the next morning
        let t = if secs < 5 * 3600 { secs + 24 * 3600 } else { secs };
        if t < open + quiet || t >= close - quiet {
            return;
        }
        let every = self.corporate.buyback_every_min.max(1) * 60;
        let slot = t / every;
        let left_slots = ((close - quiet - t) / every).max(1);
        let active: Vec<Action> = self.st.corp.actions.values().filter(|a| a.kind == "buyback" && a.status == "active").cloned().collect();
        for a in active {
            if self.st.corp.slot.get(&a.id) == Some(&slot) {
                continue;
            }
            self.st.corp.slot.insert(a.id, slot);
            let c = a.company_id;
            let Some(co) = self.st.companies.get(&c).cloned().filter(|x| x.status == "listed") else { continue };
            let Some(acct) = self.account_of("company", c) else { continue };
            self.cancel_company_orders(c, ts, Some(acct));
            let held = self.st.holdings.get(&(acct, c)).map_or(0, |h| h.qty);
            let today_cost: i64 = self.lots_of(acct, c).iter().map(|l| l.qty * l.cost).sum();
            let vols = self.st.corp.volumes.get(&c);
            let adv = vols.map_or(0.0, |v| if v.is_empty() { 0.0 } else { v.iter().sum::<i64>() as f64 / v.len() as f64 });
            let daily = (adv * self.corporate.buyback_adv).floor() as i64;
            let room = daily - held;
            if room <= 0 {
                continue;
            }
            let bid = self.book(c, 1).bids.first().map(|l| l.price).unwrap_or(co.last_price);
            let limit = bid.min(co.last_price).min(a.params.cap).min(co.fair_value);
            if limit <= 0 {
                continue;
            }
            let budget_left = a.params.budget - a.params.spent - today_cost;
            let by_cash = budget_left.min(self.available_cash(acct)) / self.fees.buy_reserve(limit, 1).max(1);
            let qty = ((room + left_slots - 1) / left_slots).min(room).min(by_cash);
            if qty <= 0 {
                continue;
            }
            let _ = self.open_and_place(&NewOrder {
                account_id: acct,
                company_id: c,
                side: "buy",
                kind: "limit",
                qty,
                price: Some(limit),
                day,
                ts,
                match_: true,
            });
        }
    }

    /// Company notices from the public schedule after a work's valuation step (docs/v2-corporate.md §2.2–2.3): a
    /// halt (AniList HIATUS) and its end, and the next episode moving later (延期). `before`: the work's state
    /// before the step.
    pub(crate) fn schedule_news(
        &mut self,
        c: &Company,
        s: &crate::inputs::ShowInputs,
        before: Option<&crate::vclose::JState>,
        cn_titles: &BTreeMap<i64, String>,
        day: &str,
        ts: &str,
    ) {
        if c.status != "listed" {
            return;
        }
        let (Some(old), Some(new)) = (before.and_then(|j| j.meta.clone()), self.valuation_state(s.id).and_then(|j| j.meta.clone())) else { return };
        let name = cn_titles.get(&s.id).filter(|t| !t.is_empty()).cloned().unwrap_or_else(|| s.title_ja.clone());
        let fmt = |d: i64| NaiveDate::from_num_days_from_ce_opt(d as i32).map(|x| x.format("%-m 月 %-d 日").to_string()).unwrap_or_default();
        match (old.hiatus, new.hiatus) {
            (None, Some(_)) => {
                self.push_news(ts, day, "hiatus", Some(c.id), format!("《{name}》暂停播出"), "制作方暂停了播出，剩余各集的播出时间未定。".into());
                return;
            }
            (Some(_), None) => {
                self.push_news(ts, day, "hiatus", Some(c.id), format!("《{name}》恢复播出"), "剩余各集按公开排期计入营收和估值。".into());
                return;
            }
            (Some(_), Some(_)) => return,
            _ => {}
        }
        // (the first day AniList's next episode is known — or the first close with this code — is not a change)
        if s.long_runner || new.air.is_empty() || (old.next_airing.is_none() && new.next_airing.is_some()) {
            return;
        }
        let Some(n) = s.episodes.filter(|n| *n > new.air.len() as i64) else { return };
        let (a, b) = (old.upcoming(&new.air, Some(n)), new.upcoming(&new.air, Some(n)));
        let (Some(&was), Some(&now)) = (a.first(), b.first()) else { return };
        if now > was {
            let ep = new.air.len() + 1;
            self.push_news(
                ts,
                day,
                "delay",
                Some(c.id),
                format!("《{name}》第 {ep} 集延期至 {}", fmt(now)),
                format!("原定 {}。之后各集顺延，营收随之推后。", fmt(was)),
            );
        }
    }
}

#[cfg(test)]
#[path = "corporate_tests.rs"]
mod tests;
