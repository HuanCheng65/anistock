//! The daily close (port of `server/daily.py` `run_day` and everything it calls, plus `app.catch_up`).
//!
//! For day D: revenue / operating cost / corporate tax, Sunday dividends (lot-age tax), transfers and the
//! rate decision, interest, IPO production fees, fair values and delistings, order expiry, candles and the
//! next day's reference price, title refresh, IPO allotment (clawback + ballot), new subscriptions, the
//! macro row, invariants. Idempotent per day via state `last_closed`. The whole close is one engine
//! operation: on any error the in-memory state is restored and nothing is written (v1's transaction).
//!
//! v1 reads several tables with SQL inside the close; the engine keeps exactly those in memory
//! (company_days, macro_days, ipo_applications, per-day trade aggregates, gov's tax receipts). Iteration
//! orders follow v1's query plans: companies by id; holders of a company in holdings rowid order
//! (`SCAN h`); interest recipients by kind (mm, noise, player) then id (`accounts_kind` IN-list lookup).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use serde::Serialize;

use crate::config::{Config, GameRules};
use crate::engine::{Engine, State};
use crate::inputs::{Inputs, ShowInputs};
use crate::model::*;
use crate::pyfmt::{fixed, fixed_grouped, int_grouped, pct, repr, round_to};
use crate::pyrandom::PyRandom;
use crate::store::persist::Change;
use crate::valuation::{self as V, Params};

/// 面值退市: closing under this many cents ...
pub const PENNY_CENTS: i64 = 100;
/// ... for this many trading days in a row (warning at `PENNY_WARN_DAYS`)
pub const PENNY_DELIST_DAYS: i64 = 20;
pub const PENNY_WARN_DAYS: i64 = 10;

/// Accounts in the money stock (M2). `npc` = the v2 NPC agents (households).
pub const MARKET_KINDS: [&str; 6] = ["player", "mm", "noise", "committee", "company", "npc"];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    pub day: String,
    pub injected: i64,
    pub exported: i64,
    pub taxes: i64,
    pub interest: i64,
    pub rate: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum CloseOutcome {
    /// `last_closed` is already this day or later
    Skipped,
    Closed(Summary),
}

fn date(s: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|e| format!("bad date {s:?}: {e}"))
}

fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Python `round(x)` -> int
fn pround(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Python `a // b` for ints
fn floordiv(a: i64, b: i64) -> i64 {
    a.div_euclid(b) - if b < 0 && a.rem_euclid(b) != 0 { 1 } else { 0 }
}

/// Python `max(a, b)` / `min(a, b)` for floats (first argument wins ties)
fn pmax(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}
fn pmin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

/// Holding-period tax rate on dividends for a lot acquired on `acquired_on` (v1 `holding_tax_rate`).
pub fn holding_tax_rate(game: &GameRules, acquired_on: &str, day: &str) -> Result<f64, String> {
    let days = (date(day)? - date(acquired_on)?).num_days() as f64;
    for (max_days, r) in &game.dividend_tax {
        if days <= *max_days {
            return Ok(*r);
        }
    }
    Ok(0.0)
}

/// Share of the float that goes to the public after clawback (回拨).
pub fn public_share(game: &GameRules, oversubscribed: f64) -> f64 {
    let mut frac = game.ipo_public_base;
    for (multiple, f) in &game.ipo_clawback {
        if oversubscribed >= *multiple {
            frac = *f;
        }
    }
    frac
}

struct Ctx<'a> {
    cfg: &'a Config,
    p: Params,
    day: String,
    d: NaiveDate,
    ts: String,
    inp: &'a Inputs,
    /// listed companies at the start of the close (v1's `companies` list of dicts)
    companies: Vec<Company>,
    /// shows of those companies, in that order
    listed: Vec<&'a ShowInputs>,
    world: i64,
    cb: i64,
    gov: i64,
    sum: Summary,
    /// the filtered valuation's market totals from this close (for IPO pricing; v2 economy)
    vtotals: Option<[Vec<f64>; 3]>,
}

impl Ctx<'_> {
    fn g(&self) -> &GameRules {
        &self.cfg.game
    }
    fn listed_expected(&self) -> Vec<V::M3> {
        self.listed.iter().map(|s| s.expected.m3()).collect()
    }
    fn expected_share(&self, s: &ShowInputs) -> f64 {
        let own_listed = self.listed.iter().any(|x| x.id == s.id);
        let exp = self.listed_expected();
        let refs: Vec<&V::M3> = exp.iter().collect();
        V::expected_share(&s.expected.m3(), own_listed, &refs, &self.p.market_weights)
    }
}

impl Engine {
    /// First account (by id) of `kind` belonging to `company_id` (v1 `account_of`).
    pub fn account_of(&self, kind: &str, company_id: i64) -> Option<i64> {
        self.st.accounts.values().find(|a| a.kind == kind && a.company_id == Some(company_id)).map(|a| a.id)
    }

    fn cash_of(&self, id: i64) -> i64 {
        self.st.accounts.get(&id).map_or(0, |a| a.cash)
    }

    fn m2(&self) -> i64 {
        self.st.accounts.values().filter(|a| MARKET_KINDS.contains(&a.kind.as_str())).map(|a| a.cash).sum()
    }

    /// Holders of a company with qty > 0 whose account exists, in holdings rowid order:
    /// (account_id, qty, kind).
    fn holders(&self, company_id: i64) -> Vec<(i64, i64, String)> {
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

    /// Close `day` with the model's `inputs` (v1 `daily.run_day`). `ts` stamps every row written.
    pub fn run_day(&mut self, cfg: &Config, day: &str, ts: &str, inputs: &Inputs) -> Result<CloseOutcome, String> {
        if let Some(lc) = self.state("last_closed") {
            if !lc.is_empty() && lc >= day {
                return Ok(CloseOutcome::Skipped);
            }
        }
        if inputs.day != day {
            return Err(format!("inputs are for {}, not {day}", inputs.day));
        }
        let p = Params::from_config(cfg)?;
        let special = |k: &str| self.special(k).ok_or_else(|| format!("no {k} account"));
        let (world, cb, gov) = (special("world")?, special("cb")?, special("gov")?);
        let rate: f64 = self.state("rate").and_then(crate::pyfmt::parse_float).ok_or("state 'rate' missing")?;
        let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).ok_or("state 'a_ref' missing")?;
        self.commit();
        let saved: State = self.st.clone();
        let companies: Vec<Company> = self.st.companies.values().filter(|c| c.status == "listed").cloned().collect();
        // every work of the listed companies (a company is a series: docs/v2-series.md §3)
        let listed: Vec<&ShowInputs> = companies.iter().flat_map(|c| self.works_of(c.id)).filter_map(|w| inputs.show(w)).collect();
        let mut cx = Ctx {
            cfg,
            p,
            day: day.to_string(),
            d: date(day)?,
            ts: ts.to_string(),
            inp: inputs,
            companies,
            listed,
            world,
            cb,
            gov,
            sum: Summary { day: day.to_string(), injected: 0, exported: 0, taxes: 0, interest: 0, rate },
            vtotals: None,
        };
        match self.close_inner(&mut cx, rate, a_ref) {
            Ok(rate) => {
                cx.sum.rate = rate;
                self.prune_after_close(day);
                self.commit();
                Ok(CloseOutcome::Closed(cx.sum))
            }
            Err(e) => {
                self.rollback(saved);
                Err(e)
            }
        }
    }

