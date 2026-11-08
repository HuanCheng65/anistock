//! 指数基金（场外）(docs/design/funds.md): an open-ended fund that holds the stocks of one of the market indices
//! (`GET /api/v1/market/indices`, cap-weighted), bought with what players subscribe and sold for what they redeem.
//!
//! Step 1 of the doc's §9: the fund in the engine's memory, driven by the replay (`crate::sim`). The tables, the API
//! and persistence come with step 3; the state below is shaped so that saving it is mechanical (one row per fund,
//! per holder, per request, per day).
//!
//! A trading day of a fund:
//! - any time: `fund_subscribe` (cash moves to the fund at once, held apart from the fund's assets) and
//!   `fund_redeem` (units leave the holder at once); a request of trading day d is executed at the open of d+1;
//! - 08:50, `funds_auction`: the fund compares what it holds with its index weights (after the requests' net flow)
//!   and puts the difference into the opening auction as limit orders;
//! - 09:00, after the auction, `funds_confirm`: the net value at the opening prices, the requests' units / cash
//!   (their trading costs charged to them), the redemptions paid;
//! - every 30 minutes, `funds_batch`: what the auction left undone (cash not invested, redemptions not raised),
//!   in small limit orders near the last price;
//! - after the close, `funds_close`: the management fee, the day's net value.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{Engine, NewOrder};

pub mod store;

/// Units are counted in hundredths (1 unit = 100).
pub const UNIT: i64 = 100;
/// A fund starts at a net value of 1 星辉 per unit (cents per unit).
pub const NAV0: f64 = 100.0;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct FundRules {
    /// funds exist and take requests
    pub enabled: bool,
    /// the indices with a fund (docs/design/funds.md §3.1)
    pub keys: Vec<String>,
    /// management fee a year, accrued daily from the net value (goes to `world`)
    pub fee_rate: f64,
    /// the opening auction orders' limits: the last close ± this share
    pub auction_limit: f64,
    /// the batches' limits: buys at the last price × (1 + this), sells at × (1 − this); negative: resting
    /// orders inside the last price (passive, filled by the sellers / buyers that come), positive: crossing
    pub batch_limit: f64,
    /// how requests execute: "close" (traded in the batches of the next session, confirmed at its close) or
    /// "auction" (traded in the next opening auction, confirmed at the opening prices)
    pub exec: String,
    /// minutes between batches
    pub batch_every_min: i64,
    /// the batches run from / to these minutes after midnight (the night session counts on past 24:00)
    pub batch_from: i64,
    pub batch_to: i64,
    /// units held fewer than `redeem_fee_days` days are redeemed less this share (it stays in the fund)
    pub redeem_fee: f64,
    pub redeem_fee_days: i64,
    /// the smallest subscription, cents
    pub min_subscribe: i64,
    /// the funds together trade at most this share of a stock's average daily volume (the last 5 sessions; a stock
    /// without history: of its volume so far today) per session; what it cannot buy stays cash until the next session
    /// (the 建仓期 of a new fund, a large subscription)
    pub max_volume_share: f64,
    /// a stock is traded only when it is off its target by more than this share of the fund's assets (rounding,
    /// tick moves: no churn)
    pub drift: f64,
}

impl Default for FundRules {
    fn default() -> Self {
        FundRules {
            enabled: false,
            keys: ["all", "top20", "long", "manga", "light_novel", "isekai", "romance", "action"]
                .map(String::from)
                .to_vec(),
            fee_rate: 0.005,
            auction_limit: 0.03,
            batch_limit: -0.003,
            exec: "close".into(),
            batch_every_min: 30,
            batch_from: 9 * 60 + 30,
            batch_to: 23 * 60 + 30,
            drift: 0.0005,
            max_volume_share: 0.1,
            redeem_fee: 0.015,
            redeem_fee_days: 7,
            min_subscribe: 10_000,
        }
    }
}

