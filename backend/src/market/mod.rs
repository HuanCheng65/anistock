//! The v2 NPC market (docs/v2-npc-market.md): a heterogeneous population of value, trend, news and retail
//! agents with their own beliefs and Poisson-timed actions, an improved market maker per stock, the rights
//! holders (committee), events, bounded fund flows, and the one-off float migration.
//!
//! The market owns only agent bookkeeping (who is who, watch lists, the action schedule, recent prices,
//! events); all money and shares live in the engine and every action is an ordinary engine operation, so
//! the ledger, invariants and persistence are unchanged. Beliefs are pure functions of
//! (seed, agent, company, time) plus the event list: nothing new is stored.
//!
//! Time is unix seconds; the game clock is Asia/Shanghai (UTC+8, no DST).

pub mod config;
pub mod events;
pub mod rng;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::engine::{Engine, NewOrder};
use crate::exchange::quote_band;
use crate::inputs::Inputs;
pub use config::MarketConfig;
pub use events::{Credibility, Event, Nature};
use rng::{hash, normal, unit, Rng};

pub const TZ_OFFSET: i64 = 8 * 3600;
const DAY: i64 = 86_400;

/// Local (UTC+8) day number and seconds into the day.
pub fn local(t: i64) -> (i64, i64) {
    ((t + TZ_OFFSET).div_euclid(DAY), (t + TZ_OFFSET).rem_euclid(DAY))
}

/// `2026-11-05T10:00:00+08:00`
pub fn ts_of(t: i64) -> String {
    let dt = chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap());
    crate::config::format_ts(&dt)
}

/// `YYYY-MM-DD` of the local day containing `t`.
pub fn day_of(t: i64) -> String {
    let (d, _) = local(t);
    chrono::NaiveDate::from_num_days_from_ce_opt((d + 719_163) as i32).unwrap().format("%Y-%m-%d").to_string()
}

/// Unix seconds of local `day` (YYYY-MM-DD) at `h:m`.
pub fn at_local(day: &str, h: i64, m: i64) -> i64 {
    let d = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("day");
    let days = d.signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days();
    days * DAY + h * 3600 + m * 60 - TZ_OFFSET
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum AgentKind {
    Value,
    Trend,
    News,
    Retail,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Value => "价值型",
            AgentKind::Trend => "趋势型",
            AgentKind::News => "消息型",
            AgentKind::Retail => "散户型",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub kind: AgentKind,
    pub account: i64,
    pub watch: Vec<i64>,
    /// starting cash, cents (fund flows are relative to it)
    pub start_cash: i64,
    pub threshold: f64,
    pub aggression: f64,
    pub lookback_s: i64,
}

#[derive(Debug, Clone)]
struct MmState {
    account: i64,
    /// EWMA of squared log price changes per second
    var_rate: f64,
    last_p: f64,
    last_t: i64,
}

/// Counters for reports.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MarketStats {
    pub actions: BTreeMap<String, u64>,
    pub orders: BTreeMap<String, u64>,
    pub rejected: u64,
    pub fund_in: i64,
    pub fund_out: i64,
    pub rebalance_shares: i64,
    pub flashes: u64,
    pub events: u64,
    /// most NPC orders in one minute, and the distribution of per-minute counts
    pub peak_orders_per_min: u64,
    pub minutes_with_orders: u64,
    #[serde(skip)]
    cur_minute: i64,
    #[serde(skip)]
    cur_count: u64,
    #[serde(skip)]
    pub per_minute: Vec<u64>,
}

/// Actor ids in the schedule: agents first, then one market maker per stock.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Actor {
    Agent(u32),
    Mm(i64),
}

pub struct Market {
    pub cfg: MarketConfig,
    pub agents: Vec<Agent>,
    mms: BTreeMap<i64, MmState>,
    schedule: BinaryHeap<Reverse<(i64, Actor)>>,
    pub events: Vec<Event>,
    heat: HashMap<i64, f64>,
    /// episode air times per company (unix seconds)
    air: HashMap<i64, Vec<i64>>,
    /// recent (t, last price) samples per company
    hist: HashMap<i64, VecDeque<(i64, i64)>>,
    known: HashSet<i64>,
    subscribed: HashSet<(usize, i64)>,
    rng: Rng,
    pub stats: MarketStats,
    pub now: i64,
    fees_commission: f64,
    sell_above: f64,
    committee_frac: f64,
    ipo_float: f64,
}

impl Market {
    pub fn new(cfg: MarketConfig, game: &crate::config::Config) -> Market {
        let ipo_float = match game.param("company.ipo_float") {
            Some(toml::Value::Float(f)) => *f,
            _ => 0.25,
        };
        Market {
            rng: Rng::new(hash(&[cfg.seed, 0x6d61726b6574])),
            cfg,
            agents: Vec::new(),
            mms: BTreeMap::new(),
            schedule: BinaryHeap::new(),
            events: Vec::new(),
            heat: HashMap::new(),
            air: HashMap::new(),
            hist: HashMap::new(),
            known: HashSet::new(),
            subscribed: HashSet::new(),
            stats: MarketStats::default(),
            now: 0,
            fees_commission: game.game.commission,
            sell_above: game.npc.committee_sell_above,
            committee_frac: game.npc.committee_sell_frac,
            ipo_float,
        }
    }

    // ------------------------------------------------------------ heat, air times, events

