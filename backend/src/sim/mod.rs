//! Market simulation (docs/v2-npc-market.md §7): the real engine in memory (no database writes), a season
//! of real per-day inputs, trading sessions with the v2 NPC market acting by Poisson arrivals, the real
//! daily close every night, events (premiere/finale/delisting warning from the close, 热度快讯 synthesized
//! from Bangumi episode comment counts), and optional exploit bots. Seeded and deterministic.

pub mod bots;
pub mod data;
pub mod metrics;
pub mod report;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::engine::Engine;
use crate::inputs::Inputs;
use crate::market::{at_local, day_of, local, ts_of, Credibility, Event, Market, MarketConfig, Nature};
use crate::store::persist::{Change, ChangeSink};
use crate::valuation::Params;
use bots::{Bot, BotKind};
use metrics::Recorder;

/// What the simulation records from the engine's changes (everything else is dropped).
#[derive(Default)]
pub struct Tap {
    pub trades: Vec<metrics::TradeRec>,
    pub ledger: Vec<(i64, i64, i64, String)>,
}

#[derive(Clone, Default)]
pub struct TapSink(pub Arc<Mutex<Tap>>);

impl ChangeSink for TapSink {
    fn send(&mut self, changes: Vec<Change>) {
        let mut tap = self.0.lock().unwrap();
        for c in changes {
            match c {
                Change::TradeInsert(t) => tap.trades.push(metrics::TradeRec {
                    t: parse_ts(&t.ts),
                    company: t.company_id,
                    price: t.price,
                    qty: t.qty,
                    buyer: t.buyer_id,
                    seller: t.seller_id,
                }),
                Change::LedgerInsert(l) => {
                    if !matches!(l.reason.as_str(), "trade" | "commission" | "stamp_duty") {
                        tap.ledger.push((l.from_id, l.to_id, l.amount, l.reason));
                    }
                }
                _ => {}
            }
        }
    }
    fn flush(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// Unix seconds of `YYYY-MM-DDTHH:MM:SS+08:00`.
pub fn parse_ts(s: &str) -> i64 {
    let b = s.as_bytes();
    let n = |a: usize, l: usize| -> i64 { s[a..a + l].parse().unwrap_or(0) };
    let _ = b;
    at_local(&s[..10], n(11, 2), n(14, 2)) + n(17, 2)
}

#[derive(Clone)]
pub struct SimOptions {
    pub world_db: PathBuf,
    pub inputs_dir: PathBuf,
    /// collector DB (Bangumi episode comments for 热度快讯; optional)
    pub collector_db: Option<PathBuf>,
    pub max_days: Option<usize>,
    pub market: MarketConfig,
    pub bot: Option<BotKind>,
    /// sample spreads every this many minutes
    pub spread_every_min: i64,
    /// inactive player accounts (they share the weekly tax transfers with the bot)
    pub idle_players: usize,
}

pub struct SimResult {
    pub metrics: serde_json::Value,
    pub bot: Option<serde_json::Value>,
    pub series: metrics::Series,
}

/// Run one season.
pub fn run(cfg: &Config, opts: &SimOptions) -> Result<SimResult, String> {
    let t_start = std::time::Instant::now();
    let snap = {
        let conn = rusqlite::Connection::open_with_flags(&opts.world_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        crate::store::load(&conn).map_err(|e| e.to_string())?
    };
    let tap = TapSink::default();
    let mut e = Engine::from_snapshot(snap, cfg.fees(), cfg.rules(), Box::new(tap.clone()));
    let params = Params::from_config(cfg)?;
    let last_closed = e.state("last_closed").ok_or("world has no last_closed")?.to_string();

    let mut days: Vec<String> = std::fs::read_dir(&opts.inputs_dir)
        .map_err(|e| e.to_string())?
        .filter_map(|f| f.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .filter(|d| d.as_str() > last_closed.as_str())
        .collect();
    days.sort();
    if let Some(n) = opts.max_days {
        days.truncate(n);
    }
    if days.is_empty() {
        return Err("no inputs after last_closed".into());
    }
    let load = |d: &str| Inputs::read(&opts.inputs_dir.join(format!("{d}.json")));
    let prev_inputs = Inputs::read(&opts.inputs_dir.join(format!("{last_closed}.json"))).or_else(|_| load(&days[0]))?;

    // ---- the NPC market
    let mut m = Market::new(opts.market.clone(), cfg);
    m.set_heat(&e, &prev_inputs, params.market_weights);
    let t0 = at_local(&days[0], 8, 0);
    let ts0 = ts_of(t0);
    m.create_population(&mut e, &last_closed, &ts0);
    m.rebalance_float(&mut e, &last_closed, &ts0);
    let data = data::SeasonData::load(cfg, &opts.market, opts.collector_db.as_deref(), &e, &days);
    m.set_air_times(data.air.clone());
    m.refresh_universe(&e, t0);

    // ---- the bot (a player)
    let mut bot = opts.bot.map(|k| {
        let start_cash = (cfg.game.initial_cash * 100.0).round() as i64;
        let acct = e.create_account("player", k.name(), Some(k.name()), None, None, &ts0);
        let world = e.special("world").unwrap();
        e.transfer(world, acct, start_cash, "grant", &last_closed, &ts0, None, None);
        Bot::new(k, acct, start_cash)
    });

    // a realistic number of (inactive) players: weekly tax transfers are shared among all players
    {
        let start_cash = (cfg.game.initial_cash * 100.0).round() as i64;
        let world = e.special("world").unwrap();
        for k in 0..opts.idle_players {
            let name = format!("idle{k:02}");
            let acct = e.create_account("player", &name, Some(&name), None, None, &ts0);
            e.transfer(world, acct, start_cash, "grant", &last_closed, &ts0, None, None);
        }
    }
    // setup rows (grants, the float migration) are not part of the season's flows
    {
        let mut tp = tap.0.lock().unwrap();
        tp.trades.clear();
        tp.ledger.clear();
    }
    let npc_accounts: HashSet<i64> = m.agents.iter().map(|a| a.account).collect();
    let mut rec = Recorder::new(&e, &m, &npc_accounts);
    rec.start_wealth(&e, &m);
    let mut prev = prev_inputs;
    let mut flashes = data.flashes.clone();
    flashes.sort_by_key(|f| f.at);
    let mut fi = 0;
    let mut npc_time = 0.0f64;
    if let Some(b) = &bot {
        rec.watch_account(b.account);
    }

    for (k, day) in days.iter().enumerate() {
        let inp = load(day)?;
        m.set_heat(&e, &prev, params.market_weights);
        let t_open = at_local(day, 8, 45);
        m.refresh_universe(&e, t_open);
        rec.day_start(&e, day);
        // the trading day, minute by minute (NPCs act inside each minute at their own times)
        let mut t = t_open;
        let t_end = at_local(day, 23, 0);
        while t < t_end {
            let next = t + 60;
            while fi < flashes.len() && flashes[fi].at < next {
                let f = flashes[fi].clone();
                fi += 1;
                if e.company(f.company_id).is_none_or(|c| c.status != "listed") {
                    continue;
                }
                // overnight flashes are announced when they come out; the NPCs see them from the open
                m.step_until(&mut e, f.at.max(m.now));
                rec.event(&f);
                m.post_event(&mut e, f, Some("flash"));
            }
            let tick = std::time::Instant::now();
            m.step_until(&mut e, next);
            npc_time += tick.elapsed().as_secs_f64();
            t = next;
            let (_, s) = local(t);
            if s == 12 * 3600 {
                m.subscribe_ipos(&mut e, t);
            }
            if s >= 9 * 3600 && s < 23 * 3600 {
                if let Some(b) = bot.as_mut() {
                    let heat = |c: i64| m.heat(c);
                    b.on_minute(&mut e, t, &heat);
                }
            }
            if (s / 60) % opts.spread_every_min == 0 && s >= 9 * 3600 {
                rec.sample_spreads(&e, &m, t);
            }
        }
        rec.day_end(&e, day);

        // the close, next morning at 06:30
        let t_close = at_local(day, 30, 30); // 06:30 the next day
        let ts_close = ts_of(t_close);
        let before: HashMap<i64, i64> = e.companies().map(|c| (c.id, c.low_weeks)).collect();
        match e.run_day(cfg, day, &ts_close, &inp).map_err(|err| format!("close {day}: {err}"))? {
            crate::daily::CloseOutcome::Closed(_) => {}
            crate::daily::CloseOutcome::Skipped => return Err(format!("close {day} skipped")),
        }
        e.check_invariants().map_err(|err| format!("after {day}: {err}"))?;
        // events from the close: premiere, finale, delisting warning
        for s in &inp.shows {
            let Some(c) = e.company(s.id) else { continue };
            if c.status != "listed" {
                continue;
            }
            let prev_aired = prev.show(s.id).map_or(0, |p| p.aired);
            let mk = |kind: &str, dir: i8, mag: f64, nature: Nature, title: String| Event {
                company_id: s.id,
                kind: kind.into(),
                direction: dir,
                magnitude: mag,
                credibility: Credibility::Official,
                source: "日结".into(),
                at: t_close,
                nature,
                half_life_h: 24.0,
                title,
            };
            if prev_aired == 0 && s.aired > 0 {
                let ev = mk("premiere", 1, 4.0, Nature::Sentiment, format!("《{}》开播", c.title));
                rec.event(&ev);
                m.post_event(&mut e, ev, None);
            } else if s.episodes_known && s.episodes.is_some_and(|n| prev_aired < n && s.aired >= n) {
                let ev = mk("finale", -1, 3.0, Nature::Sentiment, format!("《{}》完结", c.title));
                rec.event(&ev);
                m.post_event(&mut e, ev, None);
            }
        }
        let warn_at = cfg.game.delist_after_weeks - 1;
        let warned: Vec<(i64, String)> = e
            .companies()
            .filter(|c| {
                c.status == "listed" && c.low_weeks == warn_at && before.get(&c.id).is_some_and(|b| *b < warn_at)
            })
            .map(|c| (c.id, c.title.clone()))
            .collect();
        for (c, title) in warned {
            let ev = Event {
                company_id: c,
                kind: "delist_warning".into(),
                direction: -1,
                magnitude: 6.0,
                credibility: Credibility::Official,
                source: "交易所".into(),
                at: t_close,
                nature: Nature::Fundamental,
                half_life_h: 24.0,
                title: format!("退市预警：《{title}》营收连续偏低，下周日可能退市"),
            };
            rec.event(&ev);
            m.post_event(&mut e, ev, Some("warning"));
        }
        let next_day = day_of(t_close);
        m.fund_flows(&mut e, day, &ts_close);
        m.prune(t_close);
        rec.after_close(&e, &m, day, &next_day);
        {
            let mut tp = tap.0.lock().unwrap();
            rec.absorb(&mut tp);
        }
        prev = inp;
        if k % 10 == 9 {
            eprintln!("  {day}: {:.0}s", t_start.elapsed().as_secs_f64());
        }
    }
    e.check_invariants()?;
    let elapsed = t_start.elapsed().as_secs_f64();
    let watched = rec.watched();
    let bot_json = bot.as_ref().map(|b| {
        let w = b.wealth(&e);
        serde_json::json!({
            "kind": b.kind.name(), "start": b.start_cash, "end": w,
            "return": w as f64 / b.start_cash as f64 - 1.0, "orders": b.trades_placed,
            "transfers": watched.0, "dividends": watched.1,
            "return_ex_transfers": (w - watched.0) as f64 / b.start_cash as f64 - 1.0,
        })
    });
    let actions: u64 = m.stats.actions.values().sum::<u64>();
    let (mut metrics, series) = rec.finish(&e, &m, elapsed, &days);
    let mut pm = m.stats.per_minute.clone();
    pm.sort_unstable();
    let q = |p: f64| pm.get(((pm.len().max(1) - 1) as f64 * p) as usize).copied().unwrap_or(0);
    metrics["performance"] = serde_json::json!({
        "npc_step_seconds": npc_time,
        "npc_agent_actions": actions,
        "npc_orders": m.stats.orders.values().sum::<u64>(),
        "us_per_npc_order": npc_time * 1e6 / m.stats.orders.values().sum::<u64>().max(1) as f64,
        "orders_per_min_median": q(0.5), "orders_per_min_p99": q(0.99), "orders_per_min_max": m.stats.peak_orders_per_min,
    });
    Ok(SimResult { metrics, bot: bot_json, series })
}
