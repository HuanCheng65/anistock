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
pub mod runtime;
mod short;

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::engine::{Engine, NewOrder};
use crate::exchange::quote_band;
use crate::inputs::Inputs;
pub use config::MarketConfig;
pub use events::{Credibility, Event, Nature};
use rng::{hash, normal, unit, Rng};

pub const TZ_OFFSET: i64 = 8 * 3600;
const DAY: i64 = 86_400;
/// The trading day changes at 04:00 local (`game.day_rollover`; checked at startup): the night session
/// 00:00-04:00 belongs to the previous day.
pub const ROLLOVER_S: i64 = 4 * 3600;

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

/// `YYYY-MM-DD` of the trading day containing `t` (the local date of `t` - 04:00).
pub fn day_of(t: i64) -> String {
    let (d, _) = local(t - ROLLOVER_S);
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
    /// bounce-free volatility (`mm_vol_roll`): EWMAs of the squared change, of the product of consecutive
    /// changes and of the interval, and the last change
    roll: [f64; 3],
    last_r: f64,
    last_p: f64,
    last_t: i64,
    /// inventory at the last requote, and an EWMA of the net shares it bought (+) or sold (-) since, in
    /// quote units: one-sided flow is the toxicity a dealer widens against
    last_inv: Option<i64>,
    /// EWMA of what its recent fills lost by the next requote (markout), in quote units of notional
    toxicity: f64,
    /// human activity in the stock: player-traded notional at the last requote (engine counter), and an
    /// EWMA of the rate, 币 per hour; players' resting depth near the mid, 币
    human_flow_seen: Option<i64>,
    human_rate: f64,
    human_depth: f64,
    /// last price at the previous requote
    last_quote_p: f64,
    /// the last requote's half-spread (share of the centre) and its parts: base, volatility term, the
    /// toxicity and information-window multipliers (reports)
    last_half: [f64; 4],
    /// `mm_follow_revisions`: log change of the public value since the last trade at `news_p` (the market
    /// price has not yet seen it); the quotes move with it at once
    news: f64,
    news_p: i64,
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
    /// retail agents' decisions on a holding at a paper gain: looked at, sold; at a paper loss: looked at, sold
    pub retail_realise: [u64; 4],
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
    /// the futures market maker of one contract (crate::futures::mm)
    Fmm(i64),
    /// one of the other futures traders (crate::futures::agents), by index
    Ftrader(u32),
}

pub struct Market {
    pub cfg: MarketConfig,
    pub agents: Vec<Agent>,
    mms: BTreeMap<i64, MmState>,
    /// futures contracts whose market maker is scheduled
    fmms: BTreeSet<i64>,
    /// the other futures traders, the companies whose committee hedges, and the 热度快讯 they see
    ftraders: Vec<crate::futures::agents::Trader>,
    fhedged: BTreeSet<i64>,
    fflashes: Vec<crate::futures::agents::Flash>,
    schedule: BinaryHeap<Reverse<(i64, Actor)>>,
    pub events: Vec<Event>,
    heat: HashMap<i64, f64>,
    /// episode air times per company (unix seconds)
    air: HashMap<i64, Vec<i64>>,
    /// recent (t, last price) samples per company
    hist: HashMap<i64, VecDeque<(i64, i64)>>,
    known: HashSet<i64>,
    subscribed: HashSet<(usize, i64)>,
    /// the public value at the last revision check
    last_fair: HashMap<i64, i64>,
    /// stocks to requote after a revision (`requote_revised`)
    requote: Vec<i64>,
    /// (company, episode) whose 热度快讯 has been published (one per episode; later hours only revise)
    flashed: std::collections::HashSet<(i64, u64)>,
    /// sentiment layer: the salt mixed into its draws (from the server secret), the sentiment of the
    /// current trading day (day number, level), premiere air times, (day, company) already on the movers'
    /// lists today, the next movers' check, and the over/under-reaction of each revision (company, at) ->
    /// (multiplier, half-life h)
    salt: u64,
    sent_cache: Option<(i64, f64)>,
    sens_mean: f64,
    premiere_at: HashMap<i64, i64>,
    ranked: HashSet<(i64, i64)>,
    next_rank_check: i64,
    reactions: HashMap<(i64, i64), (f64, f64)>,
    /// sentiment layer batch 2: each stock's recent daily closes (bubble triggers; kept in the state table)
    closes: HashMap<i64, VecDeque<i64>>,
    /// share of the retail agents' holdings at a paper gain (disposition effect; refreshed at each close)
    gain_share: Option<f64>,
    /// run starts at the last close: (company, close time, became a bubble) (reports)
    pub run_starts: Vec<(i64, i64, bool)>,
    /// events the market created itself (revisions, attention), for the caller to log (restart safety)
    pub fresh: Vec<Event>,
    rng: Rng,
    pub stats: MarketStats,
    pub now: i64,
    fees_commission: f64,
    sell_above: f64,
    committee_frac: f64,
    ipo_float: f64,
    /// price adjustments (splits, ex-rights: crate::corporate) already applied to the price memory
    adj_seen: Option<usize>,
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
            fmms: BTreeSet::new(),
            ftraders: Vec::new(),
            fhedged: BTreeSet::new(),
            fflashes: Vec::new(),
            schedule: BinaryHeap::new(),
            events: Vec::new(),
            heat: HashMap::new(),
            air: HashMap::new(),
            hist: HashMap::new(),
            known: HashSet::new(),
            subscribed: HashSet::new(),
            last_fair: HashMap::new(),
            requote: Vec::new(),
            flashed: Default::default(),
            salt: secret_salt(&game.secret),
            sent_cache: None,
            sens_mean: 1.0,
            premiere_at: HashMap::new(),
            ranked: HashSet::new(),
            next_rank_check: 0,
            reactions: HashMap::new(),
            closes: HashMap::new(),
            gain_share: None,
            run_starts: Vec::new(),
            fresh: Vec::new(),
            stats: MarketStats::default(),
            now: 0,
            fees_commission: game.game.commission,
            sell_above: game.npc.committee_sell_above,
            committee_frac: game.npc.committee_sell_frac,
            ipo_float,
            adj_seen: None,
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
        // sentiment sensitivities average 1 over the listed stocks
        let raw: Vec<f64> = self.heat.keys().map(|c| self.sentiment_sens_raw(*c)).collect();
        self.sens_mean = if raw.is_empty() { 1.0 } else { raw.iter().sum::<f64>() / raw.len() as f64 };
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

    /// Scheduled or fresh information about `c` at `t`: from 1 h before an episode airs until 2 h after its
    /// 热度快讯 is due, 2 h after any event, and the first half hour of the session (after the close's
    /// revisions). All of it is public.
    fn info_window(&self, c: i64, t: i64) -> bool {
        let (_, s) = local(t);
        if (8 * 3600 + 45 * 60..9 * 3600 + 30 * 60).contains(&s) {
            return true;
        }
        let after = ((self.cfg.flash_delay_h + 2.0) * 3600.0) as i64;
        if self.air.get(&c).is_some_and(|ts| ts.iter().any(|&at| t >= at - 3600 && t <= at + after)) {
            return true;
        }
        self.events
            .iter()
            .any(|e| e.company_id == c && e.kind != "attention" && e.kind != "bubble" && t >= e.at && t - e.at <= 7200)
    }

    /// The market maker's last quote in `c`: its half-spread parts [base, volatility term, toxicity multiplier,
    /// information-window multiplier] (reports).
    pub fn mm_quote_parts(&self, c: i64) -> Option<[f64; 4]> {
        self.mms.get(&c).map(|m| m.last_half)
    }

    /// Is `t` in an information window for `c` (reports)?
    pub fn in_info_window(&self, c: i64, t: i64) -> bool {
        self.info_window(c, t)
    }

    /// Adaptive liquidity: the retail and trend agents' activity in `c`, ×`adapt_rate_max` when no human
    /// trades or rests orders there, down to ×`adapt_rate_min` when humans are busy (their traded notional
    /// per hour plus resting depth, against `adapt_activity_ref` 币).
    pub fn rate_mult(&self, c: i64) -> f64 {
        let (lo, hi) = (self.cfg.adapt_rate_min, self.cfg.adapt_rate_max);
        let h = self.mms.get(&c).map_or(0.0, |m| m.human_rate + m.human_depth);
        lo + (hi - lo) * (-h / self.cfg.adapt_activity_ref.max(1.0)).exp()
    }

    fn prune_events(&mut self, t: i64) {
        self.events.retain(|e| t - e.at < 8 * DAY);
    }

    // ------------------------------------------------------------ sentiment layer (docs/v2-sentiment.md)

