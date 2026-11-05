//! Rust side of the differential test: apply a difftest operation file (see backend/difftest/gen.py) to a
//! game.db with the engine, write one result line per operation, flush, and check the invariants.
//!
//!     difftest GAME.db OPS.jsonl RESULTS.jsonl

use std::io::{BufRead, BufWriter, Write};

use anistock::daily::CloseOutcome;
use anistock::engine::NewOrder;
use anistock::inputs::Inputs;
use anistock::pyrandom::PyRandom;
use anistock::{store, Config, Engine, Fees, Rules};
use serde_json::{json, Value};

fn s<'a>(op: &'a Value, k: &str) -> &'a str {
    op[k].as_str().unwrap_or_else(|| panic!("op field {k}: {op}"))
}
fn i(op: &Value, k: &str) -> i64 {
    op[k].as_i64().unwrap_or_else(|| panic!("op field {k}: {op}"))
}

/// Same as `edge_price` in driver.py: a price on a VCM edge of the company's current state.
fn edge_price(e: &Engine, op: &Value) -> i64 {
    let Some(co) = e.company(i(op, "co")) else { return 100 };
    let (what, end) = s(op, "edge").split_once('_').unwrap();
    let off = i(op, "off");
    if what == "band" && co.vcm_low.is_some() {
        return if end == "hi" { co.vcm_high.unwrap() } else { co.vcm_low.unwrap() } + off;
    }
    let thr0 = Rules::default().vcm_threshold;
    let (r, thr) = if what == "last" {
        (co.last_price, thr0)
    } else {
        (anistock::exchange::vcm_anchor(co), thr0 * (co.vcm_count + 1) as f64)
    };
    if end == "hi" {
        (r as f64 * (1.0 + thr) + 1e-9).floor() as i64 + off
    } else {
        (r as f64 * (1.0 - thr) - 1e-9).ceil() as i64 + off
    }
}

fn subscribing(e: &Engine, idx: i64) -> i64 {
    let ids: Vec<i64> = e.companies().filter(|c| c.status == "subscribing").map(|c| c.id).collect();
    if ids.is_empty() {
        999_999_999
    } else {
        ids[(idx as usize) % ids.len()]
    }
}

