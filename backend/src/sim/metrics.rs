//! Recording and the §7 statistics.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Serialize;
use serde_json::json;

use crate::engine::Engine;
use crate::market::{at_local, local, Event, Market};

#[derive(Debug, Clone, Copy)]
pub struct TradeRec {
    pub t: i64,
    pub company: i64,
    pub price: i64,
    pub qty: i64,
    pub buyer: i64,
    pub seller: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DayRow {
    /// dividend per share paid at this day's close (the next day trades ex-dividend)
    pub div: i64,
    pub close: i64,
    pub fair: i64,
    pub heat: f64,
    pub trades: u32,
}

/// Time series kept for the report's charts.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Series {
    pub days: Vec<String>,
    pub npc_wealth: Vec<f64>,
    pub mm_wealth: Vec<f64>,
    /// (company, title, group, closes, fairs)
    pub samples: Vec<(i64, String, String, Vec<f64>, Vec<f64>)>,
    pub std_daily_returns: Vec<f64>,
    pub acf_abs_5min: Vec<f64>,
    pub acf_abs_daily: Vec<f64>,
    pub intraday: Vec<(i64, String, Vec<(i64, f64)>)>,
    /// every stock: (company, group, [(day, close, fair)])
    pub paths: Vec<(i64, String, Vec<(String, i64, i64, i64)>)>,
}

pub struct Recorder {
    trades: Vec<TradeRec>,
    days: Vec<String>,
    rows: BTreeMap<i64, BTreeMap<String, DayRow>>,
    spreads: HashMap<i64, Vec<f64>>,
    two_sided: HashMap<i64, (u32, u32)>,
    events: Vec<(i64, i64, String, i8, f64)>,
    npc: HashSet<i64>,
    mm_accounts: HashSet<i64>,
    /// the market makers' cash and inventory at the start (their buy-and-hold benchmark)
    mm_start_cash: i64,
    mm_start_holdings: HashMap<i64, i64>,
    wealth0: (i64, i64),
    npc_income: i64,
    /// transfers and interest received by the NPC agents (a passive holder gets them too)
    npc_unearned: i64,
    npc_fund: i64,
    mm_fund: i64,
    mm_income: i64,
    series: Series,
    titles: HashMap<i64, String>,
    ledger_reasons: BTreeMap<String, i64>,
    watch: Option<i64>,
    /// (company, day) -> the close of `day` brought new information (an episode, a cn/jp value, a flash)
    info: HashSet<(i64, String)>,
    /// company -> its premiere day and last episode day, from the inputs
    premiere: HashMap<i64, String>,
    finale: HashMap<i64, String>,
    air_day: HashSet<(i64, String)>,
    money: Vec<serde_json::Value>,
    /// companies listed on the first day, and their dividends per share paid since (cents)
    start_set: Option<Vec<i64>>,
    start_set_paid: i64,
    /// the game's dividend tax brackets (max holding days, rate), for the passive benchmark
    pub dividend_tax: Vec<(f64, f64)>,
    start_set_accounts: std::collections::HashSet<i64>,
    /// NPC agents' holdings and cash at the start (for the passive benchmark)
    start_holdings: HashMap<i64, i64>,
    start_cash: i64,
    first_day: String,
    watch_transfers: i64,
    watch_dividends: i64,
}

fn quantile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i]
}

fn median(mut v: Vec<f64>) -> f64 {
    quantile(&mut v, 0.5)
}

/// Lag-k autocorrelation pooled over several series (each demeaned by the pooled mean).
fn acf_pooled(series: &[Vec<f64>], lag: usize) -> f64 {
    let all: Vec<f64> = series.iter().flatten().copied().collect();
    if all.len() < 10 {
        return f64::NAN;
    }
    let mean = all.iter().sum::<f64>() / all.len() as f64;
    let var: f64 = all.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>();
    let mut cov = 0.0;
    for s in series {
        if s.len() > lag {
            for i in lag..s.len() {
                cov += (s[i] - mean) * (s[i - lag] - mean);
            }
        }
    }
    cov / var
}

fn kurtosis(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    if n < 10.0 {
        return f64::NAN;
    }
    let m = x.iter().sum::<f64>() / n;
    let m2 = x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / n;
    let m4 = x.iter().map(|v| (v - m).powi(4)).sum::<f64>() / n;
    m4 / (m2 * m2)
}