    /// Market sentiment of the trading day containing `t`, standard units (0 when off).
    fn sentiment_at(&mut self, t: i64) -> f64 {
        if self.cfg.sentiment_strength == 0.0 {
            return 0.0;
        }
        let (d, _) = local(t - ROLLOVER_S);
        match self.sent_cache {
            Some((cd, v)) if cd == d => v,
            _ => {
                let v = sentiment_level(&self.cfg, self.salt, d);
                self.sent_cache = Some((d, v));
                v
            }
        }
    }

    /// Band of the market sentiment on trading day `day` (-1 冷淡, 0 平稳, 1 火热; None when off).
    pub fn sentiment_band(&self, day: &str) -> Option<i8> {
        sentiment_band(&self.cfg, self.salt, day)
    }

    /// Market sentiment of trading day `day`, standard units (None when off; for reports).
    pub fn sentiment_of_day(&self, day: &str) -> Option<f64> {
        if self.cfg.sentiment_strength == 0.0 {
            return None;
        }
        Some(sentiment_level(&self.cfg, self.salt, day_number(day)?))
    }

    /// How sentiment-prone stock `c` is: (1 / heat)^x within `sentiment_sens` (Baker–Wurgler: small, young,
    /// hard-to-value stocks move most with sentiment; here heat stands for size and coverage).
    fn sentiment_sens_raw(&self, c: i64) -> f64 {
        let [lo, hi] = self.cfg.sentiment_sens;
        (1.0 / self.heat(c).max(1e-6)).powf(self.cfg.sentiment_heat_exp).clamp(lo, hi)
    }

    /// The same, scaled so the listed stocks average 1 (`sentiment_strength` is then the tilt per standard
    /// deviation for the average stock; the coldest are `sentiment_sens[1] / sentiment_sens[0]` times as
    /// sensitive as the hottest).
    fn sentiment_sens(&self, c: i64) -> f64 {
        self.sentiment_sens_raw(c) / self.sens_mean.max(1e-6)
    }

    /// The log tilt market sentiment gives a retail or trend agent's appetite for stock `c` at `t`.
    fn sentiment_tilt(&mut self, c: i64, t: i64) -> f64 {
        if self.cfg.sentiment_strength == 0.0 {
            return 0.0;
        }
        self.cfg.sentiment_strength * self.sentiment_at(t) * self.sentiment_sens(c)
    }

    /// Attention on stock `c` at `t` (Barber–Odean): the sum of live attention episodes, each triggered by the
    /// premiere night, a published 热度快讯 or entering the movers' lists; whether a trigger draws attention,
    /// how much and for how long are random (hashed, so the same after a restart).
    pub fn attention(&self, c: i64, t: i64) -> f64 {
        if self.cfg.attention == 0.0 {
            return 0.0;
        }
        let mut a = 0.0;
        if let Some(&t0) = self.premiere_at.get(&c) {
            a += self.attention_episode(c, t0, 1, 1.0, t);
        }
        for e in &self.events {
            if e.kind == "attention" && e.company_id == c {
                let (code, w) = if e.source == "rank" { (3, self.cfg.attention_rank_weight) } else { (2, 1.0) };
                a += self.attention_episode(c, e.at, code, w, t);
            }
        }
        a
    }

    fn attention_episode(&self, c: i64, t0: i64, code: u64, w: f64, t: i64) -> f64 {
        if t < t0 {
            return 0.0;
        }
        let key = |j: u64| [self.cfg.seed, 0xa77e, self.salt, c as u64, t0 as u64, code, j];
        if unit(hash(&key(1))) >= self.cfg.attention_prob {
            return 0.0;
        }
        let sg = self.cfg.attention_sigma;
        let strength = (sg * normal(&key(2)) - sg * sg / 2.0).exp();
        let hl = log_uniform(self.cfg.attention_half_life_h, unit(hash(&key(3))));
        let age_h = (t - t0) as f64 / 3600.0;
        if age_h > 12.0 * hl {
            return 0.0;
        }
        w * strength * 0.5f64.powf(age_h / hl)
    }

    /// An attention trigger for `c` at `t` (`source`: flash | rank), kept with the events (and logged).
    fn post_attention(&mut self, c: i64, t: i64, source: &str) {
        if self.cfg.attention == 0.0 {
            return;
        }
        let ev = Event {
            company_id: c,
            kind: "attention".into(),
            direction: 0,
            magnitude: 0.0,
            credibility: Credibility::Media,
            source: source.into(),
            at: t,
            nature: Nature::Sentiment,
            half_life_h: 0.0,
            title: String::new(),
            change: 0.0,
        };
        self.fresh.push(ev.clone());
        self.events.push(ev);
    }

    /// 上热榜: stocks entering the day's top gainers or losers (the lists on the market page) draw attention,
    /// once per stock and trading day. Checked every half hour of the session.
    fn note_movers(&mut self, engine: &Engine, t: i64) {
        let n = self.cfg.attention_rank_n;
        if n == 0 || self.cfg.attention == 0.0 {
            return;
        }
        let (_, s) = local(t);
        if self.curve(s) == 0.0 || (8 * 3600..9 * 3600 + 1800).contains(&s) {
            return;
        }
        let mut moves: Vec<(f64, i64)> = engine
            .companies()
            .filter(|c| c.status == "listed" && c.prev_close > 0 && c.last_price > 0)
            .map(|c| (c.last_price as f64 / c.prev_close as f64 - 1.0, c.id))
            .collect();
        moves.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let gainers = moves.iter().filter(|m| m.0 > 0.0).take(n).map(|m| m.1);
        let losers = moves.iter().rev().filter(|m| m.0 < 0.0).take(n).map(|m| m.1);
        let picked: Vec<i64> = gainers.chain(losers).collect();
        let (today, _) = local(t - ROLLOVER_S);
        for c in picked {
            if self.ranked.insert((today, c)) {
                self.post_attention(c, t, "rank");
            }
        }
    }

    /// Reaction multiplier and half-life (hours) of the news agents to event `e`: for a revision with the
    /// reaction layer on, from its streak (Barberis–Shleifer–Vishny); otherwise (1, the usual half-life).
    fn reaction_of(&self, e: &Event) -> (f64, f64) {
        if self.cfg.reaction && e.kind == "revision" {
            if let Some(&r) = self.reactions.get(&(e.company_id, e.at)) {
                return r;
            }
        }
        (1.0, self.cfg.event_activity_half_life_h)
    }

    /// Index the revisions not yet indexed: the streak of each (same-sign revisions of the company in a row,
    /// each within `reaction_window_days` of the previous) and its random reaction. The first surprise in a
    /// direction is under-weighted (a factor drawn from `reaction_first`), the k-th in a row over-weighted
    /// (1 + (k - 1) * step, step drawn from `reaction_step`, at most `reaction_max`); the half-life of the
    /// reaction is drawn too. Call after events are added (they are in time order).
    pub fn index_reactions(&mut self) {
        // stocks already on the movers' lists (also after a restart: the triggers come back from the log)
        let ranked: Vec<(i64, i64)> = self
            .events
            .iter()
            .filter(|e| e.kind == "attention" && e.source == "rank")
            .map(|e| (local(e.at - ROLLOVER_S).0, e.company_id))
            .collect();
        self.ranked.extend(ranked);
        if !self.cfg.reaction {
            return;
        }
        let window = (self.cfg.reaction_window_days * 86_400.0) as i64;
        let mut last: HashMap<i64, (i64, f64, u32)> = HashMap::new();
        let mut out = Vec::new();
        for e in self.events.iter().filter(|e| e.kind == "revision" && e.change != 0.0) {
            let k = match last.get(&e.company_id) {
                Some(&(at, ch, k)) if e.at - at <= window && (ch > 0.0) == (e.change > 0.0) => k + 1,
                _ => 1,
            };
            last.insert(e.company_id, (e.at, e.change, k));
            if self.reactions.contains_key(&(e.company_id, e.at)) {
                continue;
            }
            let u = |j: u64| unit(hash(&[self.cfg.seed, 0xb5f1, self.salt, e.company_id as u64, e.at as u64, j]));
            let mult = if k == 1 {
                lerp(self.cfg.reaction_first, u(1))
            } else {
                (1.0 + (k - 1) as f64 * lerp(self.cfg.reaction_step, u(2))).min(self.cfg.reaction_max)
            };
            let hl = log_uniform(self.cfg.reaction_half_life_h, u(3));
            out.push(((e.company_id, e.at), (mult, hl)));
        }
        self.reactions.extend(out);
    }

    /// The news agents' reaction (multiplier, half-life h) to the revision of `c` at `at`, once indexed.
    pub fn reaction(&self, c: i64, at: i64) -> Option<(f64, f64)> {
        self.reactions.get(&(c, at)).copied()
    }

    /// Length of the current streak of same-sign revisions of `c` (0: none).
    pub fn streak(&self, c: i64) -> u32 {
        let window = (self.cfg.reaction_window_days * 86_400.0) as i64;
        let mut k = 0;
        let mut prev: Option<(i64, f64)> = None;
        for e in self.events.iter().rev().filter(|e| e.kind == "revision" && e.company_id == c && e.change != 0.0) {
            match prev {
                None => k = 1,
                Some((at, ch)) if at - e.at <= window && (ch > 0.0) == (e.change > 0.0) => k += 1,
                _ => break,
            }
            prev = Some((e.at, e.change));
        }
        k
    }