    /// Heat of each show from the day's inputs: its expected share of demand (weighted over markets),
    /// scaled so the listed companies average 1.
    pub fn set_heat(&mut self, engine: &Engine, inputs: &Inputs, weights: [f64; 3]) {
        let listed: Vec<i64> = engine.companies().filter(|c| c.status != "delisted").map(|c| c.id).collect();
        let mut tot = [0.0; 3];
        for id in &listed {
            if let Some(s) = inputs.show(*id) {
                let e = s.expected.m3();
                for m in 0..3 {
                    tot[m] += e[m];
                }
            }
        }
        let mut raw = HashMap::new();
        for id in &listed {
            let v = inputs.show(*id).map_or(0.0, |s| {
                let e = s.expected.m3();
                (0..3).map(|m| if tot[m] > 0.0 { weights[m] * e[m] / tot[m] } else { 0.0 }).sum::<f64>()
            });
            raw.insert(*id, v);
        }
        let n = listed.len().max(1) as f64;
        let total: f64 = raw.values().sum();
        self.heat.clear();
        for (id, v) in raw {
            // a floor so cold stocks still trade now and then
            self.heat.insert(id, if total > 0.0 { (v * n / total).max(0.05) } else { 1.0 });
        }
    }

    pub fn heat(&self, company: i64) -> f64 {
        self.heat.get(&company).copied().unwrap_or(0.3)
    }

    pub fn set_air_times(&mut self, air: HashMap<i64, Vec<i64>>) {
        self.air = air;
    }

    /// Announce an event: a news row in the game (players see it at the same moment) and, from `at`, an
    /// effect on the NPCs' beliefs and activity.
    pub fn post_event(&mut self, engine: &mut Engine, ev: Event, news_kind: Option<&str>) {
        if let Some(kind) = news_kind {
            let body = format!(
                "{}（强度 {:.0}/10，来源：{}）",
                match ev.direction {
                    d if d > 0 => "利好",
                    d if d < 0 => "利空",
                    _ => "中性",
                },
                ev.magnitude,
                ev.source
            );
            engine.post_news(&ts_of(ev.at), &day_of(ev.at), kind, Some(ev.company_id), ev.title.clone(), body);
        }
        self.stats.events += 1;
        if ev.kind == "flash" {
            self.stats.flashes += 1;
        }
        self.events.push(ev);
    }

    /// Unix seconds of the first daily close (06:30 local) after `t`.
    fn next_close(t: i64) -> i64 {
        let (d, s) = local(t);
        let close_s = 6 * 3600 + 30 * 60;
        let day = if s < close_s { d } else { d + 1 };
        day * DAY + close_s - TZ_OFFSET
    }

    /// Log belief shock of company `c` at `t` from the events in force.
    pub fn shock(&self, c: i64, t: i64) -> f64 {
        let mut s = 0.0;
        for e in &self.events {
            if e.company_id == c && e.direction != 0 {
                let w = e.belief_weight(t, Self::next_close(e.at));
                if w > 0.0 {
                    s += e.direction as f64 * e.magnitude / 10.0
                        * self.cfg.event_shock
                        * e.credibility_weight(&self.cfg.credibility)
                        * w;
                }
            }
        }
        s
    }

    /// Activity multiplier of company `c` at `t` (events and air-time windows).
    pub fn activity(&self, c: i64, t: i64) -> f64 {
        let mut a = 1.0;
        for e in &self.events {
            if e.company_id == c {
                a += self.cfg.event_activity * e.magnitude * e.activity_weight(t, self.cfg.event_activity_half_life_h);
            }
        }
        if let Some(times) = self.air.get(&c) {
            if times.iter().any(|&at| t >= at - 3600 && t <= at + 7200) {
                a *= self.cfg.air_activity;
            }
        }
        a
    }

    fn prune_events(&mut self, t: i64) {
        self.events.retain(|e| t - e.at < 8 * DAY);
    }

    // ------------------------------------------------------------ beliefs

    /// An agent's log valuation bias for a company at `t`: a daily AR(1) (half-life `bias_half_life_days`,
    /// stationary sd `bias_sigma`) from hashed innovations, interpolated within the day.
    pub fn bias(&self, agent: usize, c: i64, t: i64) -> f64 {
        let phi = 0.5f64.powf(1.0 / self.cfg.bias_half_life_days.max(0.1));
        let scale = self.cfg.bias_sigma * (1.0 - phi * phi).sqrt();
        let (d, s) = local(t);
        let at_day = |k: i64| -> f64 {
            let mut b = 0.0;
            let mut w = 1.0;
            for j in 0..48 {
                b += w * normal(&[self.cfg.seed, 0xb1a5, agent as u64, c as u64, (k - j) as u64]);
                w *= phi;
                if w < 1e-4 {
                    break;
                }
            }
            b * scale
        };
        let f = s as f64 / DAY as f64;
        at_day(d) * (1.0 - f) + at_day(d + 1) * f
    }

    /// What agent `agent` thinks one share of `c` is worth at `t`, cents (None without a hidden value).
    pub fn belief(&self, engine: &Engine, agent: usize, c: i64, t: i64) -> Option<f64> {
        let fair = engine.company(c)?.fair_value;
        if fair <= 0 {
            return None;
        }
        Some(fair as f64 * (self.bias(agent, c, t) + self.shock(c, t)).exp())
    }

    // ------------------------------------------------------------ population