    fn close_inner(&mut self, cx: &mut Ctx, mut rate: f64, a_ref: f64) -> Result<f64, String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        let p = cx.p.clone();
        let d = cx.d;
        // 融资融券: open credit orders end before the close (crate::margin)
        self.margin_pre_close(&ts);
        // buybacks: orders end, today's repurchased shares are cancelled (crate::corporate)
        if !cx.cfg.economy_v1 {
            self.corporate_pre_close(&ts);
        }

        // 0. adult titles are not traded here: buy out any listed one at today's last price and close
        //    any open subscription with full refunds
        let adult: Vec<(Company, &'static str)> = cx
            .companies
            .iter()
            .filter_map(|c| cx.inp.show(c.id).and_then(|s| s.excluded()).map(|why| (c.clone(), why)))
            .collect();
        for (c, why) in &adult {
            self.buyout(cx, c, why)?;
        }
        if !adult.is_empty() {
            cx.companies.retain(|c| !adult.iter().any(|(a, _)| a.id == c.id));
            cx.listed.retain(|s| s.excluded().is_none());
        }
        let subs: Vec<Company> = self
            .st
            .companies
            .values()
            .filter(|c| c.status == "subscribing" && cx.inp.show(c.id).is_some_and(|s| s.excluded().is_some()))
            .cloned()
            .collect();
        for c in subs {
            let why = cx.inp.show(c.id).and_then(|s| s.excluded()).unwrap_or("该作品不在安易交易");
            let apps: Vec<i64> = self.ipo_applications(c.id).iter().map(|a| a.account_id).collect();
            for a in apps {
                self.do_withdraw(c.id, a);
            }
            let co = self.st.companies.get_mut(&c.id).unwrap();
            co.status = "delisted".into();
            co.delisted_on = Some(day.clone());
            self.mark_company(c.id);
            self.push_news(&ts, &day, "delisting", Some(c.id), format!("《{}》取消发行", c.title), format!("{why}，认购资金已全部退回。"));
        }

        // 1. revenue
        // (v2: post-finale cn/jp days without data earn the filter's expectation, `revenue_att`)
        let att_of = |e: &Engine, s: &ShowInputs| if cx.cfg.economy_v1 || !p.impute_tail { s.att.m3() } else { e.revenue_att(s) };
        let mut totals = [0.0f64; 3];
        for m in 0..3 {
            let mut t = 0.0;
            for s in &cx.listed {
                t += att_of(self, s)[m];
            }
            totals[m] = t;
        }
        let tot_us = totals[0];
        let demand_day = V::demand_day(tot_us, a_ref, &p);
        // the absolute revenue rule (docs/v2-series.md §6), once the platform size averages enough history
        let abs = if cx.cfg.economy_v1 { None } else { self.platform_switch(cx, &p, &totals, demand_day) };
        if let Ok(path) = std::env::var("SIM_FORECAST") {
            // the realised side (sim/forecast_check.py): totals, demand, each listed show's attention and revenue
            use std::io::Write;
            let shows: Vec<serde_json::Value> = cx
                .companies
                .iter()
                .filter_map(|c| cx.inp.show(c.id).map(|s| serde_json::json!([c.id, s.att.m3(), V::blended_share(&s.att.m3(), &totals, &p.market_weights).0])))
                .collect();
            let line = serde_json::json!({"real": crate::vclose::day_no(&day), "tot": totals, "dd": demand_day, "shows": shows}).to_string();
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(f, "{line}");
            }
        }
        for c in cx.companies.clone() {
            let Some(s) = cx.inp.show(c.id) else { continue };
            // the company's revenue sums its works'
            let works: Vec<&ShowInputs> = self.works_of(c.id).into_iter().filter_map(|w| cx.inp.show(w)).collect();
            let (rev, split): (i64, Vec<i64>) = match &abs {
                Some((pl, sc)) => {
                    let mut parts = [0.0; 3];
                    for x in &works {
                        let q = crate::vfilter::absolute_parts(&att_of(self, x), pl, *sc, &p.market_weights, &p);
                        for m in 0..3 {
                            parts[m] += q[m];
                        }
                    }
                    (pround((parts[0] + parts[1] + parts[2]) * 100.0), parts.iter().map(|x| pround(x * 100.0)).collect())
                }
                None => {
                    let (mut share, mut parts) = (0.0, [0.0; 3]);
                    for x in &works {
                        let (sh, q) = V::blended_share(&att_of(self, x), &totals, &p.market_weights);
                        share += sh;
                        for m in 0..3 {
                            parts[m] += q[m];
                        }
                    }
                    (pround(demand_day * share * 100.0), parts.iter().map(|x| pround(demand_day * x * 100.0)).collect())
                }
            };
            let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
            let cid = Some(c.id);
            self.do_transfer(cx.world, acct, rev, "revenue", &day, &ts, cid, None);
            let cost = pround(rev as f64 * p.operating_cost_ratio);
            self.do_transfer(acct, cx.world, cost, "operating_cost", &day, &ts, cid, None);
            let tax = pround((rev - cost) as f64 * p.corp_tax);
            self.do_transfer(acct, cx.gov, tax, "corp_tax", &day, &ts, cid, None);
            self.st.companies.get_mut(&c.id).unwrap().profit_week += rev - cost - tax;
            self.mark_company(c.id);
            let aired = s.aired;
            let prev = self.st.company_days.range((c.id, String::new())..).take_while(|(k, _)| k.0 == c.id).last();
            let prev_aired = prev.map(|(_, r)| r.aired);
            let row = CompanyDay {
                company_id: c.id,
                day: day.clone(),
                revenue: rev,
                rev_us: split[0],
                rev_cn: split[1],
                rev_jp: split[2],
                aired,
                dividend_ps: 0,
            };
            self.st.company_days.insert((c.id, day.clone()), row.clone());
            self.out.push(Change::CompanyDayReplace(row));
            cx.sum.injected += rev;
            cx.sum.exported += cost;
            cx.sum.taxes += tax;
            self.note_new_sources(&ts, &day, &c, s);
            if let Some(pa) = prev_aired {
                if aired > pa {
                    if pa == 0 {
                        self.push_news(
                            &ts,
                            &day,
                            "premiere",
                            cid,
                            format!("《{}》开播", c.title),
                            "第 1 集已播出，各市场的热度数据将陆续进入营收。".into(),
                        );
                    } else if c.episodes.is_some_and(|e| e != 0 && aired >= e) {
                        self.push_news(
                            &ts,
                            &day,
                            "finale",
                            cid,
                            format!("《{}》完结", c.title),
                            "完结后营收通常会在几周内明显下降。".into(),
                        );
                    }
                }
            }
        }

        // 2. Sunday: dividends, transfers, rate decision
        let sunday = d.weekday() == Weekday::Sun;
        if sunday {
            self.pay_dividends(cx)?;
            let from = iso(d - Duration::days(7));
            let weekly_taxes: i64 = self
                .st
                .gov_taxes
                .iter()
                .filter(|(k, _)| k.as_str() > from.as_str() && k.as_str() <= day.as_str())
                .map(|(_, v)| v)
                .sum();
            let amount = pround(weekly_taxes as f64 * cx.g().transfer_ratio);
            self.pay_transfers(cx, amount);
            rate = self.rate_decision(cx, rate);
        }

        // 3. interest on deposits (created by the central bank)
        let r_dep = pmax(0.0, rate - p.deposit_spread);
        if r_dep > 0.0 {
            let mut recipients: Vec<(&str, i64, i64)> = self
                .st
                .accounts
                .values()
                .filter(|a| matches!(a.kind.as_str(), "player" | "mm" | "noise" | "npc") && a.cash > 0)
                .map(|a| (a.kind.as_str(), a.id, a.cash))
                .collect();
            recipients.sort();
            let recipients: Vec<(i64, i64)> = recipients.into_iter().map(|(_, id, cash)| (id, cash)).collect();
            for (id, cash) in recipients {
                let i = (cash as f64 * r_dep / 365.0).floor() as i64;
                self.do_transfer(cx.cb, id, i, "interest", &day, &ts, None, None);
                cx.sum.interest += i;
            }
        }

