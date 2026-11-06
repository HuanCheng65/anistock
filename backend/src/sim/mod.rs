//! Market simulation (docs/v2-npc-market.md §7): the real engine in memory (no database writes), a season
//! of real per-day inputs, trading sessions with the v2 NPC market acting by Poisson arrivals, the real
//! daily close every night, events (premiere/finale/delisting warning from the close, 热度快讯 synthesized
//! from Bangumi episode comment counts), and optional exploit bots. Seeded and deterministic.

pub mod bots;
pub mod data;
pub mod metrics;
pub mod probe;
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
    /// for the replay report (events.json): every news item, and every VCM trigger (company, until, count)
    pub news: Vec<serde_json::Value>,
    pub vcm: Vec<(i64, String, i64)>,
    vcm_last: std::collections::HashMap<i64, String>,
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
                Change::NewsInsert(n) => tap.news.push(serde_json::json!({
                    "ts": n.ts, "day": n.day, "kind": n.kind, "company_id": n.company_id, "title": n.title, "body": n.body,
                })),
                Change::CompanyMarket { id, vcm_until: Some(u), vcm_count, .. } => {
                    if tap.vcm_last.get(&id) != Some(&u) {
                        tap.vcm_last.insert(id, u.clone());
                        tap.vcm.push((id, u, vcm_count));
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

/// Day index (0 = the first simulated day) on which the bots start trading.
pub const BOT_START_DAY: usize = 7;

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
    /// the fan bots' foresight: (company, day) -> hidden value at that day's open (from a baseline run)
    pub foresight: Option<Arc<std::collections::HashMap<(i64, String), (i64, i64)>>>,
    /// the liquidity probe's players (`probe.rs`; 0: none)
    pub probe: usize,
    /// a running population grown the way a deploy grows it (`Market::grow_population`): to this many
    /// value agents (configured count), spread over this many daily closes (0: at once, before the first
    /// day), each new agent with this share of the configured starting cash
    pub grow: Option<(usize, usize, f64)>,
    /// replay inputs written before v0.3.2 (no point-in-time prior): a degraded valuation, for comparisons only
    pub allow_old_inputs: bool,
}

pub struct SimResult {
    /// news, VCM triggers and the companies (for the replay report)
    pub events: serde_json::Value,
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
    e.margin = cfg.margin.clone();
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
    // a replay on inputs older than the model measures a different (degraded) valuation: refuse it
    if !cfg.economy_v1 && !opts.allow_old_inputs {
        let fp = crate::vfilter::FilterParams::load(cfg)?;
        if let Some(msg) = crate::vclose::inputs_schema_problem(&fp, &load(&days[0])?) {
            return Err(format!("{msg} (--allow-old-inputs to run anyway)"));
        }
    }

    // ---- the filtered valuation from the season so far (the world was seeded with v1 values)
    if !cfg.economy_v1 {
        let mut hist: Vec<String> = std::fs::read_dir(&opts.inputs_dir)
            .map_err(|e| e.to_string())?
            .filter_map(|f| f.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
            .filter(|d| d.as_str() <= last_closed.as_str())
            .collect();
        hist.sort();
        let history: Vec<Inputs> = hist.iter().map(|d| load(d)).collect::<Result<_, _>>()?;
        e.valuation_bootstrap(cfg, &history, &ts_of(at_local(&days[0], 7, 0)))?;
    }

    // ---- the NPC market
    let mut m = Market::new(opts.market.clone(), cfg);
    m.set_heat(&e, &prev_inputs, params.market_weights);
    let t0 = at_local(&days[0], 8, 0);
    let ts0 = ts_of(t0);
    m.create_population(&mut e, &last_closed, &ts0);
    m.rebalance_float(&mut e, &last_closed, &ts0);
    m.designate_market_makers(&mut e, &last_closed, &ts0);
    let mut grown_cash = 0i64;
    let grow_step = |m: &mut Market, e: &mut Engine, k: usize, day: &str, ts: &str| -> i64 {
        let Some((n_to, days, scale)) = opts.grow else { return 0 };
        let n_from = opts.market.n_value;
        let target = if days == 0 { n_to } else { n_from + (n_to.saturating_sub(n_from)) * (k + 1).min(days) / days };
        let world = e.special("world").unwrap();
        let before = e.account(world).map_or(0, |a| a.cash);
        m.cfg.n_value = target;
        m.cfg.grow_cash_scale = scale;
        m.grow_population(e, day, ts);
        before - e.account(world).map_or(0, |a| a.cash)
    };
    if opts.grow.is_some_and(|g| g.1 == 0) {
        grown_cash += grow_step(&mut m, &mut e, 0, &last_closed, &ts0);
    }
    let data = data::SeasonData::load(cfg, &opts.market, opts.collector_db.as_deref(), &e, &days);
    m.set_air_times(data.air.clone());
    m.refresh_universe(&e, t0);

    // ---- the bot (a player)
    let mut bot = opts.bot.map(|k| {
        let start_cash = (cfg.game.initial_cash * 100.0).round() as i64;
        let acct = e.create_account("player", k.name(), Some(k.name()), None, None, &ts0);
        let world = e.special("world").unwrap();
        e.transfer(world, acct, start_cash, "grant", &last_closed, &ts0, None, None);
        let mut b = Bot::new(k, acct, start_cash);
        b.foresight = opts.foresight.clone();
        b
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
    let mut probe = (opts.probe > 0).then(|| {
        let start_cash = (cfg.game.initial_cash * 100.0).round() as i64;
        probe::Probe::new(&mut e, opts.probe, start_cash, &last_closed, &ts0, opts.market.seed)
    });
    // NPC capital against the players' (the liquidity study): total, and per stock for the hottest ten
    let capital = {
        let npc: HashSet<i64> = m.agents.iter().map(|a| a.account).collect();
        let cash: i64 = m.agents.iter().map(|a| e.account(a.account).map_or(0, |x| x.cash)).sum();
        let mut hot: Vec<(f64, i64)> = e.companies().filter(|c| c.status == "listed").map(|c| (m.heat(c.id), c.id)).collect();
        hot.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let per_stock = |c: i64| {
            let co = e.company(c).unwrap();
            let (mut npc_q, mut mm_q) = (0, 0);
            for a in e.accounts() {
                let q = e.holding(a.id, c).map_or(0, |h| h.qty);
                if npc.contains(&a.id) {
                    npc_q += q;
                } else if a.kind == "mm" {
                    mm_q += q;
                }
            }
            let watchers = m.agents.iter().filter(|a| a.watch.contains(&c)).count();
            serde_json::json!({"price": co.last_price as f64 / 100.0, "market_cap": (co.last_price * co.shares) as f64 / 100.0,
                "npc_holdings": (npc_q * co.last_price) as f64 / 100.0, "mm_holdings": (mm_q * co.last_price) as f64 / 100.0,
                "watchers": watchers})
        };
        let mut watchers: Vec<usize> = hot.iter().map(|(_, c)| m.agents.iter().filter(|a| a.watch.contains(c)).count()).collect();
        let cold_watchers: Vec<usize> = watchers.split_off(watchers.len() * 2 / 3);
        serde_json::json!({
            "npc_agents": m.agents.len(),
            "npc_cash": cash as f64 / 100.0,
            "npc_wealth": m.npc_wealth(&e).0 as f64 / 100.0,
            "mm_wealth": m.npc_wealth(&e).1 as f64 / 100.0,
            "hot10": hot.iter().take(10).map(|(_, c)| per_stock(*c)).collect::<Vec<_>>(),
            "cold_third_watchers_min_median": [cold_watchers.iter().min(), cold_watchers.get(cold_watchers.len() / 2)],
        })
    };
    // setup rows (grants, the float migration) are not part of the season's flows
    {
        let mut tp = tap.0.lock().unwrap();
        tp.trades.clear();
        tp.ledger.clear();
    }
    let npc_accounts: HashSet<i64> = m.agents.iter().map(|a| a.account).collect();
    let mm_accounts: HashSet<i64> = e.accounts().filter(|a| a.kind == "mm").map(|a| a.id).collect();
    let agent_kind: HashMap<i64, crate::market::AgentKind> = m.agents.iter().map(|a| (a.account, a.kind)).collect();
    let mut mm_counterparty: std::collections::BTreeMap<String, i64> = Default::default();
    let mut bot_by_cp: std::collections::BTreeMap<String, i64> = Default::default();
    let mut bot_by_hour: std::collections::BTreeMap<i64, i64> = Default::default();
    let mut bot_volume: std::collections::BTreeMap<&str, i64> = Default::default();
    let mut rec = Recorder::new(&e, &m, &npc_accounts);
    rec.dividend_tax = cfg.game.dividend_tax.clone();
    rec.start_wealth(&e, &m);
    let mut prev = prev_inputs;
    let mut flashes = data.flashes.clone();
    flashes.sort_by_key(|f| f.at);
    let mut fi = 0;
    // published 热度快讯 (what players see): (company, at, direction)
    let mut published: Vec<(i64, i64, i8)> = Vec::new();
    let mut published_keys: HashSet<(i64, u64)> = HashSet::new();
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
        rec.sentiment(day, m.sentiment_of_day(day));
        let band = m.sentiment_band(day);
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
                if let Some(out) = m.apply_flash(&mut e, cfg, f.company_id, f.episode, f.hours, f.comments, &f.prev, f.at.max(m.now)) {
                    rec.flash(f.company_id, f.at, out.surprise);
                    if out.surprise.abs() >= m.cfg.flash_news_min && published_keys.insert((f.company_id, f.episode.to_bits())) {
                        published.push((f.company_id, f.at.max(m.now), if out.surprise > 0.0 { 1 } else { -1 }));
                    }
                }
            }
            let tick = std::time::Instant::now();
            m.step_until(&mut e, next);
            npc_time += tick.elapsed().as_secs_f64();
            t = next;
            let (_, s) = local(t);
            if s == 12 * 3600 {
                m.subscribe_ipos(&mut e, t);
            }
            // the bots start in the second week: the world starts at v1 prices, and the first days are the
            // market repricing to the new valuation (a one-off of the go-live, not a strategy's merit)
            if s >= 9 * 3600 && s < 23 * 3600 && k >= BOT_START_DAY {
                if let Some(b) = bot.as_mut() {
                    let heat = |c: i64| m.heat(c);
                    let ctx = bots::BotCtx { band, flashes: &published };
                    b.on_minute(&mut e, t, &heat, &ctx);
                }
                if let Some(p) = probe.as_mut() {
                    let heat = |c: i64| m.heat(c);
                    p.on_minute(&mut e, t, &heat);
                    if (s / 60) % 30 == 0 {
                        p.sample_impact(&e, &heat);
                    }
                }
            }
            // 融资融券: forced liquidation runs once a minute in continuous trading
            if s >= 9 * 3600 {
                e.margin_sweep(&day_of(t), &ts_of(t));
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
                change: 0.0,
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
                change: 0.0,
            };
            rec.event(&ev);
            m.post_event(&mut e, ev, Some("warning"));
        }
        let next_day = day_of(t_close);
        m.note_revisions(&e, t_close);
        m.designate_market_makers(&mut e, day, &ts_close);
        if opts.grow.is_some_and(|g| g.1 > 0) && k >= BOT_START_DAY {
            grown_cash += grow_step(&mut m, &mut e, k - BOT_START_DAY, day, &ts_close);
        }
        m.fund_flows(&mut e, day, &ts_close);
        m.prune(t_close);
        // (the live server logs these for restarts; a simulation has nothing to restart)
        m.fresh.clear();
        rec.after_close(&e, &m, day, &next_day);
        rec.note_info(&inp, &prev, day);
        {
            let mut tp = tap.0.lock().unwrap();
            // the bot's trades: its markout to the day's close by counterparty and by hour of the day
            if let Some(b) = &bot {
                for tr in &tp.trades {
                    let (bought, other) = if tr.buyer == b.account {
                        (true, tr.seller)
                    } else if tr.seller == b.account {
                        (false, tr.buyer)
                    } else {
                        continue;
                    };
                    let Some(close) = e.company(tr.company).map(|c| c.last_price) else { continue };
                    let edge = if bought { close - tr.price } else { tr.price - close } * tr.qty;
                    let label = match agent_kind.get(&other) {
                        Some(k) => format!("{k:?}"),
                        None => e.account(other).map_or("?".into(), |a| a.kind.clone()),
                    };
                    *bot_by_cp.entry(label).or_default() += edge;
                    let (_, s) = local(tr.t);
                    *bot_by_hour.entry(s / 3600).or_default() += edge;
                    *bot_volume.entry(if bought { "bought" } else { "sold" }).or_default() += tr.price * tr.qty;
                }
            }
            // where the market makers' dealing result goes: each counterparty's markout to the day's close
            for tr in &tp.trades {
                let Some(close) = e.company(tr.company).map(|c| c.last_price) else { continue };
                let (mm_buys, other) = if mm_accounts.contains(&tr.buyer) {
                    (true, tr.seller)
                } else if mm_accounts.contains(&tr.seller) {
                    (false, tr.buyer)
                } else {
                    continue;
                };
                // the counterparty sold to (bought from) the market maker
                let edge = if mm_buys { tr.price - close } else { close - tr.price } * tr.qty;
                let label = match agent_kind.get(&other) {
                    Some(k) => format!("{k:?}"),
                    None => e.account(other).map_or("?".into(), |a| match a.kind.as_str() {
                        "player" if a.name.starts_with("probe") => "probe".into(),
                        "player" if a.name.starts_with("raid") => "raiders".into(),
                        "player" => "bot".into(),
                        k => k.to_string(),
                    }),
                };
                *mm_counterparty.entry(label).or_default() += edge;
            }
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
            "margin": b.mstats,
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
    let events = {
        let tp = tap.0.lock().unwrap();
        let companies: Vec<serde_json::Value> = e
            .companies()
            .map(|c| {
                serde_json::json!({"id": c.id, "title": c.title, "title_ja": c.title_ja, "ipo_price": c.ipo_price,
                    "listed_on": c.listed_on, "first_air": c.first_air, "episodes": c.episodes, "long_runner": c.long_runner != 0,
                    "status": c.status, "delisted_on": c.delisted_on, "shares": c.shares})
            })
            .collect();
        serde_json::json!({
            "news": tp.news,
            "vcm": tp.vcm.iter().map(|(c, u, n)| serde_json::json!({"company_id": c, "until": u, "count": n})).collect::<Vec<_>>(),
            "companies": companies,
        })
    };
    let bot_json = bot_json.map(|mut b| {
        // what the market makers made or lost dealing in this bot's season (vs the baseline: the bot's share)
        b["mm_dealing_pnl"] = metrics["npc"]["mm_dealing_pnl"].clone();
        let y = |v: i64| v as f64 / 100.0;
        b["markout_by_counterparty"] = serde_json::json!(bot_by_cp.iter().map(|(k, v)| (k.clone(), y(*v))).collect::<std::collections::BTreeMap<_, _>>());
        b["markout_by_hour"] = serde_json::json!(bot_by_hour.iter().map(|(k, v)| (k.to_string(), y(*v))).collect::<std::collections::BTreeMap<_, _>>());
        b["volume"] = serde_json::json!(bot_volume.iter().map(|(k, v)| (k.to_string(), y(*v))).collect::<std::collections::BTreeMap<_, _>>());
        b
    });
    metrics["capital"] = capital;
    metrics["grown_cash"] = serde_json::json!(grown_cash as f64 / 100.0);
    metrics["mm_counterparty_markout"] =
        serde_json::json!(mm_counterparty.iter().map(|(k, v)| (k.clone(), *v as f64 / 100.0)).collect::<std::collections::BTreeMap<_, _>>());
    if let Some(p) = &probe {
        metrics["probe"] = p.report();
    }
    Ok(SimResult { events, metrics, bot: bot_json, series })
}