    /// Create the agents' accounts with their starting cash (from the world account) and watch lists.
    /// Deterministic per seed. Call once (live: migration 002; simulation: at the start).
    pub fn create_population(&mut self, engine: &mut Engine, day: &str, ts: &str) {
        let world = engine.special("world").expect("world account");
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let kinds = [
            (AgentKind::Value, self.cfg.n_value, self.cfg.cash_value),
            (AgentKind::Trend, self.cfg.n_trend, self.cfg.cash_trend),
            (AgentKind::News, self.cfg.n_news, self.cfg.cash_news),
            (AgentKind::Retail, self.cfg.n_retail, self.cfg.cash_retail),
        ];
        for (kind, n, cash) in kinds {
            for _ in 0..n {
                let i = self.agents.len();
                let h = |k: u64| unit(hash(&[self.cfg.seed, 0xa6e7, i as u64, k]));
                let name = format!("{}NPC-{:04}", kind.label(), i + 1);
                let account = engine.create_account("noise", &name, None, None, None, ts);
                // starting cash varies ±50% between agents
                let start_cash = (cash * 100.0 * (0.5 + h(1))).round() as i64;
                engine.transfer(world, account, start_cash, "npc_fund_in", day, ts, None, None);
                let nw = self.cfg.watch_min + ((h(2) * (self.cfg.watch_max - self.cfg.watch_min + 1) as f64) as usize);
                let watch = self.draw_watch(&listed, nw, i as u64);
                let lb = self.cfg.trend_lookback_min;
                let agent = Agent {
                    kind,
                    account,
                    watch,
                    start_cash,
                    threshold: match kind {
                        AgentKind::Trend => lerp(self.cfg.trend_threshold, h(3)),
                        _ => lerp(self.cfg.value_threshold, h(3)),
                    },
                    aggression: lerp(self.cfg.value_aggression, h(4)),
                    lookback_s: (60.0 * (lb[0].ln() + (lb[1].ln() - lb[0].ln()) * h(5)).exp()) as i64,
                };
                self.agents.push(agent);
            }
        }
        for c in listed {
            self.known.insert(c);
        }
    }

    fn draw_watch(&self, listed: &[i64], n: usize, salt: u64) -> Vec<i64> {
        // weighted sampling without replacement (Efraimidis–Spirakis keys)
        let mut keyed: Vec<(f64, i64)> = listed
            .iter()
            .map(|&c| {
                let w = self.heat(c).powf(self.cfg.heat_alpha).max(1e-6);
                let u = unit(hash(&[self.cfg.seed, 0x3a7c, salt, c as u64]));
                (u.ln() / w, c)
            })
            .collect();
        keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        keyed.into_iter().take(n).map(|(_, c)| c).collect()
    }

    /// Re-create the agent list for an existing population (accounts named `<type>NPC-<n>`), e.g. after a
    /// restart: same seed, same watch lists and parameters.
    pub fn attach_population(&mut self, engine: &Engine) {
        // the accounts were created in order; their ids are found by name
        let by_name: HashMap<String, i64> =
            engine.accounts().filter(|a| a.kind == "noise").map(|a| (a.name.clone(), a.id)).collect();
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let kinds = [
            (AgentKind::Value, self.cfg.n_value, self.cfg.cash_value),
            (AgentKind::Trend, self.cfg.n_trend, self.cfg.cash_trend),
            (AgentKind::News, self.cfg.n_news, self.cfg.cash_news),
            (AgentKind::Retail, self.cfg.n_retail, self.cfg.cash_retail),
        ];
        self.agents.clear();
        for (kind, n, cash) in kinds {
            for _ in 0..n {
                let i = self.agents.len();
                let h = |k: u64| unit(hash(&[self.cfg.seed, 0xa6e7, i as u64, k]));
                let name = format!("{}NPC-{:04}", kind.label(), i + 1);
                let Some(&account) = by_name.get(&name) else { continue };
                let nw = self.cfg.watch_min + ((h(2) * (self.cfg.watch_max - self.cfg.watch_min + 1) as f64) as usize);
                let lb = self.cfg.trend_lookback_min;
                // the watch list is drawn from today's heat; whatever the agent already holds stays watched
                let mut watch = self.draw_watch(&listed, nw, i as u64);
                for h in engine.holdings_of(account) {
                    if h.qty > 0 && !watch.contains(&h.company_id) && listed.contains(&h.company_id) {
                        watch.push(h.company_id);
                    }
                }
                self.agents.push(Agent {
                    kind,
                    account,
                    watch,
                    start_cash: (cash * 100.0 * (0.5 + h(1))).round() as i64,
                    threshold: match kind {
                        AgentKind::Trend => lerp(self.cfg.trend_threshold, h(3)),
                        _ => lerp(self.cfg.value_threshold, h(3)),
                    },
                    aggression: lerp(self.cfg.value_aggression, h(4)),
                    lookback_s: (60.0 * (lb[0].ln() + (lb[1].ln() - lb[0].ln()) * h(5)).exp()) as i64,
                });
            }
        }
        for c in listed {
            self.known.insert(c);
        }
    }

    /// New listings join some watch lists (∝ heat); every listed stock gets its market maker scheduled.
    pub fn refresh_universe(&mut self, engine: &Engine, now: i64) {
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let avg_watch = (self.cfg.watch_min + self.cfg.watch_max) as f64 / 2.0;
        let n_listed = listed.len().max(1) as f64;
        for &c in &listed {
            if self.known.insert(c) {
                let p = (avg_watch / n_listed * self.heat(c).powf(self.cfg.heat_alpha)).min(0.9);
                for (i, a) in self.agents.iter_mut().enumerate() {
                    if unit(hash(&[self.cfg.seed, 0x9e3d, i as u64, c as u64])) < p {
                        a.watch.push(c);
                    }
                }
            }
        }
        // drop delisted stocks from watch lists; anything an agent holds (IPO allotments) is watched
        let live: HashSet<i64> = listed.iter().copied().collect();
        for a in &mut self.agents {
            a.watch.retain(|c| live.contains(c));
            for h in engine.holdings_of(a.account) {
                if h.qty > 0 && live.contains(&h.company_id) && !a.watch.contains(&h.company_id) {
                    a.watch.push(h.company_id);
                }
            }
        }
        for &c in &listed {
            if !self.mms.contains_key(&c) {
                if let Some(acct) = engine.account_of("mm", c) {
                    let p = engine.company(c).map_or(1.0, |x| x.last_price as f64);
                    self.mms.insert(c, MmState { account: acct, var_rate: 0.0, last_p: p, last_t: now });
                    let first = now + self.rng.exp(self.cfg.mm_rate / 3600.0) as i64;
                    self.schedule.push(Reverse((first, Actor::Mm(c))));
                }
            }
        }
        self.mms.retain(|c, _| live.contains(c));
        if self.schedule.iter().all(|Reverse((_, a))| !matches!(a, Actor::Agent(_))) {
            for i in 0..self.agents.len() {
                let t = now + self.next_gap(i);
                self.schedule.push(Reverse((t, Actor::Agent(i as u32))));
            }
        }
    }