        // 4. production fees on IPO proceeds received today
        for c in cx.companies.clone() {
            let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
            let proceeds = self.st.company_proceeds.get(&(acct, day.clone())).copied().unwrap_or(0);
            let fee = pround(proceeds as f64 * p.production_fee_ratio);
            if fee != 0 {
                self.do_transfer(acct, cx.world, fee, "production_fee", &day, &ts, Some(c.id), None);
                cx.sum.exported += fee;
            }
        }

        // 5. valuations, delistings
        let demand_week = V::demand_week(tot_us, a_ref, &p);
        let week_ago = iso(d - Duration::days(7));
        let total_week_rev: i64 =
            self.st.company_days.values().filter(|r| r.day.as_str() > week_ago.as_str()).map(|r| r.revenue).sum();
        let mut announced: Vec<(i64, i64, i64)> = Vec::new();
        let v2 = if cx.cfg.economy_v1 {
            None
        } else {
            // the filtered valuation (docs/v2-valuation.md): update every show's filter with today's data,
            // then value every company from the expected attention paths
            let fp = crate::vfilter::FilterParams::load(cx.cfg)?;
            if let Some(msg) = crate::vclose::inputs_schema_problem(&fp, cx.inp) {
                eprintln!("valuation: {msg}");
            }
            for c in cx.companies.clone() {
                for w in self.works_of(c.id) {
                    if let Some(s) = cx.inp.show(w) {
                        let before = self.valuation_state(w).cloned();
                        self.valuation_step(&fp, &p, s, &day, &ts);
                        // news from the public schedule (docs/v2-corporate.md §2.2–2.3)
                        self.schedule_news(&c, s, before.as_ref(), &cx.inp.cn_titles, &day, &ts);
                        // a sequel announced today: its production cost (docs/v2-corporate.md §3.3)
                        let was = before.as_ref().and_then(|j| j.sequel.as_ref()).map(|q| q.0);
                        if let Some(q) = self.valuation_state(w).and_then(|j| j.sequel.as_ref()).map(|q| q.0) {
                            if was != Some(q) {
                                announced.push((c.id, w, q));
                            }
                        }
                    }
                }
            }
            // shows in their subscription window have a value too (the offer is priced off it)
            let mut subscribing: Vec<i64> = self.st.companies.values().filter(|c| c.status == "subscribing").map(|c| c.id).collect();
            subscribing.sort_unstable();
            for id in &subscribing {
                if let Some(s) = cx.inp.show(*id) {
                    self.valuation_step(&fp, &p, s, &day, &ts);
                }
            }
            let today = crate::vclose::day_no(&day).ok_or("bad day")?;
            self.record_total(today);
            let (vals, totals) = self.valuation_values(&fp, &p, today, rate, a_ref);
            for id in &subscribing {
                if let Some(v) = vals.get(id) {
                    self.st.companies.get_mut(id).unwrap().fair_value = *v;
                    self.mark_company(*id);
                }
            }
            if !announced.is_empty() {
                let vc = crate::corporate::ValueCtx { fp: &fp, p: &p, totals: &totals, today, rate, a_ref };
                for (c, w, q) in &announced {
                    let f = self.corporate_on_sequel(*c, *w, *q, &day, &ts, &vc);
                    cx.sum.exported += f.exported;
                    cx.sum.injected += f.injected;
                }
            }
            cx.vtotals = Some(totals);
            Some(vals)
        };
        for c in cx.companies.clone() {
            let Some(s) = cx.inp.show(c.id) else { continue };
            let fv = match &v2 {
                // no filter state in any market (nothing to earn): its cash per share
                Some(vals) => match vals.get(&c.id) {
                    Some(v) => *v,
                    None => {
                        let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
                        (self.cash_of(acct) / c.shares.max(1)).max(1)
                    }
                },
                None => {
                    let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
                    let book = self.cash_of(acct);
                    let pre = cx.expected_share(s);
                    self.fair_value(cx, s, c.id, c.shares, book, rate, demand_week, pre)?
                }
            };
            self.st.companies.get_mut(&c.id).unwrap().fair_value = fv;
            self.mark_company(c.id);
            if sunday {
                self.maybe_delist(cx, &c, s, total_week_rev)?;
            }
        }

        // 6. orders are good for the day only, to the session's end at the day rollover (04:00 the next
        // morning); a late close must not cancel what was placed after D's session
        let cutoff = format!("{}T{}:00", iso(d + Duration::days(1)), cx.g().day_rollover);
        let mut expire: Vec<i64> = self
            .st
            .orders
            .values()
            .filter(|o| o.status == OrderStatus::Open && o.created_at.as_str() < cutoff.as_str())
            .filter(|o| self.st.accounts.get(&o.account_id).is_some_and(|a| a.kind != "company"))
            .map(|o| o.id)
            .collect();
        expire.sort();
        for id in expire {
            self.close(id, OrderStatus::Cancelled, &ts);
        }

        // 7. candles and next day's reference price
        let listed_now: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
        for &id in &listed_now {
            self.close_candle(id, &day);
        }
        if !cx.cfg.economy_v1 {
            self.penny_rule(cx, &listed_now)?;
            // company capital policy: rights issues, splits, buybacks (crate::corporate)
            let fp = crate::vfilter::FilterParams::load(cx.cfg)?;
            let today = crate::vclose::day_no(&day).ok_or("bad day")?;
            let totals = cx.vtotals.clone();
            let vc = totals.as_ref().map(|t| crate::corporate::ValueCtx { fp: &fp, p: &p, totals: t, today, rate, a_ref });
            let f = self.corporate_close(&day, &ts, sunday, vc.as_ref())?;
            cx.sum.exported += f.exported;
            cx.sum.injected += f.injected;
        }

        // refresh titles
        for (cid, name) in &cx.inp.cn_titles {
            if let Some(c) = self.st.companies.get_mut(cid) {
                if &c.title != name {
                    c.title = name.clone();
                    self.mark_company(*cid);
                }
            }
        }

        // 融资融券: manufactured dividends, interest and fees, maintenance (crate::margin)
        self.margin_post_close(&day, &ts);