impl FundRules {
    pub fn from_table(t: Option<&toml::Value>) -> Result<FundRules, String> {
        match t {
            None => Ok(FundRules::default()),
            Some(v) => v.clone().try_into().map_err(|e| format!("[funds]: {e}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReqKind {
    /// cents
    Subscribe(i64),
    /// hundredths of a unit
    Redeem(i64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: i64,
    pub account: i64,
    pub kind: ReqKind,
    /// the trading day it was submitted on: executed at the next day's open
    pub day: String,
    /// a redemption: how many of its units (hundredths) were held fewer than `redeem_fee_days` days
    pub young: i64,
    pub created_at: String,
    /// a redemption: the holder's lots it took (day, units), given back if it is cancelled
    pub taken: Vec<(String, i64)>,
}

/// One executed request (the replay's record; `fund_requests` rows later).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Confirmed {
    /// the request
    pub id: i64,
    pub account: i64,
    pub day: String,
    /// + subscribed cents / − redeemed units (hundredths)
    pub amount: i64,
    /// units issued (+) or redeemed (−), hundredths
    pub units: i64,
    /// cash in (+) or paid out (−), cents, after costs
    pub cash: i64,
    /// trading costs charged to the request, cents
    pub cost: i64,
    pub nav: f64,
}

/// What the fund did in one opening auction, per stock: the indicative price before and after its orders went in.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuctionMark {
    pub day: String,
    pub company: i64,
    /// + bought / − sold shares ordered
    pub qty: i64,
    pub reference: i64,
    pub before: Option<i64>,
    pub after: Option<i64>,
    /// the price the auction cleared at
    pub open: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NavDay {
    pub day: String,
    pub nav: f64,
    pub units: i64,
    /// net assets, cents
    pub aum: i64,
    pub cash: i64,
    /// fee charged at this close, cents
    pub fee: i64,
}

#[derive(Debug, Clone, Default)]
pub struct Fund {
    pub key: String,
    pub account: i64,
    /// hundredths
    pub units: i64,
    /// account -> hundredths
    pub holders: BTreeMap<i64, i64>,
    /// account -> its units by the day they were confirmed, oldest first (the redemption fee)
    pub lots: BTreeMap<i64, Vec<(String, i64)>>,
    pub pending: Vec<Request>,
    /// subscribed cash in the fund's account that is not the fund's yet
    pub pending_cash: i64,
    /// redemptions confirmed but not paid (the sells did not fill): account -> cents
    pub owed: BTreeMap<i64, i64>,
    pub nav: f64,
    pub history: Vec<NavDay>,
    pub confirmed: Vec<Confirmed>,
    pub marks: Vec<AuctionMark>,
    /// the requests executing today (from `funds_auction` to their confirmation)
    executing: Vec<Request>,
    /// trading costs (commission + stamp duty) the fund itself paid, cents (rebalancing, batches)
    pub own_costs: i64,
    /// the market impact charged to requests (+) or passed on to them (−), cents
    pub impact_charged: i64,
    pub fees_paid: i64,
}

#[derive(Debug, Clone, Default)]
pub struct FundsState {
    pub funds: BTreeMap<String, Fund>,
    pub last_request_id: i64,
    /// each stock's volume of the current session so far, and of the last 5 sessions (newest last)
    pub vol_today: BTreeMap<i64, i64>,
    pub vol_days: BTreeMap<i64, std::collections::VecDeque<i64>>,
    /// (fund account, stock) -> shares the fund traded this session
    pub fund_today: BTreeMap<(i64, i64), i64>,
    /// `Top` funds: the month they were chosen in (its first session) and the members
    pub top: BTreeMap<String, (String, std::collections::BTreeSet<i64>)>,
    /// the month's index float: the day it was taken (the month's first session, before trading: the float at the
    /// last close) and each listed company's free float then (crate::indices: the published indices use the same)
    pub floats: Option<(String, BTreeMap<i64, i64>)>,
    /// the funds' fills since their last confirmation: (fund account, stock, + bought / − sold shares, price)
    pub fills: Vec<(i64, i64, i64, i64)>,
    /// per fund account: commission and stamp duty of its fills since `funds_confirm` / the close took them
    pub costs: BTreeMap<i64, i64>,
    /// the session whose requests `funds_auction` started executing (once a session)
    pub auction_day: String,
}

impl Fund {
    /// The requests executing in this session (from `funds_auction` to their confirmation).
    pub fn executing(&self) -> &[Request] {
        &self.executing
    }

    /// Hundredths of a unit held by `account`.
    pub fn units_of(&self, account: i64) -> i64 {
        self.holders.get(&account).copied().unwrap_or(0)
    }
}

/// A fund's name (the index's short name) and its group on the fund list: 市场, 原作 (source), 题材 (theme).
pub fn fund_name(key: &str) -> (&'static str, &'static str) {
    match key {
        "all" => ("综合", "market"),
        "top20" => ("安易 20", "market"),
        "long" => ("长篇", "market"),
        "manga" => ("漫改", "source"),
        "light_novel" => ("轻改", "source"),
        "isekai" => ("异世界", "theme"),
        "romance" => ("恋爱", "theme"),
        "action" => ("热血", "theme"),
        _ => ("", "market"),
    }
}

/// Each AniList work's source and genres (plus the theme tags the indices use: Isekai at rank 60 or above, as
/// `engine/economy.py` `show_genres`), from its latest media snapshot in the collector database.
pub fn labels_from_collector(path: &std::path::Path) -> BTreeMap<i64, (Option<String>, Vec<String>)> {
    let mut out = BTreeMap::new();
    let Ok(c) = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return out;
    };
    let Ok(mut st) = c.prepare(
        "SELECT s.external_id, s.data FROM snapshots s WHERE s.source='anilist' AND s.kind='media'
         AND s.day = (SELECT MAX(day) FROM snapshots WHERE source='anilist' AND kind='media' AND external_id=s.external_id)",
    ) else {
        return out;
    };
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)));
    for (id, data) in rows.into_iter().flatten().flatten() {
        let (Ok(id), Ok(m)) = (id.parse::<i64>(), serde_json::from_str::<serde_json::Value>(&data)) else { continue };
        let mut g: Vec<String> =
            m["genres"].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(String::from)).collect();
        if m["tags"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| t["name"] == "Isekai" && t["rank"].as_i64().unwrap_or(0) >= 60)
        {
            g.push("Isekai".into());
        }
        out.insert(id, (m["source"].as_str().map(String::from), g));
    }
    out
}

/// Whether company `c` belongs to index `key` on trading day `day` (docs/api-v1.md §4.1.4: `all`, `long`; `season`
/// from `season_start`): listed, with a close before `day` (a new listing joins the day after its first session).
pub fn member(key: &str, c: &crate::model::Company, day: &str, season_from: &str) -> bool {
    if c.status != "listed" || c.listed_on.as_str() >= day {
        return false;
    }
    let Some(d) = crate::indices::def(key) else { return false };
    let l = crate::indices::Labels {
        long: c.long_runner != 0,
        first_air: &c.first_air,
        source: c.source.as_deref(),
        genres: &c.genres,
    };
    crate::indices::eligible(d.rule, &l, season_from)
}