/// The close result in v1's dict shape.
fn close_json(r: Result<CloseOutcome, String>) -> Value {
    match r {
        Ok(CloseOutcome::Skipped) => json!({"skipped": true}),
        Ok(CloseOutcome::Closed(s)) => json!({
            "day": s.day, "injected": s.injected, "exported": s.exported, "taxes": s.taxes,
            "interest": s.interest, "rate": s.rate,
        }),
        Err(err) => {
            println!("rust: close failed: {err}");
            json!({"error": "close failed"})
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: difftest GAME.db OPS.jsonl RESULTS.jsonl");
        std::process::exit(2);
    }
    let db = std::path::Path::new(&args[1]);
    store::migrate(db).expect("migrate");
    let mut cfg = Config::load_from(&anistock::config::find_root(), |_| None).expect("config");
    // the reference is the v1 Python server: v1 economy rules
    cfg.economy_v1 = true;
    let (fees, rules) = (cfg.fees(), cfg.rules());
    assert_eq!((fees, rules), (Fees::default(), Rules::v1()), "driver.py uses v1's defaults");
    let mut e = Engine::open(db, fees, rules).expect("load");
    let mut rng = PyRandom::new(0);
    let ops = std::io::BufReader::new(std::fs::File::open(&args[2]).expect("ops"));
    let mut out = BufWriter::new(std::fs::File::create(&args[3]).expect("results"));
    let mut placed: Vec<(i64, i64)> = Vec::new();
    let t0 = std::time::Instant::now();
    let mut n = 0;
    for line in ops.lines() {
        let line = line.unwrap();
        let op: Value = serde_json::from_str(&line).unwrap();
        n += 1;
        let res: Value = match s(&op, "op") {
            "place" => {
                let price = if op.get("edge").is_some() { Some(edge_price(&e, &op)) } else { op["price"].as_i64() };
                let r = e.place_order(&NewOrder {
                    account_id: i(&op, "acct"),
                    company_id: i(&op, "co"),
                    side: s(&op, "side"),
                    kind: s(&op, "kind"),
                    qty: i(&op, "qty"),
                    price,
                    day: s(&op, "day"),
                    ts: s(&op, "ts"),
                    match_: op["match"].as_bool().unwrap(),
                });
                match r {
                    Ok(p) => {
                        placed.push((p.order_id, i(&op, "acct")));
                        serde_json::to_value(&p).unwrap()
                    }
                    Err(err) => json!({"error": err.0}),
                }
            }
            k @ ("cancel" | "cancel_raw") => {
                let (oid, acct) = if k == "cancel" {
                    let (oid, owner) = if placed.is_empty() {
                        (1_000_000_000, i(&op, "acct"))
                    } else {
                        let pool = if op["recent"].as_bool().unwrap() {
                            &placed[placed.len().saturating_sub(20)..]
                        } else {
                            &placed[..]
                        };
                        pool[(i(&op, "idx") as usize) % pool.len()]
                    };
                    (oid, if op["own"].as_bool().unwrap() { owner } else { i(&op, "acct") })
                } else {
                    (i(&op, "order_id"), i(&op, "acct"))
                };
                match e.cancel_order(acct, oid, s(&op, "ts")) {
                    Ok(()) => json!({"ok": true, "order_id": oid}),
                    Err(err) => json!({"error": err.0}),
                }
            }
            "cancel_all" => {
                e.cancel_all(i(&op, "acct"), i(&op, "co"), s(&op, "ts"));
                json!({"ok": true})
            }
            "restart" => {
                // drop the engine (flushing everything) and load it back from the database
                e.flush().expect("flush");
                drop(e);
                e = Engine::open(db, fees, rules).expect("reload");
                json!({"ok": true})
            }
            "rng" => {
                rng = PyRandom::new(i(&op, "seed") as i128);
                json!({"ok": true})
            }
            "npc_tick" => {
                e.npc_tick(&cfg, s(&op, "day"), s(&op, "ts"), &mut rng, op["auction"].as_bool().unwrap());
                json!({"ok": true})
            }
            "place_rel" => {
                let ids: Vec<i64> = e.companies().filter(|c| c.status == "listed").map(|c| c.id).collect();
                let co = ids[(i(&op, "co_idx") as usize) % ids.len()];
                let last = e.company(co).unwrap().last_price;
                let kind = s(&op, "kind");
                let price = (kind == "limit")
                    .then(|| 1i64.max((last as f64 * op["rel"].as_f64().unwrap()).round_ties_even() as i64));
                match e.place_order(&NewOrder {
                    account_id: i(&op, "acct"),
                    company_id: co,
                    side: s(&op, "side"),
                    kind,
                    qty: i(&op, "qty"),
                    price,
                    day: s(&op, "day"),
                    ts: s(&op, "ts"),
                    match_: op["match"].as_bool().unwrap(),
                }) {
                    Ok(p) => {
                        placed.push((p.order_id, i(&op, "acct")));
                        serde_json::to_value(&p).unwrap()
                    }
                    Err(err) => json!({"error": err.0}),
                }
            }
            "subscribe" => {
                let co = subscribing(&e, i(&op, "idx"));
                let qty = i(&op, "qty");
                match e.subscribe(i(&op, "acct"), co, qty, s(&op, "today"), s(&op, "ts")) {
                    Ok(frozen) => json!({"ok": true, "qty": qty, "frozen": frozen}),
                    Err(err) => json!({"error": err.0}),
                }
            }
            "unsubscribe" => {
                let co = subscribing(&e, i(&op, "idx"));
                match e.unsubscribe(i(&op, "acct"), co) {
                    Ok(()) => json!({"ok": true}),
                    Err(err) => json!({"error": err.0}),
                }
            }
            "close" => {
                let inp = Inputs::read(std::path::Path::new(s(&op, "inputs"))).expect("inputs");
                close_json(e.run_day(&cfg, s(&op, "day"), s(&op, "ts"), &inp))
            }
            "catch_up" => {
                let dir = s(&op, "inputs").to_string();
                let mut res = Vec::new();
                // like the Python driver: one result per day, stopping at the first failure
                let ts = s(&op, "ts").to_string();
                let r = e.catch_up_clocked(&cfg, s(&op, "today"), || ts.clone(), |day| {
                    Inputs::read(&std::path::Path::new(&dir).join(format!("{day}.json")))
                });
                match r {
                    Ok(v) => res.extend(v.into_iter().map(|o| close_json(Ok(o)))),
                    Err(err) => {
                        println!("rust: catch_up failed: {err}");
                        res.push(json!({"error": "close failed"}));
                    }
                }
                Value::Array(res)
            }
            "auction" => {
                e.open_market(s(&op, "day"), s(&op, "ts"));
                json!({"ok": true})
            }
            "transfer" => {
                let from = i(&op, "from");
                // Python: int(available_cash * frac)
                let amount = op["raw"]
                    .as_i64()
                    .unwrap_or_else(|| (e.available_cash(from) as f64 * op["frac"].as_f64().unwrap()).trunc() as i64);
                e.transfer(from, i(&op, "to"), amount, s(&op, "reason"), s(&op, "day"), s(&op, "ts"), None, None);
                json!({"ok": true})
            }
            "move_shares" => {
                let (from, to, co) = (i(&op, "from"), i(&op, "to"), i(&op, "co"));
                // Python: int(available * frac), truncation toward zero
                let q = (e.available_shares(from, co) as f64 * op["frac"].as_f64().unwrap()).trunc() as i64;
                let taken = if q > 0 { e.remove_shares(from, co, q, 0).expect("move_shares") } else { vec![] };
                for (tq, cost, _) in &taken {
                    e.add_shares(to, co, *tq, *cost, s(&op, "day"));
                }
                json!({"taken": taken})
            }
            "remove_too_many" => {
                let (a, co) = (i(&op, "acct"), i(&op, "co"));
                let q = e.holding(a, co).map_or(0, |h| h.qty) + 1;
                match e.remove_shares(a, co, q, 0) {
                    Ok(_) => json!({"ok": true}),
                    Err(err) => json!({"error": err}),
                }
            }
            other => panic!("unknown op {other}"),
        };
        writeln!(out, "{}", serde_json::to_string(&res).unwrap()).unwrap();
    }
    let dt = t0.elapsed();
    e.flush().expect("flush");
    out.flush().unwrap();
    println!("rust: {n} ops in {:.1} ms (incl. closed-order cancel flushes)", dt.as_secs_f64() * 1e3);
    match e.check_invariants() {
        Ok(()) => println!("rust: invariants ok"),
        Err(err) => println!("rust: INVARIANTS FAILED: {err}"),
    }
    let conn = store::connect(db).unwrap();
    if let Err(err) = store::check_invariants(&conn) {
        println!("rust: DB INVARIANTS FAILED: {err}");
    }
}