fn sd(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    if n < 2.0 {
        return f64::NAN;
    }
    let m = x.iter().sum::<f64>() / n;
    (x.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
}

fn wealth(e: &Engine, acct: i64) -> i64 {
    e.net_assets(acct)
}

impl Recorder {
    pub fn new(e: &Engine, _m: &Market, npc: &HashSet<i64>) -> Recorder {
        let mm_accounts = e.accounts().filter(|a| a.kind == "mm").map(|a| a.id).collect();
        Recorder {
            trades: Vec::new(),
            days: Vec::new(),
            rows: BTreeMap::new(),
            spreads: HashMap::new(),
            two_sided: HashMap::new(),
            events: Vec::new(),
            npc: npc.clone(),
            mm_accounts,
            mm_start_cash: 0,
            mm_start_holdings: HashMap::new(),
            wealth0: (0, 0),
            npc_income: 0,
            npc_unearned: 0,
            npc_fund: 0,
            mm_fund: 0,
            mm_income: 0,
            series: Series::default(),
            titles: HashMap::new(),
            ledger_reasons: BTreeMap::new(),
            watch: None,
            info: HashSet::new(),
            premiere: HashMap::new(),
            finale: HashMap::new(),
            air_day: HashSet::new(),
            money: Vec::new(),
            start_set: None,
            start_set_paid: 0,
            dividend_tax: vec![(7.0, 0.2), (91.0, 0.1)],
            start_set_accounts: Default::default(),
            start_holdings: HashMap::new(),
            start_cash: 0,
            first_day: String::new(),
            watch_transfers: 0,
            watch_dividends: 0,
        }
    }

    fn mm_wealth(&self, e: &Engine) -> i64 {
        self.mm_accounts.iter().map(|a| wealth(e, *a)).sum()
    }

    pub fn start_wealth(&mut self, e: &Engine, m: &Market) {
        self.wealth0 = (m.npc_wealth(e).0, self.mm_wealth(e));
        for a in &self.mm_accounts {
            self.mm_start_cash += e.account(*a).map_or(0, |x| x.cash);
            for h in e.holdings_of(*a) {
                *self.mm_start_holdings.entry(h.company_id).or_default() += h.qty;
            }
        }
        for a in &m.agents {
            self.start_cash += e.account(a.account).map_or(0, |x| x.cash);
            for h in e.holdings_of(a.account) {
                *self.start_holdings.entry(h.company_id).or_default() += h.qty;
            }
        }
    }

    pub fn day_start(&mut self, e: &Engine, day: &str) {
        if self.first_day.is_empty() {
            self.first_day = day.to_string();
        }
        self.days.push(day.to_string());
        for c in e.companies().filter(|c| c.status == "listed") {
            self.titles.entry(c.id).or_insert_with(|| c.title.clone());
            let r = self.rows.entry(c.id).or_default().entry(day.to_string()).or_default();
            r.fair = c.fair_value;
        }
    }

    /// What the close of `day` learned, per company (for the revision statistics).
    pub fn note_info(&mut self, inp: &crate::inputs::Inputs, prev: &crate::inputs::Inputs, day: &str) {
        for s in &inp.shows {
            let p = prev.show(s.id);
            let aired_up = p.is_some_and(|p| s.aired > p.aired);
            let cnjp = p.is_some_and(|p| p.att.cn != s.att.cn || p.att.jp != s.att.jp);
            if aired_up || cnjp {
                self.info.insert((s.id, day.to_string()));
            }
            if aired_up {
                self.air_day.insert((s.id, day.to_string()));
            }
            if s.aired >= 1 {
                self.premiere.entry(s.id).or_insert_with(|| day.to_string());
            }
            if s.episodes.is_some_and(|n| s.aired >= n) && !s.long_runner {
                self.finale.entry(s.id).or_insert_with(|| day.to_string());
            }
        }
        for (c, at, kind, _, _) in &self.events {
            if kind == "flash" && crate::market::day_of(*at) == day {
                self.info.insert((*c, day.to_string()));
            }
        }
    }

    pub fn flash(&mut self, company: i64, at: i64, surprise: f64) {
        self.events.push((company, at, "flash".into(), if surprise > 0.0 { 1 } else { -1 }, surprise.abs()));
    }

    pub fn event(&mut self, ev: &Event) {
        self.events.push((ev.company_id, ev.at, ev.kind.clone(), ev.direction, ev.magnitude));
    }

    pub fn sample_spreads(&mut self, e: &Engine, m: &Market, _t: i64) {
        for c in e.companies().filter(|c| c.status == "listed") {
            let b = e.book(c.id, 1);
            let ent = self.two_sided.entry(c.id).or_default();
            ent.1 += 1;
            if let (Some(bid), Some(ask)) = (b.bids.first(), b.asks.first()) {
                ent.0 += 1;
                let mid = (bid.price + ask.price) as f64 / 2.0;
                self.spreads.entry(c.id).or_default().push((ask.price - bid.price) as f64 / mid);
            }
        }
        let _ = m;
    }

    pub fn day_end(&mut self, e: &Engine, day: &str) {
        for c in e.companies().filter(|c| c.status == "listed") {
            if let Some(r) = self.rows.get_mut(&c.id).and_then(|x| x.get_mut(day)) {
                r.close = c.last_price;
            }
        }
    }

    pub fn after_close(&mut self, e: &Engine, m: &Market, day: &str, _next: &str) {
        for (k, cd) in e.st.company_days.range((i64::MIN, day.to_string())..) {
            if k.1 == day && cd.dividend_ps > 0 {
                if let Some(r) = self.rows.get_mut(&k.0).and_then(|x| x.get_mut(day)) {
                    r.div = cd.dividend_ps;
                }
            }
        }
        for (c, h) in e.companies().filter(|c| c.status == "listed").map(|c| (c.id, m.heat(c.id))) {
            if let Some(r) = self.rows.get_mut(&c).and_then(|x| x.get_mut(day)) {
                r.heat = h;
            }
        }
        let sum = |k: &[&str]| e.accounts().filter(|a| k.contains(&a.kind.as_str())).map(|a| a.cash).sum::<i64>() as f64 / 100.0;
        let index: f64 = e.companies().filter(|c| c.status == "listed").map(|c| (c.last_price * c.shares) as f64).sum::<f64>() / 100.0;
        let hold = |k: &str| -> f64 {
            e.accounts()
                .filter(|a| a.kind == k)
                .map(|a| e.holdings_of(a.id).map(|h| (h.qty * e.company(h.company_id).map_or(0, |c| c.last_price)) as f64).sum::<f64>() + a.cash as f64)
                .sum::<f64>()
                / 100.0
        };
        if self.start_set.is_none() {
            let mut ids: Vec<i64> = e.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
            ids.sort_unstable();
            self.start_set_accounts = ids.iter().filter_map(|id| e.account_of("company", *id)).collect();
            self.start_set = Some(ids);
        }
        let set = self.start_set.clone().unwrap_or_default();
        // the first day's listed companies: market value now (0 once delisted) plus what they paid out since
        // (dividends and liquidation, all holders)
        let set_cap: f64 = set
            .iter()
            .filter_map(|id| e.company(*id))
            .filter(|c| c.status == "listed")
            .map(|c| (c.last_price * c.shares) as f64)
            .sum::<f64>()
            / 100.0;
        self.money.push(json!({
            "rate": e.state("rate").and_then(crate::pyfmt::parse_float), "mm_wealth": hold("mm"), "gov_cash": sum(&["gov"]), "start_set_cap": set_cap,
            "start_set_paid": self.start_set_paid as f64 / 100.0, "listed": e.companies().filter(|c| c.status == "listed").count(),
            "day": day, "players_cash": sum(&["player"]), "npc_cash": sum(&["npc"]), "company_cash": sum(&["company"]),
            "mm_cash": sum(&["mm"]), "money_stock": sum(&["player", "npc", "company", "mm"]), "market_cap": index,
            "players_wealth": hold("player"), "npc_wealth": hold("npc"),
        }));
        self.series.days.push(day.to_string());
        self.series.npc_wealth.push(m.npc_wealth(e).0 as f64 / 100.0);
        self.series.mm_wealth.push(self.mm_wealth(e) as f64 / 100.0);
    }

    pub fn watch_account(&mut self, a: i64) {
        self.watch = Some(a);
    }

    /// (transfers, dividends) received by the watched account.
    pub fn watched(&self) -> (i64, i64) {
        (self.watch_transfers, self.watch_dividends)
    }

    pub fn absorb(&mut self, tap: &mut super::Tap) {
        for tr in tap.trades.drain(..) {
            if let Some(r) = self.rows.get_mut(&tr.company) {
                let (d, _) = local(tr.t);
                let _ = d;
                if let Some(day) = self.days.last() {
                    if let Some(row) = r.get_mut(day) {
                        row.trades += 1;
                    }
                }
            }
            self.trades.push(tr);
        }
        for (from, to, amt, reason) in tap.ledger.drain(..) {
            *self.ledger_reasons.entry(reason.clone()).or_default() += amt;
            if self.start_set_accounts.contains(&from)
                && matches!(reason.as_str(), "dividend" | "dividend_export" | "dividend_tax" | "delisting")
            {
                self.start_set_paid += amt;
            }
            if Some(to) == self.watch {
                match reason.as_str() {
                    "transfer" => self.watch_transfers += amt,
                    "dividend" => self.watch_dividends += amt,
                    _ => {}
                }
            }
            let fund = matches!(reason.as_str(), "npc_fund_in" | "npc_fund_out" | "npc_rebalance");
            let income = matches!(reason.as_str(), "dividend" | "interest" | "delisting" | "transfer");
            for (acct, sign) in [(to, 1), (from, -1)] {
                if self.npc.contains(&acct) {
                    if fund {
                        self.npc_fund += sign * amt;
                    } else if income {
                        self.npc_income += sign * amt;
                        if matches!(reason.as_str(), "transfer" | "interest") {
                            self.npc_unearned += sign * amt;
                        }
                    }
                } else if self.mm_accounts.contains(&acct) {
                    if fund {
                        self.mm_fund += sign * amt;
                    } else if income {
                        self.mm_income += sign * amt;
                    }
                }
            }
        }
    }

    pub fn finish(mut self, e: &Engine, m: &Market, elapsed: f64, days: &[String]) -> (serde_json::Value, Series) {
        // groups by mean heat
        let mut heat_avg: Vec<(f64, i64)> = self
            .rows
            .iter()
            .map(|(c, r)| (r.values().map(|x| x.heat).sum::<f64>() / r.len().max(1) as f64, *c))
            .collect();
        heat_avg.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let n = heat_avg.len();
        let mut group: HashMap<i64, &'static str> = HashMap::new();
        for (k, (_, c)) in heat_avg.iter().enumerate() {
            group.insert(
                *c,
                if k < n / 3 {
                    "hot"
                } else if k < 2 * n / 3 {
                    "mid"
                } else {
                    "cold"
                },
            );
        }
        // stocks quoted under 1 币 most of the time are left out of the statistics: at a few cents a tick is
        // 10-50% of the price and the VCM band rounds to a single price (reported separately)
        let penny: HashSet<i64> = self
            .rows
            .iter()
            .filter(|(_, r)| {
                let mut p: Vec<f64> = r.values().map(|x| x.close as f64).filter(|x| *x > 0.0).collect();
                quantile(&mut p, 0.5) < 100.0
            })
            .map(|(c, _)| *c)
            .collect();
        self.rows.retain(|c, _| !penny.contains(c));
        heat_avg.retain(|(_, c)| !penny.contains(c));
        let groups = ["hot", "mid", "cold", "all"];
        let in_group = |c: i64, g: &str| g == "all" || group.get(&c).copied() == Some(g);

        // trades by company (continuous session only for gaps)
        self.trades.sort_by_key(|t| (t.company, t.t));
        let mut by_c: HashMap<i64, Vec<TradeRec>> = HashMap::new();
        for t in &self.trades {
            by_c.entry(t.company).or_default().push(*t);
        }
        let mut gap_med: HashMap<i64, f64> = HashMap::new();
        let mut trades_per_day: HashMap<i64, f64> = HashMap::new();
        for (c, ts) in &by_c {
            let mut gaps = Vec::new();
            for w in ts.windows(2) {
                let (d0, s0) = local(w[0].t);
                let (d1, _) = local(w[1].t);
                if d0 == d1 && s0 >= 9 * 3600 {
                    gaps.push((w[1].t - w[0].t) as f64 / 60.0);
                }
            }
            if !gaps.is_empty() {
                gap_med.insert(*c, median(gaps));
            }
            let nd = self.rows.get(c).map_or(1, |r| r.len().max(1));
            trades_per_day.insert(*c, ts.len() as f64 / nd as f64);
        }

        // minute closes per company-day (09:00-23:00), carried forward from the first price of the day
        let mut minute: HashMap<i64, Vec<Vec<f64>>> = HashMap::new();
        for (c, ts) in &by_c {
            let mut per_day: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
            for t in ts {
                let (d, s) = local(t.t);
                if s >= 9 * 3600 {
                    per_day.entry(d).or_default().push(((s - 9 * 3600) / 60, t.price));
                }
            }
            let mut out = Vec::new();
            for (_, v) in per_day {
                let mut bars = vec![f64::NAN; 840];
                for (mi, p) in v {
                    if (mi as usize) < 840 {
                        bars[mi as usize] = (p as f64).ln();
                    }
                }
                let Some(first) = bars.iter().copied().find(|x| !x.is_nan()) else { continue };
                let mut last = first;
                for b in bars.iter_mut() {
                    if b.is_nan() {
                        *b = last;
                    } else {
                        last = *b;
                    }
                }
                out.push(bars);
            }
            minute.insert(*c, out);
        }
        let returns_at = |c: i64, step: usize| -> Vec<Vec<f64>> {
            minute
                .get(&c)
                .map(|days| {
                    days.iter()
                        .map(|b| (step..b.len()).step_by(step).map(|i| b[i] - b[i - step]).collect::<Vec<f64>>())
                        .collect()
                })
                .unwrap_or_default()
        };

        // daily returns, event days
        let event_days: HashSet<(i64, String)> = self
            .events
            .iter()
            .map(|(c, at, _, _, _)| {
                // an event announced at the close counts for the next trading day; one during the day for that day
                let (_, s) = local(*at);
                let day = if s < 8 * 3600 + 45 * 60 { crate::market::day_of(*at) } else { crate::market::day_of(*at) };
                (*c, day)
            })
            .collect();
        let mut daily: HashMap<i64, Vec<(String, f64)>> = HashMap::new();
        let mut dev: HashMap<i64, Vec<f64>> = HashMap::new();
        for (c, rows) in &self.rows {
            let mut prev: Option<i64> = None;
            let mut prev_div = 0;
            for (d, r) in rows {
                if r.close <= 0 {
                    prev = None;
                    continue;
                }
                if let Some(p) = prev {
                    // total return: yesterday's dividend is added back (the stock trades ex-dividend today)
                    daily.entry(*c).or_default().push((d.clone(), ((r.close + prev_div) as f64 / p as f64).ln()));
                }
                prev = Some(r.close);
                prev_div = r.div;
                if r.fair > 0 {
                    dev.entry(*c).or_default().push((r.close as f64 / r.fair as f64).ln());
                }
            }
        }

        let mut table = serde_json::Map::new();
        for g in groups {
            let cs: Vec<i64> = self.rows.keys().copied().filter(|c| in_group(*c, g)).collect();
            let gap = median(cs.iter().filter_map(|c| gap_med.get(c).copied()).collect());
            let tpd = median(cs.iter().filter_map(|c| trades_per_day.get(c).copied()).collect());
            let sig = median(
                cs.iter()
                    .filter_map(|c| daily.get(c).map(|v| sd(&v.iter().map(|x| x.1).collect::<Vec<_>>())))
                    .filter(|x| x.is_finite())
                    .collect(),
            );
            let mut ev_r = Vec::new();
            let mut no_r = Vec::new();
            let mut std_r = Vec::new();
            for c in &cs {
                if let Some(v) = daily.get(c) {
                    let s = sd(&v.iter().map(|x| x.1).collect::<Vec<_>>());
                    for (d, r) in v {
                        if event_days.contains(&(*c, d.clone())) {
                            ev_r.push(*r);
                        } else {
                            no_r.push(*r);
                        }
                        if s.is_finite() && s > 0.0 {
                            std_r.push(r / s);
                        }
                    }
                }
            }
            let rms = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt();
            // the hidden value's own daily volatility (the game's valuation model) for comparison
            let fair_sig = median(
                cs.iter()
                    .filter_map(|c| self.rows.get(c))
                    .map(|rows| {
                        let f: Vec<(f64, f64)> =
                            rows.values().filter(|r| r.fair > 0).map(|r| (r.fair as f64, r.div as f64)).collect();
                        sd(&f.windows(2).map(|w| ((w[1].0 + w[0].1) / w[0].0).ln()).collect::<Vec<_>>())
                    })
                    .filter(|x| x.is_finite())
                    .collect(),
            );
            // volatility of the mispricing log(price/value), day to day
            let mis_sig = median(
                cs.iter()
                    .filter_map(|c| dev.get(c))
                    .map(|x| sd(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()))
                    .filter(|x| x.is_finite())
                    .collect(),
            );
            let r1: Vec<Vec<f64>> = cs.iter().flat_map(|c| returns_at(*c, 1)).collect();
            let r5: Vec<Vec<f64>> = cs.iter().flat_map(|c| returns_at(*c, 5)).collect();
            let r60: Vec<Vec<f64>> = cs.iter().flat_map(|c| returns_at(*c, 60)).collect();
            let abs5: Vec<Vec<f64>> = r5.iter().map(|v| v.iter().map(|x| x.abs()).collect()).collect();
            let r5_std: Vec<f64> = {
                let s = sd(&r5.iter().flatten().copied().collect::<Vec<_>>());
                r5.iter().flatten().map(|x| x / s).collect()
            };
            let abs_daily: Vec<Vec<f64>> =
                cs.iter().filter_map(|c| daily.get(c)).map(|v| v.iter().map(|x| x.1.abs()).collect()).collect();
            // deviation half-life: pooled AR(1) of log(price/value)
            let (mut num, mut den) = (0.0, 0.0);
            for c in &cs {
                if let Some(x) = dev.get(c) {
                    for w in x.windows(2) {
                        num += w[1] * w[0];
                        den += w[0] * w[0];
                    }
                }
            }
            let phi = num / den;
            let half_life = if phi > 0.0 && phi < 1.0 { -(2f64.ln()) / phi.ln() } else { f64::INFINITY };
            let dev_sd = sd(&cs.iter().filter_map(|c| dev.get(c)).flatten().copied().collect::<Vec<_>>());
            let mut sp: Vec<f64> = cs.iter().filter_map(|c| self.spreads.get(c)).flatten().copied().collect();
            let two: (u32, u32) =
                cs.iter().filter_map(|c| self.two_sided.get(c)).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
            if g == "all" {
                self.series.std_daily_returns = std_r.clone();
                self.series.acf_abs_5min = (1..=24).map(|k| acf_pooled(&abs5, k)).collect();
                self.series.acf_abs_daily = (1..=10).map(|k| acf_pooled(&abs_daily, k)).collect();
            }
            table.insert(
                g.to_string(),
                json!({
                    "stocks": cs.len(),
                    "median_trade_gap_min": gap,
                    "median_trades_per_day": tpd,
                    "daily_sigma_median": sig,
                    "daily_sigma_fair_median": fair_sig,
                    "daily_sigma_mispricing_median": mis_sig,
                    "daily_sigma_event_days": rms(&ev_r),
                    "daily_sigma_other_days": rms(&no_r),
                    "event_days": ev_r.len(),
                    "kurtosis_daily": kurtosis(&std_r),
                    "kurtosis_5min": kurtosis(&r5_std),
                    "acf_abs_5min": [acf_pooled(&abs5, 1), acf_pooled(&abs5, 3), acf_pooled(&abs5, 6), acf_pooled(&abs5, 12)],
                    "acf_abs_daily": [acf_pooled(&abs_daily, 1), acf_pooled(&abs_daily, 2), acf_pooled(&abs_daily, 5)],
                    "acf_1min_lag1": acf_pooled(&r1, 1),
                    "acf_1h_lag1": acf_pooled(&r60, 1),
                    "deviation_ar1": phi,
                    "deviation_half_life_days": half_life,
                    "deviation_sd": dev_sd,
                    "deviation_abs_median": median(cs.iter().filter_map(|c| dev.get(c)).flatten().map(|x| x.abs()).collect()),
                    "spread_median": quantile(&mut sp, 0.5),
                    "spread_p90": quantile(&mut sp, 0.9),
                    "two_sided_share": two.0 as f64 / two.1.max(1) as f64,
                }),
            );
        }

        // ---- revisions of the hidden value (docs/v2-valuation.md §7)
        let mut rev_all: Vec<f64> = Vec::new();
        let mut rev_pairs: Vec<(f64, f64)> = Vec::new();
        let mut by_weekday: [Vec<f64>; 7] = Default::default();
        let (mut on_info, mut no_info, mut on_air, mut off_air) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut first_wk, mut middle, mut end) = (Vec::new(), Vec::new(), Vec::new());
        // simple returns (the bias a calendar trader would earn): air days in the first two weeks after the
        // premiere, later air days, other days
        let (mut air_early, mut air_later, mut other_simple) = (Vec::new(), Vec::new(), Vec::new());
        for (c, rows) in &self.rows {
            let v: Vec<(&String, &DayRow)> = rows.iter().collect();
            let mut prev_rev: Option<f64> = None;
            for w in v.windows(2) {
                let (d0, r0) = w[0];
                let (_, r1) = w[1];
                if r0.fair <= 0 || r1.fair <= 0 {
                    prev_rev = None;
                    continue;
                }
                // the change of the value between the two days' opens (the close of d0 in between), dividends added back
                let rv = ((r1.fair + r0.div) as f64 / r0.fair as f64).ln();
                rev_all.push(rv);
                if let Some(p) = prev_rev {
                    rev_pairs.push((p, rv));
                }
                prev_rev = Some(rv);
                if let Some(dd) = chrono::NaiveDate::parse_from_str(d0, "%Y-%m-%d").ok() {
                    by_weekday[chrono::Datelike::weekday(&dd).num_days_from_monday() as usize].push(rv);
                }
                let key = (*c, d0.clone());
                if self.info.contains(&key) { on_info.push(rv) } else { no_info.push(rv) }
                if self.air_day.contains(&key) { on_air.push(rv) } else { off_air.push(rv) }
                if self.air_day.contains(&key) {
                    let since = self.premiere.get(c).and_then(|p| {
                        Some((chrono::NaiveDate::parse_from_str(d0, "%Y-%m-%d").ok()? - chrono::NaiveDate::parse_from_str(p, "%Y-%m-%d").ok()?).num_days())
                    });
                    if since.is_some_and(|s| (0..14).contains(&s)) { air_early.push(rv.exp() - 1.0) } else { air_later.push(rv.exp() - 1.0) }
                } else {
                    other_simple.push(rv.exp() - 1.0);
                }
                if let Some(p) = self.premiere.get(c) {
                    let since = chrono::NaiveDate::parse_from_str(d0, "%Y-%m-%d").ok().zip(chrono::NaiveDate::parse_from_str(p, "%Y-%m-%d").ok()).map(|(a, b)| (a - b).num_days());
                    let to_end = self.finale.get(c).and_then(|f| chrono::NaiveDate::parse_from_str(f, "%Y-%m-%d").ok()).zip(chrono::NaiveDate::parse_from_str(d0, "%Y-%m-%d").ok()).map(|(f, a)| (f - a).num_days());
                    match (since, to_end) {
                        (Some(s), _) if (0..7).contains(&s) => first_wk.push(rv),
                        (Some(s), Some(t)) if s >= 7 && (0..14).contains(&t) => end.push(rv),
                        (Some(s), _) if s >= 7 => middle.push(rv),
                        _ => {}
                    }
                }
            }
        }
        let rmsv = |v: &[f64]| (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt();
        let meanv = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let corr = |p: &[(f64, f64)]| {
            let (mx, my) = (meanv(&p.iter().map(|x| x.0).collect::<Vec<_>>()), meanv(&p.iter().map(|x| x.1).collect::<Vec<_>>()));
            let cov: f64 = p.iter().map(|(a, b)| (a - mx) * (b - my)).sum();
            let va: f64 = p.iter().map(|(a, _)| (a - mx).powi(2)).sum();
            let vb: f64 = p.iter().map(|(_, b)| (b - my).powi(2)).sum();
            cov / (va * vb).sqrt()
        };
        let revisions = json!({
            "n": rev_all.len(),
            "rms": rmsv(&rev_all),
            "lag1_corr": corr(&rev_pairs),
            "weekday_mean": by_weekday.iter().map(|v| meanv(v)).collect::<Vec<_>>(),
            "weekday_rms": by_weekday.iter().map(|v| rmsv(v)).collect::<Vec<_>>(),
            "air_day_mean": meanv(&on_air), "other_day_mean": meanv(&off_air),
            "air_day_rms": rmsv(&on_air), "other_day_rms": rmsv(&off_air),
            "info_rms": rmsv(&on_info), "no_info_rms": rmsv(&no_info), "info_days": on_info.len(), "no_info_days": no_info.len(),
            "no_info_median_abs": median(no_info.iter().map(|x| x.abs()).collect()),
            "first_week_rms": rmsv(&first_wk), "middle_rms": rmsv(&middle), "last_two_weeks_rms": rmsv(&end),
            "first_week_n": first_wk.len(), "middle_n": middle.len(), "end_n": end.len(),
            "air_first2w_simple_mean": meanv(&air_early), "air_first2w_n": air_early.len(),
            "air_first2w_se": rmsv(&air_early) / (air_early.len().max(1) as f64).sqrt(),
            "air_later_simple_mean": meanv(&air_later), "other_simple_mean": meanv(&other_simple),
        });

        // NPC wealth
        let (w_agents, _) = m.npc_wealth(e);
        let w_mm = self.mm_wealth(e);
        let trading_pnl = w_agents - self.wealth0.0 - self.npc_fund - self.npc_income;
        let mm_trading = w_mm - self.wealth0.1 - self.mm_fund - self.mm_income;
        // passive benchmark: the agents' starting cash and holdings, held all season (dividends after tax;
        // delisted shares count as 0)
        // (plus the per-capita transfers and interest the agents received: not a trading result)
        let mut passive = (self.start_cash + self.npc_unearned) as f64;
        for (c, q) in &self.start_holdings {
            let Some(co) = e.company(*c) else { continue };
            let px = if co.status == "listed" { co.last_price as f64 } else { 0.0 };
            // dividends after the tax for a lot bought on the first day (its age on each payday)
            let first = crate::vclose::day_no(&self.first_day).unwrap_or(0);
            let net: f64 = e
                .st
                .company_days
                .range((*c, self.first_day.clone())..)
                .take_while(|(k, _)| k.0 == *c)
                .map(|(_, d)| {
                    let age = (crate::vclose::day_no(&d.day).unwrap_or(first) - first) as f64;
                    let rate = self.dividend_tax.iter().find(|(max, _)| age <= *max).map_or(0.0, |x| x.1);
                    d.dividend_ps as f64 * (1.0 - rate)
                })
                .sum();
            passive += *q as f64 * (px + net);
        }
        let actual = (w_agents - self.npc_fund) as f64;
        // the market makers against holding their starting inventory (delisted shares count as 0): what
        // dealing earned or lost, without the inventory's price drift
        let mm_hold: i64 = self.mm_start_cash
            + self
                .mm_start_holdings
                .iter()
                .map(|(c, q)| q * e.company(*c).filter(|co| co.status == "listed").map_or(0, |co| co.last_price))
                .sum::<i64>();
        let mm_dealing = w_mm - self.mm_fund - self.mm_income - mm_hold;
        let npc = json!({
            "passive_end": passive / 100.0,
            "passive_return": passive / self.wealth0.0.max(1) as f64 - 1.0,
            "excess_vs_passive_pct": (actual - passive) / self.wealth0.0.max(1) as f64,
            "agents_start": self.wealth0.0 as f64 / 100.0,
            "agents_end": w_agents as f64 / 100.0,
            "agents_fund_net": self.npc_fund as f64 / 100.0,
            "agents_income": self.npc_income as f64 / 100.0,
            "agents_transfers_interest": self.npc_unearned as f64 / 100.0,
            "agents_trading_pnl": trading_pnl as f64 / 100.0,
            "agents_trading_pnl_pct": trading_pnl as f64 / self.wealth0.0.max(1) as f64,
            "agents_wealth_drift_pct": w_agents as f64 / self.wealth0.0.max(1) as f64 - 1.0,
            "agents_total_return": (w_agents - self.wealth0.0 - self.npc_fund) as f64 / self.wealth0.0.max(1) as f64,
            "mm_start": self.wealth0.1 as f64 / 100.0,
            "mm_end": w_mm as f64 / 100.0,
            "mm_fund_net": self.mm_fund as f64 / 100.0,
            "mm_trading_pnl": mm_trading as f64 / 100.0,
            "mm_trading_pnl_pct": mm_trading as f64 / self.wealth0.1.max(1) as f64,
            "mm_dealing_pnl": mm_dealing as f64 / 100.0,
            "fund_in": m.stats.fund_in as f64 / 100.0,
            "fund_out": m.stats.fund_out as f64 / 100.0,
        });
        // sample stocks for charts: the median-heat stock of each group
        for g in ["hot", "mid", "cold"] {
            let cs: Vec<&(f64, i64)> = heat_avg.iter().filter(|(_, c)| group.get(c).copied() == Some(g)).collect();
            if let Some((_, c)) = cs.get(cs.len() / 2) {
                let rows = &self.rows[c];
                self.series.samples.push((
                    *c,
                    self.titles.get(c).cloned().unwrap_or_default(),
                    g.to_string(),
                    rows.values().map(|r| r.close as f64 / 100.0).collect(),
                    rows.values().map(|r| r.fair as f64 / 100.0).collect(),
                ));
                // intraday path of one mid-season day
                if let Some(ts) = by_c.get(c) {
                    let mid_day = days[days.len() / 2].clone();
                    let (t0, t1) = (at_local(&mid_day, 9, 0), at_local(&mid_day, 23, 0));
                    let path: Vec<(i64, f64)> = ts
                        .iter()
                        .filter(|t| t.t >= t0 && t.t < t1)
                        .map(|t| ((t.t - t0) / 60, t.price as f64 / 100.0))
                        .collect();
                    self.series.intraday.push((
                        *c,
                        format!("{} {}", self.titles.get(c).cloned().unwrap_or_default(), mid_day),
                        path,
                    ));
                }
            }
        }
        for (c, rows) in &self.rows {
            self.series.paths.push((
                *c,
                group.get(c).unwrap_or(&"").to_string(),
                rows.iter().map(|(d, r)| (d.clone(), r.close, r.fair, r.div)).collect(),
            ));
        }
        let mut by_kind: BTreeMap<String, (i64, i64, usize)> = BTreeMap::new();
        for ag in &m.agents {
            let ent = by_kind.entry(format!("{:?}", ag.kind)).or_default();
            ent.0 += e.account(ag.account).map_or(0, |a| a.cash);
            ent.1 += wealth(e, ag.account);
            ent.2 += 1;
        }
        let kinds_json: serde_json::Map<String, serde_json::Value> = by_kind
            .iter()
            .map(|(k, (cash, w, n))| (k.clone(), json!({"n": n, "cash": *cash as f64 / 100.0, "wealth": *w as f64 / 100.0, "cash_share": *cash as f64 / (*w).max(1) as f64})))
            .collect();
        // first/last close and value of every stock
        let movers: Vec<serde_json::Value> = self
            .rows
            .iter()
            .map(|(c, rows)| {
                let first = rows.values().find(|r| r.close > 0);
                let last = rows.values().rev().find(|r| r.close > 0);
                json!({"id": c, "title": self.titles.get(c), "group": group.get(c),
                       "close0": first.map(|r| r.close), "close1": last.map(|r| r.close),
                       "fair0": first.map(|r| r.fair), "fair1": last.map(|r| r.fair), "days": rows.len()})
            })
            .collect();
        let total_orders: u64 = m.stats.orders.values().sum();
        let session_s = days.len() as f64 * (14.25 * 3600.0);
        let out = json!({
            "days": days.len(),
            "first_day": days.first(),
            "last_day": days.last(),
            "stocks": self.rows.len(),
            "penny_stocks_excluded": penny.len(),
            "npc_agents": m.agents.len(),
            "trades": self.trades.len(),
            "groups": table,
            "npc": npc,
            "market": m.stats,
            "npc_orders_per_s": total_orders as f64 / session_s,
            "events": self.events.len(),
            "ledger_by_reason": self.ledger_reasons.iter().map(|(k, v)| (k.clone(), *v as f64 / 100.0)).collect::<BTreeMap<_, _>>(),
            "invariants_ok": e.check_invariants().is_ok(),
            "revisions": revisions,
            "money": self.money,
            "stocks_detail": movers,
            "agents_by_kind": kinds_json,
            "elapsed_s": elapsed,
        });
        (out, self.series)
    }
}
