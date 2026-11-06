//! `anistock replay-history`: the pre-launch history under the current model (docs/ops-rebuild-replay.md): the integrated replay
//! (`sim::run`: the real engine, the real daily close, the NPC market) from a start world to a given day, with
//! the candles and company_days its closes write. `server/tools/rebuild_replay.py` makes the start world and
//! writes the result into the game database; this command only reads the inputs and its own start world. It is
//! a subcommand of the server binary because the production host has no Rust toolchain (deploy/binary.sh).
//!
//! ```text
//! anistock replay-history --world START.db --inputs DIR --until DAY --out HISTORY.json [--collector anistock.db]
//! ```
//!
//! START.db: a scratch game database seeded up to the window's first day (`server.seed --until`). It is migrated
//! in place (it is the tool's own copy). DIR: the daily inputs; every day after START.db's last close up to
//! DAY must be there, and only those days are read (each close sees that day's file, nothing later).
//! HISTORY.json: {"start": day, "until": day, "companies": {id: {ipo_price, listed_on, status, start_scale}},
//! "candles": [[id, day, open, high, low, close, volume]], "days": [[id, day, revenue, rev_us, rev_cn, rev_jp,
//! aired, dividend_ps]]}, money in cents. The start world's first-day candles are included, put on the model's
//! value the same way the run's opening prices are; its first-day company_days are not (they come from the old
//! seed model, whose revenue scale differs), so the operating history starts with the first close of this run.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{run, SimOptions};
use crate::{store, Config};
use serde_json::json;

fn fail(msg: String) -> ! {
    eprintln!("replay-history: {msg}");
    std::process::exit(2)
}

