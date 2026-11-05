//! The NPC market inside the running server: startup (one-off migration of a v1 database, or re-attaching an
//! existing population), the per-second tick, the work after each daily close, and the 热度快讯 inbox.
//!
//! Restart safety: everything the market decides is in the database (accounts, orders, holdings) or derived
//! from it and the seed; the in-memory extras are rebuilt at startup: watch lists (seeded, plus whatever an
//! agent holds), heat (the latest inputs file), air times (repository data), events (the event log under
//! `data/events/log/`, the last 8 days).
//!
//! The 热度快讯 handoff: `engine/flash_fetch.py` writes one JSON file per observation into
//! `data/events/inbox/` (written to a temporary name, then renamed, so a half-written file is never read);
//! the tick ingests each file once and moves it to `done/` (or `bad/` if it does not parse).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{day_of, local, ts_of, Credibility, Event, Market, Nature};
use crate::config::Config;
use crate::engine::Engine;
use crate::inputs::Inputs;
use crate::valuation::Params;

/// One 热度快讯 observation from `engine/flash_fetch.py`.
#[derive(Debug, Clone, Deserialize)]
pub struct FlashObservation {
    pub company_id: i64,
    pub episode: f64,
    /// the episode's Bangumi comment count when fetched
    pub comments: i64,
    /// final comment counts of the previous episodes (most recent last)
    pub prev_comments: Vec<i64>,
    pub hours_since_air: f64,
    /// ISO timestamp of the fetch
    pub fetched_at: String,
}

pub struct MarketRuntime {
    pub market: Market,
    cfg: Config,
    events_dir: PathBuf,
    prev_inputs: Option<Inputs>,
    subscribed_day: Option<String>,
    last_inbox_scan: i64,
}

impl MarketRuntime {
    pub fn new(cfg: &Config) -> MarketRuntime {
        let events_dir = Self::events_dir(cfg);
        MarketRuntime {
            market: Market::new(cfg.market.clone(), cfg),
            cfg: cfg.clone(),
            events_dir,
            prev_inputs: None,
            subscribed_day: None,
            last_inbox_scan: 0,
        }
    }

    pub fn inbox(&self) -> PathBuf {
        self.events_dir.join("inbox")
    }

