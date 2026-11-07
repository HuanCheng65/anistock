//! Season simulation of the v2 NPC market (docs/v2-npc-market.md §7) and the §6 exploit tests.
//!
//!     marketsim --world WORLD.db --inputs DIR --out DIR [--collector anistock.db] [--days N]
//!               [--set key=value ...] [--margin key=value ...] [--bots all|none|name,name] [--threshold PTS]
//!               [--probe | --probe-players N] [--grow N_VALUE,DAYS,CASH_SCALE] [--bot-start auto|YYYY-MM-DD]
//!
//! WORLD.db: a launched game DB (read only). The baseline run has no bots; each bot strategy runs in its
//! own season replay (same seed) on its own thread. Writes DIR/metrics.json, DIR/bots.json, DIR/report.html.
//! --probe adds the liquidity probe (src/sim/probe.rs) to the baseline run: metrics.json "probe".
//! --bot-start: the bots (benchmarks included) start trading that day instead of the replay's second week; `auto` =
//! two weeks after the first premiere wave in the inputs (`sim::premiere_wave`).

use std::path::PathBuf;

use anistock::sim::bots::BotKind;
use anistock::sim::{report, run, SimOptions};
use anistock::Config;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut world = None;
    let mut inputs = None;
    let mut out = PathBuf::from("marketsim-out");
    let mut collector = None;
    let mut days = None;
    let mut sets = Vec::new();
    let mut margin_sets: Vec<String> = Vec::new();
    let mut futures_sets: Vec<String> = Vec::new();
    let mut bots_arg = "all".to_string();
    let mut threshold = 5.0;
    let mut probe = 0usize;
    let mut grow = None;
    let mut allow_old_inputs = false;
    let mut bot_start = None;
    let mut value_only = false;
    let mut no_night = false;
    let mut open_at_value = false;
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--world" => world = Some(PathBuf::from(&v)),
            "--inputs" => inputs = Some(PathBuf::from(&v)),
            "--out" => out = PathBuf::from(&v),
            "--collector" => collector = Some(PathBuf::from(&v)),
            "--days" => days = Some(v.parse().expect("--days")),
            "--set" => sets.push(v),
            "--margin" => margin_sets.push(v),
            "--futures" => futures_sets.push(v),
            "--bots" => bots_arg = v,
            "--threshold" => threshold = v.parse().expect("--threshold"),
            "--bot-start" => bot_start = Some(v),
            "--probe" => {
                probe = 20;
                i += 1;
                continue;
            }
            "--probe-players" => probe = v.parse().expect("--probe-players"),
            "--value-only" => {
                value_only = true;
                i += 1;
                continue;
            }
            "--no-night" => {
                no_night = true;
                i += 1;
                continue;
            }
            "--open-at-value" => {
                open_at_value = true;
                i += 1;
                continue;
            }
            "--allow-old-inputs" => {
                allow_old_inputs = true;
                i += 1;
                continue;
            }
            "--grow" => {
                let x: Vec<&str> = v.split(',').collect();
                grow = Some((x[0].parse().expect("--grow n"), x[1].parse().expect("--grow days"), x[2].parse().expect("--grow cash")));
            }
            other => panic!("unknown argument {other}"),
        }
        i += 2;
    }
    let mut cfg = Config::load().expect("config");
    // --margin / --futures key=value: override [margin] (融资融券) / [futures] (热度期货) for this run
    cfg.margin = overridden(&cfg.margin, &margin_sets, "--margin");
    cfg.futures = overridden(&cfg.futures, &futures_sets, "--futures");
    let mut market = cfg.market.clone();
    for kv in &sets {
        market.set(kv).unwrap_or_else(|e| panic!("--set {kv}: {e}"));
    }
    let inputs = inputs.expect("--inputs");
    let world = world.expect("--world");
    // a world fresh from `server.seed --until` has only v1's schema (replay-history migrates it the same way)
    anistock::store::migrate(&world).unwrap_or_else(|e| panic!("migrate {}: {e}", world.display()));
    let bot_start = bot_start.map(|v| {
        if v != "auto" {
            return v;
        }
        let last: String = {
            let conn = rusqlite::Connection::open_with_flags(&world, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("world");
            conn.query_row("SELECT value FROM state WHERE key = 'last_closed'", [], |r| r.get(0)).expect("last_closed")
        };
        let first = chrono::NaiveDate::parse_from_str(&last, "%Y-%m-%d").expect("last_closed date").succ_opt().unwrap().to_string();
        let (a, b, start) = anistock::sim::premiere_wave(&inputs, &first).expect("--bot-start auto");
        eprintln!("first premiere wave {a}..{b}: bots start {start}");
        start
    });
    let base = SimOptions {
        world_db: world,
        inputs_dir: inputs,
        collector_db: collector,
        max_days: days,
        market: market.clone(),
        bot: None,
        spread_every_min: 5,
        idle_players: 39,
        foresight: None,
        probe: 0,
        grow,
        allow_old_inputs,
        bot_start,
        value_only,
        no_night,
        open_at_value,
        history: false,
    };
    let bots: Vec<BotKind> = match if value_only { "none" } else { bots_arg.as_str() } {
        "none" => vec![],
        "all" => BotKind::all().to_vec(),
        list => list.split(',').map(|b| BotKind::parse(b).unwrap_or_else(|| panic!("unknown bot {b}"))).collect(),
    };
    std::fs::create_dir_all(&out).unwrap();
    let t0 = std::time::Instant::now();
    // the fan bots need the baseline's hidden values (deterministic: they depend on the inputs, not on trading)
    let (fans, others): (Vec<BotKind>, Vec<BotKind>) = bots.iter().partition(|b| b.foresight_sigma().is_some());
    let spawn = |b: BotKind, o: &SimOptions| {
        let (cfg, mut o) = (cfg.clone(), o.clone());
        o.bot = Some(b);
        std::thread::spawn(move || (b, run(&cfg, &o)))
    };
    let mut handles: Vec<_> = others.iter().map(|&b| spawn(b, &base)).collect();
    // the liquidity probe's players trade in the baseline run only (the bots' runs stay comparable)
    let res = run(&cfg, &SimOptions { probe, ..base.clone() }).unwrap_or_else(|e| panic!("baseline: {e}"));
    if !fans.is_empty() {
        let mut fs = std::collections::HashMap::new();
        for (c, _, rows) in &res.series.paths {
            for (d, _, fair, div) in rows {
                fs.insert((*c, d.clone()), (*fair, *div));
            }
        }
        let mut o = base.clone();
        o.foresight = Some(std::sync::Arc::new(fs));
        handles.extend(fans.iter().map(|&b| spawn(b, &o)));
    }
    let mut bot_results = Vec::new();
    for h in handles {
        let (b, r) = h.join().unwrap();
        match r {
            Ok(r) => bot_results.push((b.name().to_string(), r.bot.unwrap())),
            Err(e) => eprintln!("bot {}: {e}", b.name()),
        }
    }
    std::fs::write(out.join("metrics.json"), serde_json::to_string_pretty(&res.metrics).unwrap()).unwrap();
    std::fs::write(out.join("series.json"), serde_json::to_string(&res.series).unwrap()).unwrap();
    std::fs::write(out.join("events.json"), serde_json::to_string(&res.events).unwrap()).unwrap();
    std::fs::write(out.join("bots.json"), serde_json::to_string_pretty(&bot_results).unwrap()).unwrap();
    let mut metrics = res.metrics.clone();
    if let Ok(t) = std::fs::read_to_string(cfg.root.join("sim/calibration/valuation_filter.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
            metrics["filter_fit"] = v["fit"].clone();
        }
    }
    let html = report::render(&metrics, &res.series, &bot_results, &market.to_toml(), threshold);
    std::fs::write(out.join("report.html"), html).unwrap();
    eprintln!("done in {:.0}s -> {}", t0.elapsed().as_secs_f64(), out.display());
    println!("{}", serde_json::to_string(&res.metrics["groups"]["all"]).unwrap());
    println!("{}", serde_json::to_string(&res.metrics["npc"]).unwrap());
    if probe > 0 {
        println!("{}", serde_json::to_string(&res.metrics["probe"]).unwrap());
    }
    for (k, v) in &bot_results {
        println!("{k}: {:.2}%", v["return"].as_f64().unwrap_or(f64::NAN) * 100.0);
    }
}

/// `rules` with `key=value` overrides (TOML values) applied.
fn overridden<T: serde::Serialize + serde::de::DeserializeOwned>(rules: &T, sets: &[String], flag: &str) -> T {
    let mut t = toml::Value::try_from(rules).expect("rules");
    for kv in sets {
        let (k, v) = kv.split_once('=').unwrap_or_else(|| panic!("{flag} {kv}: key=value"));
        let v: toml::Value = toml::from_str(&format!("x = {v}")).map(|t: toml::Table| t["x"].clone()).unwrap_or_else(|e| panic!("{flag} {kv}: {e}"));
        t.as_table_mut().unwrap().insert(k.trim().to_string(), v);
    }
    t.try_into().unwrap_or_else(|e| panic!("{flag}: {e}"))
}