    // ------------------------------------------------------------ sentiment layer, batch 2

    /// After a close: record each listed stock's close and start the bubbles (De Long et al. positive
    /// feedback). A run starts when a stock at least `bubble_heat` hot has risen `bubble_run` or more over the
    /// last `bubble_run_days` closes and had not at the close before; it becomes a bubble with probability
    /// `bubble_prob` (a hash of seed, secret, stock and time, like every draw of the layer). A bubble is an
    /// event (logged, so it survives a restart); the closes are kept in the state table.
    pub fn note_closes(&mut self, engine: &mut Engine, t: i64) {
        if self.cfg.disposition_ratio != 1.0 {
            self.refresh_gain_share(engine);
        }
        if self.cfg.bubble_prob <= 0.0 {
            return;
        }
        let n = self.cfg.bubble_run_days.max(1);
        if self.closes.is_empty() {
            // after a restart: the closes kept in the state table
            for part in engine.state("npc_closes").unwrap_or("").split(';').filter(|p| !p.is_empty()) {
                if let Some((c, ps)) = part.split_once(':') {
                    if let Ok(c) = c.parse::<i64>() {
                        self.closes.insert(c, ps.split(',').filter_map(|x| x.parse().ok()).collect());
                    }
                }
            }
        }
        let listed: Vec<(i64, i64)> =
            engine.companies().filter(|c| c.status == "listed" && c.last_price > 0).map(|c| (c.id, c.last_price)).collect();
        let live: HashSet<i64> = listed.iter().map(|x| x.0).collect();
        self.closes.retain(|c, _| live.contains(c));
        let mut started = Vec::new();
        for (c, p) in listed {
            let h = self.closes.entry(c).or_default();
            h.push_back(p);
            while h.len() > n + 2 {
                h.pop_front();
            }
            let k = h.len();
            if k < n + 1 {
                continue;
            }
            let run = |end: usize| (h[end] as f64 / h[end - n].max(1) as f64) - 1.0;
            let now = run(k - 1);
            let before = if k >= n + 2 { run(k - 2) } else { 0.0 };
            if now >= self.cfg.bubble_run && before < self.cfg.bubble_run && self.heat(c) >= self.cfg.bubble_heat {
                started.push(c);
            }
        }
        let mut state = String::new();
        let mut ids: Vec<&i64> = self.closes.keys().collect();
        ids.sort();
        for c in ids {
            let v: Vec<String> = self.closes[c].iter().map(|x| x.to_string()).collect();
            state.push_str(&format!("{c}:{};", v.join(",")));
        }
        engine.set_state("npc_closes", &state);
        self.run_starts.clear();
        for c in started {
            // one bubble at a time per stock
            if self.events.iter().any(|e| e.kind == "bubble" && e.company_id == c && self.bubble_phase(e, t).is_some()) {
                continue;
            }
            let hit = unit(hash(&[self.cfg.seed, 0xb0bb1e, self.salt, c as u64, t as u64, 0])) < self.cfg.bubble_prob;
            self.run_starts.push((c, t, hit));
            if !hit {
                continue;
            }
            let ev = Event {
                company_id: c,
                kind: "bubble".into(),
                direction: 0,
                magnitude: 0.0,
                credibility: Credibility::Media,
                source: "trend".into(),
                at: t,
                nature: Nature::Sentiment,
                half_life_h: 0.0,
                title: String::new(),
                change: 0.0,
            };
            self.stats.events += 1;
            self.fresh.push(ev.clone());
            self.events.push(ev);
        }
    }

    /// The draws of the bubble started by event `e`: strength s (log-normal, mean 1), when it bursts (constant
    /// hazard: an exponential life whose mean is drawn from `bubble_life_days`, at most `bubble_max_days`) and
    /// how long the chasers dump after the burst.
    pub fn bubble_draws(&self, e: &Event) -> (f64, i64, i64) {
        let key = |j: u64| [self.cfg.seed, 0xb0bb1e, self.salt, e.company_id as u64, e.at as u64, j];
        let sg = self.cfg.bubble_sigma;
        let s = (sg * normal(&key(1)) - sg * sg / 2.0).exp();
        let mean_life = log_uniform(self.cfg.bubble_life_days, unit(hash(&key(2))));
        let crash_h = log_uniform(self.cfg.bubble_crash_h, unit(hash(&key(4))));
        // the whole episode stays within the 8 days of events kept (`prune_events`)
        let life = (-mean_life * (1.0 - unit(hash(&key(3)))).ln()).min(self.cfg.bubble_max_days).min(7.9 - crash_h / 24.0);
        let burst = e.at + (life * 86_400.0) as i64;
        (s, burst, burst + (crash_h * 3600.0) as i64)
    }

    /// Phase of bubble event `e` at `t`: Some((true, s)) inflating, Some((false, s)) after the burst, None over.
    fn bubble_phase(&self, e: &Event, t: i64) -> Option<(bool, f64)> {
        if t < e.at {
            return None;
        }
        let (s, burst, end) = self.bubble_draws(e);
        if t < burst {
            Some((true, s))
        } else if t < end {
            Some((false, s))
        } else {
            None
        }
    }

    /// The bubble in stock `c` at `t`, if any: (inflating?, strength).
    pub fn bubble(&self, c: i64, t: i64) -> Option<(bool, f64)> {
        if self.cfg.bubble_prob <= 0.0 {
            return None;
        }
        self.events.iter().filter(|e| e.kind == "bubble" && e.company_id == c).find_map(|e| self.bubble_phase(e, t))
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
        // all information reaches the agents through the public value (docs/v2-valuation.md §4)
        Some(fair as f64 * self.bias(agent, c, t).exp())
    }

    // ------------------------------------------------------------ population