    /// Where events live: `$ANISTOCK_EVENTS_DIR`, else `events/` next to the game database.
    pub fn events_dir(cfg: &Config) -> PathBuf {
        std::env::var("ANISTOCK_EVENTS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cfg.game_db.parent().map(Path::to_path_buf).unwrap_or_default().join("events"))
    }

    /// Bring the market up: migrate a database that has never run it, else re-attach the population; load
    /// heat, air times and recent events.
    pub fn startup(&mut self, e: &mut Engine, cfg: &Config, now: i64) -> Result<(), String> {
        if e.state("npc_market").is_none() {
            if e.db_path().is_some() {
                let stem = cfg.game_db.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let dir = cfg.game_db.parent().map(Path::to_path_buf).unwrap_or_default().join("backups");
                let dest = e.backup_to(&dir.join(format!("{stem}-before-npc-market-{stamp}.db")))?;
                eprintln!("npc market: first start, database backed up to {}", dest.display());
            }
            self.migrate(e, cfg, now)?;
            eprintln!("npc market: {} agents created, float migrated", self.market.agents.len());
        } else {
            self.market.attach_population(e);
        }
        let params = Params::from_config(cfg)?;
        if let Some(last) = e.state("last_closed").map(str::to_string) {
            if let Ok(inp) = Inputs::read(&crate::inputs::default_path(cfg, &last)) {
                self.market.set_heat(e, &inp, params.market_weights);
                self.prev_inputs = Some(inp);
            }
        }
        let today = day_of(now);
        let until = day_of(now + 35 * 86_400);
        let data = crate::sim::data::SeasonData::load(cfg, &self.market.cfg, None, e, &[day_of(now - 86_400), today, until]);
        self.market.set_air_times(data.air);
        for ev in self.read_log(now - 8 * 86_400) {
            self.market.events.push(ev);
        }
        self.market.refresh_universe(e, now);
        self.market.reschedule(now);
        Ok(())
    }

    /// The one-off migration of an existing (v1) game database: retire the six v1 noise traders (their shares
    /// go to the stock's market maker, their cash back to the world account), create the population, move
    /// the float from the market makers to the agents, mark the database. One database transaction; the
    /// caller backs the database up first.
    pub fn migrate(&mut self, e: &mut Engine, _cfg: &Config, now: i64) -> Result<(), String> {
        let (day, ts) = (day_of(now), ts_of(now));
        let world = e.special("world").ok_or("no world account")?;
        e.begin_batch();
        let retired: Vec<i64> = e.accounts().filter(|a| a.kind == "noise").map(|a| a.id).collect();
        for a in retired {
            let held: Vec<(i64, i64)> = e.holdings_of(a).map(|h| (h.company_id, h.qty)).collect();
            for (c, _) in &held {
                e.cancel_all(a, *c, &ts);
            }
            let open: Vec<i64> = e.open_orders_of(a).iter().map(|o| o.company_id).collect();
            for c in open {
                e.cancel_all(a, c, &ts);
            }
            for (c, qty) in held {
                let price = e.company(c).map_or(1, |x| x.last_price.max(1));
                match e.account_of("mm", c) {
                    Some(mm) => {
                        e.remove_shares(a, c, qty, 0)?;
                        e.add_shares(mm, c, qty, price, &day);
                    }
                    None => continue,
                }
            }
            let cash = e.account(a).map_or(0, |x| x.cash);
            e.transfer(a, world, cash, "npc_retire", &day, &ts, None, None);
            e.set_account_kind(a, "retired");
        }
        self.market.create_population(e, &day, &ts);
        self.market.rebalance_float(e, &day, &ts);
        e.set_state("npc_market", "1");
        e.end_batch();
        e.check_invariants()
    }

    /// Once a second: run the agents up to `now`, the IPO subscriptions at noon, the 热度快讯 inbox.
    pub fn tick(&mut self, e: &mut Engine, now: i64) {
        if now - self.market.now > 300 {
            self.market.reschedule(now);
        }
        self.market.step_until(e, now);
        let (_, s) = local(now);
        let today = day_of(now);
        if s >= 12 * 3600 && self.subscribed_day.as_deref() != Some(today.as_str()) {
            self.market.subscribe_ipos(e, now);
            self.subscribed_day = Some(today);
        }
        if now - self.last_inbox_scan >= 30 {
            self.last_inbox_scan = now;
            self.ingest_inbox(e, now);
        }
    }

    /// After the daily close of `day`: events from the close (premiere, finale, delisting and 面值退市
    /// warnings), fund flows, the new heat.
    #[allow(clippy::too_many_arguments)]
    pub fn after_close(
        &mut self,
        e: &mut Engine,
        cfg: &Config,
        inputs: &Inputs,
        t_close: i64,
        low_weeks_before: &HashMap<i64, i64>,
    ) -> Result<(), String> {
        let day = inputs.day.clone();
        let ts = ts_of(t_close);
        let mut events = Vec::new();
        for s in &inputs.shows {
            let Some(c) = e.company(s.id) else { continue };
            if c.status != "listed" {
                continue;
            }
            let prev_aired = self.prev_inputs.as_ref().and_then(|p| p.show(s.id)).map_or(0, |p| p.aired);
            if prev_aired == 0 && s.aired > 0 {
                events.push(close_event(s.id, "premiere", 1, 4.0, Nature::Sentiment, format!("《{}》开播", c.title), t_close));
            } else if s.episodes_known && s.episodes.is_some_and(|n| prev_aired < n && s.aired >= n) {
                events.push(close_event(s.id, "finale", -1, 3.0, Nature::Sentiment, format!("《{}》完结", c.title), t_close));
            }
        }
        let warn_at = cfg.game.delist_after_weeks - 1;
        for c in e.companies().filter(|c| c.status == "listed") {
            if c.low_weeks == warn_at && low_weeks_before.get(&c.id).is_some_and(|b| *b < warn_at) {
                events.push(close_event(
                    c.id,
                    "delist_warning",
                    -1,
                    6.0,
                    Nature::Fundamental,
                    format!("退市预警：《{}》营收连续偏低，下周日可能退市", c.title),
                    t_close,
                ));
            }
        }
        for ev in events {
            // premiere/finale news are written by the close; the delisting warning is new
            let kind = if ev.kind == "delist_warning" { Some("warning") } else { None };
            self.log(&ev);
            self.market.post_event(e, ev, kind);
        }
        self.market.note_revisions(e, t_close);
        self.market.fund_flows(e, &day, &ts);
        self.market.prune(t_close);
        let params = Params::from_config(cfg)?;
        self.market.set_heat(e, inputs, params.market_weights);
        self.prev_inputs = Some(inputs.clone());
        self.market.refresh_universe(e, t_close);
        Ok(())
    }

    // ------------------------------------------------------------ 热度快讯 inbox and the event log

    fn ingest_inbox(&mut self, e: &mut Engine, now: i64) {
        let inbox = self.inbox();
        let Ok(dir) = std::fs::read_dir(&inbox) else { return };
        let mut files: Vec<PathBuf> =
            dir.filter_map(|f| f.ok().map(|f| f.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        for f in files {
            let parsed = std::fs::read_to_string(&f).ok().and_then(|t| serde_json::from_str::<FlashObservation>(&t).ok());
            let target = match parsed {
                Some(o) => {
                    // every observation updates the show's filter; a large surprise is also published
                    let cfg = self.cfg.clone();
                    self.market.apply_flash(e, &cfg, o.company_id, o.episode, o.hours_since_air, o.comments, &o.prev_comments, now);
                    "done"
                }
                None => "bad",
            };
            let dest = self.events_dir.join(target);
            let _ = std::fs::create_dir_all(&dest);
            let _ = std::fs::rename(&f, dest.join(f.file_name().unwrap()));
        }
    }

    fn log(&self, ev: &Event) {
        let dir = self.events_dir.join("log");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{}.jsonl", day_of(ev.at)))) {
            let _ = writeln!(f, "{}", serde_json::to_string(ev).unwrap_or_default());
        }
    }

    fn read_log(&self, since: i64) -> Vec<Event> {
        let dir = self.events_dir.join("log");
        let Ok(rd) = std::fs::read_dir(&dir) else { return vec![] };
        let mut files: Vec<PathBuf> = rd.filter_map(|f| f.ok().map(|f| f.path())).collect();
        files.sort();
        let mut out = Vec::new();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            for line in text.lines() {
                if let Ok(ev) = serde_json::from_str::<Event>(line) {
                    if ev.at >= since {
                        out.push(ev);
                    }
                }
            }
        }
        out
    }
}

fn close_event(company_id: i64, kind: &str, direction: i8, magnitude: f64, nature: Nature, title: String, at: i64) -> Event {
    Event {
        company_id,
        kind: kind.into(),
        direction,
        magnitude,
        credibility: Credibility::Official,
        source: "日结".into(),
        at,
        nature,
        half_life_h: 24.0,
        title,
    }
}