/// Run the command with its arguments (after `replay-history`); exits the process with 2 on an error.
pub fn cli(args: &[String]) {
    let (mut world, mut inputs, mut until, mut out, mut collector) = (None, None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).cloned().unwrap_or_else(|| fail(format!("{} needs a value", args[i])));
        match args[i].as_str() {
            "--world" => world = Some(PathBuf::from(v)),
            "--inputs" => inputs = Some(PathBuf::from(v)),
            "--until" => until = Some(v),
            "--out" => out = Some(PathBuf::from(v)),
            "--collector" => collector = Some(PathBuf::from(v)),
            other => fail(format!("unknown argument {other}")),
        }
        i += 2;
    }
    let world = world.unwrap_or_else(|| fail("--world is required".into()));
    let inputs = inputs.unwrap_or_else(|| fail("--inputs is required".into()));
    let until = until.unwrap_or_else(|| fail("--until is required".into()));
    let out = out.unwrap_or_else(|| fail("--out is required".into()));

    store::migrate(&world).unwrap_or_else(|e| fail(format!("migrate {}: {e}", world.display())));
    let conn = rusqlite::Connection::open(&world).unwrap_or_else(|e| fail(e.to_string()));
    let start: String = conn
        .query_row("SELECT value FROM state WHERE key='last_closed'", [], |r| r.get(0))
        .unwrap_or_else(|e| fail(format!("start world has no last_closed: {e}")));

    // the replay's days: every day after the start up to `until`, each with its own inputs file
    let mut want = Vec::new();
    let mut d = chrono::NaiveDate::parse_from_str(&start, "%Y-%m-%d").unwrap_or_else(|e| fail(e.to_string()));
    let end = chrono::NaiveDate::parse_from_str(&until, "%Y-%m-%d").unwrap_or_else(|e| fail(format!("--until: {e}")));
    while d < end {
        d = d.succ_opt().unwrap();
        want.push(d.format("%Y-%m-%d").to_string());
    }
    if want.is_empty() {
        fail(format!("--until {until} is not after the start world's last close {start}"));
    }
    for (k, day) in want.iter().enumerate() {
        if !inputs.join(format!("{day}.json")).exists() {
            fail(format!("inputs for {day} missing in {} (day {} of {})", inputs.display(), k + 1, want.len()));
        }
    }

    let cfg = Config::load().unwrap_or_else(|e| fail(format!("config: {e}")));
    let opts = SimOptions {
        world_db: world.clone(),
        inputs_dir: inputs,
        collector_db: collector,
        max_days: Some(want.len()),
        market: cfg.market.clone(),
        bot: None,
        spread_every_min: 5,
        idle_players: 0,
        foresight: None,
        probe: 0,
        grow: None,
        allow_old_inputs: false,
        bot_start: None,
        value_only: false,
        history: true,
    };
    let t0 = std::time::Instant::now();
    let res = run(&cfg, &opts).unwrap_or_else(|e| fail(e));
    let h = res.history.unwrap_or_else(|| fail("no history captured".into()));
    eprintln!("replayed {} days ({}..{}) in {:.0}s", want.len(), want[0], until, t0.elapsed().as_secs_f64());

    // the start world's own candles (its first day), put on the model's value like the run's opening prices
    let scale = |id: i64| h.start_scale.get(&id).copied().unwrap_or(1.0);
    let sc = |x: i64, k: f64| ((x as f64 * k).round() as i64).max(1);
    let mut candles: BTreeMap<(i64, String), [i64; 5]> = BTreeMap::new();
    let mut days: BTreeMap<(i64, String), [i64; 6]> = BTreeMap::new();
    let mut companies: BTreeMap<i64, serde_json::Value> = BTreeMap::new();
    {
        let mut s = conn.prepare("SELECT company_id, day, open, high, low, close, volume FROM candles").unwrap();
        let rows = s
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, [r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?])))
            .unwrap();
        for row in rows {
            let (id, day, v): (i64, String, [i64; 5]) = row.unwrap();
            let k = scale(id);
            candles.insert((id, day), [sc(v[0], k), sc(v[1], k), sc(v[2], k), sc(v[3], k), v[4]]);
        }
        let mut s = conn.prepare("SELECT id, ipo_price, listed_on, status FROM companies").unwrap();
        let rows = s
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?)))
            .unwrap();
        for row in rows {
            let (id, ipo, on, st) = row.unwrap();
            companies.insert(id, json!({"ipo_price": sc(ipo, scale(id)), "listed_on": on, "status": st, "start_scale": scale(id)}));
        }
    }
    for ((id, day), k) in &h.candles {
        candles.insert((*id, day.clone()), [k.open, k.high, k.low, k.close, k.volume]);
    }
    for ((id, day), r) in &h.days {
        days.insert((*id, day.clone()), [r.revenue, r.rev_us, r.rev_cn, r.rev_jp, r.aired, r.dividend_ps]);
    }
    for (id, (ipo, on, st)) in &h.companies {
        // a company listed during the replay: the IPO price its close set (start_scale 1)
        let prev = companies.get(id).cloned();
        let ipo = prev.as_ref().filter(|_| h.start_scale.contains_key(id)).and_then(|p| p["ipo_price"].as_i64()).unwrap_or(*ipo);
        companies.insert(*id, json!({"ipo_price": ipo, "listed_on": on, "status": st, "start_scale": scale(*id)}));
    }
    let doc = json!({
        "start": start,
        "until": until,
        "companies": companies.iter().map(|(k, v)| (k.to_string(), v.clone())).collect::<serde_json::Map<_, _>>(),
        "candles": candles.iter().map(|((id, d), v)| json!([id, d, v[0], v[1], v[2], v[3], v[4]])).collect::<Vec<_>>(),
        "values": h.values.iter().map(|((id, d), v)| json!([id, d, v])).collect::<Vec<_>>(),
        "days": days.iter().map(|((id, d), v)| json!([id, d, v[0], v[1], v[2], v[3], v[4], v[5]])).collect::<Vec<_>>(),
    });
    std::fs::write(&out, serde_json::to_string(&doc).unwrap()).unwrap_or_else(|e| fail(format!("{}: {e}", out.display())));
    eprintln!("{} companies, {} candles, {} company days -> {}", companies.len(), candles.len(), days.len(), out.display());
}