    // ------------------------------------------------------------ the schedule

    fn base_rate(&self, kind: AgentKind) -> f64 {
        match kind {
            AgentKind::Value => self.cfg.rate_value,
            AgentKind::Trend => self.cfg.rate_trend,
            AgentKind::News => self.cfg.rate_news,
            AgentKind::Retail => self.cfg.rate_retail,
        }
    }

    /// Intraday activity factor at local seconds `s` (0 outside 08:45-23:00).
    pub fn curve(&self, s: i64) -> f64 {
        let hm = |h: i64, m: i64| h * 3600 + m * 60;
        let c = &self.cfg.curve;
        match s {
            x if x < hm(8, 45) => 0.0,
            x if x < hm(9, 0) => c[0],
            x if x < hm(12, 0) => c[1],
            x if x < hm(13, 30) => c[2],
            x if x < hm(18, 0) => c[3],
            x if x < hm(20, 0) => c[4],
            x if x < hm(23, 0) => c[5],
            _ => 0.0,
        }
    }

    fn peak(&self, kind: AgentKind) -> f64 {
        let cmax = self.cfg.curve.iter().cloned().fold(0.0, f64::max);
        match kind {
            // news agents wake up with events: thinning bound includes the event amplification
            AgentKind::News => cmax * (1.0 + self.cfg.event_activity * 10.0) * 3.0,
            _ => cmax,
        }
    }

    /// Candidate gap (seconds) for agent `i` at the bounding rate; acceptance thins it (`accept`).
    fn next_gap(&mut self, i: usize) -> i64 {
        let kind = self.agents[i].kind;
        let rate = self.base_rate(kind) * self.peak(kind) / 3600.0;
        (self.rng.exp(rate).ceil() as i64).max(1)
    }

    fn news_pressure(&self, i: usize, t: i64) -> f64 {
        let a = &self.agents[i];
        let mut p = 0.0;
        for e in &self.events {
            if e.direction != 0 && a.watch.contains(&e.company_id) {
                p += e.magnitude / 10.0 * e.activity_weight(t, self.cfg.event_activity_half_life_h);
            }
        }
        p
    }

    fn accept(&mut self, i: usize, t: i64) -> bool {
        let kind = self.agents[i].kind;
        let (_, s) = local(t);
        let mut f = self.curve(s);
        if f == 0.0 {
            return false;
        }
        if kind == AgentKind::News {
            // mostly idle; events wake them up
            f *= (0.3 + 10.0 * self.news_pressure(i, t)).min(1.0 + self.cfg.event_activity * 10.0) * 1.0;
        }
        self.rng.f64() < f / self.peak(kind)
    }

    /// Run every NPC action scheduled before `t_end` (unix seconds).
    pub fn step_until(&mut self, engine: &mut Engine, t_end: i64) {
        while let Some(&Reverse((t, actor))) = self.schedule.peek() {
            if t >= t_end {
                break;
            }
            self.schedule.pop();
            self.now = t;
            match actor {
                Actor::Agent(i) => {
                    let i = i as usize;
                    if i < self.agents.len() && self.accept(i, t) {
                        self.act(engine, i, t);
                    }
                    let next = t + self.next_gap(i);
                    self.schedule.push(Reverse((next, Actor::Agent(i as u32))));
                }
                Actor::Mm(c) => {
                    if !self.mms.contains_key(&c) {
                        continue;
                    }
                    let (_, s) = local(t);
                    if self.curve(s) > 0.0 {
                        self.act_mm(engine, c, t);
                    }
                    let gap = (self.rng.exp(self.cfg.mm_rate / 3600.0).ceil() as i64).max(1);
                    self.schedule.push(Reverse((t + gap, Actor::Mm(c))));
                }
            }
        }
        self.now = t_end.max(self.now);
    }

    // ------------------------------------------------------------ helpers

    fn phase_auction(t: i64) -> bool {
        let (_, s) = local(t);
        s < 9 * 3600
    }

    fn place(
        &mut self,
        engine: &mut Engine,
        label: &str,
        acct: i64,
        c: i64,
        side: &str,
        kind: &str,
        qty: i64,
        price: Option<i64>,
        t: i64,
    ) {
        if qty <= 0 {
            return;
        }
        let auction = Self::phase_auction(t);
        if auction && kind == "market" {
            return;
        }
        let (day, ts) = (day_of(t), ts_of(t));
        if !auction && engine.state("auction_day") != Some(day.as_str()) {
            engine.open_market(&day, &ts);
        }
        let r = engine.place_order(&NewOrder {
            account_id: acct,
            company_id: c,
            side,
            kind,
            qty,
            price,
            day: &day,
            ts: &ts,
            match_: !auction,
        });
        *self.stats.orders.entry(label.to_string()).or_default() += 1;
        let minute = t / 60;
        if minute != self.stats.cur_minute {
            if self.stats.cur_count > 0 {
                self.stats.per_minute.push(self.stats.cur_count);
                self.stats.minutes_with_orders += 1;
            }
            self.stats.cur_minute = minute;
            self.stats.cur_count = 0;
        }
        self.stats.cur_count += 1;
        self.stats.peak_orders_per_min = self.stats.peak_orders_per_min.max(self.stats.cur_count);
        if r.is_err() {
            self.stats.rejected += 1;
        }
        self.sample(engine, c, t);
    }