        // 8. allot subscriptions that closed today, then open new ones
        let due: Vec<i64> = self
            .st
            .companies
            .values()
            .filter(|c| c.status == "subscribing" && c.sub_end.as_deref().is_some_and(|e| e <= day.as_str()))
            .map(|c| c.id)
            .collect();
        for id in due {
            self.allot(cx, id)?;
            // listed tomorrow: its value now includes the offer's proceeds (cash per share)
            if !cx.cfg.economy_v1 {
                if let Some(totals) = cx.vtotals.clone() {
                    let fp = crate::vfilter::FilterParams::load(cx.cfg)?;
                    let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).unwrap_or(1.0);
                    let today = crate::vclose::day_no(&day).ok_or("bad day")?;
                    if let Some(v) = self.valuation_value_of(&fp, &cx.p, id, today, &totals, rate, a_ref) {
                        self.st.companies.get_mut(&id).unwrap().fair_value = v;
                        self.mark_company(id);
                    }
                }
            }
        }
        self.list_new(cx, rate, demand_week)?;

        let m2 = self.m2();
        let old = self.st.macro_days.get(&iso(d - Duration::days(28)));
        let growth = match old {
            Some(o) if o.m2 > 0 => Some((m2 as f64 / o.m2 as f64).powf(365.0 / 28.0) - 1.0),
            _ => None,
        };
        let row = MacroDay {
            day: day.clone(),
            rate,
            m2,
            m2_growth: growth,
            injected: cx.sum.injected,
            exported: cx.sum.exported,
            taxes: cx.sum.taxes,
            interest: cx.sum.interest,
        };
        self.st.macro_days.insert(day.clone(), row.clone());
        self.out.push(Change::MacroDayReplace(row));
        self.do_set_state("rate", &repr(rate));
        self.do_set_state("last_closed", &day);
        self.snapshot_players(&day, (cx.cfg.game.initial_cash * 100.0).round_ties_even() as i64);
        self.check_invariants()?;
        Ok(rate)
    }

    /// A market's data source newly mapped to the show (the inputs' `sources` flag turns on): its revenue and
    /// hidden value in that market are restated from today, so the company posts a notice. The flags last
    /// seen are kept in the game state (`sources:<company>`); the first sighting only records them, so a
    /// running game does not post a notice for every show at once.
    fn note_new_sources(&mut self, ts: &str, day: &str, c: &crate::model::Company, s: &crate::inputs::ShowInputs) {
        const NAMES: [&str; 3] = ["欧美", "中国", "日本"];
        let Some(f) = s.sources.as_ref() else { return };
        let now = [f.us, f.cn, f.jp];
        let code: String = now.iter().map(|&b| if b { '1' } else { '0' }).collect();
        let key = format!("sources:{}", c.id);
        let before: Option<Vec<bool>> = self.state(&key).map(|v| v.chars().map(|ch| ch == '1').collect());
        if before.as_deref() == Some(&now[..]) {
            return;
        }
        if let Some(before) = before {
            for m in 0..3 {
                if now[m] && !before.get(m).copied().unwrap_or(false) {
                    self.push_news(
                        ts,
                        day,
                        "system",
                        Some(c.id),
                        format!("《{}》接入{}市场数据", c.title, NAMES[m]),
                        format!("{}市场的热度数据从今天起计入营收和估值。", NAMES[m]),
                    );
                }
            }
        }
        self.do_set_state(&key, &code);
    }

    /// The absolute revenue rule for this close (docs/v2-series.md §6): each market's platform size from the
    /// inputs, once it averages at least `revenue.platform_min_days` days, and the scale set on the first such day
    /// so that the day's total revenue is unchanged (state `platform_rule`: "us,cn,jp,scale"). Inputs without a
    /// platform size keep the last one. None: the relative rule.
    fn platform_switch(&mut self, cx: &Ctx, p: &Params, totals: &V::M3, demand_day: f64) -> Option<(V::M3, f64)> {
        if !p.absolute_revenue {
            return None;
        }
        let prev = self.platform_rule();
        let Some(pl) = cx.inp.platform.as_ref().map(|x| x.m3()) else { return prev };
        if pl.iter().any(|x| *x <= 0.0) {
            return prev;
        }
        // the scale that leaves today's total revenue unchanged: set on the switch day; before it, today's estimate
        let scale_today = || {
            let (mut old, mut new) = (0.0, 0.0);
            for c in &cx.companies {
                for w in self.works_of(c.id) {
                    let Some(s) = cx.inp.show(w) else { continue };
                    let a = self.revenue_att(s);
                    old += demand_day * V::blended_share(&a, totals, &p.market_weights).0;
                    let parts = crate::vfilter::absolute_parts(&a, &pl, 1.0, &p.market_weights, p);
                    new += parts[0] + parts[1] + parts[2];
                }
            }
            (new > 0.0).then(|| old / new)
        };
        if cx.inp.platform_days < p.platform_min_days {
            if prev.is_none() {
                // the switch is on a known day: the value prices the absolute rule from then on (vclose `value_rule`)
                if let (Some(sc), Some(today)) = (scale_today(), crate::vclose::day_no(&cx.day)) {
                    let at = today + p.platform_min_days - cx.inp.platform_days;
                    self.do_set_state(
                        "platform_pending",
                        &format!("{},{},{},{},{}", repr(pl[0]), repr(pl[1]), repr(pl[2]), repr(sc), at),
                    );
                }
            }
            return prev;
        }
        let scale = match prev {
            Some((_, sc)) => sc,
            None => scale_today()?,
        };
        self.do_set_state("platform_rule", &format!("{},{},{},{}", repr(pl[0]), repr(pl[1]), repr(pl[2]), repr(scale)));
        Some((pl, scale))
    }

    /// Per-share value in cents from the company's own revenue history (v1 `daily.fair_value`).
    #[allow(clippy::too_many_arguments)]
    fn fair_value(
        &self,
        cx: &Ctx,
        s: &ShowInputs,
        company_id: i64,
        shares: i64,
        book_cents: i64,
        rate: f64,
        demand_week: f64,
        pre_share: f64,
    ) -> Result<i64, String> {
        let revs: Vec<i64> = self
            .st
            .company_days
            .range((company_id, String::new())..=(company_id, cx.day.clone()))
            .rev()
            .take(7)
            .map(|(_, r)| r.revenue)
            .collect();
        let since = (cx.d - date(&s.first_air)?).num_days() + 1;
        V::fair_value_cents(
            &revs,
            since,
            shares,
            book_cents,
            rate,
            demand_week,
            pre_share,
            s.aired,
            s.episodes,
            s.episodes_known,
            s.long_runner,
            s.has_real_data,
            &cx.p,
        )
    }

    fn pay_dividends(&mut self, cx: &mut Ctx) -> Result<(), String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        let payout = cx.p.payout_ratio;
        for c in cx.companies.clone() {
            let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
            let cash = self.cash_of(acct);
            // `profit_week` from the snapshot taken before today's revenue was booked (v1)
            let pool = cash.min(pround(c.profit_week.max(0) as f64 * payout));
            let dps = floordiv(pool, c.shares);
            if dps <= 0 {
                continue;
            }
            let cid = Some(c.id);
            let mut paid = 0;
            for (holder, qty, kind) in self.holders(c.id) {
                if holder == acct {
                    continue; // unsold IPO shares held by the company itself
                }
                let gross = dps * qty;
                if kind == "committee" {
                    self.do_transfer(acct, cx.world, gross, "dividend_export", &day, &ts, cid, None);
                    cx.sum.exported += gross;
                } else {
                    let mut tax = 0;
                    let lots: Vec<(i64, String)> =
                        self.lots_of(holder, c.id).iter().map(|l| (l.qty, l.acquired_on.clone())).collect();
                    for (lq, acquired) in lots {
                        tax += ((dps * lq) as f64 * holding_tax_rate(cx.g(), &acquired, &day)?).floor() as i64;
                    }
                    self.do_transfer(acct, holder, gross - tax, "dividend", &day, &ts, cid, None);
                    self.do_transfer(acct, cx.gov, tax, "dividend_tax", &day, &ts, cid, None);
                    cx.sum.taxes += tax;
                }
                paid += gross;
            }
            self.st.companies.get_mut(&c.id).unwrap().profit_week = 0;
            self.mark_company(c.id);
            let key = (c.id, day.clone());
            self.st.company_days.entry(key.clone()).or_insert_with(|| CompanyDay {
                company_id: c.id,
                day: day.clone(),
                revenue: 0,
                rev_us: 0,
                rev_cn: 0,
                rev_jp: 0,
                aired: 0,
                dividend_ps: 0,
            });
            self.out.push(Change::CompanyDayInsertIgnore { company_id: c.id, day: day.clone() });
            self.st.company_days.get_mut(&key).unwrap().dividend_ps = dps;
            self.out.push(Change::CompanyDayDividend { company_id: c.id, day: day.clone(), dividend_ps: dps });
            self.push_news(
                &ts,
                &day,
                "dividend",
                cid,
                format!("《{}》每股分红 {} 星辉", c.title, fixed(dps as f64 / 100.0, 2)),
                format!(
                    "共派发 {} 星辉。持有不满 1 周的部分按 20% 缴税，不满 1 季按 10%，超过 1 季免税。",
                    fixed_grouped(paid as f64 / 100.0, 0)
                ),
            );
        }
        Ok(())
    }

    fn pay_transfers(&mut self, cx: &mut Ctx, amount: i64) {
        let eligible = if cx.cfg.economy_v1 { None } else { self.transfer_eligible(cx) };
        self.pay_transfers_to(cx, amount, eligible.as_ref());
    }

    /// The players who may receive 财政补贴 (crate::kyc::transfer_gate), read from the committed database:
    /// bindings are written outside the engine, trades of earlier days are on disk. `None` for an engine
    /// without a database (tests, simulations): every player.
    fn transfer_eligible(&mut self, cx: &Ctx) -> Option<std::collections::HashSet<i64>> {
        let players: Vec<(i64, String)> =
            self.st.accounts.values().filter(|a| a.kind == "player").map(|a| (a.id, a.created_at.clone())).collect();
        let c = self.reader_conn()?;
        let verified = match crate::kyc::verified_ids(c) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("transfers: cannot read bindings ({e}); paying no player this week");
                return Some(Default::default());
            }
        };
        let rules = &cx.cfg.eligibility;
        Some(
            players
                .into_iter()
                .filter(|(id, created)| {
                    crate::kyc::transfer_gate(c, rules, *id, verified.contains(id), created, &cx.day)
                        .is_ok_and(|g| g == crate::kyc::Gate::Open)
                })
                .map(|(id, _)| id)
                .collect(),
        )
    }

    /// Per capita to every household: eligible players and (v2) the NPC agents, who stand for the rest of
    /// the population (v1 paid every player and no NPC: with 40 players that was ~30% of starting cash a
    /// season). A player who is not eligible gets nothing; the others' shares grow.
    fn pay_transfers_to(&mut self, cx: &mut Ctx, amount: i64, eligible: Option<&std::collections::HashSet<i64>>) {
        let v1 = cx.cfg.economy_v1;
        let players: Vec<i64> = self
            .st
            .accounts
            .values()
            .filter(|a| (a.kind == "player" && eligible.is_none_or(|s| s.contains(&a.id))) || (!v1 && a.kind == "npc"))
            .map(|a| a.id)
            .collect();
        if players.is_empty() || amount <= 0 {
            return;
        }
        let have = self.cash_of(cx.gov);
        let each = floordiv(amount.min(have), players.len() as i64);
        for pid in players {
            self.do_transfer(cx.gov, pid, each, "transfer", &cx.day, &cx.ts, None, None);
        }
        if each != 0 {
            let body = if v1 {
                "本周税收的一部分按人头返还。".to_string()
            } else {
                let r = &cx.cfg.eligibility;
                format!(
                    "本周税收的一部分按人头返还。完成身份验证、注册满 {} 天、过去 {} 天有成交的玩家可以领取。",
                    r.transfer_min_days, r.transfer_active_days
                )
            };
            self.push_news(
                &cx.ts,
                &cx.day,
                "system",
                None,
                format!("财政补贴：每位玩家 {} 星辉", fixed_grouped(each as f64 / 100.0, 2)),
                body,
            );
        }
    }

    fn rate_decision(&mut self, cx: &mut Ctx, rate: f64) -> f64 {
        let p = &cx.p;
        let l = p.growth_lookback_days;
        let now = self.m2();
        let old = match self.st.macro_days.get(&iso(cx.d - Duration::days(l))) {
            Some(o) if o.m2 > 0 => o.m2,
            _ => return rate,
        };
        let g = (now as f64 / old as f64).powf(365.0 / l as f64) - 1.0;
        let want = pmin(p.r_max, pmax(p.r_min, p.r_neutral + p.k_money * (g - p.m2_growth_target)));
        let new = rate + pmax(-p.max_step, pmin(p.max_step, want - rate));
        let new = round_to(new, 6);
        if (new - rate).abs() > 1e-9 {
            let verb = if new > rate { "加息" } else { "降息" };
            let dep = new - p.deposit_spread;
            let dep_txt = if dep > 0.0 { pct(dep, 2) } else { pct(0.0, 2) };
            let (target, ts, day) = (p.m2_growth_target, cx.ts.clone(), cx.day.clone());
            self.push_news(
                &ts,
                &day,
                "rate",
                None,
                format!("央行{verb}：政策利率 {} → {}", pct(rate, 2), pct(new, 2)),
                format!(
                    "过去 {l} 天货币供应量年化增速 {}，目标 {}。存款利率随之变为 {dep_txt}。",
                    pct(g, 1),
                    pct(target, 0)
                ),
            );
        }
        new
    }

    fn maybe_delist(&mut self, cx: &mut Ctx, c: &Company, s: &ShowInputs, total_week_rev: i64) -> Result<(), String> {
        if !cx.cfg.economy_v1 {
            return self.maybe_delist_series(cx, c);
        }
        let finished = V::finished(s.aired, s.episodes, s.long_runner);
        let week_ago = iso(cx.d - Duration::days(7));
        let week_rev: i64 = self
            .st
            .company_days
            .range((c.id, String::new())..)
            .take_while(|(k, _)| k.0 == c.id)
            .filter(|(k, _)| k.1.as_str() > week_ago.as_str())
            .map(|(_, r)| r.revenue)
            .sum();
        let low = finished && (week_rev as f64) < cx.g().delist_revenue_share * (total_week_rev.max(1) as f64);
        let weeks = if low { c.low_weeks + 1 } else { 0 };
        self.st.companies.get_mut(&c.id).unwrap().low_weeks = weeks;
        self.mark_company(c.id);
        if weeks < cx.g().delist_after_weeks {
            return Ok(());
        }
        self.liquidate(cx, c, format!("《{}》退市", c.title), "按每股净资产 {per} 星辉向股东返还现金。")
    }

    /// Delisting of a series (docs/v2-series.md §9): every work finished, the last week's revenue under
    /// `delist_tail_share` of the median weekly revenue of the companies with a work on air, and a sequel
    /// announcement within three years less likely than `delist_sequel_p` (no announced sequel), for
    /// `delist_after_weeks` Sundays in a row. Liquidation at book value per share, as before.
    fn maybe_delist_series(&mut self, cx: &mut Ctx, c: &Company) -> Result<(), String> {
        let week_ago = iso(cx.d - Duration::days(7));
        let week_rev = |e: &Engine, id: i64| -> i64 {
            e.st.company_days
                .range((id, String::new())..)
                .take_while(|(k, _)| k.0 == id)
                .filter(|(k, _)| k.1.as_str() > week_ago.as_str())
                .map(|(_, r)| r.revenue)
                .sum()
        };
        let airing = |e: &Engine, id: i64| {
            e.works_of(id).into_iter().any(|w| cx.inp.show(w).is_some_and(|x| x.aired >= 1 && !V::finished(x.aired, x.episodes, x.long_runner)))
        };
        let mut on_air: Vec<i64> = cx.companies.iter().filter(|x| airing(self, x.id)).map(|x| week_rev(self, x.id)).collect();
        on_air.sort_unstable();
        let reference = if on_air.is_empty() { 0 } else { on_air[on_air.len() / 2] };
        let works = self.works_of(c.id);
        let all_finished = works.iter().all(|w| cx.inp.show(*w).is_some_and(|x| V::finished(x.aired, x.episodes, x.long_runner)));
        let fp = crate::vfilter::FilterParams::load(cx.cfg)?;
        let today = crate::vclose::day_no(&cx.day).unwrap_or(0);
        // the series' latest work carries the sequel chance
        let latest = works.iter().filter_map(|w| self.valuation_state(*w).map(|j| (j, *w))).max_by_key(|(j, _)| j.meta.as_ref().map_or(0, |m| m.first_air));
        let sequel_p = match (latest, &fp.sequel) {
            (Some((j, _)), _) if j.sequel.is_some() => 1.0,
            (Some((j, _)), Some(sp)) => {
                let fin = j.meta.as_ref().and_then(|m| m.air.last().copied()).unwrap_or(today);
                crate::vfilter::sequel_within(sp, j.seq_z, today - fin, 3.0)
            }
            _ => 0.0,
        };
        let low = all_finished && reference > 0 && (week_rev(self, c.id) as f64) < cx.p.delist_tail_share * reference as f64 && sequel_p < cx.p.delist_sequel_p;
        let weeks = if low { c.low_weeks + 1 } else { 0 };
        self.st.companies.get_mut(&c.id).unwrap().low_weeks = weeks;
        self.mark_company(c.id);
        if weeks < cx.g().delist_after_weeks {
            return Ok(());
        }
        self.liquidate(cx, c, format!("《{}》退市", c.title), "按每股净资产 {per} 星辉向股东返还现金。")
    }

    /// Take `c` off the market at today's last price (an adult title or a promo entry, step 0): cancel its open orders, the
    /// world account pays every holder but the company and the committee `last_price` per share, the shares
    /// are retired and the company's cash goes to the world account. Holders keep exactly what the stock was
    /// worth on their screen.
    fn buyout(&mut self, cx: &mut Ctx, c: &Company, why: &str) -> Result<(), String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        let mut open: Vec<i64> = self
            .st
            .orders
            .values()
            .filter(|o| o.company_id == c.id && o.status == OrderStatus::Open)
            .map(|o| o.id)
            .collect();
        open.sort();
        for id in open {
            self.close(id, OrderStatus::Cancelled, &ts);
        }
        let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
        let per = self.st.companies[&c.id].last_price;
        self.margin_on_delist(c.id, per, &day, &ts);
        self.corporate_on_delist(c.id, &ts);
        for (h, qty, kind) in self.holders(c.id) {
            if h != acct && kind != "committee" {
                self.do_transfer(cx.world, h, per * qty, "delisting", &day, &ts, Some(c.id), None);
            }
            self.do_remove_shares(h, c.id, qty, 0)?;
        }
        let left = self.cash_of(acct);
        self.do_transfer(acct, cx.world, left, "delisting", &day, &ts, Some(c.id), None);
        let co = self.st.companies.get_mut(&c.id).unwrap();
        co.status = "delisted".into();
        co.delisted_on = Some(day.clone());
        self.mark_company(c.id);
        let body = format!("{why}，按 {} 星辉每股收回全部股份。", fixed(per as f64 / 100.0, 2));
        self.push_news(&ts, &day, "delisting", Some(c.id), format!("《{}》摘牌", c.title), body);
        Ok(())
    }

    /// Delist `c`: cancel its open orders, pay holders the company's cash per share (rights holders' part to
    /// the world account), retire the shares, post the news (`{per}` in the body is the payout per share).
    fn liquidate(&mut self, cx: &mut Ctx, c: &Company, title: String, body: &str) -> Result<(), String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        // cancel open orders, return company cash per share, retire the shares
        let mut open: Vec<i64> = self
            .st
            .orders
            .values()
            .filter(|o| o.company_id == c.id && o.status == OrderStatus::Open)
            .map(|o| o.id)
            .collect();
        open.sort();
        for id in open {
            self.close(id, OrderStatus::Cancelled, &ts);
        }
        let acct = self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
        let cash = self.cash_of(acct);
        let holders = self.holders(c.id);
        let per = floordiv(cash, c.shares);
        self.margin_on_delist(c.id, per, &day, &ts);
        self.corporate_on_delist(c.id, &ts);
        for (h, qty, kind) in holders {
            let amt = per * qty;
            if h != acct {
                let to = if kind == "committee" { cx.world } else { h };
                self.do_transfer(acct, to, amt, "delisting", &day, &ts, Some(c.id), None);
            }
            self.do_remove_shares(h, c.id, qty, 0)?;
        }
        let left = self.cash_of(acct);
        self.do_transfer(acct, cx.world, left, "delisting", &day, &ts, Some(c.id), None);
        let co = self.st.companies.get_mut(&c.id).unwrap();
        co.status = "delisted".into();
        co.delisted_on = Some(day.clone());
        self.mark_company(c.id);
        self.push_news(&ts, &day, "delisting", Some(c.id), title, body.replace("{per}", &fixed(per as f64 / 100.0, 2)));
        Ok(())
    }

    /// 面值退市: a stock closing under 1 币 for 20 trading days in a row is delisted (warning at 10).
    fn penny_rule(&mut self, cx: &mut Ctx, listed: &[i64]) -> Result<(), String> {
        for &id in listed {
            let close = self.st.companies[&id].prev_close;
            let n = self.st.penny_days.entry(id).or_insert(0);
            *n = if close < PENNY_CENTS { *n + 1 } else { 0 };
            let n = *n;
            let c = self.st.companies[&id].clone();
            if n == PENNY_WARN_DAYS {
                let (ts, day) = (cx.ts.clone(), cx.day.clone());
                self.push_news(
                    &ts,
                    &day,
                    "warning",
                    Some(id),
                    format!("面值退市预警：《{}》已连续 {n} 个交易日收盘低于 1 星辉", c.title),
                    format!("连续 {PENNY_DELIST_DAYS} 个交易日收盘低于 1 星辉将按每股净资产退市。"),
                );
            } else if n >= PENNY_DELIST_DAYS {
                self.liquidate(
                    cx,
                    &c,
                    format!("《{}》面值退市", c.title),
                    "连续 20 个交易日收盘低于 1 星辉，按每股净资产 {per} 星辉向股东返还现金。",
                )?;
                self.st.penny_days.remove(&id);
            }
        }
        Ok(())
    }

    fn close_candle(&mut self, company_id: i64, day: &str) {
        let c = &self.st.companies[&company_id];
        let agg = self.st.day_aggs.get(&(company_id, day.to_string())).filter(|a| a.volume != 0).copied();
        let (o, h, lo, cl, v) = match agg {
            Some(a) => (a.open, a.high, a.low, a.close, a.volume),
            None => (c.last_price, c.last_price, c.last_price, c.last_price, 0),
        };
        let candle = Candle {
            company_id,
            day: day.to_string(),
            open: o,
            high: h,
            low: lo,
            close: cl,
            volume: v,
            source: "live".into(),
        };
        self.out.push(Change::CandleReplace(candle));
        let later = self
            .st
            .day_aggs
            .range((company_id, day.to_string())..)
            .take_while(|(k, _)| k.0 == company_id)
            .any(|(k, _)| k.1.as_str() > day);
        let c = self.st.companies.get_mut(&company_id).unwrap();
        c.prev_close = cl;
        if !later {
            // (a late close while the next day is already trading keeps today's price, anchor and cooling)
            c.last_price = cl;
            c.open_ref = Some(cl);
            c.vcm_until = None;
            c.vcm_low = None;
            c.vcm_high = None;
            c.vcm_count = 0;
        }
        self.mark_company(company_id);
    }

    /// Close a subscription (v1 `allot`): pro rata with a ballot for the odd shares, payment, release,
    /// unsold shares to the rights holders, studio fee, listing.
    fn allot(&mut self, cx: &mut Ctx, company_id: i64) -> Result<(), String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        let c = self.st.companies[&company_id].clone();
        let world = cx.world;
        let company_acct =
            self.account_of("company", c.id).ok_or_else(|| format!("no company account for {}", c.id))?;
        let committee = self.account_of("committee", c.id).ok_or_else(|| format!("no committee for {}", c.id))?;
        let price = c.ipo_price;
        let float_qty = pround(c.shares as f64 * cx.p.ipo_float);
        let base = pround(float_qty as f64 * cx.g().ipo_public_base);
        let mut apps: Vec<IpoApplication> =
            self.st.ipo_apps.range((c.id, i64::MIN)..=(c.id, i64::MAX)).map(|(_, a)| a.clone()).collect();
        apps.sort_by(|a, b| (a.created_at.as_str(), a.account_id).cmp(&(b.created_at.as_str(), b.account_id)));
        let total: i64 = apps.iter().map(|a| a.qty).sum();
        let over = if base != 0 { total as f64 / base as f64 } else { 0.0 };
        let share = public_share(cx.g(), over);
        let public = total.min(pround(float_qty as f64 * share));
        let mut allotted: Vec<i64>;
        if total <= public {
            allotted = apps.iter().map(|a| a.qty).collect();
        } else {
            let ratio = public as f64 / total as f64;
            allotted = apps.iter().map(|a| (a.qty as f64 * ratio).floor() as i64).collect();
            let mut left = public - allotted.iter().sum::<i64>();
            let mut rng = PyRandom::new(c.id as i128); // the ballot (抽签) for the odd shares
            while left > 0 {
                let pool: Vec<usize> = (0..apps.len()).filter(|i| allotted[*i] < apps[*i].qty).collect();
                if pool.is_empty() {
                    break;
                }
                let weights: Vec<i64> = pool.iter().map(|i| apps[*i].qty - allotted[*i]).collect();
                let i = pool[rng.choices_index(&weights)];
                allotted[i] += 1;
                left -= 1;
            }
        }
        let mut proceeds = 0;
        for (a, &got) in apps.iter().zip(&allotted) {
            self.add_cash(a.account_id, 0, -a.frozen);
            let cost = got * price;
            if cost != 0 {
                self.do_transfer(a.account_id, company_acct, cost, "ipo", &day, &ts, Some(c.id), None);
                let comm = self.fees.commission_on(cost);
                self.do_transfer(a.account_id, world, comm, "commission", &day, &ts, Some(c.id), None);
                self.do_add_shares(a.account_id, c.id, got, price, &c.listed_on);
                proceeds += cost;
            }
            let app = self.st.ipo_apps.get_mut(&(c.id, a.account_id)).unwrap();
            app.allotted = Some(got);
            app.frozen = 0;
            self.out.push(Change::IpoAppAllot { company_id: c.id, account_id: a.account_id, allotted: got });
        }
        let allotted_sum: i64 = allotted.iter().sum();
        self.do_add_shares(committee, c.id, float_qty - allotted_sum, price, &c.listed_on);
        let fee = pround(proceeds as f64 * cx.p.production_fee_ratio);
        self.do_transfer(company_acct, world, fee, "production_fee", &day, &ts, Some(c.id), None);
        cx.sum.exported += fee;
        self.st.companies.get_mut(&c.id).unwrap().status = "listed".into();
        self.mark_company(c.id);
        let rate_txt = if total != 0 {
            format!("中签率 {}", pct(allotted_sum as f64 / total as f64, 1))
        } else {
            "无人认购".into()
        };
        self.push_news(
            &ts,
            &day,
            "allotment",
            Some(c.id),
            format!("《{}》认购结果：超额认购 {} 倍", c.title, fixed(over, 1)),
            format!(
                "共 {} 人认购 {} 股，公开发售 {} 股（流通盘的 {}），{rate_txt}。未售出的 {} 股由制作委员会持有。{} 上市交易。",
                apps.len(),
                int_grouped(total),
                int_grouped(public),
                pct(share, 0),
                int_grouped(float_qty - allotted_sum),
                c.listed_on
            ),
        );
        Ok(())
    }

    /// Open IPO subscriptions for shows that list within the subscription window (v1 `list_new`).
    fn list_new(&mut self, cx: &mut Ctx, rate: f64, demand_week: f64) -> Result<(), String> {
        let tomorrow = cx.d + Duration::days(1);
        let lead = cx.p.ipo_lead_days;
        let window = cx.g().ipo_subscription_days;
        let existing: HashSet<i64> = self.st.companies.keys().chain(self.st.company_works.keys()).copied().collect();
        let inp = cx.inp;
        // the series of the listed companies (docs/v2-series.md §3.3): a new season of one joins its company
        let mut series: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
        if !cx.cfg.economy_v1 {
            let mut listed: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
            listed.sort_unstable();
            for c in listed {
                for w in self.works_of(c) {
                    if let Some(k) = inp.show(w).and_then(|x| x.series) {
                        series.entry(k).or_insert(c);
                    }
                }
            }
        }
        for s in &inp.shows {
            if existing.contains(&s.id) || s.excluded().is_some() {
                continue;
            }
            if let Some(&c) = s.series.and_then(|k| series.get(&k)) {
                // a work of a listed series: no new shares, a company notice; its attention joins the company. A new
                // season joins when its subscription would have opened; an earlier work (its long tail) when the
                // series is first known, unless it premiered long before (the same 60 days as a listing)
                let first_air = date(&s.first_air)?;
                if first_air - Duration::days(lead + window) > tomorrow {
                    continue;
                }
                if first_air < tomorrow - Duration::days(60) && !s.long_runner {
                    continue;
                }
                self.st.company_works.insert(s.id, c);
                self.out.push(Change::CompanyWork { work_id: s.id, company_id: c, joined_on: cx.day.clone() });
                let name = inp.cn_titles.get(&s.id).cloned().unwrap_or_else(|| s.title_ja.clone());
                let co = self.st.companies[&c].title.clone();
                let (title, body) = if first_air >= tomorrow {
                    (format!("《{name}》{} 月 {} 日开播", first_air.month(), first_air.day()), format!("《{co}》的新一季。热度计入公司营收和估值，不发行新股。"))
                } else {
                    (format!("《{name}》并入《{co}》"), "同一系列的作品。热度计入公司营收和估值，不发行新股。".to_string())
                };
                self.push_news(&cx.ts, &cx.day, "sequel", Some(c), title, body);
                continue;
            }
            if s.popularity_end < cx.g().min_listing_popularity {
                continue;
            }
            let first_air = date(&s.first_air)?;
            if first_air - Duration::days(lead + window) > tomorrow {
                continue;
            }
            if first_air < tomorrow - Duration::days(60) && !s.long_runner {
                continue; // long finished before the game noticed it
            }
            let listed_on = (first_air - Duration::days(lead)).max(tomorrow + Duration::days(window));
            let sub = (iso(tomorrow), iso(listed_on - Duration::days(1)));
            self.create_company(cx, s, &iso(listed_on), rate, demand_week, sub)?;
        }
        Ok(())
    }

    /// A company with its accounts and an open IPO subscription (v1 `create_company` with `subscription`).
    fn create_company(
        &mut self,
        cx: &mut Ctx,
        s: &ShowInputs,
        listed_on: &str,
        rate: f64,
        demand_week: f64,
        subscription: (String, String),
    ) -> Result<i64, String> {
        let (day, ts) = (cx.day.clone(), cx.ts.clone());
        let shares = cx.p.shares;
        let title = match cx.inp.cn_titles.get(&s.id) {
            Some(t) if !t.is_empty() => t.clone(),
            _ => s.title_ja.clone(),
        };
        let share = cx.expected_share(s);
        let ipo_price = match (&cx.vtotals, cx.cfg.economy_v1) {
            (Some(totals), false) => {
                let fp = crate::vfilter::FilterParams::load(cx.cfg)?;
                let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).unwrap_or(1.0);
                let today = crate::vclose::day_no(&day).ok_or("bad day")?;
                self.ipo_price_v2(&fp, &cx.p, s, shares, today, totals, rate, a_ref)
            }
            _ => {
                let v = self.fair_value(cx, s, s.id, shares, 0, rate, demand_week, share)?;
                pround(cx.p.ipo_price_min * 100.0).max(pround(v as f64 * (1.0 - cx.p.ipo_discount)))
            }
        };
        let n = self.st.companies.len() as i64;
        let code = format!("{}", 100001 + n);
        let (sub_start, sub_end) = subscription;
        let company = Company {
            id: s.id,
            code: code.clone(),
            title: title.clone(),
            title_ja: Some(s.title_ja.clone()),
            title_en: s.title_en.clone(),
            studio: s.studio.clone(),
            episodes: if s.episodes_known { s.episodes } else { None },
            long_runner: s.long_runner as i64,
            first_air: s.first_air.clone(),
            listed_on: listed_on.to_string(),
            status: "subscribing".into(),
            sub_start: Some(sub_start.clone()),
            sub_end: Some(sub_end.clone()),
            shares,
            ipo_price,
            prev_close: ipo_price,
            open_ref: Some(ipo_price),
            vcm_until: None,
            vcm_low: None,
            vcm_high: None,
            vcm_count: 0,
            last_price: ipo_price,
            fair_value: ipo_price,
            profit_week: 0,
            low_weeks: 0,
            delisted_on: None,
        };
        self.out.push(Change::CompanyInsert(company.clone()));
        self.st.companies.insert(s.id, company);
        self.do_create_account("company", &format!("{title} 公司账户"), None, None, Some(s.id), &ts);
        let committee =
            self.do_create_account("committee", &format!("{title} 制作委员会"), None, None, Some(s.id), &ts);
        let mm = self.do_create_account("mm", &format!("做市商·{code}"), None, None, Some(s.id), &ts);
        let grant = pround(cx.cfg.npc.market_maker_cash * 100.0);
        self.do_transfer(cx.world, mm, grant, "grant", &day, &ts, Some(s.id), None);
        let float_qty = pround(shares as f64 * cx.p.ipo_float);
        self.do_add_shares(committee, s.id, shares - float_qty, ipo_price, listed_on);
        let base = pround(float_qty as f64 * cx.g().ipo_public_base);
        self.push_news(
            &ts,
            &day,
            "ipo",
            Some(s.id),
            format!("新股认购：《{title}》（{code}）"),
            format!(
                "{sub_start} 至 {sub_end} 开放认购，发行价 {} 星辉，公开发售 {} 股（超额认购 15/50/100 倍时回拨到 70%/80%/90% 的流通盘）。{listed_on} 上市，首播日 {}。",
                fixed(ipo_price as f64 / 100.0, 2),
                int_grouped(base),
                s.first_air
            ),
        );
        Ok(s.id)
    }

    /// One `player_days` row per player: assets after this close, chain-linked return index (V6).
    fn snapshot_players(&mut self, day: &str, initial: i64) {
        let rows: Vec<PlayerDay> = self
            .st
            .accounts
            .values()
            .filter(|a| a.kind == "player")
            .map(|a| {
                let stock: i64 = self
                    .holdings_of(a.id)
                    .map(|h| h.qty * self.st.companies.get(&h.company_id).map_or(0, |c| c.last_price))
                    .sum();
                let pr = self.st.player_returns.get(&a.id);
                let (factor, base) = pr.map_or((1.0, initial), |p| (p.return_factor, p.segment_base));
                let liabilities = self.liabilities(a.id);
                let total = a.cash + stock - liabilities;
                PlayerDay {
                    account_id: a.id,
                    day: day.to_string(),
                    cash: a.cash,
                    stock_value: stock,
                    liabilities,
                    return_index: if base > 0 { factor * total as f64 / base as f64 } else { factor },
                    restructured: pr.is_some_and(|p| p.last_day == day),
                }
            })
            .collect();
        for r in rows {
            self.out.push(Change::PlayerDayReplace(r));
        }
    }

    /// Forget per-day aggregates the next closes no longer need.
    fn prune_after_close(&mut self, day: &str) {
        self.st.day_aggs.retain(|k, _| k.1.as_str() > day);
        self.st.company_proceeds.retain(|k, _| k.1.as_str() > day);
        if let Ok(d) = date(day) {
            let keep = iso(d - Duration::days(10));
            self.st.gov_taxes.retain(|k, _| k.as_str() > keep.as_str());
        }
    }

    /// Close every day from the last closed one up to the day before `today` (the closing loop of v1
    /// `app.catch_up`), stamping every close with `ts` and taking each day's inputs from `inputs_for`.
    /// Stops at the first failure. v1 then backs up the database when anything was closed: the caller
    /// does that with `daily_backup`. v1 reads the clock again for each day; `catch_up_clocked` does that.
    pub fn catch_up(
        &mut self,
        cfg: &Config,
        today: &str,
        ts: &str,
        inputs_for: impl FnMut(&str) -> Result<Inputs, String>,
    ) -> Result<Vec<CloseOutcome>, String> {
        self.catch_up_clocked(cfg, today, || ts.to_string(), inputs_for)
    }

    /// `catch_up` with each day's timestamp taken from `ts_now` (the game clock's `ts()`) when that day is
    /// closed, exactly like v1.
    pub fn catch_up_clocked(
        &mut self,
        cfg: &Config,
        today: &str,
        mut ts_now: impl FnMut() -> String,
        mut inputs_for: impl FnMut(&str) -> Result<Inputs, String>,
    ) -> Result<Vec<CloseOutcome>, String> {
        let last = self.state("last_closed").ok_or("state 'last_closed' missing")?.to_string();
        let mut d = date(&last)? + Duration::days(1);
        let yesterday = date(today)? - Duration::days(1);
        let mut out = Vec::new();
        while d <= yesterday {
            let day = iso(d);
            let inp = inputs_for(&day)?;
            let ts = ts_now();
            out.push(self.run_day(cfg, &day, &ts, &inp)?);
            d += Duration::days(1);
        }
        Ok(out)
    }

    /// One copy per closed day in `<db dir>/backups/`; the newest 7 are kept (v1 `daily_backup`).
    pub fn daily_backup(&mut self, game_db: &Path, day: &str) -> Result<PathBuf, String> {
        const KEPT: usize = 7;
        let folder = game_db.parent().map(Path::to_path_buf).unwrap_or_default().join("backups");
        let stem = game_db.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let dest = self.backup_to(&folder.join(format!("{stem}-{day}.db")))?;
        let prefix = format!("{stem}-2");
        let mut old: Vec<PathBuf> = std::fs::read_dir(&folder)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".db"))
            })
            .collect();
        old.sort();
        if old.len() > KEPT {
            for p in &old[..old.len() - KEPT] {
                let _ = std::fs::remove_file(p);
            }
        }
        Ok(dest)
    }
}

#[cfg(test)]
#[path = "daily_tests.rs"]
mod tests;