/// The earliest premiere in the 本季新番 index on `day` (`crate::api::v1::market::season_from`).
pub fn season_from(day: &str) -> String {
    crate::api::v1::market::season_from(day)
}

impl Engine {
    /// Create the funds of `[funds] keys` that do not exist yet (an account of kind `fund` each).
    pub fn funds_setup(&mut self, ts: &str) {
        if !self.funds.enabled {
            return;
        }
        for key in self.funds.keys.clone() {
            if self.st.funds.funds.contains_key(&key) {
                continue;
            }
            let acct = self.create_account("fund", &format!("fund:{key}"), None, None, None, ts);
            self.st
                .funds
                .funds
                .insert(key.clone(), Fund { key: key.clone(), account: acct, nav: NAV0, ..Default::default() });
            self.fund_dirty(&key);
        }
        self.commit();
    }

    /// Companies without labels (none yet: the closes write them, V21) take them from the collector database now,
    /// so the theme and source funds have their members before the next close. Returns how many were labelled.
    pub fn labels_backfill(&mut self, collector_db: &std::path::Path) -> usize {
        let missing: Vec<i64> =
            self.st.companies.values().filter(|c| c.source.is_none() && c.genres.is_empty()).map(|c| c.id).collect();
        if missing.is_empty() {
            return 0;
        }
        let labels = labels_from_collector(collector_db);
        let mut n = 0;
        for id in missing {
            let Some((source, genres)) = labels.get(&id) else { continue };
            if source.is_none() && genres.is_empty() {
                continue;
            }
            let c = self.st.companies.get_mut(&id).unwrap();
            c.source = source.clone();
            c.genres = genres.join(",");
            self.mark_company(id);
            n += 1;
        }
        self.commit();
        n
    }

    /// A company's free float: its shares less those its production committee and the company itself hold (the
    /// market indices' weights, `api::v1::market::compute_indices`).
    pub fn free_float(&self, company: i64) -> i64 {
        let locked: i64 = ["committee", "company"]
            .iter()
            .filter_map(|k| self.account_of(k, company))
            .map(|a| self.holding(a, company).map_or(0, |h| h.qty))
            .sum();
        self.company(company).map_or(0, |c| (c.shares - locked).max(0))
    }

    /// A company's operating row of trading day `day` (its dividend per share).
    pub fn company_day(&self, company: i64, day: &str) -> Option<&crate::model::CompanyDay> {
        self.st.company_days.get(&(company, day.to_string()))
    }

    pub fn fund(&self, key: &str) -> Option<&Fund> {
        self.st.funds.funds.get(key)
    }

    pub fn funds_iter(&self) -> impl Iterator<Item = &Fund> {
        self.st.funds.funds.values()
    }

    /// What `account` has in the funds, cents: its units at the last net value, plus its requests not executed yet
    /// (subscribed cash; redeemed units at the last net value) and redemptions not paid yet.
    pub fn fund_value_of(&self, account: i64) -> i64 {
        let at = |u: i64, nav: f64| (u as f64 / UNIT as f64 * nav).round() as i64;
        self.st
            .funds
            .funds
            .values()
            .map(|f| {
                let pending: i64 = f
                    .pending
                    .iter()
                    .chain(&f.executing)
                    .filter(|r| r.account == account)
                    .map(|r| match r.kind {
                        ReqKind::Subscribe(a) => a,
                        ReqKind::Redeem(u) => at(u, f.nav),
                    })
                    .sum();
                at(f.units_of(account), f.nav) + pending + f.owed.get(&account).copied().unwrap_or(0)
            })
            .sum()
    }

    /// Whether `account` has anything in a fund: units, a request not confirmed, a redemption not paid.
    pub fn fund_involved(&self, account: i64) -> bool {
        self.st.funds.funds.values().any(|f| {
            f.units_of(account) > 0
                || f.pending.iter().chain(&f.executing).any(|r| r.account == account)
                || f.owed.contains_key(&account)
        })
    }

    /// Subscribe `amount` cents to fund `key`: the cash moves now, the units come at the next open.
    pub fn fund_subscribe(&mut self, account: i64, key: &str, amount: i64, day: &str, ts: &str) -> Result<i64, String> {
        let Some(f) = self.st.funds.funds.get(key) else { return Err("no such fund".into()) };
        let fund_acct = f.account;
        if amount < self.funds.min_subscribe {
            return Err("below the minimum subscription".into());
        }
        if self.available_cash(account) < amount {
            return Err("not enough cash".into());
        }
        self.transfer(account, fund_acct, amount, "fund_subscribe", day, ts, None, None);
        self.st.funds.last_request_id += 1;
        let id = self.st.funds.last_request_id;
        let f = self.st.funds.funds.get_mut(key).unwrap();
        f.pending_cash += amount;
        f.pending.push(Request {
            id,
            account,
            kind: ReqKind::Subscribe(amount),
            day: day.into(),
            young: 0,
            created_at: ts.into(),
            taken: vec![],
        });
        self.fund_dirty(key);
        self.commit();
        Ok(id)
    }