    fn sample(&mut self, engine: &Engine, c: i64, t: i64) {
        let Some(p) = engine.company(c).map(|x| x.last_price) else { return };
        let h = self.hist.entry(c).or_default();
        if h.back().is_none_or(|&(lt, lp)| t - lt >= 20 || lp != p) {
            h.push_back((t, p));
        }
        while h.front().is_some_and(|&(ft, _)| t - ft > DAY + 3600) {
            h.pop_front();
        }
    }

    /// Last sampled price at or before `t`.
    fn price_at(&self, c: i64, t: i64) -> Option<i64> {
        let h = self.hist.get(&c)?;
        let i = h.partition_point(|&(st, _)| st <= t);
        if i == 0 {
            None
        } else {
            Some(h[i - 1].1)
        }
    }

    fn choose_stock(&mut self, i: usize, t: i64, engine: &Engine) -> Option<i64> {
        let watch = self.agents[i].watch.clone();
        let w: Vec<f64> = watch
            .iter()
            .map(|&c| match engine.company(c) {
                Some(co) if co.status == "listed" => self.heat(c).powf(self.cfg.heat_alpha) * self.activity(c, t),
                _ => 0.0,
            })
            .collect();
        self.rng.weighted(&w).map(|k| watch[k])
    }

    fn wealth(&self, engine: &Engine, acct: i64) -> i64 {
        let cash = engine.account(acct).map_or(0, |a| a.cash);
        cash + engine
            .holdings_of(acct)
            .map(|h| h.qty * engine.company(h.company_id).map_or(0, |c| c.last_price))
            .sum::<i64>()
    }

    fn clamp_price(engine: &Engine, rules: &crate::exchange::Rules, c: i64, p: f64, t: i64) -> i64 {
        let co = engine.company(c).unwrap();
        let (lo, hi) = quote_band(co, &ts_of(t), rules);
        (p.round() as i64).clamp(lo.max(1), hi)
    }

    /// Shares affordable at `price` with `budget` cents (commission and the per-share reserve included).
    fn affordable(&self, budget: f64, price: i64) -> i64 {
        if price <= 0 || budget <= 0.0 {
            return 0;
        }
        (budget / (price as f64 * (1.0 + self.fees_commission) + 1.0)).floor() as i64
    }

    // ------------------------------------------------------------ agents

    fn act(&mut self, engine: &mut Engine, i: usize, t: i64) {
        let kind = self.agents[i].kind;
        *self.stats.actions.entry(format!("{kind:?}")).or_default() += 1;
        match kind {
            AgentKind::Value => self.act_value(engine, i, t),
            AgentKind::Trend => self.act_trend(engine, i, t),
            AgentKind::News => self.act_news(engine, i, t),
            AgentKind::Retail => self.act_retail(engine, i, t),
        }
    }

    fn act_value(&mut self, engine: &mut Engine, i: usize, t: i64) {
        let Some(c) = self.choose_stock(i, t, engine) else { return };
        let Some(b) = self.belief(engine, i, c, t) else { return };
        let a = self.agents[i].clone();
        let rules = engine.rules;
        let p = engine.company(c).unwrap().last_price.max(1) as f64;
        let m = (b / p).ln();
        engine.cancel_all(a.account, c, &ts_of(t));
        let conviction = ((m.abs() - a.threshold) / self.cfg.value_full).clamp(0.0, 1.0);
        if conviction <= 0.0 {
            return;
        }
        let held = engine.available_shares(a.account, c);
        if m > 0.0 {
            let wealth = self.wealth(engine, a.account) as f64;
            let room = self.cfg.max_position_frac * wealth - held as f64 * p;
            let budget = (engine.available_cash(a.account) as f64 * self.cfg.value_size * conviction).min(room);
            let limit = Self::clamp_price(engine, &rules, c, (p * (a.aggression * m).exp()).min(b), t);
            let q = self.affordable(budget, limit);
            self.place(engine, "value", a.account, c, "buy", "limit", q, Some(limit), t);
        } else if held > 0 {
            let q = ((held as f64 * self.cfg.value_size * 2.0 * conviction).ceil() as i64).min(held);
            let limit = Self::clamp_price(engine, &rules, c, (p * (a.aggression * m).exp()).max(b), t);
            self.place(engine, "value", a.account, c, "sell", "limit", q, Some(limit), t);
        }
    }

    fn act_trend(&mut self, engine: &mut Engine, i: usize, t: i64) {
        if Self::phase_auction(t) {
            return;
        }
        let Some(c) = self.choose_stock(i, t, engine) else { return };
        let a = self.agents[i].clone();
        let rules = engine.rules;
        let now_p = engine.company(c).unwrap().last_price.max(1);
        self.sample(engine, c, t);
        let Some(then) = self.price_at(c, t - a.lookback_s) else { return };
        let r = (now_p as f64 / then.max(1) as f64).ln();
        let held = engine.available_shares(a.account, c);
        engine.cancel_all(a.account, c, &ts_of(t));
        if r > a.threshold {
            let wealth = self.wealth(engine, a.account) as f64;
            let room = self.cfg.max_position_frac * wealth - held as f64 * now_p as f64;
            let strength = (r / a.threshold / 2.0).min(1.0);
            let budget = (engine.available_cash(a.account) as f64 * self.cfg.trend_size * strength).min(room);
            let limit = Self::clamp_price(engine, &rules, c, now_p as f64 * 1.01, t);
            let q = self.affordable(budget, limit);
            self.place(engine, "trend", a.account, c, "buy", "limit", q, Some(limit), t);
        } else if r < -a.threshold && held > 0 {
            let limit = Self::clamp_price(engine, &rules, c, now_p as f64 * 0.99, t);
            self.place(engine, "trend", a.account, c, "sell", "limit", held, Some(limit), t);
        } else if held > 0 && r.abs() < a.threshold / 3.0 && self.rng.f64() < 0.2 {
            // the move is over: take it off slowly
            let limit = Self::clamp_price(engine, &rules, c, now_p as f64 * 0.995, t);
            self.place(engine, "trend", a.account, c, "sell", "limit", (held + 1) / 2, Some(limit), t);
        }
    }

