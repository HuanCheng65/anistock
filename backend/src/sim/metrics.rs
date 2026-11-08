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
    /// the day's opening price: the auction's, else the previous close (`open_ref` at the day's end)
    pub open: i64,
    /// the day's first trade (the candle's open) and its time (unix seconds); 0 without a trade
    pub first: i64,
    pub first_t: i64,
    /// the first trade's buyer and seller accounts
    pub first_buyer: i64,
    pub first_seller: i64,
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
    /// market sentiment per day, standard units (empty when the sentiment layer is off)
    pub sentiment: Vec<f64>,
    /// every stock: (company, [(day, auction open, first trade, its time, its buyer, its seller)]) (the opening
    /// gap; buyer and seller as an agent kind or an account kind)
    pub opens: Vec<(i64, Vec<(String, i64, i64, i64, String, String)>)>,
    /// every published 热度快讯: the price path right after it (docs/v2-api-access.md §7: how much a script
    /// that reads the news within a second could take before the market catches up)
    pub flash_paths: Vec<FlashPath>,
}

/// Seconds after a published flash at which `FlashPath` samples the stock.
pub const FLASH_OFFSETS: [i64; 8] = [0, 60, 300, 900, 1800, 3600, 7200, 21600];

#[derive(Debug, Clone, Serialize)]
pub struct FlashPath {
    pub company: i64,
    pub at: i64,
    pub direction: i8,
    /// the valuation before and after the flash (the news body shows both)
    pub old_value: i64,
    pub new_value: i64,
    /// at each `FLASH_OFFSETS`: (offset s, last price, best bid, best ask); 0 when a side is empty
    pub samples: Vec<(i64, i64, i64, i64)>,
}

pub struct Recorder {
    trades: Vec<TradeRec>,
    days: Vec<String>,
    rows: BTreeMap<i64, BTreeMap<String, DayRow>>,
    spreads: HashMap<i64, Vec<f64>>,
    two_sided: HashMap<i64, (u32, u32)>,
    /// (company, day) -> (samples, samples with no offer, samples with no bid)
    book_gaps: HashMap<(i64, String), (u32, u32, u32)>,
    events: Vec<(i64, i64, String, i8, f64)>,
    /// indices into `series.flash_paths` still being sampled
    flash_open: Vec<usize>,
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
    /// market sentiment per day (sentiment layer on)
    sentiment: HashMap<String, f64>,
    /// liquidity of the three hottest stocks, every spread sample: (hour, information window, spread, the
    /// market maker's best bid and ask distance from the mid (NaN: none), its half-spread parts, depth within
    /// 1.5% of the mid (币, both sides) and the market maker's share of it)
    hot_liq: Vec<(i64, bool, f64, f64, f64, [f64; 4], f64, f64)>,
    /// run starts (bubble triggers): company, the day whose close started it, became a bubble, its draws
    /// (strength, burst, end of the dump; unix seconds)
    runs: Vec<(i64, String, bool, Option<(f64, i64, i64)>)>,
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

fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len().min(y.len());
    if n < 3 {
        return f64::NAN;
    }
    let mx = x[..n].iter().sum::<f64>() / n as f64;
    let my = y[..n].iter().sum::<f64>() / n as f64;
    let (mut c, mut vx, mut vy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        c += (x[i] - mx) * (y[i] - my);
        vx += (x[i] - mx).powi(2);
        vy += (y[i] - my).powi(2);
    }
    c / (vx * vy).sqrt()
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
    e.net_assets(acct) + e.fund_value_of(acct)
}