    /// Redeem `units` hundredths of fund `key`: they leave the holder now, the cash comes at the next open.
    pub fn fund_redeem(&mut self, account: i64, key: &str, units: i64, day: &str, ts: &str) -> Result<i64, String> {
        let Some(f) = self.st.funds.funds.get(key) else { return Err("no such fund".into()) };
        if units <= 0 || units > f.units_of(account) {
            return Err("not enough units".into());
        }
        self.st.funds.last_request_id += 1;
        let id = self.st.funds.last_request_id;
        let f = self.st.funds.funds.get_mut(key).unwrap();
        *f.holders.get_mut(&account).unwrap() -= units;
        // oldest units first; those confirmed fewer than redeem_fee_days days ago pay the redemption fee
        let since = crate::vclose::day_no(day).unwrap_or(0) - self.funds.redeem_fee_days;
        let lots = f.lots.entry(account).or_default();
        let (mut left, mut young) = (units, 0);
        let mut taken = Vec::new();
        while left > 0 && !lots.is_empty() {
            let take = left.min(lots[0].1);
            if crate::vclose::day_no(&lots[0].0).unwrap_or(0) > since {
                young += take;
            }
            taken.push((lots[0].0.clone(), take));
            lots[0].1 -= take;
            left -= take;
            if lots[0].1 == 0 {
                lots.remove(0);
            }
        }
        f.pending.push(Request {
            id,
            account,
            kind: ReqKind::Redeem(units),
            day: day.into(),
            young,
            created_at: ts.into(),
            taken,
        });
        self.fund_dirty(key);
        self.commit();
        Ok(id)
    }

    /// Withdraw a request before it starts executing (the next session's 08:50): a subscription's cash comes back,
    /// a redemption's units (and their lots) return to the holder.
    pub fn fund_cancel(&mut self, account: i64, request_id: i64, day: &str, ts: &str) -> Result<(), String> {
        let Some(key) = self
            .st
            .funds
            .funds
            .iter()
            .find(|(_, f)| f.pending.iter().any(|r| r.id == request_id && r.account == account))
            .map(|(k, _)| k.clone())
        else {
            return Err("no such request".into());
        };
        let f = self.st.funds.funds.get_mut(&key).unwrap();
        let i = f.pending.iter().position(|r| r.id == request_id).unwrap();
        let r = f.pending.remove(i);
        let fund_acct = f.account;
        match r.kind {
            ReqKind::Subscribe(a) => {
                f.pending_cash -= a;
                self.transfer(fund_acct, account, a, "fund_cancel", day, ts, None, None);
            }
            ReqKind::Redeem(u) => {
                *f.holders.entry(account).or_default() += u;
                let lots = f.lots.entry(account).or_default();
                lots.extend(r.taken.iter().cloned());
                lots.sort();
                let mut merged: Vec<(String, i64)> = Vec::new();
                for (d, n) in std::mem::take(lots) {
                    match merged.last_mut() {
                        Some(last) if last.0 == d => last.1 += n,
                        _ => merged.push((d, n)),
                    }
                }
                *lots = merged;
            }
        }
        self.out.push(crate::store::persist::Change::Funds(store::FundsChange::RequestDone {
            id: r.id,
            status: "cancelled".into(),
        }));
        self.fund_dirty(&key);
        self.commit();
        Ok(())
    }

    /// Mark a fund for saving at the end of the operation (`Engine::commit`).
    pub(crate) fn fund_dirty(&mut self, key: &str) {
        self.dirty_funds.insert(key.to_string());
    }

    /// The rows of the funds touched in this operation (called by `commit`).
    pub(crate) fn funds_rows(&mut self) {
        for key in std::mem::take(&mut self.dirty_funds) {
            if let Some(f) = self.st.funds.funds.get(&key) {
                self.out.push(crate::store::persist::Change::Funds(store::FundsChange::Fund(store::snapshot(f))));
            }
        }
    }

    /// The fund's holdings at `price(c)` plus its own cash (not the pending subscriptions, not what it owes), cents.
    fn fund_assets(&self, f: &Fund, price: impl Fn(&crate::model::Company) -> i64) -> i64 {
        let stock: i64 =
            self.holdings_of(f.account).map(|h| h.qty * self.st.companies.get(&h.company_id).map_or(0, &price)).sum();
        let cash = self.account(f.account).map_or(0, |a| a.cash);
        stock + cash - f.pending_cash - f.owed.values().sum::<i64>()
    }

    /// Target value per member stock for `total` cents of assets (cap-weighted at `price`).
    fn fund_targets(
        &self,
        key: &str,
        day: &str,
        total: i64,
        price: impl Fn(&crate::model::Company) -> i64,
    ) -> BTreeMap<i64, i64> {
        let qs = season_from(day);
        let Some(def) = crate::indices::def(key) else { return BTreeMap::new() };
        let mut values: Vec<(i64, f64)> = self
            .st
            .companies
            .values()
            .filter(|c| member(key, c, day, &qs))
            .map(|c| (c.id, price(c) as f64 * self.index_float(c.id)))
            .collect();
        if let crate::indices::Rule::Top(n) = def.rule {
            // the month's members; before the first review (a fund not traded yet), the largest now
            let chosen: std::collections::BTreeSet<i64> = match self.st.funds.top.get(key) {
                Some(x) => x.1.clone(),
                None => crate::indices::top(values.clone(), n).into_iter().collect(),
            };
            values.retain(|v| chosen.contains(&v.0));
        }
        crate::indices::weights(&values, def.cap)
            .into_iter()
            .map(|(id, w)| (id, (total.max(0) as f64 * w) as i64))
            .collect()
    }