    /// Create the agents' accounts with their starting cash (from the world account) and watch lists.
    /// Deterministic per seed. Call once (live: migration 002; simulation: at the start).
    pub fn create_population(&mut self, engine: &mut Engine, day: &str, ts: &str) {
        let world = engine.special("world").expect("world account");
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let n = self.population_counts(listed.len());
        engine.set_state("npc_population", &format!("{},{},{},{}", n[0], n[1], n[2], n[3]));
        let kinds = [
            (AgentKind::Value, n[0], self.cfg.cash_value),
            (AgentKind::Trend, n[1], self.cfg.cash_trend),
            (AgentKind::News, n[2], self.cfg.cash_news),
            (AgentKind::Retail, n[3], self.cfg.cash_retail),
        ];
        for (kind, n, cash) in kinds {
            for _ in 0..n {
                let i = self.agents.len();
                let h = |k: u64| unit(hash(&[self.cfg.seed, 0xa6e7, i as u64, k]));
                let name = format!("{}NPC-{:04}", kind.label(), i + 1);
                let account = engine.create_account("npc", &name, None, None, None, ts);
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

    /// Grow a running population to the configured counts (`population_counts` for the stocks listed now):
    /// the missing agents of each type are appended as a new block (recorded in `npc_population_extra`, so
    /// `attach_population` finds them after a restart), with `grow_cash_scale` of the configured starting
    /// cash from the world account and no shares. A population is never shrunk. Returns the number added.
    pub fn grow_population(&mut self, engine: &mut Engine, day: &str, ts: &str) -> usize {
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let want = self.population_counts(listed.len());
        let mut have = [0usize; 4];
        for a in &self.agents {
            have[a.kind as usize] += 1;
        }
        let add: Vec<usize> = (0..4).map(|k| want[k].saturating_sub(have[k])).collect();
        if add.iter().all(|&n| n == 0) {
            return 0;
        }
        let world = engine.special("world").expect("world account");
        // a large population appearing at once with full cash lifts the price level (measured in marketsim
        // --grow): the new agents start smaller
        let scale = self.cfg.grow_cash_scale;
        let kinds = [
            (AgentKind::Value, add[0], self.cfg.cash_value * scale),
            (AgentKind::Trend, add[1], self.cfg.cash_trend * scale),
            (AgentKind::News, add[2], self.cfg.cash_news * scale),
            (AgentKind::Retail, add[3], self.cfg.cash_retail * scale),
        ];
        for (kind, n, cash) in kinds {
            for _ in 0..n {
                let i = self.agents.len();
                let h = |k: u64| unit(hash(&[self.cfg.seed, 0xa6e7, i as u64, k]));
                let name = format!("{}NPC-{:04}", kind.label(), i + 1);
                let account = engine.create_account("npc", &name, None, None, None, ts);
                let start_cash = (cash * 100.0 * (0.5 + h(1))).round() as i64;
                engine.transfer(world, account, start_cash, "npc_fund_in", day, ts, None, None);
                let nw = self.cfg.watch_min + ((h(2) * (self.cfg.watch_max - self.cfg.watch_min + 1) as f64) as usize);
                let watch = self.draw_watch(&listed, nw, i as u64);
                let lb = self.cfg.trend_lookback_min;
                self.agents.push(Agent {
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
                });
                let t = self.now.max(0) + self.next_gap(i);
                self.schedule.push(Reverse((t, Actor::Agent(i as u32))));
            }
        }
        let mut extra = engine.state("npc_population_extra").unwrap_or("").to_string();
        if !extra.is_empty() {
            extra.push(';');
        }
        extra.push_str(&format!("{},{},{},{},{}", add[0], add[1], add[2], add[3], scale));
        engine.set_state("npc_population_extra", &extra);
        add.iter().sum()
    }

    /// Agents per type for `listed` stocks: the configured counts scaled by listed / `population_reference`
    /// (the calibration's universe), at least a tenth of them.
    pub fn population_counts(&self, listed: usize) -> [usize; 4] {
        let c = &self.cfg;
        let k = if c.population_reference > 0.0 { (listed as f64 / c.population_reference).max(0.1) } else { 1.0 };
        let f = |n: usize| ((n as f64 * k).round() as usize).max(1);
        [f(c.n_value), f(c.n_trend), f(c.n_news), f(c.n_retail)]
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
            engine.accounts().filter(|a| a.kind == "npc").map(|a| (a.name.clone(), a.id)).collect();
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        // the counts chosen when the population was created (they scale with the listed stocks then)
        let n: [usize; 4] = engine
            .state("npc_population")
            .and_then(|v| {
                let x: Vec<usize> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
                (x.len() == 4).then(|| [x[0], x[1], x[2], x[3]])
            })
            .unwrap_or([self.cfg.n_value, self.cfg.n_trend, self.cfg.n_news, self.cfg.n_retail]);
        let kinds = [
            (AgentKind::Value, n[0], self.cfg.cash_value),
            (AgentKind::Trend, n[1], self.cfg.cash_trend),
            (AgentKind::News, n[2], self.cfg.cash_news),
            (AgentKind::Retail, n[3], self.cfg.cash_retail),
        ];
        self.agents.clear();
        // blocks appended later by `grow_population`, in order
        let mut kinds = kinds.to_vec();
        for block in engine.state("npc_population_extra").unwrap_or("").split(';').filter(|b| !b.is_empty()) {
            // counts per type, then the cash scale the block was funded with
            let f: Vec<&str> = block.split(',').map(str::trim).collect();
            let x: Vec<usize> = f.iter().take(4).filter_map(|p| p.parse().ok()).collect();
            let scale: f64 = f.get(4).and_then(|p| p.parse().ok()).unwrap_or(1.0);
            if x.len() == 4 {
                kinds.push((AgentKind::Value, x[0], self.cfg.cash_value * scale));
                kinds.push((AgentKind::Trend, x[1], self.cfg.cash_trend * scale));
                kinds.push((AgentKind::News, x[2], self.cfg.cash_news * scale));
                kinds.push((AgentKind::Retail, x[3], self.cfg.cash_retail * scale));
            }
        }
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
        if self.adj_seen.is_none() {
            self.adj_seen = Some(engine.price_adjustments().len());
        }
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
                    self.mms.insert(c, MmState {
                        account: acct,
                        var_rate: 0.0,
                        roll: [0.0; 3],
                        last_r: 0.0,
                        last_p: p,
                        last_t: now,
                        last_inv: None,
                        toxicity: 0.0,
                        human_flow_seen: None,
                        human_rate: 0.0,
                        human_depth: 0.0,
                        last_quote_p: 0.0,
                        last_half: [0.0; 4],
                        news: 0.0,
                        news_p: 0,
                    });
                    let first = now + self.rng.exp(self.cfg.mm_rate / 3600.0) as i64;
                    self.schedule.push(Reverse((first, Actor::Mm(c))));
                }
            }
        }
        self.mms.retain(|c, _| live.contains(c));
        // 热度期货: one market maker per contract trading
        let trading = engine.futures_trading();
        for &k in &trading {
            if self.fmms.insert(k) {
                let first = now + self.rng.exp(self.cfg.mm_rate / 3600.0) as i64;
                self.schedule.push(Reverse((first, Actor::Fmm(k))));
            }
        }
        self.fmms.retain(|k| trading.contains(k));
        // premiere nights: the first air time, when it is the show's first episode
        for &c in &listed {
            if self.premiere_at.contains_key(&c) {
                continue;
            }
            let first = engine.company(c).and_then(|x| day_number(&x.first_air));
            if let Some(&t0) = self.air.get(&c).and_then(|v| v.iter().min()) {
                if first.zip(day_number(&day_of(t0))).is_some_and(|(a, b)| (a - b).abs() <= 1) {
                    self.premiere_at.insert(c, t0);
                }
            }
        }
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

    /// Intraday activity factor at local seconds `s` (0 in the closed hours 04:00-08:45).
    pub fn curve(&self, s: i64) -> f64 {
        let hm = |h: i64, m: i64| h * 3600 + m * 60;
        let c = &self.cfg.curve;
        let n = &self.cfg.curve_night;
        match s {
            x if x < hm(2, 0) => n[0],
            x if x < hm(4, 0) => n[1],
            x if x < hm(8, 45) => 0.0,
            x if x < hm(9, 0) => c[0],
            x if x < hm(12, 0) => c[1],
            x if x < hm(13, 30) => c[2],
            x if x < hm(18, 0) => c[3],
            x if x < hm(20, 0) => c[4],
            x if x < hm(23, 0) => c[5],
            _ => n[0],
        }
    }

    fn peak(&self, kind: AgentKind) -> f64 {
        let cmax = self.cfg.curve.iter().chain(&self.cfg.curve_night).cloned().fold(0.0, f64::max);
        match kind {
            // news agents wake up with events: thinning bound includes the event amplification
            AgentKind::News => cmax * (1.0 + self.cfg.event_activity * 10.0) * 3.0,
            _ => cmax,
        }
    }

    /// Candidate gap (seconds) for agent `i` at the bounding rate; acceptance thins it (`accept`).
    fn next_gap(&mut self, i: usize) -> i64 {
        let kind = self.agents[i].kind;
        let rate = self.base_rate(kind) * self.peak(kind) * self.adapt_bound(kind) / 3600.0;
        (self.rng.exp(rate).ceil() as i64).max(1)
    }

    /// Bounding factor of the adaptive rate (retail and trend only; value agents are the anchor).
    fn adapt_bound(&self, kind: AgentKind) -> f64 {
        match kind {
            AgentKind::Retail | AgentKind::Trend => self.cfg.adapt_rate_max.max(1.0),
            _ => 1.0,
        }
    }

    /// Thinning of a retail or trend agent's action by the stock's adaptive rate.
    fn adapt_accept(&mut self, i: usize, c: i64) -> bool {
        let bound = self.adapt_bound(self.agents[i].kind);
        if bound == 1.0 && self.cfg.adapt_rate_min == 1.0 {
            return true;
        }
        self.rng.f64() < self.rate_mult(c) / bound
    }

    fn news_pressure(&self, i: usize, t: i64) -> f64 {
        let a = &self.agents[i];
        let mut p = 0.0;
        for e in &self.events {
            if e.direction != 0 && a.watch.contains(&e.company_id) {
                let (mult, hl) = self.reaction_of(e);
                p += e.magnitude / 10.0 * e.activity_weight(t, hl) * mult;
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
        self.ensure_futures_traders(engine);
        while let Some(&Reverse((t, actor))) = self.schedule.peek() {
            if t >= t_end {
                break;
            }
            self.schedule.pop();
            self.now = t;
            if t >= self.next_rank_check {
                self.note_movers(engine, t);
                self.next_rank_check = (t / 1800 + 1) * 1800;
            }
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
                Actor::Fmm(k) => {
                    if !self.fmms.contains(&k) {
                        continue;
                    }
                    let (_, s) = local(t);
                    if self.curve(s) > 0.0 && !Self::phase_auction(t) {
                        let (day, ts) = (day_of(t), ts_of(t));
                        if engine.state("auction_day") != Some(day.as_str()) {
                            engine.open_market(&day, &ts);
                        }
                        engine.futures_mm_requote(k, &day, &ts);
                    }
                    let gap = (self.rng.exp(self.cfg.mm_rate / 3600.0).ceil() as i64).max(1);
                    self.schedule.push(Reverse((t + gap, Actor::Fmm(k))));
                }
                Actor::Ftrader(i) => {
                    let (_, s) = local(t);
                    if self.curve(s) > 0.0 && !Self::phase_auction(t) {
                        let (day, ts) = (day_of(t), ts_of(t));
                        let m = crate::futures::agents::Moment { day: &day, t, flashes: &self.fflashes, heat_today: None };
                        self.ftraders[i as usize].act(engine, m, &ts);
                    }
                    let gap = (self.rng.exp(engine.futures.npc.rate / 3600.0).ceil() as i64).max(1);
                    self.schedule.push(Reverse((t + gap, Actor::Ftrader(i))));
                }
            }
        }
        self.now = t_end.max(self.now);
    }

    // ------------------------------------------------------------ helpers

    fn phase_auction(t: i64) -> bool {
        let (_, s) = local(t);
        (8 * 3600 + 45 * 60..9 * 3600).contains(&s)
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
        let chaser = matches!(self.agents[i].kind, AgentKind::Trend | AgentKind::Retail);
        let w: Vec<f64> = watch
            .iter()
            .map(|&c| match engine.company(c) {
                Some(co) if co.status == "listed" => {
                    // a bubble draws the chasers (trend and retail agents) into the stock: turnover (1 + s) times
                    let b = if chaser { self.bubble(c, t).map_or(1.0, |(_, s)| 1.0 + s) } else { 1.0 };
                    self.heat(c).powf(self.cfg.heat_alpha) * self.activity(c, t) * b
                }
                _ => 0.0,
            })
            .collect();
        self.rng.weighted(&w).map(|k| watch[k])
    }

    fn wealth(&self, engine: &Engine, acct: i64) -> i64 {
        engine.net_assets(acct)
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
        if self.value_short(engine, i, c, t, m, b) {
            return; // 融资融券 (short.rs)
        }
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
        if !self.adapt_accept(i, c) {
            return;
        }
        let a = self.agents[i].clone();
        let rules = engine.rules;
        let now_p = engine.company(c).unwrap().last_price.max(1);
        self.sample(engine, c, t);
        let Some(then) = self.price_at(c, t - a.lookback_s) else { return };
        let r = (now_p as f64 / then.max(1) as f64).ln();
        let held = engine.available_shares(a.account, c);
        engine.cancel_all(a.account, c, &ts_of(t));
        // market sentiment is risk appetite: in a hot market a smaller rise is enough to chase it, positions
        // are larger, and a fall must be larger before they sell (cold: the reverse)
        let ss = self.sentiment_tilt(c, t);
        let (mut thr_buy, mut thr_sell) = (a.threshold * (-ss).exp(), a.threshold * ss.exp());
        // a bubble: while it inflates, a smaller rise is enough to chase and dips are held through; after the
        // burst, the reverse (they dump)
        match self.bubble(c, t) {
            Some((true, s)) => {
                thr_buy /= 1.0 + s;
                thr_sell *= 1.0 + s;
            }
            Some((false, s)) => {
                thr_buy *= 1.0 + s;
                thr_sell /= 1.0 + s;
            }
            None => {}
        }
        if r > thr_buy {
            let wealth = self.wealth(engine, a.account) as f64;
            let room = self.cfg.max_position_frac * wealth - held as f64 * now_p as f64;
            let strength = (r / thr_buy / 2.0).min(1.0);
            let budget = (engine.available_cash(a.account) as f64 * self.cfg.trend_size * strength * ss.exp()).min(room);
            let limit = Self::clamp_price(engine, &rules, c, now_p as f64 * 1.01, t);
            let q = self.affordable(budget, limit);
            self.place(engine, "trend", a.account, c, "buy", "limit", q, Some(limit), t);
        } else if r < -thr_sell && held > 0 {
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
            // under- and over-reaction: a revision's reaction is scaled and lasts by its place in a streak
            let (mult, hl) = self.reaction_of(e);
            let w = e.magnitude / 10.0 * e.activity_weight(t, hl) * e.credibility_weight(&self.cfg.credibility) * mult;
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
        if !self.adapt_accept(i, c) {
            return;
        }
        let a = self.agents[i].clone();
        let rules = engine.rules;
        let p = engine.company(c).unwrap().last_price.max(1) as f64;
        let held = engine.available_shares(a.account, c);
        // attention (a stock in the news is bought, not sold: buying means choosing among all stocks, selling
        // only among one's own) and market sentiment (risk appetite) tilt the buy/sell split and the size
        let ss = self.sentiment_tilt(c, t);
        // a bubble: buy odds (1 + s) times while it inflates, sell odds (1 + s) times after the burst
        let bt = self.bubble(c, t).map_or(0.0, |(up, s)| if up { 0.5 } else { -0.5 } * (1.0 + s).ln());
        let tilt = self.cfg.attention * self.attention(c, t) + ss + bt;
        let (disp, gain) = if held > 0 { self.disposition(engine, a.account, c, p) } else { (1.0, 0) };
        let sell = held > 0 && self.rng.f64() < 0.5 * (1.0 - tilt.tanh()) * disp;
        if gain != 0 {
            // realisation counts (Odean's PGR / PLR): holdings at a gain / loss looked at, and sold
            let k = if gain > 0 { 0 } else { 2 };
            self.stats.retail_realise[k] += 1;
            self.stats.retail_realise[k + 1] += sell as u64;
        }
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
            let frac = self.rng.range(self.cfg.retail_size[0], self.cfg.retail_size[1]) * ss.exp();
            let wealth = self.wealth(engine, a.account) as f64;
            let room = self.cfg.max_position_frac * wealth - held as f64 * p;
            let budget = (engine.available_cash(a.account) as f64 * frac).min(room);
            let mlim = Self::clamp_price(engine, &rules, c, p * 1.015, t);
            let at = if market { mlim } else { limit };
            let mut q = self.affordable(budget, at);
            // a retail agent stands for many small investors: their money demand does not depend on the nominal
            // price, so the whole-share remainder is bought with that probability (stochastic rounding). Without
            // it a share above a small order's size is never bought, and a stock split would release a burst of
            // retail demand on a known day (docs/v2-corporate.md §3.2)
            let rest = budget / (at.max(1) as f64 * (1.0 + self.fees_commission) + 1.0) - q as f64;
            if rest > 0.0 && self.rng.range(0.0, 1.0) < rest && self.affordable(engine.available_cash(a.account) as f64, at) > q {
                q += 1;
            }
            if market {
                self.place(engine, "retail", a.account, c, "buy", "limit", q, Some(mlim), t);
            } else {
                self.place(engine, "retail", a.account, c, "buy", "limit", q, Some(limit), t);
            }
        }
    }

    /// Disposition effect (Shefrin–Statman): the factor on a retail agent's chance of selling its holding of
    /// `c` at price `p`: 2r / (1 + r) with a paper gain against its own average cost, 2 / (1 + r) with a paper
    /// loss (r = `disposition_ratio`: Odean's PGR / PLR), so gains are realised r times as readily as losses
    /// and an even book sells as often as without the effect.
    fn disposition(&mut self, engine: &Engine, acct: i64, c: i64, p: f64) -> (f64, i8) {
        let gain = match Self::paper_gain(engine, acct, c, p) {
            Some(g) if g > 0.0 => 1,
            Some(g) if g < 0.0 => -1,
            _ => return (1.0, 0),
        };
        let r = self.cfg.disposition_ratio;
        if r == 1.0 {
            return (1.0, gain);
        }
        let w = match self.gain_share {
            Some(w) => w,
            None => self.refresh_gain_share(engine),
        };
        // gains are sold r times as readily as losses, and the retail agents' sales in total stay the same: the
        // effect decides which holdings they sell, not how much (Odean 1998)
        let loss = 1.0 / (w * r + (1.0 - w));
        (if gain > 0 { r * loss } else { loss }, gain)
    }

    /// ln(price / average cost) of an agent's holding of `c` (None without lots).
    fn paper_gain(engine: &Engine, acct: i64, c: i64, p: f64) -> Option<f64> {
        let (q, v) = engine.lots_of(acct, c).iter().fold((0i64, 0f64), |(q, v), l| (q + l.qty, v + l.qty as f64 * l.cost as f64));
        (q > 0 && v > 0.0 && p > 0.0).then(|| (p * q as f64 / v).ln())
    }

    /// The share of the retail agents' holdings (positions) showing a paper gain at the last prices; kept until
    /// the next close.
    fn refresh_gain_share(&mut self, engine: &Engine) -> f64 {
        let (mut up, mut n) = (0u64, 0u64);
        for a in self.agents.iter().filter(|a| a.kind == AgentKind::Retail) {
            for h in engine.holdings_of(a.account).filter(|h| h.qty > 0) {
                let p = engine.company(h.company_id).map_or(0, |c| c.last_price) as f64;
                match Self::paper_gain(engine, a.account, h.company_id, p) {
                    Some(g) if g > 0.0 => {
                        up += 1;
                        n += 1
                    }
                    Some(g) if g < 0.0 => n += 1,
                    _ => {}
                }
            }
        }
        let w = if n == 0 { 0.5 } else { up as f64 / n as f64 };
        self.gain_share = Some(w);
        w
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
        // Roll (1984): trade prices bounce between bid and ask, which adds 2 s^2 to the variance of each change
        // and -s^2 to the covariance of consecutive changes; var + 2 cov is the variance of the efficient
        // price, so its own spread does not feed back into its volatility estimate
        st.roll[0] = decay * st.roll[0] + (1.0 - decay) * r * r;
        st.roll[1] = decay * st.roll[1] + (1.0 - decay) * r * st.last_r;
        st.roll[2] = decay * st.roll[2] + (1.0 - decay) * dt;
        st.last_r = r;
        st.last_p = p;
        st.last_t = t;
        let var_rate = if self.cfg.mm_vol_roll {
            (st.roll[0] + 2.0 * st.roll[1]).max(0.0) / st.roll[2].max(1.0)
        } else {
            st.var_rate
        };
        let sigma5 = (var_rate * 300.0).sqrt();
        let p_prev = st.last_quote_p;
        st.last_quote_p = p;

        engine.cancel_all(acct, c, &ts);
        let held = engine.holding(acct, c).map_or(0, |h| h.qty);
        // its position is signed: shares held less shares owed to the lending pool (it is short only when it
        // holds none, see below)
        let short = engine.short_qty(acct, c);
        let position = held - short;
        let unit_cents = (co.shares as f64 * self.cfg.mm_quote_frac * p).max(self.cfg.mm_quote_min * 100.0).max(1.0);
        let human_half = 0.5f64.powf(dt / (self.cfg.adapt_half_life_min * 60.0));
        let flow_now = engine.player_flow(c);
        let st = self.mms.get_mut(&c).unwrap();
        // adverse selection: what the shares it bought (sold) since the last requote lost by now (markout)
        let net = st.last_inv.map_or(0, |l| position - l) as f64;
        st.last_inv = Some(position);
        let markout = if p_prev > 0.0 { net * (p - p_prev) } else { 0.0 };
        st.toxicity = decay * st.toxicity + (1.0 - decay) * (-markout).max(0.0) / unit_cents;
        let toxicity = st.toxicity;
        // human activity: players' traded notional per hour (EWMA)
        let dflow = st.human_flow_seen.map_or(0, |f| (flow_now - f).max(0)) as f64 / 100.0;
        st.human_flow_seen = Some(flow_now);
        st.human_rate = human_half * st.human_rate + (1.0 - human_half) * dflow * 3600.0 / dt;
        let fair = co.fair_value as f64;
        let a = self.cfg.mm_anchor_to_price;
        // the market price, moved by the revisions of the public value it has not traded on yet (a dealer
        // requotes on public news at once, or informed traders pick off the stale quotes: Glosten–Milgrom);
        // it never learns the value's level, only how much it changed
        let news = match self.mms.get_mut(&c) {
            Some(st) if st.news_p == co.last_price => st.news,
            Some(st) => {
                st.news = 0.0;
                st.news_p = co.last_price;
                0.0
            }
            None => 0.0,
        };
        let pm = p * news.exp();
        let reference = if fair > 0.0 { (1.0 - a) * fair + a * pm } else { pm };
        let target = self.mm_target(&co).max(1);
        // below zero it sells borrowed stock, at most `short_cap` shares: the skew runs on through zero, so
        // its offers keep shrinking as the short grows (no step at zero)
        let short_cap = self.short_cap(&co);
        let x = ((position - target) as f64 / target as f64).clamp(-((target + short_cap) as f64 / target as f64), 1.0);
        let center = reference * (-self.cfg.mm_inventory_skew * 0.1 * x).exp();
        // scheduled information (air windows, fresh events, the first half hour of the session): a dealer
        // widens and quotes smaller while informed flow is likely
        let info = 1.0 + if self.info_window(c, t) { self.cfg.mm_event_widen } else { 0.0 };
        let half = (self.cfg.mm_base_spread / 2.0 * center).max(self.cfg.mm_vol_mult * sigma5 * center)
            * (1.0 + self.cfg.mm_toxicity_mult * toxicity)
            * info;
        let half = half.max(1.0);
        if let Some(st) = self.mms.get_mut(&c) {
            st.last_half = [
                self.cfg.mm_base_spread / 2.0,
                self.cfg.mm_vol_mult * sigma5,
                1.0 + self.cfg.mm_toxicity_mult * toxicity,
                info,
            ];
        }
        let (lo, hi) = quote_band(&co, &ts, &rules);
        // quote unit: a share of the company, at least `mm_quote_min` 币 (the designated market maker's
        // minimum quote size)
        let size = ((co.shares as f64 * self.cfg.mm_quote_frac).round() as i64)
            .max((self.cfg.mm_quote_min * 100.0 / center).round() as i64)
            .max(1);
        let size = ((size as f64 / info).round() as i64).max(1);
        // players' resting orders: depth at the inside that has rested a while (placing and cancelling
        // cannot fake it), and their best prices
        let band = self.cfg.adapt_depth_halves * half;
        let old_ts = ts_of(t - (self.cfg.adapt_min_age_min * 60.0) as i64);
        let (mut pdepth_bid, mut pdepth_ask) = (0i64, 0i64);
        let (mut pbest_bid, mut pbest_ask) = (None::<i64>, None::<i64>);
        for (buy, price, left, created) in engine.player_orders(c) {
            let near = (price as f64 - center).abs() <= band;
            let rested = created <= old_ts.as_str();
            if buy {
                pbest_bid.get_or_insert(price);
                if near && rested {
                    pdepth_bid += left;
                }
            } else {
                pbest_ask.get_or_insert(price);
                if near && rested {
                    pdepth_ask += left;
                }
            }
        }
        let st = self.mms.get_mut(&c).unwrap();
        st.human_depth = (pdepth_bid + pdepth_ask) as f64 * p / 100.0;
        // fill, don't crowd out: quote the target depth less what players already show (at least a minimal
        // two-sided quote)
        let levels = self.cfg.mm_levels.max(1);
        let full: i64 = (1..=levels as i64).map(|k| size * k).sum();
        let min_keep = self.cfg.adapt_min_quote;
        let keep = |player: i64| ((full - player) as f64 / full as f64).clamp(min_keep, 1.0);
        let (keep_bid, keep_ask) = if self.cfg.adapt_mm { (keep(pdepth_bid), keep(pdepth_ask)) } else { (1.0, 1.0) };
        // never past its inventory bound: then it only offers
        let mut room = ((target as f64 * self.cfg.mm_inventory_cap).round() as i64 - held).max(0);
        // size skew: long, it bids less and offers more (short, the reverse); unlike shading the price this
        // does not feed back into the last price it quotes around
        let sk = self.cfg.mm_size_skew * x;
        let (bid_mult, ask_mult) = ((1.0 - sk).max(0.0) * keep_bid, (1.0 + sk).max(0.0) * keep_ask);
        // it does not step ahead of a player's best price close to its own (it joins behind it); a player's
        // price far from its own is ignored, so a bad price cannot pull its quotes
        let mut bid_cap = i64::MAX;
        let mut ask_floor = i64::MIN;
        if self.cfg.adapt_mm {
            let bid1 = (center - half).floor() as i64;
            let ask1 = (center + half).ceil() as i64;
            if let Some(pb) = pbest_bid.filter(|&pb| pb < bid1 && pb as f64 >= bid1 as f64 - half) {
                bid_cap = pb;
            }
            if let Some(pa) = pbest_ask.filter(|&pa| pa > ask1 && pa as f64 <= ask1 as f64 + half) {
                ask_floor = pa;
            }
        }
        // selling short (docs/v2-margin.md §8): the same credit account, lending pool, fees, manufactured
        // dividends and forced liquidation as a player; only in continuous trading, only a marginable stock,
        // only once it holds none (a credit account cannot sell short a stock it holds). While short, every
        // bid is a buy to cover (买券还券), up to what it owes.
        let day = day_of(t);
        let may_short = short_cap > 0
            && engine.margin.enabled
            && !Self::phase_auction(t)
            && held == 0
            && engine.margin_ineligible(c, &day).is_none()
            && engine.credit(acct).is_none_or(|x| x.status == crate::margin::NORMAL);
        if may_short && engine.credit(acct).is_none() {
            engine.open_credit_unchecked(acct, &ts);
        }
        let mut cover = short;
        // short sales go in after the bids: once it owes stock, an ordinary buy is refused
        let mut shorts: Vec<(i64, i64)> = Vec::new();
        let mut last_bid = i64::MAX;
        let mut last_ask = i64::MIN;
        for k in 1..=levels {
            let off = half * (1.0 + (k - 1) as f64 * self.cfg.mm_level_step);
            let ask = ((center + off).ceil() as i64).max(ask_floor).clamp(lo + 1, hi.max(lo + 1)).max(last_ask + 1);
            let bid = ((center - off).floor() as i64).min(bid_cap).clamp(lo, ask - 1).min(last_bid - 1);
            let q = size * k as i64;
            let cash = engine.available_cash(acct);
            let qb = ((q as f64 * bid_mult).round() as i64)
                .min(cash.div_euclid(engine.fees.buy_reserve(bid.max(1), 1).max(1)))
                .min(room);
            if qb > 0 && bid >= lo && bid >= 1 {
                if short > 0 {
                    let qc = qb.min(cover);
                    if qc > 0 && !Self::phase_auction(t) {
                        self.credit_order(engine, "mm_cover", acct, c, crate::margin::Mode::BuyReturn, qc, bid, t);
                        cover -= qc;
                    }
                } else {
                    self.place(engine, "mm", acct, c, "buy", "limit", qb, Some(bid), t);
                    room -= qb;
                }
            }
            let want = (q as f64 * ask_mult).round() as i64;
            if held > 0 {
                let qa = want.min(engine.available_shares(acct, c));
                if qa > 0 && ask <= hi {
                    self.place(engine, "mm", acct, c, "sell", "limit", qa, Some(ask), t);
                }
            } else if may_short && ask <= hi {
                shorts.push((ask, want));
            }
            last_bid = bid;
            last_ask = ask;
        }
        for (ask, want) in shorts {
            let qs = want.min(short_cap - engine.short_qty(acct, c)).min(engine.lendable(c));
            // at or above the last trade (the short-sale price rule)
            let last = engine.company(c).map_or(0, |x| x.last_price);
            self.credit_order(engine, "mm_short", acct, c, crate::margin::Mode::ShortSell, qs, ask.max(last), t);
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
                    let target = self.float_target(co);
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
            let target = self.float_target(&co);
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

    /// The market maker's share of the public float (`mm_target_frac`).
    fn float_target(&self, co: &crate::model::Company) -> i64 {
        (co.shares as f64 * self.ipo_float * self.cfg.mm_target_frac).round() as i64
    }

    /// Stock the rights holders lend to the market maker (`mm_loan_frac` of the company's shares).
    fn loan_target(&self, co: &crate::model::Company) -> i64 {
        (co.shares as f64 * self.cfg.mm_loan_frac).round() as i64
    }

    /// The most the market maker sells short (borrowed from the lending pool): the same `mm_loan_frac` of the
    /// shares the rights holders committed to it. The pool's own limits (`lend_frac`, `stock_cap`), the initial
    /// margin and the size skew bind before this in practice.
    fn short_cap(&self, co: &crate::model::Company) -> i64 {
        self.loan_target(co)
    }

    /// The market maker's inventory target: its share of the float plus the stock lent to it.
    pub fn mm_target(&self, co: &crate::model::Company) -> i64 {
        self.float_target(co) + self.loan_target(co)
    }

    /// Designated market maker set-up, idempotent (runs at every start and after every close): the rights
    /// holders (committee) place `mm_loan_frac` of each listed company's shares with its market maker, off
    /// the book at the last price, paid by the world account (the issuer placing stock with the market
    /// maker it designated, so the quotes have inventory behind them); a market maker below `mm_cash` is
    /// topped up to it from the world account. What has been placed per stock is kept in the game state
    /// (`mm_loan:<company>`), so raising `mm_loan_frac` places only the difference and lowering it takes
    /// nothing back. Players are not touched and no trade prints. Reason `npc_mm_loan`.
    pub fn designation_pending(&self, engine: &Engine) -> bool {
        let want_cash = (self.cfg.mm_cash * 100.0) as i64;
        let cash_applied: i64 = engine.state("npc_mm_cash").and_then(|v| v.parse().ok()).unwrap_or(1_000_000);
        want_cash > cash_applied
            || engine.companies().filter(|c| c.status == "listed").any(|co| {
                let done: i64 = engine.state(&format!("mm_loan:{}", co.id)).and_then(|v| v.parse().ok()).unwrap_or(0);
                self.loan_target(co) > done
            })
    }

    pub fn designate_market_makers(&mut self, engine: &mut Engine, day: &str, ts: &str) -> i64 {
        let world = engine.special("world").expect("world");
        let want_cash = (self.cfg.mm_cash * 100.0) as i64;
        let cash_key = "npc_mm_cash";
        // a database from before this setting got 10,000 币 per market maker in the v0.3.0 float migration
        let cash_applied: i64 = engine.state(cash_key).and_then(|v| v.parse().ok()).unwrap_or(1_000_000);
        // (`designation_pending` reads the same two states)
        let listed: Vec<i64> = engine.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
        let mut moved = 0;
        for c in listed {
            let Some(mm) = engine.account_of("mm", c) else { continue };
            let co = engine.company(c).unwrap().clone();
            let key = format!("mm_loan:{c}");
            let done: i64 = engine.state(&key).and_then(|v| v.parse().ok()).unwrap_or(0);
            let goal = self.loan_target(&co);
            if goal > done {
                let q = match engine.account_of("committee", c) {
                    Some(com) => (goal - done).min(engine.available_shares(com, c)).max(0),
                    None => 0,
                };
                if q > 0 {
                    let com = engine.account_of("committee", c).unwrap();
                    let price = co.last_price.max(1);
                    engine.transfer(world, com, q * price, "npc_mm_loan", day, ts, Some(c), None);
                    engine.remove_shares(com, c, q, 0).expect("committee shares");
                    engine.add_shares(mm, c, q, price, day);
                    moved += q;
                }
                // a committee that has sold its stock cannot place more: recorded as done all the same
                engine.set_state(&key, &goal.to_string());
            }
            if want_cash > cash_applied {
                let cash = engine.account(mm).map_or(0, |a| a.cash);
                if cash < want_cash {
                    engine.transfer(world, mm, want_cash - cash, "npc_mm_loan", day, ts, Some(c), None);
                }
            }
        }
        if want_cash != cash_applied {
            engine.set_state(cash_key, &want_cash.to_string());
        }
        self.stats.rebalance_shares += moved;
        moved
    }

    /// Wealth of all NPC agents (cash + holdings at last prices), cents.
    pub fn npc_wealth(&self, engine: &Engine) -> (i64, i64) {
        let agents: i64 = self.agents.iter().map(|a| self.wealth(engine, a.account)).sum();
        let mms: i64 = self.mms.values().map(|m| self.wealth(engine, m.account)).sum();
        (agents, mms)
    }

    /// Corporate actions (crate::corporate): rescale each stock's price memory (price samples, the last public
    /// value, the market maker's last prices) by the adjustments recorded since the last call, so that a split or
    /// an ex-rights day is not read as a price move or a revision.
    pub fn apply_adjustments(&mut self, engine: &Engine) {
        let adj = engine.price_adjustments();
        let mut seen = self.adj_seen.unwrap_or(adj.len());
        while seen < adj.len() {
            let (c, f) = (adj[seen].company_id, adj[seen].price_factor);
            seen += 1;
            if let Some(h) = self.hist.get_mut(&c) {
                for x in h.iter_mut() {
                    x.1 = ((x.1 as f64) * f).round() as i64;
                }
            }
            if let Some(v) = self.last_fair.get_mut(&c) {
                *v = ((*v as f64) * f).round() as i64;
            }
            if let Some(m) = self.mms.get_mut(&c) {
                m.last_p *= f;
                m.last_quote_p *= f;
            }
            // the closes behind the bubble trigger (a split is not a fall; note_closes runs after this)
            if let Some(h) = self.closes.get_mut(&c) {
                for x in h.iter_mut() {
                    *x = ((*x as f64) * f).round() as i64;
                }
            }
        }
        self.adj_seen = Some(seen);
    }

    /// The market makers of the stocks whose value just changed requote now, before anyone else acts
    /// (`mm_follow_revisions`; outside trading hours their next quotes carry the change).
    pub fn requote_revised(&mut self, engine: &mut Engine, t: i64) {
        let (_, s) = local(t);
        let open = self.curve(s) > 0.0;
        for c in std::mem::take(&mut self.requote) {
            if open && self.mms.contains_key(&c) {
                self.act_mm(engine, c, t);
            }
        }
    }

    /// Large revisions of the public value become internal events: they wake up news agents and raise the
    /// stock's activity for a few hours (no news row; the reason is already public).
    pub fn note_revisions(&mut self, engine: &Engine, t: i64) {
        self.apply_adjustments(engine);
        let min = self.cfg.revision_min;
        let mut evs = Vec::new();
        for c in engine.companies().filter(|c| c.status == "listed" && c.fair_value > 0) {
            let prev = self.last_fair.insert(c.id, c.fair_value);
            if let Some(p) = prev.filter(|p| *p > 0) {
                let r = (c.fair_value as f64 / p as f64).ln();
                if self.cfg.mm_follow_revisions && r != 0.0 {
                    if let Some(st) = self.mms.get_mut(&c.id) {
                        if st.news_p != c.last_price {
                            st.news = 0.0;
                            st.news_p = c.last_price;
                        }
                        st.news += r;
                        self.requote.push(c.id);
                    }
                }
                if r.abs() >= min {
                    evs.push(Event {
                        company_id: c.id,
                        kind: "revision".into(),
                        direction: if r > 0.0 { 1 } else { -1 },
                        magnitude: (r.abs() * 50.0).min(10.0),
                        credibility: Credibility::Official,
                        source: "估值修正".into(),
                        at: t,
                        nature: Nature::Sentiment,
                        half_life_h: self.cfg.event_activity_half_life_h,
                        title: String::new(),
                        change: r,
                    });
                }
            }
        }
        for ev in evs {
            self.stats.events += 1;
            self.fresh.push(ev.clone());
            self.events.push(ev);
        }
        self.index_reactions();
    }

    /// A 热度快讯 observation: update the show's filter and value (`Engine::valuation_flash`), publish the
    /// flash when the surprise is large enough, and let the revision reach the agents.
    pub fn apply_flash(&mut self, engine: &mut Engine, cfg: &crate::config::Config, company: i64, episode: f64, hours: f64, comments: i64, prev: &[i64], t: i64) -> Option<crate::vclose::FlashOutcome> {
        let ts = ts_of(t);
        let out = engine.valuation_flash(cfg, company, episode, hours, comments, prev, &ts).ok().flatten()?;
        if out.surprise.abs() >= self.cfg.flash_news_min && self.flashed.insert((company, episode.to_bits())) {
            let title = engine.company(company).map(|c| c.title.clone()).unwrap_or_default();
            let word = if out.surprise > 0.0 { "高于" } else { "低于" };
            let ep = if episode.fract() == 0.0 { format!("{}", episode as i64) } else { format!("{episode}") };
            engine.post_news(
                &ts,
                &day_of(t),
                "flash",
                Some(company),
                format!("热度快讯：《{title}》第 {ep} 集讨论量{word}预期 {:.0}%", out.surprise.abs() * 100.0),
                // the valuation is hidden: the news says what was observed, never the value it leads to (a script
                // reading `old → new` could buy everything under the new value before anyone else acts)
                format!("播出 {:.0} 小时后的 Bangumi 单集吐槽数，与前几集同一时间相比。", hours),
            );
            self.stats.flashes += 1;
            self.post_attention(company, t, "flash");
            self.fflashes.push(crate::futures::agents::Flash { company, t, up: out.surprise > 0.0 });
        }
        self.note_revisions(engine, t);
        self.requote_revised(engine, t);
        Some(out)
    }

    /// The futures traders besides the market maker (crate::futures::agents), once contracts trade: the
    /// speculators and news traders (own accounts, capital from world), and a hedging committee for every
    /// company with a contract trading (the company's own account). Nothing while futures are off, so a replay
    /// without them draws the same random numbers.
    fn ensure_futures_traders(&mut self, engine: &mut Engine) {
        use crate::futures::agents::{Hedger, News, Trader, Value};
        if !engine.futures.enabled {
            return;
        }
        let trading = engine.futures_trading();
        if trading.is_empty() {
            return;
        }
        let now = self.now;
        let (day, ts) = (day_of(now), ts_of(now));
        let n = engine.futures.npc.clone();
        let mut new: Vec<Trader> = vec![];
        if self.ftraders.is_empty() {
            for i in 0..n.value_n {
                let account = engine.futures_npc_account(&format!("期货价值型 {}", i + 1), n.value_capital, &day, &ts);
                let strategy = Value { seed: self.cfg.seed.wrapping_mul(131).wrapping_add(i as u64), bias_sd: n.value_bias_sd, threshold: n.value_threshold, budget: n.value_budget };
                new.push(Trader { label: "fut_value", account, strategy: Box::new(strategy) });
            }
            for i in 0..n.news_n {
                let account = engine.futures_npc_account(&format!("期货消息型 {}", i + 1), n.news_capital, &day, &ts);
                let strategy = News { hold_s: (n.news_hold_h * 3600.0) as i64, budget: n.news_budget, slippage: n.news_slippage };
                new.push(Trader { label: "fut_news", account, strategy: Box::new(strategy) });
            }
        }
        let companies: BTreeSet<i64> = trading.iter().map(|k| engine.st.futures.contracts[k].company_id).collect();
        for c in companies {
            if self.fhedged.contains(&c) {
                continue;
            }
            let Some(account) = engine.account_of("company", c) else { continue };
            self.fhedged.insert(c);
            let strategy = Hedger { company: c, ratio: n.hedge_ratio, lean: n.hedge_lean, floor_sds: n.hedge_floor_sds };
            new.push(Trader { label: "fut_hedger", account, strategy: Box::new(strategy) });
        }
        for t in new {
            let i = self.ftraders.len() as u32;
            self.ftraders.push(t);
            let first = now + (self.rng.exp(n.rate / 3600.0).ceil() as i64).max(1);
            self.schedule.push(Reverse((first, Actor::Ftrader(i))));
        }
    }

    /// Restart the action schedule from `now` (after a pause: no burst of overdue actions).
    pub fn reschedule(&mut self, now: i64) {
        self.schedule.clear();
        for i in 0..self.agents.len() {
            let t = now + self.next_gap(i);
            self.schedule.push(Reverse((t, Actor::Agent(i as u32))));
        }
        let mms: Vec<i64> = self.mms.keys().copied().collect();
        for c in mms {
            let t = now + (self.rng.exp(self.cfg.mm_rate / 3600.0).ceil() as i64).max(1);
            self.schedule.push(Reverse((t, Actor::Mm(c))));
        }
        let fmms: Vec<i64> = self.fmms.iter().copied().collect();
        for k in fmms {
            let t = now + (self.rng.exp(self.cfg.mm_rate / 3600.0).ceil() as i64).max(1);
            self.schedule.push(Reverse((t, Actor::Fmm(k))));
        }
        for i in 0..self.ftraders.len() {
            let t = now + (self.rng.exp(2.0 / 3600.0).ceil() as i64).max(1);
            self.schedule.push(Reverse((t, Actor::Ftrader(i as u32))));
        }
        self.now = now;
    }

    pub fn prune(&mut self, t: i64) {
        self.prune_events(t);
        let (today, _) = local(t - ROLLOVER_S);
        self.ranked.retain(|(d, _)| *d >= today - 1);
        let live: HashSet<(i64, i64)> = self.events.iter().map(|e| (e.company_id, e.at)).collect();
        self.reactions.retain(|k, _| live.contains(k));
    }
}

/// A salt from the server secret: the sentiment layer's draws are deterministic per (seed, secret) but cannot
/// be computed from the public repository (the seed is in server/game.toml).
pub fn secret_salt(secret: &[u8]) -> u64 {
    let mut h = 0x5ec2_e75a_17u64;
    for chunk in secret.chunks(8) {
        let mut b = [0u8; 8];
        b[..chunk.len()].copy_from_slice(chunk);
        h = hash(&[h, u64::from_le_bytes(b)]);
    }
    h
}

/// Days since 1970-01-01 of `YYYY-MM-DD`.
pub fn day_number(day: &str) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(d.signed_duration_since(chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()).num_days())
}

/// Market sentiment of trading day number `d` (days since 1970-01-01), standard units: a daily AR(1) with
/// half-life `sentiment_half_life_days` and unit stationary variance, from hashed innovations (a pure function
/// of seed, secret and day: nothing to store, the same after a restart).
pub fn sentiment_level(cfg: &MarketConfig, salt: u64, d: i64) -> f64 {
    let phi = 0.5f64.powf(1.0 / cfg.sentiment_half_life_days.max(0.1));
    let scale = (1.0 - phi * phi).sqrt();
    let mut s = 0.0;
    let mut w = 1.0;
    let mut j = 0i64;
    while w >= 1e-4 && j < 2000 {
        s += w * normal(&[cfg.seed, 0x5e47, salt, (d - j) as u64]);
        w *= phi;
        j += 1;
    }
    s * scale
}

/// The band players see for trading day `day` (YYYY-MM-DD): -1 冷淡, 0 平稳, 1 火热; None when the layer is off.
pub fn sentiment_band(cfg: &MarketConfig, salt: u64, day: &str) -> Option<i8> {
    if cfg.sentiment_strength == 0.0 {
        return None;
    }
    let v = sentiment_level(cfg, salt, day_number(day)?);
    Some(if v > cfg.sentiment_band {
        1
    } else if v < -cfg.sentiment_band {
        -1
    } else {
        0
    })
}

fn log_uniform(r: [f64; 2], u: f64) -> f64 {
    let (a, b) = (r[0].max(0.01).ln(), r[1].max(0.01).ln());
    (a + (b - a) * u).exp()
}

fn lerp(r: [f64; 2], u: f64) -> f64 {
    r[0] + (r[1] - r[0]) * u
}

#[cfg(test)]
#[path = "mm_tests.rs"]
mod mm_tests;