    fn act_news(&mut self, engine: &mut Engine, i: usize, t: i64) {
        if Self::phase_auction(t) {
            return;
        }
        let a = self.agents[i].clone();
        let rules = engine.rules;
        // the strongest live event among its watch list
        let mut best: Option<(f64, i64, i8, f64)> = None;
        for e in &self.events {
            if e.direction == 0 || !a.watch.contains(&e.company_id) {
                continue;
            }
            let w = e.magnitude / 10.0
                * e.activity_weight(t, self.cfg.event_activity_half_life_h)
                * e.credibility_weight(&self.cfg.credibility);
            if w > 0.05 && best.is_none_or(|b| w > b.0) {
                best = Some((w, e.company_id, e.direction, e.magnitude));
            }
        }
        match best {
            Some((w, c, dir, _)) if engine.company(c).is_some_and(|x| x.status == "listed") => {
                let p = engine.company(c).unwrap().last_price.max(1) as f64;
                let held = engine.available_shares(a.account, c);
                engine.cancel_all(a.account, c, &ts_of(t));
                if dir > 0 {
                    let wealth = self.wealth(engine, a.account) as f64;
                    let room = self.cfg.max_position_frac * wealth - held as f64 * p;
                    let budget = (engine.available_cash(a.account) as f64 * self.cfg.news_size * w.min(1.0)).min(room);
                    let limit = Self::clamp_price(engine, &rules, c, p * 1.02, t);
                    let q = self.affordable(budget, limit);
                    self.place(engine, "news", a.account, c, "buy", "limit", q, Some(limit), t);
                } else if held > 0 {
                    let q = ((held as f64 * (0.3 + w).min(1.0)).ceil() as i64).min(held);
                    let limit = Self::clamp_price(engine, &rules, c, p * 0.98, t);
                    self.place(engine, "news", a.account, c, "sell", "limit", q, Some(limit), t);
                }
            }
            _ => {
                // no event: unwind a position
                let held: Vec<(i64, i64)> = engine
                    .holdings_of(a.account)
                    .filter(|h| h.qty - h.qty_frozen > 0)
                    .map(|h| (h.company_id, h.qty - h.qty_frozen))
                    .collect();
                if held.is_empty() {
                    return;
                }
                let (c, q) = held[self.rng.below(held.len())];
                if engine.company(c).is_none_or(|x| x.status != "listed") {
                    return;
                }
                let p = engine.company(c).unwrap().last_price.max(1) as f64;
                let limit = Self::clamp_price(engine, &rules, c, p * 0.99, t);
                let q = ((q as f64 * self.cfg.news_unwind).ceil() as i64).min(q);
                self.place(engine, "news", a.account, c, "sell", "limit", q, Some(limit), t);
            }
        }
    }

    fn act_retail(&mut self, engine: &mut Engine, i: usize, t: i64) {
        let Some(c) = self.choose_stock(i, t, engine) else { return };
        let a = self.agents[i].clone();
        let rules = engine.rules;
        let p = engine.company(c).unwrap().last_price.max(1) as f64;
        let held = engine.available_shares(a.account, c);
        let sell = held > 0 && self.rng.f64() < 0.5;
        let market = !Self::phase_auction(t) && self.rng.f64() < self.cfg.retail_market_prob;
        let limit = Self::clamp_price(engine, &rules, c, p * (1.0 + self.cfg.retail_price_sd * self.rng.normal()), t);
        if sell {
            let q = ((held as f64 * self.rng.range(0.2, 1.0)).ceil() as i64).min(held);
            if market {
                let lim = Self::clamp_price(engine, &rules, c, p * 0.985, t);
                self.place(engine, "retail", a.account, c, "sell", "limit", q, Some(lim), t);
            } else {
                self.place(engine, "retail", a.account, c, "sell", "limit", q, Some(limit), t);
            }
        } else {
            let frac = self.rng.range(self.cfg.retail_size[0], self.cfg.retail_size[1]);
            let wealth = self.wealth(engine, a.account) as f64;
            let room = self.cfg.max_position_frac * wealth - held as f64 * p;
            let budget = (engine.available_cash(a.account) as f64 * frac).min(room);
            let mlim = Self::clamp_price(engine, &rules, c, p * 1.015, t);
            let q = self.affordable(budget, if market { mlim } else { limit });
            if market {
                self.place(engine, "retail", a.account, c, "buy", "limit", q, Some(mlim), t);
            } else {
                self.place(engine, "retail", a.account, c, "buy", "limit", q, Some(limit), t);
            }
        }
    }

    // ------------------------------------------------------------ market maker and committee