    /// The index weights' free float of `company`: the month's (`floats`), in today's share terms (times the
    /// share-count changes since it was taken); without one, today's.
    pub fn index_float(&self, company: i64) -> f64 {
        if let Some((day, f)) = self.st.funds.floats.as_ref().and_then(|(d, m)| m.get(&company).map(|f| (d, *f))) {
            let q: f64 = self
                .price_adjustments()
                .iter()
                .filter(|a| a.company_id == company && a.day.as_str() > day.as_str())
                .map(|a| a.qty_factor)
                .product();
            return f as f64 * q;
        }
        self.free_float(company) as f64
    }

    /// The index `key`'s weights on `day` as its fund would hold them at the last close (prev_close): the fund's
    /// targets per unit of assets (the replay's benchmark).
    pub fn index_weights(&self, key: &str, day: &str) -> BTreeMap<i64, f64> {
        const UNIT_ASSETS: i64 = 1_000_000_000_000;
        self.fund_targets(key, day, UNIT_ASSETS, |c| c.prev_close)
            .into_iter()
            .map(|(c, v)| (c, v as f64 / UNIT_ASSETS as f64))
            .collect()
    }

    /// The first session of a month: the month's index float, and the `Top` funds' members for the month by
    /// free-float value at the last close (`crate::indices`: the published indices do the same from the same closes).
    fn funds_review(&mut self, day: &str) {
        if self.st.funds.floats.as_ref().is_none_or(|x| x.0.get(..7) != day.get(..7)) {
            let m = self
                .st
                .companies
                .values()
                .filter(|c| c.status == "listed")
                .map(|c| (c.id, self.free_float(c.id)))
                .collect();
            self.st.funds.floats = Some((day.to_string(), m));
            let value = serde_json::to_string(&self.st.funds.floats).unwrap_or_default();
            self.out
                .push(crate::store::persist::Change::Funds(store::FundsChange::Mark { key: "floats".into(), value }));
        }
        for key in self.st.funds.funds.keys().cloned().collect::<Vec<_>>() {
            let Some(crate::indices::Rule::Top(n)) = crate::indices::def(&key).map(|d| d.rule) else { continue };
            if self.st.funds.top.get(&key).is_some_and(|x| x.0.get(..7) == day.get(..7)) {
                continue;
            }
            let values: Vec<(i64, f64)> = self
                .st
                .companies
                .values()
                .filter(|c| c.status == "listed" && c.listed_on.as_str() < day && c.prev_close > 0)
                .map(|c| (c.id, c.prev_close as f64 * self.index_float(c.id)))
                .collect();
            let chosen = crate::indices::top(values, n).into_iter().collect();
            let x = (day.to_string(), chosen);
            let value = serde_json::to_string(&x).unwrap_or_default();
            self.out.push(crate::store::persist::Change::Funds(store::FundsChange::Mark {
                key: format!("top:{key}"),
                value,
            }));
            self.st.funds.top.insert(key, x);
        }
    }

    /// The requests a day executes and the cash / units they bring: (subscribed cents net of their estimated
    /// commission, redeemed units at the last net value in cents).
    fn fund_flow(&self, f: &Fund) -> (i64, i64) {
        let subs: i64 = f.executing.iter().map(|r| if let ReqKind::Subscribe(a) = r.kind { a } else { 0 }).sum();
        let reds: i64 = f.executing.iter().map(|r| if let ReqKind::Redeem(u) = r.kind { u } else { 0 }).sum();
        (subs - self.fees.commission_on(subs), (reds as f64 / UNIT as f64 * f.nav).round() as i64)
    }

    /// Orders that move the fund's holdings `share` of the way to its targets for `total` cents of assets at
    /// `price`, limits `limit` away from it: (company, signed qty, limit). Buys are bounded by `cash`.
    fn fund_orders(
        &self,
        key: &str,
        day: &str,
        total: i64,
        share: f64,
        limit: f64,
        mut cash: i64,
        price: impl Fn(&crate::model::Company) -> i64 + Copy,
    ) -> Vec<(i64, i64, i64)> {
        let acct = self.st.funds.funds[key].account;
        let targets = self.fund_targets(key, day, total, price);
        let held: BTreeMap<i64, i64> = self.holdings_of(acct).map(|h| (h.company_id, h.qty)).collect();
        let tol = (total as f64 * self.funds.drift) as i64;
        let mut ids: Vec<i64> = targets.keys().chain(held.keys()).copied().collect();
        ids.sort_unstable();
        ids.dedup();
        let mut orders = Vec::new();
        for c in ids {
            let co = &self.st.companies[&c];
            let p = price(co);
            if co.status != "listed" || p <= 0 {
                continue;
            }
            let have = held.get(&c).copied().unwrap_or(0);
            let diff = targets.get(&c).copied().unwrap_or(0) - have * p;
            if diff.abs() <= tol.max(p) {
                continue;
            }
            let step = ((diff as f64) * share).round() as i64;
            let room = self.fund_room(acct, c);
            if step > 0 {
                let lim = (((p as f64) * (1.0 + limit)).round() as i64 + if limit > 0.0 { 1 } else { 0 }).max(1);
                let q = (step.max(p).min(cash.max(0)) / self.fees.buy_reserve(lim, 1).max(1)).min(room);
                if q > 0 {
                    cash -= self.fees.buy_reserve(lim, q);
                    orders.push((c, q, lim));
                }
            } else if step < 0 {
                let lim = (((p as f64) * (1.0 - limit)).round() as i64 - if limit > 0.0 { 1 } else { 0 }).max(1);
                let q = ((-step).max(p) / p).min(self.available_shares(acct, c)).min(room);
                if q > 0 {
                    orders.push((c, -q, lim));
                }
            }
        }
        orders
    }