impl Recorder {
    /// Liquidity of the three hottest stocks (docs/v2-npc-market.md §13): spread, the market maker's quotes and
    /// the depth near the mid, inside and outside information windows, and the spread by hour.
    fn hot_liquidity(&self) -> serde_json::Value {
        let med = |v: Vec<f64>| median(v.into_iter().filter(|x| x.is_finite()).collect());
        let part = |info: bool| {
            let rows: Vec<_> = self.hot_liq.iter().filter(|r| r.1 == info).collect();
            let mm_not_best = rows.iter().filter(|r| r.3.is_nan() || r.4.is_nan() || r.3 > r.2 / 2.0 + 1e-9 || r.4 > r.2 / 2.0 + 1e-9).count();
            let mut sp: Vec<f64> = rows.iter().map(|r| r.2).collect();
            let p90 = quantile(&mut sp, 0.9);
            json!({
                "samples": rows.len(),
                "spread_median": med(rows.iter().map(|r| r.2).collect()),
                "spread_p90": p90,
                "mm_half_median": med(rows.iter().map(|r| (r.3 + r.4) / 2.0).collect()),
                "mm_not_inside_share": mm_not_best as f64 / rows.len().max(1) as f64,
                "half_base": med(rows.iter().map(|r| r.5[0]).collect()),
                "half_vol_term": med(rows.iter().map(|r| r.5[1]).collect()),
                "toxicity_mult": med(rows.iter().map(|r| r.5[2]).collect()),
                "info_mult": med(rows.iter().map(|r| r.5[3]).collect()),
                "depth_1p5pct": med(rows.iter().map(|r| r.6).collect()),
                "mm_depth_share": med(rows.iter().map(|r| r.7).collect()),
            })
        };
        let mut by_hour = BTreeMap::new();
        for h in 0..24 {
            let v: Vec<f64> = self.hot_liq.iter().filter(|r| r.0 == h).map(|r| r.2).collect();
            if !v.is_empty() {
                let info = self.hot_liq.iter().filter(|r| r.0 == h && r.1).count() as f64 / v.len() as f64;
                by_hour.insert(format!("{h:02}"), json!({"spread_median": median(v), "info_share": info}));
            }
        }
        json!({"normal": part(false), "info_window": part(true), "by_hour": by_hour})
    }