    fn act_mm(&mut self, engine: &mut Engine, c: i64, t: i64) {
        let Some(co) = engine.company(c).cloned() else { return };
        if co.status != "listed" {
            return;
        }
        let rules = engine.rules;
        let ts = ts_of(t);
        let st = self.mms.get_mut(&c).unwrap();
        let acct = st.account;
        // recent volatility from its own observations of the last price (never the order book)
        let p = co.last_price.max(1) as f64;
        let dt = (t - st.last_t).max(1) as f64;
        let r = (p / st.last_p.max(1.0)).ln();
        let decay = 0.5f64.powf(dt / (self.cfg.mm_vol_half_life_min * 60.0));
        st.var_rate = decay * st.var_rate + (1.0 - decay) * (r * r / dt);
        st.last_p = p;
        st.last_t = t;
        let sigma5 = (st.var_rate * 300.0).sqrt();

        engine.cancel_all(acct, c, &ts);
        let fair = co.fair_value as f64;
        let a = self.cfg.mm_anchor_to_price;
        let reference = if fair > 0.0 { (1.0 - a) * fair + a * p } else { p };
        let inv = engine.available_shares(acct, c);
        let target = ((co.shares as f64 * self.ipo_float * self.cfg.mm_target_frac).round() as i64).max(1);
        let x = ((inv - target) as f64 / target as f64).clamp(-1.0, 1.0);
        let center = reference * (-self.cfg.mm_inventory_skew * 0.1 * x).exp();
        let half = (self.cfg.mm_base_spread / 2.0 * center).max(self.cfg.mm_vol_mult * sigma5 * center).max(1.0);
        let (lo, hi) = quote_band(&co, &ts, &rules);
        let size = ((co.shares as f64 * self.cfg.mm_quote_frac).round() as i64).max(1);
        let mut last_bid = i64::MAX;
        let mut last_ask = i64::MIN;
        for k in 1..=self.cfg.mm_levels.max(1) {
            let off = half * (1.0 + (k - 1) as f64 * self.cfg.mm_level_step);
            let ask = ((center + off).ceil() as i64).clamp(lo + 1, hi.max(lo + 1)).max(last_ask + 1);
            let bid = ((center - off).floor() as i64).clamp(lo, ask - 1).min(last_bid - 1);
            let q = size * k as i64;
            let cash = engine.available_cash(acct);
            let qb = q.min(cash.div_euclid(engine.fees.buy_reserve(bid.max(1), 1).max(1)));
            if qb > 0 && bid >= lo && bid >= 1 {
                self.place(engine, "mm", acct, c, "buy", "limit", qb, Some(bid), t);
            }
            let qa = q.min(engine.available_shares(acct, c));
            if qa > 0 && ask <= hi {
                self.place(engine, "mm", acct, c, "sell", "limit", qa, Some(ask), t);
            }
            last_bid = bid;
            last_ask = ask;
        }

        // rights holders (v1 rule): trim when the price runs far above value
        if self.rng.f64() < self.cfg.committee_rate / self.cfg.mm_rate {
            if let Some(com) = engine.account_of("committee", c) {
                engine.cancel_all(com, c, &ts);
                let co = engine.company(c).unwrap().clone();
                if co.fair_value > 0 {
                    let ratio = co.last_price as f64 / co.fair_value as f64;
                    if ratio > self.sell_above {
                        let pace = if ratio < 1.6 {
                            1.0
                        } else if ratio < 2.0 {
                            3.0
                        } else {
                            10.0
                        };
                        let q = engine
                            .available_shares(com, c)
                            .min(((co.shares as f64 * self.committee_frac * pace).round() as i64).max(1));
                        let price = ((co.fair_value as f64 * self.sell_above).ceil() as i64).clamp(lo, hi);
                        self.place(engine, "committee", com, c, "sell", "limit", q, Some(price), t);
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ IPOs, fund flows, migration

    /// Value agents apply for open IPO subscriptions they find cheap (once per subscription).
    pub fn subscribe_ipos(&mut self, engine: &mut Engine, t: i64) {
        let day = day_of(t);
        let ts = ts_of(t);
        let open: Vec<(i64, i64)> = engine
            .companies()
            .filter(|c| {
                c.status == "subscribing"
                    && c.sub_start.as_deref().is_some_and(|s| s <= day.as_str())
                    && c.sub_end.as_deref().is_some_and(|e| day.as_str() <= e)
            })
            .map(|c| (c.id, c.ipo_price))
            .collect();
        for &(c, price) in &open {
            // the market maker takes its target inventory in the offer (it cannot quote without stock)
            if let Some(mm) = engine.account_of("mm", c) {
                if engine.ipo_application(c, mm).is_none() {
                    let co = engine.company(c).unwrap();
                    let target = (co.shares as f64 * self.ipo_float * self.cfg.mm_target_frac).round() as i64;
                    let q = target.min(self.affordable(engine.available_cash(mm) as f64 * 0.8, price));
                    if q > 0 {
                        let _ = engine.subscribe(mm, c, q, &day, &ts);
                    }
                }
            }
        }
        for (c, price) in open {
            let fair = engine.company(c).unwrap().fair_value.max(price) as f64;
            for i in 0..self.agents.len() {
                if self.agents[i].kind != AgentKind::Value || self.subscribed.contains(&(i, c)) {
                    continue;
                }
                // a third of the value agents look at any one new issue
                if unit(hash(&[self.cfg.seed, 0x1b0, i as u64, c as u64])) > 0.33 {
                    continue;
                }
                let b = fair * self.bias(i, c, t).exp();
                if b > price as f64 * (1.0 + self.agents[i].threshold) {
                    let budget = engine.available_cash(self.agents[i].account) as f64 * self.cfg.ipo_subscribe_frac;
                    let q = self.affordable(budget, price);
                    if q > 0 && engine.subscribe(self.agents[i].account, c, q, &day, &ts).is_ok() {
                        self.subscribed.insert((i, c));
                    }
                }
            }
        }
    }

    /// After a close: bounded fund flows between NPCs and the world account (zero-sum through the ledger).
    pub fn fund_flows(&mut self, engine: &mut Engine, day: &str, ts: &str) {
        let world = engine.special("world").expect("world");
        let cap_frac = self.cfg.fund_weekly_cap / 7.0;
        let mut flows: Vec<(i64, i64, i64)> = Vec::new();
        for a in &self.agents {
            let cash = engine.account(a.account).map_or(0, |x| x.cash);
            let cap = (a.start_cash as f64 * cap_frac).round() as i64;
            if (cash as f64) < self.cfg.fund_floor * a.start_cash as f64 {
                let need = (self.cfg.fund_floor * a.start_cash as f64) as i64 - cash;
                flows.push((world, a.account, need.min(cap)));
            } else if cash as f64 > self.cfg.fund_ceiling * a.start_cash as f64 {
                // income (dividends, interest, profits) piling up as cash flows back out, a little a day
                let free = engine.available_cash(a.account);
                let excess = cash - (self.cfg.fund_ceiling * a.start_cash as f64) as i64;
                flows.push((a.account, world, excess.min(cap).min(free)));
            }
        }
        let mm_start = (self.cfg.mm_cash * 100.0) as i64;
        for st in self.mms.values() {
            let cash = engine.account(st.account).map_or(0, |x| x.cash);
            let cap = (mm_start as f64 * cap_frac * 3.0).round() as i64;
            if (cash as f64) < self.cfg.fund_floor * mm_start as f64 {
                flows.push((world, st.account, ((self.cfg.fund_floor * mm_start as f64) as i64 - cash).min(cap)));
            } else if cash > 3 * mm_start {
                let free = engine.available_cash(st.account);
                flows.push((st.account, world, (cash - 3 * mm_start).min(cap).min(free)));
            }
        }
        for (from, to, amt) in flows {
            if amt > 0 {
                let reason = if from == world { "npc_fund_in" } else { "npc_fund_out" };
                engine.transfer(from, to, amt, reason, day, ts, None, None);
                if from == world {
                    self.stats.fund_in += amt;
                } else {
                    self.stats.fund_out += amt;
                }
            }
        }
    }

    /// The one-off float migration (§8): each market maker keeps `mm_target_frac` of the public float and
    /// sells the rest at the current price to value and retail agents who watch the stock (paid with money
    /// from the world account); its cash is then set to `mm_cash`, the difference going to or from the
    /// world account. Reason `npc_rebalance`; players are not touched.
    pub fn rebalance_float(&mut self, engine: &mut Engine, day: &str, ts: &str) {
        let world = engine.special("world").expect("world");
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        for c in listed {
            let Some(mm) = engine.account_of("mm", c) else { continue };
            engine.cancel_all(mm, c, ts);
            let co = engine.company(c).unwrap().clone();
            let target = (co.shares as f64 * self.ipo_float * self.cfg.mm_target_frac).round() as i64;
            let excess = engine.available_shares(mm, c) - target;
            let price = co.last_price.max(1);
            if excess > 0 {
                let takers: Vec<(usize, f64)> = self
                    .agents
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| matches!(a.kind, AgentKind::Value | AgentKind::Retail) && a.watch.contains(&c))
                    .map(|(i, a)| {
                        let w = if a.kind == AgentKind::Value { 2.0 } else { 1.0 };
                        (i, w * (0.5 + unit(hash(&[self.cfg.seed, 0x7eba, i as u64, c as u64]))))
                    })
                    .collect();
                if takers.is_empty() {
                    continue;
                }
                let total: f64 = takers.iter().map(|x| x.1).sum();
                // largest remainder
                let mut alloc: Vec<(usize, i64, f64)> = takers
                    .iter()
                    .map(|&(i, w)| {
                        let exact = excess as f64 * w / total;
                        (i, exact.floor() as i64, exact - exact.floor())
                    })
                    .collect();
                let mut left = excess - alloc.iter().map(|x| x.1).sum::<i64>();
                let mut order: Vec<usize> = (0..alloc.len()).collect();
                order.sort_by(|&a, &b| alloc[b].2.partial_cmp(&alloc[a].2).unwrap().then(a.cmp(&b)));
                for k in order {
                    if left <= 0 {
                        break;
                    }
                    alloc[k].1 += 1;
                    left -= 1;
                }
                for (i, q, _) in alloc {
                    if q <= 0 {
                        continue;
                    }
                    let acct = self.agents[i].account;
                    let cost = q * price;
                    engine.transfer(world, acct, cost, "npc_rebalance", day, ts, Some(c), None);
                    engine.transfer(acct, mm, cost, "npc_rebalance", day, ts, Some(c), None);
                    engine.remove_shares(mm, c, q, 0).expect("market maker shares");
                    engine.add_shares(acct, c, q, price, day);
                    self.stats.rebalance_shares += q;
                }
            }
            let cash = engine.account(mm).map_or(0, |a| a.cash);
            let want = (self.cfg.mm_cash * 100.0) as i64;
            if cash > want {
                engine.transfer(mm, world, cash - want, "npc_rebalance", day, ts, Some(c), None);
            } else if cash < want {
                engine.transfer(world, mm, want - cash, "npc_rebalance", day, ts, Some(c), None);
            }
        }
    }

    /// Wealth of all NPC agents (cash + holdings at last prices), cents.
    pub fn npc_wealth(&self, engine: &Engine) -> (i64, i64) {
        let agents: i64 = self.agents.iter().map(|a| self.wealth(engine, a.account)).sum();
        let mms: i64 = self.mms.values().map(|m| self.wealth(engine, m.account)).sum();
        (agents, mms)
    }

    pub fn prune(&mut self, t: i64) {
        self.prune_events(t);
    }
}

fn lerp(r: [f64; 2], u: f64) -> f64 {
    r[0] + (r[1] - r[0]) * u
}