    /// Shares fund account `acct` may still trade in `company` this session (`max_volume_share`).
    fn fund_room(&self, acct: i64, company: i64) -> i64 {
        let fs = &self.st.funds;
        let hist = fs.vol_days.get(&company).filter(|d| !d.is_empty());
        let adv = match hist {
            Some(d) => d.iter().sum::<i64>() as f64 / d.len() as f64,
            None => fs.vol_today.get(&company).copied().unwrap_or(0) as f64,
        };
        // every fund together: what they traded today and what they have resting
        let _ = acct;
        let funds: Vec<i64> = fs.funds.values().map(|f| f.account).collect();
        let traded: i64 = funds.iter().map(|a| fs.fund_today.get(&(*a, company)).copied().unwrap_or(0)).sum();
        let resting: i64 = funds
            .iter()
            .flat_map(|a| self.open_orders_of(*a))
            .filter(|o| o.company_id == company)
            .map(|o| o.qty - o.filled)
            .sum();
        ((adv * self.funds.max_volume_share) as i64 - traded - resting).max(0)
    }

    /// A fill (called by the engine): the session's volume per stock, and the funds' part of it.
    pub(crate) fn funds_note_fill(&mut self, company: i64, qty: i64, price: i64, buyer: i64, seller: i64) {
        if !self.funds.enabled {
            return;
        }
        *self.st.funds.vol_today.entry(company).or_default() += qty;
        for (a, sign) in [(buyer, 1), (seller, -1)] {
            if self.st.accounts.get(&a).is_some_and(|x| x.kind == "fund") {
                *self.st.funds.fund_today.entry((a, company)).or_default() += qty;
                self.st.funds.fills.push((a, company, sign * qty, price));
            }
        }
    }

    fn fund_cancel_all(&mut self, acct: i64, ts: &str) {
        for o in self.open_orders_of(acct).iter().map(|o| o.id).collect::<Vec<_>>() {
            let _ = self.cancel_order(acct, o, ts);
        }
    }

    /// 08:50: the requests of the days before `day` start executing. `exec = "auction"`: the difference between
    /// what each fund holds and its targets goes into the opening auction; `"close"`: the batches trade it during
    /// the session and the requests are confirmed at the close.
    pub fn funds_auction(&mut self, day: &str, ts: &str) {
        if !self.funds.enabled {
            return;
        }
        if self.st.funds.auction_day.as_str() >= day {
            return;
        }
        self.st.funds.auction_day = day.to_string();
        self.out.push(crate::store::persist::Change::Funds(store::FundsChange::Mark {
            key: "auction_day".into(),
            value: day.to_string(),
        }));
        let rules = self.funds.clone();
        self.funds_review(day);
        for key in self.st.funds.funds.keys().cloned().collect::<Vec<_>>() {
            let f = self.st.funds.funds.get_mut(&key).unwrap();
            let (now, later): (Vec<Request>, Vec<Request>) = f.pending.drain(..).partition(|r| r.day.as_str() < day);
            f.pending = later;
            // (requests a missed close left executing stay: they confirm at the next close)
            f.executing.extend(now);
            let acct = f.account;
            self.fund_dirty(&key);
            self.fund_cancel_all(acct, ts);
            if rules.exec != "auction" {
                continue;
            }
            let f = &self.st.funds.funds[&key];
            let reference = |c: &crate::model::Company| c.prev_close;
            let (invest, red_cash) = self.fund_flow(f);
            let total = self.fund_assets(f, reference) + invest - red_cash;
            // the sells' cash is not there before the auction: the buys are bounded by the cash on hand
            let cash = self.available_cash(acct) - (f.pending_cash - invest - self.fees.commission_on(invest)).max(0);
            let orders = self.fund_orders(&key, day, total, 1.0, rules.auction_limit, cash, reference);
            let mut marks = Vec::new();
            for (c, q, lim) in orders {
                let reference = self.st.companies[&c].prev_close;
                let before = self.auction_price(c, reference).0;
                let r = self.place_order(&NewOrder {
                    account_id: acct,
                    company_id: c,
                    side: if q > 0 { "buy" } else { "sell" },
                    kind: "limit",
                    qty: q.abs(),
                    price: Some(lim),
                    day,
                    ts,
                    match_: false,
                });
                if r.is_ok() {
                    let after = self.auction_price(c, reference).0;
                    marks.push(AuctionMark {
                        day: day.into(),
                        company: c,
                        qty: q,
                        reference,
                        before,
                        after,
                        open: None,
                    });
                }
            }
            self.st.funds.funds.get_mut(&key).unwrap().marks.extend(marks);
        }
        self.commit();
    }