    pub fn new(e: &Engine, _m: &Market, npc: &HashSet<i64>) -> Recorder {
        let mm_accounts = e.accounts().filter(|a| a.kind == "mm").map(|a| a.id).collect();
        Recorder {
            trades: Vec::new(),
            days: Vec::new(),
            rows: BTreeMap::new(),
            spreads: HashMap::new(),
            two_sided: HashMap::new(),
            book_gaps: HashMap::new(),
            events: Vec::new(),
            flash_open: Vec::new(),
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
            sentiment: HashMap::new(),
            hot_liq: Vec::new(),
            runs: Vec::new(),
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

    pub fn sentiment(&mut self, day: &str, level: Option<f64>) {
        if let Some(v) = level {
            self.sentiment.insert(day.to_string(), v);
            self.series.sentiment.push(v);
        }
    }

    pub fn run_start(&mut self, c: i64, day: &str, bubble: bool, draws: Option<(f64, i64, i64)>) {
        self.runs.push((c, day.to_string(), bubble, draws));
    }

    /// Bubbles (docs/v2-sentiment.md §7): log(price / value) of every run start from 5 days before to 10 days
    /// after, for the runs that became bubbles and those that did not (the control), and the bubbles' draws.
    fn bubble_report(&self) -> serde_json::Value {
        let offsets: Vec<i64> = vec![-5, 0, 1, 2, 3, 5, 7, 10];
        let mut groups: BTreeMap<&str, Vec<Vec<f64>>> = BTreeMap::new();
        let mut peak: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut list = Vec::new();
        for (c, day, hit, draws) in &self.runs {
            let Some(rows) = self.rows.get(c) else { continue };
            let days: Vec<&String> = rows.keys().collect();
            let Some(k) = days.iter().position(|d| *d == day) else { continue };
            let dev = |j: i64| -> f64 {
                let i = k as i64 + j;
                if i < 0 || i as usize >= days.len() {
                    return f64::NAN;
                }
                let r = &rows[days[i as usize]];
                if r.close > 0 && r.fair > 0 { (r.close as f64 / r.fair as f64).ln() } else { f64::NAN }
            };
            let g = if *hit { "bubble" } else { "control" };
            groups.entry(g).or_default().push(offsets.iter().map(|&j| dev(j)).collect());
            let pk = (1..=7).map(dev).filter(|x| x.is_finite()).fold(f64::NAN, f64::max);
            peak.entry(g).or_default().push(pk - dev(0));
            if *hit {
                let (st, burst, _) = draws.unwrap_or((f64::NAN, 0, 0));
                let at = at_local(day, 30, 30);
                list.push(json!({"company": c, "day": day, "strength": st, "life_days": (burst - at) as f64 / 86_400.0,
                    "dev": offsets.iter().map(|&j| dev(j)).collect::<Vec<_>>()}));
            }
        }
        let mean_at = |rows: &Vec<Vec<f64>>, i: usize| {
            let v: Vec<f64> = rows.iter().map(|r| r[i]).filter(|x| x.is_finite()).collect();
            if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
        };
        let summary: BTreeMap<&str, serde_json::Value> = groups
            .iter()
            .map(|(g, rows)| {
                let f: Vec<f64> = peak[g].iter().copied().filter(|x| x.is_finite()).collect();
                let pk = f.iter().sum::<f64>() / f.len().max(1) as f64;
                (*g, json!({"n": rows.len(), "offsets": offsets, "mean_log_p_over_v": (0..offsets.len()).map(|i| mean_at(rows, i)).collect::<Vec<_>>(),
                    "mean_peak_rise_7d": pk}))
            })
            .collect();
        json!({"summary": summary, "bubbles": list})
    }

    pub fn flash(&mut self, company: i64, at: i64, surprise: f64) {
        self.events.push((company, at, "flash".into(), if surprise > 0.0 { 1 } else { -1 }, surprise.abs()));
    }

    /// Start sampling the price path after a published flash (the first sample is taken now).
    pub fn flash_path(&mut self, e: &Engine, company: i64, at: i64, direction: i8, old_value: i64, new_value: i64) {
        self.series.flash_paths.push(FlashPath { company, at, direction, old_value, new_value, samples: Vec::new() });
        let i = self.series.flash_paths.len() - 1;
        self.flash_open.push(i);
        self.flash_tick(e, at);
    }

    /// Take the flash-path samples that are due at `t` (called every simulated minute).
    pub fn flash_tick(&mut self, e: &Engine, t: i64) {
        let paths = &mut self.series.flash_paths;
        self.flash_open.retain(|&i| {
            let p = &mut paths[i];
            while let Some(&off) = FLASH_OFFSETS.get(p.samples.len()) {
                if p.at + off > t {
                    return true;
                }
                let b = e.book(p.company, 1);
                let last = e.company(p.company).map_or(0, |c| c.last_price);
                p.samples.push((off, last, b.bids.first().map_or(0, |l| l.price), b.asks.first().map_or(0, |l| l.price)));
            }
            false
        });
    }

    pub fn event(&mut self, ev: &Event) {
        self.events.push((ev.company_id, ev.at, ev.kind.clone(), ev.direction, ev.magnitude));
    }

    pub fn sample_spreads(&mut self, e: &Engine, m: &Market, t: i64) {
        let day = crate::market::day_of(t);
        for c in e.companies().filter(|c| c.status == "listed") {
            let b = e.book(c.id, 1);
            let gap = self.book_gaps.entry((c.id, day.clone())).or_default();
            gap.0 += 1;
            gap.1 += b.asks.is_empty() as u32;
            gap.2 += b.bids.is_empty() as u32;
            let ent = self.two_sided.entry(c.id).or_default();
            ent.1 += 1;
            if let (Some(bid), Some(ask)) = (b.bids.first(), b.asks.first()) {
                ent.0 += 1;
                let mid = (bid.price + ask.price) as f64 / 2.0;
                self.spreads.entry(c.id).or_default().push((ask.price - bid.price) as f64 / mid);
            }
        }
        // the three hottest stocks: who makes the inside, and how wide
        let mut hot: Vec<(f64, i64)> = e.companies().filter(|c| c.status == "listed").map(|c| (m.heat(c.id), c.id)).collect();
        hot.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let (_, sec) = local(t);
        for &(_, c) in hot.iter().take(3) {
            let b = e.book(c, 200);
            let (Some(bid), Some(ask)) = (b.bids.first(), b.asks.first()) else { continue };
            let mid = (bid.price + ask.price) as f64 / 2.0;
            let mm = e.account_of("mm", c);
            let (mut mb, mut ma) = (f64::NAN, f64::NAN);
            let mut mm_depth = 0.0;
            if let Some(acct) = mm {
                for o in e.open_orders_of(acct).into_iter().filter(|o| o.company_id == c) {
                    let Some(px) = o.price else { continue };
                    let d = (px as f64 - mid).abs() / mid;
                    if o.side == crate::model::Side::Buy {
                        mb = if mb.is_nan() { d } else { mb.min(d) };
                    } else {
                        ma = if ma.is_nan() { d } else { ma.min(d) };
                    }
                    if d <= 0.015 {
                        mm_depth += (o.left() * px) as f64 / 100.0;
                    }
                }
            }
            let depth: f64 = b
                .bids
                .iter()
                .chain(b.asks.iter())
                .filter(|l| (l.price as f64 - mid).abs() / mid <= 0.015)
                .map(|l| (l.qty * l.price) as f64 / 100.0)
                .sum();
            let parts = m.mm_quote_parts(c).unwrap_or([f64::NAN; 4]);
            self.hot_liq.push((
                sec / 3600,
                m.in_info_window(c, t),
                (ask.price - bid.price) as f64 / mid,
                mb,
                ma,
                parts,
                depth,
                if depth > 0.0 { mm_depth / depth } else { f64::NAN },
            ));
        }
    }

    pub fn day_end(&mut self, e: &Engine, day: &str) {
        for c in e.companies().filter(|c| c.status == "listed") {
            if let Some(r) = self.rows.get_mut(&c.id).and_then(|x| x.get_mut(day)) {
                r.close = c.last_price;
                r.open = c.open_ref.unwrap_or(c.prev_close);
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
                // its own trading day (04:00 rollover): trades after the replay's 23:00 minute loop are absorbed
                // with the next day's batch but belong to this one
                let day = crate::market::day_of(tr.t);
                {
                    if let Some(row) = r.get_mut(&day) {
                        if row.trades == 0 {
                            row.first = tr.price;
                            row.first_t = tr.t;
                            row.first_buyer = tr.buyer;
                            row.first_seller = tr.seller;
                        }
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
            // ---- stylised facts (docs/v2-sentiment.md §4)
            // short-term reversal at a day: lag-1 autocorrelation of daily returns
            let rd: Vec<Vec<f64>> = cs.iter().filter_map(|c| daily.get(c)).map(|v| v.iter().map(|x| x.1).collect()).collect();
            // bounded deviation: quantiles of log(price / value), and the longest run of a stock's days on one
            // side beyond ±10% (p90 and max over stocks)
            let mut devs: Vec<f64> = cs.iter().filter_map(|c| dev.get(c)).flatten().copied().collect();
            let runs: Vec<f64> = cs
                .iter()
                .filter_map(|c| dev.get(c))
                .map(|x| {
                    let (mut best, mut cur, mut side) = (0usize, 0usize, 0i8);
                    for v in x {
                        let sg = if *v > 0.1 { 1 } else if *v < -0.1 { -1 } else { 0 };
                        cur = if sg != 0 && sg == side { cur + 1 } else if sg != 0 { 1 } else { 0 };
                        side = sg;
                        best = best.max(cur);
                    }
                    best as f64
                })
                .collect();
            // volume and attention: trades per day against heat, pooled over stock-days and within a stock
            // (each stock's own means removed); trades on a stock's event days against its other days
            let (mut vx, mut vy, mut wx, mut wy) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            let mut ev_ratio = Vec::new();
            for c in &cs {
                let Some(rows) = self.rows.get(c) else { continue };
                let pts: Vec<(f64, f64)> =
                    rows.values().filter(|r| r.heat > 0.0 && r.close > 0).map(|r| (r.heat.ln(), (1.0 + r.trades as f64).ln())).collect();
                if pts.len() >= 5 {
                    let mx = pts.iter().map(|p| p.0).sum::<f64>() / pts.len() as f64;
                    let my = pts.iter().map(|p| p.1).sum::<f64>() / pts.len() as f64;
                    for (x, y) in &pts {
                        vx.push(*x);
                        vy.push(*y);
                        wx.push(x - mx);
                        wy.push(y - my);
                    }
                }
                let (mut on, mut off) = (Vec::new(), Vec::new());
                for (d, r) in rows {
                    if r.close <= 0 {
                        continue;
                    }
                    if event_days.contains(&(*c, d.clone())) { on.push(r.trades as f64) } else { off.push(r.trades as f64) }
                }
                if !on.is_empty() && off.len() >= 3 {
                    let m_off = off.iter().sum::<f64>() / off.len() as f64;
                    if m_off > 0.0 {
                        ev_ratio.push(on.iter().sum::<f64>() / on.len() as f64 / m_off);
                    }
                }
            }
            // market sentiment and mispricing: mean log(price / value) on 火热 days minus on 冷淡 days
            let cut = 0.67;
            let (mut hot_dev, mut cold_dev) = (Vec::new(), Vec::new());
            for c in &cs {
                if let Some(rows) = self.rows.get(c) {
                    for (d, r) in rows {
                        if r.close <= 0 || r.fair <= 0 {
                            continue;
                        }
                        let Some(sv) = self.sentiment.get(d) else { continue };
                        let x = (r.close as f64 / r.fair as f64).ln();
                        if *sv > cut {
                            hot_dev.push(x);
                        } else if *sv < -cut {
                            cold_dev.push(x);
                        }
                    }
                }
            }
            let mean = |v: &[f64]| if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 };
            let stylised = json!({
                "acf_daily_lag1": acf_pooled(&rd, 1),
                "acf_daily_lag2": acf_pooled(&rd, 2),
                "deviation_p05": quantile(&mut devs, 0.05),
                "deviation_p50": quantile(&mut devs, 0.5),
                "deviation_p95": quantile(&mut devs, 0.95),
                "deviation_run_10pct_p90_days": quantile(&mut runs.clone(), 0.9),
                "deviation_run_10pct_max_days": runs.iter().cloned().fold(0.0, f64::max),
                "corr_trades_heat_pooled": pearson(&vx, &vy),
                "corr_trades_heat_within": pearson(&wx, &wy),
                "trades_event_day_ratio_median": median(ev_ratio.clone()),
                "deviation_hot_minus_cold_sentiment": mean(&hot_dev) - mean(&cold_dev),
                "sentiment_days_hot_cold": [hot_dev.len(), cold_dev.len()],
            });
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
                    "stylised": stylised,
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
            let kinds: HashMap<i64, String> = m.agents.iter().map(|a| (a.account, format!("{:?}", a.kind))).collect();
            let label = |id: i64| kinds.get(&id).cloned().unwrap_or_else(|| e.account(id).map_or_else(|| "-".into(), |a| a.kind.clone()));
            self.series.opens.push((
                *c,
                rows.iter().map(|(d, r)| (d.clone(), r.open, r.first, r.first_t, label(r.first_buyer), label(r.first_seller))).collect(),
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
        // one-sided books: share of the 5-minute samples with no offer (no bid), overall and on the days the
        // hidden value jumped up (the close before brought a rise of at least `revision_min`, or 20%)
        let book = {
            let rmin = m.cfg.revision_min;
            let mut acc: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
            let mut worst: Vec<(f64, i64)> = Vec::new();
            let mut per_c: HashMap<i64, (u64, u64)> = HashMap::new();
            for ((c, d), (n, na, nb)) in &self.book_gaps {
                let mut add = |k: &'static str| {
                    let a = acc.entry(k).or_default();
                    a.0 += *n as u64;
                    a.1 += *na as u64;
                    a.2 += *nb as u64;
                };
                add("all");
                let pc = per_c.entry(*c).or_default();
                pc.0 += *n as u64;
                pc.1 += *na as u64;
                if let Some(rows) = self.rows.get(c) {
                    let prev = rows.range(..d.clone()).next_back().map(|(_, r)| r.fair);
                    let prev2 = rows.range(..d.clone()).rev().nth(1).map(|(_, r)| r.fair);
                    let cur = rows.get(d).map(|r| r.fair);
                    let up = |a: Option<i64>, b: Option<i64>| match (a, b) {
                        (Some(a), Some(b)) if a > 0 && b > 0 => (b as f64 / a as f64).ln(),
                        _ => 0.0,
                    };
                    let j0 = up(prev, cur);
                    let j1 = up(prev2, prev);
                    if j0 >= rmin {
                        add("jump_day");
                    }
                    if j0 >= 0.2f64.ln_1p() {
                        add("jump20_day");
                    }
                    if j1 >= 0.2f64.ln_1p() {
                        add("jump20_next_day");
                    }
                }
            }
            for (c, (n, na)) in &per_c {
                worst.push((*na as f64 / (*n).max(1) as f64, *c));
            }
            worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            let mut out = serde_json::Map::new();
            for (k, (n, na, nb)) in &acc {
                out.insert(
                    k.to_string(),
                    json!({"samples": n, "no_offer": *na as f64 / (*n).max(1) as f64, "no_bid": *nb as f64 / (*n).max(1) as f64}),
                );
            }
            out.insert("worst_no_offer".into(), json!(worst.iter().take(5).map(|(s, c)| json!([c, s])).collect::<Vec<_>>()));
            serde_json::Value::Object(out)
        };
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
            "book": book,
            "npc": npc,
            "market": m.stats,
            "npc_orders_per_s": total_orders as f64 / session_s,
            "events": self.events.len(),
            "ledger_by_reason": self.ledger_reasons.iter().map(|(k, v)| (k.clone(), *v as f64 / 100.0)).collect::<BTreeMap<_, _>>(),
            "invariants_ok": e.check_invariants().is_ok(),
            "revisions": revisions,
            "money": self.money,
            "stocks_detail": movers,
            "hot_liquidity": self.hot_liquidity(),
            "bubbles": self.bubble_report(),
            "agents_by_kind": kinds_json,
            "elapsed_s": elapsed,
        });
        (out, self.series)
    }
}