    /// 09:00 (`exec = "auction"`), after the auction (`open_market` has run): the requests confirmed at the
    /// opening prices.
    pub fn funds_confirm(&mut self, day: &str, ts: &str) {
        if !self.funds.enabled || self.funds.exec != "auction" {
            return;
        }
        for key in self.st.funds.funds.keys().cloned().collect::<Vec<_>>() {
            self.fund_confirm(&key, day, ts, |c| c.open_ref.unwrap_or(c.prev_close));
            let f = &self.st.funds.funds[&key];
            let opens: Vec<(usize, i64)> = f
                .marks
                .iter()
                .enumerate()
                .filter(|(_, m)| m.day == day && m.open.is_none())
                .filter_map(|(i, m)| self.st.companies.get(&m.company).and_then(|c| c.open_ref).map(|o| (i, o)))
                .collect();
            let f = self.st.funds.funds.get_mut(&key).unwrap();
            for (i, o) in opens {
                f.marks[i].open = Some(o);
            }
        }
        self.commit();
    }

    /// Confirm the executing requests at `price`: the net value of the units already there, units issued,
    /// redemptions owed and paid from cash. What the day's trading cost (commission, stamp duty, and the fills' prices
    /// against `price`: the market impact) is charged to the requests in the share of the trading their net flow
    /// accounts for, split by their size (反稀释: the units already there do not pay for others coming and going;
    /// a gain is passed on the same way).
    fn fund_confirm(&mut self, key: &str, day: &str, ts: &str, price: impl Fn(&crate::model::Company) -> i64) {
        let acct = self.st.funds.funds[key].account;
        let fees = self.st.funds.costs.remove(&acct).unwrap_or(0);
        let (mine, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.st.funds.fills).into_iter().partition(|x| x.0 == acct);
        self.st.funds.fills = rest;
        let mut impact = 0i64;
        let mut traded = 0i64;
        for &(_, c, q, px) in &mine {
            let at = self.st.companies.get(&c).map_or(px, &price);
            impact += q * (px - at);
            traded += q.abs() * px;
        }
        let f = &self.st.funds.funds[key];
        let flows = !f.executing.is_empty();
        let net: i64 = f
            .executing
            .iter()
            .map(|r| match r.kind {
                ReqKind::Subscribe(a) => a,
                ReqKind::Redeem(u) => -((u as f64 / UNIT as f64 * f.nav).round() as i64),
            })
            .sum();
        let share = if !flows {
            0.0
        } else if traded > 0 {
            (net.abs() as f64 / traded as f64).min(1.0)
        } else {
            1.0
        };
        let costs = ((fees + impact) as f64 * share).round() as i64;
        let old = self.fund_assets(f, price) + costs;
        let nav = if f.units > 0 { old as f64 / f.units as f64 * UNIT as f64 } else { NAV0 };
        let gross_of: Vec<i64> = f
            .executing
            .iter()
            .map(|r| match r.kind {
                ReqKind::Subscribe(a) => a,
                ReqKind::Redeem(u) => (u as f64 / UNIT as f64 * nav).round() as i64,
            })
            .collect();
        let gross: i64 = gross_of.iter().sum::<i64>().max(1);
        let subs: i64 = f.executing.iter().map(|r| if let ReqKind::Subscribe(a) = r.kind { a } else { 0 }).sum();
        let mut out = Vec::new();
        let mut new_units = 0i64;
        for (r, g) in f.executing.iter().zip(&gross_of) {
            let cost = (costs as i128 * *g as i128 / gross as i128) as i64;
            match r.kind {
                ReqKind::Subscribe(a) => {
                    let u = (((a - cost) as f64 / nav) * UNIT as f64).floor() as i64;
                    new_units += u;
                    out.push(Confirmed {
                        id: r.id,
                        account: r.account,
                        day: day.into(),
                        amount: a,
                        units: u,
                        cash: a - cost,
                        cost,
                        nav,
                    });
                }
                ReqKind::Redeem(u) => {
                    new_units -= u;
                    // the redemption fee stays in the fund (its other holders')
                    let fee = (*g as f64 * r.young as f64 / u.max(1) as f64 * self.funds.redeem_fee).round() as i64;
                    out.push(Confirmed {
                        id: r.id,
                        account: r.account,
                        day: day.into(),
                        amount: -u,
                        units: -u,
                        cash: -(g - cost - fee),
                        cost,
                        nav,
                    });
                }
            }
        }
        let f = self.st.funds.funds.get_mut(key).unwrap();
        f.pending_cash -= subs;
        f.units += new_units;
        for c in &out {
            if c.units > 0 {
                *f.holders.entry(c.account).or_default() += c.units;
                f.lots.entry(c.account).or_default().push((day.to_string(), c.units));
            } else {
                *f.owed.entry(c.account).or_default() += -c.cash;
            }
        }
        f.own_costs += fees - ((fees as f64) * share).round() as i64;
        f.impact_charged += costs - ((fees as f64) * share).round() as i64;
        f.nav = nav;
        for c in &out {
            self.out.push(crate::store::persist::Change::Funds(store::FundsChange::Confirmed {
                fund: key.to_string(),
                c: c.clone(),
            }));
        }
        let f = self.st.funds.funds.get_mut(key).unwrap();
        f.confirmed.extend(out);
        f.executing.clear();
        self.fund_dirty(key);
        self.fund_pay_owed(key, day, ts);
    }

    /// Pay what the fund owes redeeming holders from the cash it has (beyond the pending subscriptions).
    fn fund_pay_owed(&mut self, key: &str, day: &str, ts: &str) {
        let f = &self.st.funds.funds[key];
        let acct = f.account;
        let mut free = self.account(acct).map_or(0, |a| a.cash - a.cash_frozen) - f.pending_cash;
        let mut pays = Vec::new();
        for (&a, &c) in &f.owed {
            let x = c.min(free.max(0));
            if x > 0 {
                pays.push((a, x));
                free -= x;
            }
        }
        for (a, x) in pays {
            self.transfer(acct, a, x, "fund_redeem", day, ts, None, None);
            self.fund_dirty(key);
            let f = self.st.funds.funds.get_mut(key).unwrap();
            let left = f.owed.get_mut(&a).unwrap();
            *left -= x;
            if *left == 0 {
                f.owed.remove(&a);
            }
        }
    }

    /// Every `batch_every_min` minutes from `batch_from` to `batch_to` (minutes after midnight; the night session
    /// counts on past 24:00): orders near the last price that move the holdings toward the targets, the remaining
    /// difference spread evenly over the batches left (`exec = "close"`: the day's requests included).
    pub fn funds_batch(&mut self, day: &str, ts: &str, minute: i64) {
        if !self.funds.enabled || minute < self.funds.batch_from || minute > self.funds.batch_to {
            return;
        }
        // (not before the session's requests started executing)
        if self.st.funds.auction_day != day {
            return;
        }
        let rules = self.funds.clone();
        let left = ((rules.batch_to - minute) / rules.batch_every_min + 1).max(1);
        for key in self.st.funds.funds.keys().cloned().collect::<Vec<_>>() {
            self.fund_pay_owed(&key, day, ts);
            let acct = self.st.funds.funds[&key].account;
            self.fund_cancel_all(acct, ts);
            let f = &self.st.funds.funds[&key];
            let last = |c: &crate::model::Company| c.last_price;
            let (invest, red_cash) = if rules.exec == "auction" { (0, 0) } else { self.fund_flow(f) };
            // (a fund redeemed in full has a target of nothing: it sells everything)
            let total = (self.fund_assets(f, last) + invest - red_cash).max(0);
            // the cash the fund may spend: its own, and the executing subscriptions' (not the later ones')
            let later: i64 = f.pending.iter().map(|r| if let ReqKind::Subscribe(a) = r.kind { a } else { 0 }).sum();
            let cash = self.available_cash(acct) - later - f.owed.values().sum::<i64>();
            let orders = self.fund_orders(&key, day, total, 1.0 / left as f64, rules.batch_limit, cash, last);
            for (c, q, lim) in orders {
                let _ = self.place_order(&NewOrder {
                    account_id: acct,
                    company_id: c,
                    side: if q > 0 { "buy" } else { "sell" },
                    kind: "limit",
                    qty: q.abs(),
                    price: Some(lim),
                    day,
                    ts,
                    match_: true,
                });
            }
        }
        self.commit();
    }

    /// After the daily close: (`exec = "close"`) the day's requests confirmed at the closing prices; the
    /// management fee (to `world`) and the day's net value.
    pub fn funds_close(&mut self, day: &str, ts: &str) {
        if !self.funds.enabled {
            return;
        }
        let world = self.special("world");
        for key in self.st.funds.funds.keys().cloned().collect::<Vec<_>>() {
            let acct = self.st.funds.funds[&key].account;
            self.fund_cancel_all(acct, ts);
            if self.funds.exec != "auction" {
                self.fund_confirm(&key, day, ts, |c| c.last_price);
            }
            let f = &self.st.funds.funds[&key];
            let assets = self.fund_assets(f, |c| c.last_price);
            let fee = ((assets.max(0) as f64) * self.funds.fee_rate / 365.0).round() as i64;
            let free = self.account(acct).map_or(0, |a| a.cash - a.cash_frozen)
                - f.pending_cash
                - f.owed.values().sum::<i64>();
            let fee = fee.min(free.max(0));
            if let (Some(w), true) = (world, fee > 0) {
                self.transfer(acct, w, fee, "fund_fee", day, ts, None, None);
            }
            let f = &self.st.funds.funds[&key];
            let assets = self.fund_assets(f, |c| c.last_price);
            let cash = self.account(acct).map_or(0, |a| a.cash) - f.pending_cash;
            let units = f.units;
            let nav = if units > 0 { assets as f64 / units as f64 * UNIT as f64 } else { f.nav };
            let rest = self.st.funds.costs.remove(&acct).unwrap_or(0);
            let f = self.st.funds.funds.get_mut(&key).unwrap();
            f.own_costs += rest;
            f.nav = nav;
            f.fees_paid += fee;
            let d = NavDay { day: day.into(), nav, units, aum: assets, cash, fee };
            f.history.push(d.clone());
            self.out.push(crate::store::persist::Change::Funds(store::FundsChange::Nav { fund: key.clone(), d }));
            self.fund_dirty(&key);
        }
        // the session's volumes into the 5-session history (a stock that did not trade: a zero session)
        let listed: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let fs = &mut self.st.funds;
        let today = std::mem::take(&mut fs.vol_today);
        fs.vol_days.retain(|c, _| listed.contains(c));
        for c in listed {
            let v = today.get(&c).copied().unwrap_or(0);
            let d = fs.vol_days.entry(c).or_default();
            d.push_back(v);
            if d.len() > 5 {
                d.pop_front();
            }
        }
        fs.fund_today.clear();
        fs.fills.clear();
        self.commit();
    }

    /// Record a fill's commission and stamp duty against a fund's account (called by the fill).
    pub(crate) fn funds_note_cost(&mut self, account: i64, cost: i64) {
        if self.st.accounts.get(&account).is_some_and(|a| a.kind == "fund") {
            *self.st.funds.costs.entry(account).or_default() += cost;
        }
    }
}

#[cfg(test)]
mod tests;
