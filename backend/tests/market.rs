//! The v2 NPC market: constraints, determinism, activity, float migration, fund flows.

use anistock::market::{at_local, Market, MarketConfig};
use anistock::model::Company;
use anistock::{Config, Engine, Fees, Rules};

const DAY: &str = "2026-11-05";

fn cfg() -> Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    Config::load_from(&root, |_| None).unwrap()
}

/// Six listed companies; the committee holds 75%, the market maker the 25% float (as seeded in v1).
fn world() -> Engine {
    let (mut e, _) = Engine::in_memory(Fees::default(), Rules::default());
    let ts = "2026-11-04T20:00:00+08:00";
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, ts);
    }
    let world = e.special("world").unwrap();
    for i in 0..6i64 {
        let id = 1000 + i;
        let price = 500 + 700 * i;
        e.insert_company(Company {
            id,
            code: format!("{}", 100001 + i),
            title: format!("番{i}"),
            title_ja: None,
            title_en: None,
            studio: None,
            episodes: Some(12),
            long_runner: 0,
            first_air: "2026-10-01".into(),
            listed_on: "2026-09-10".into(),
            status: "listed".into(),
            sub_start: None,
            sub_end: None,
            shares: 10_000,
            ipo_price: price,
            prev_close: price,
            open_ref: Some(price),
            vcm_until: None,
            vcm_low: None,
            vcm_high: None,
            vcm_count: 0,
            last_price: price,
            fair_value: price * (90 + 4 * i) / 100,
            profit_week: 0,
            low_weeks: 0,
            delisted_on: None,
            source: None,
            genres: String::new(),
        });
        let committee = e.create_account("committee", "c", None, None, Some(id), ts);
        let mm = e.create_account("mm", "mm", None, None, Some(id), ts);
        e.transfer(world, mm, 3_000_000, "grant", DAY, ts, None, None);
        e.add_shares(committee, id, 7500, price, "2026-09-10");
        e.add_shares(mm, id, 2500, price, "2026-09-10");
    }
    e
}

fn small(seed: u64) -> MarketConfig {
    let mut m = MarketConfig::default();
    m.seed = seed;
    m.n_value = 40;
    m.n_trend = 20;
    m.n_news = 10;
    m.n_retail = 30;
    m.population_reference = 0.0;
    m.watch_min = 2;
    m.watch_max = 5;
    m
}

/// Create the population, migrate the float, trade one day. Returns the engine and the market.
fn one_day(seed: u64) -> (Engine, Market) {
    let cfg = cfg();
    let mut e = world();
    let mut m = Market::new(small(seed), &cfg);
    let t0 = at_local(DAY, 8, 0);
    let ts0 = anistock::market::ts_of(t0);
    m.create_population(&mut e, "2026-11-04", &ts0);
    m.rebalance_float(&mut e, "2026-11-04", &ts0);
    m.refresh_universe(&e, t0);
    m.step_until(&mut e, at_local(DAY, 23, 0));
    (e, m)
}

fn fingerprint(e: &Engine) -> Vec<(i64, i64, i64)> {
    let mut v: Vec<(i64, i64, i64)> = e.accounts().map(|a| (a.id, a.cash, a.cash_frozen)).collect();
    for c in e.companies() {
        v.push((c.id, c.last_price, c.vcm_count));
    }
    v
}

#[test]
fn a_day_of_trading_keeps_every_constraint() {
    let (e, m) = one_day(1);
    e.check_invariants().unwrap();
    let orders: u64 = m.stats.orders.values().sum();
    assert!(orders > 1000, "{:?}", m.stats);
    // every order was valid: no agent sells what it does not hold or spends cash it does not have
    assert_eq!(m.stats.rejected, 0);
    for a in &m.agents {
        let acct = e.account(a.account).unwrap();
        assert!(acct.cash >= 0 && acct.cash_frozen <= acct.cash);
        for h in e.holdings_of(a.account) {
            assert!(h.qty >= 0 && h.qty_frozen >= 0 && h.qty_frozen <= h.qty, "{h:?}");
        }
    }
    // prices moved
    assert!(e.companies().any(|c| c.last_price != c.prev_close));
}

#[test]
fn same_seed_same_market() {
    let (a, _) = one_day(7);
    let (b, _) = one_day(7);
    let (c, _) = one_day(8);
    assert_eq!(fingerprint(&a), fingerprint(&b));
    assert_ne!(fingerprint(&a), fingerprint(&c));
}

#[test]
fn actions_arrive_at_the_configured_rates() {
    // a Poisson count over one day: averaged over four seeds (one seed is a ~5% draw either way, and any
    // change in NPC behaviour moves the shared random stream)
    let cfgm = small(3);
    // ∫ intraday curve over 08:45-23:00, in hours
    let c = cfgm.curve;
    let area = 0.25 * c[0] + 3.0 * c[1] + 1.5 * c[2] + 4.5 * c[3] + 2.0 * c[4] + 3.0 * c[5];
    let runs: Vec<Market> = (1..=4).map(|seed| one_day(seed).1).collect();
    for (kind, n, rate) in [
        ("Value", cfgm.n_value, cfgm.rate_value),
        ("Retail", cfgm.n_retail, cfgm.rate_retail),
        ("Trend", cfgm.n_trend, cfgm.rate_trend),
    ] {
        let expected = n as f64 * rate * area * runs.len() as f64;
        let got: f64 = runs.iter().map(|m| *m.stats.actions.get(kind).unwrap_or(&0) as f64).sum();
        assert!((got / expected - 1.0).abs() < 0.1, "{kind}: {got} actions, expected {expected:.0}");
    }
    let m = &runs[0];
    // the curve is respected: nothing in the closed hours 04:00-08:45 (the night session is tests/night.rs)
    assert_eq!(m.curve(8 * 3600), 0.0);
    assert_eq!(m.curve(4 * 3600), 0.0);
}

#[test]
fn float_migration_leaves_the_target_with_the_market_maker() {
    let cfg = cfg();
    let mut e = world();
    let mut m = Market::new(small(5), &cfg);
    let ts = anistock::market::ts_of(at_local(DAY, 8, 0));
    m.create_population(&mut e, "2026-11-04", &ts);
    let world = e.special("world").unwrap();
    let world_before = e.account(world).unwrap().cash;
    m.rebalance_float(&mut e, "2026-11-04", &ts);
    e.check_invariants().unwrap();
    for c in e.companies().map(|c| c.id).collect::<Vec<_>>() {
        let mm = e.account_of("mm", c).unwrap();
        assert_eq!(e.holding(mm, c).unwrap().qty, 500); // 20% of the 2,500-share float
        assert_eq!(e.account(mm).unwrap().cash, (m.cfg.mm_cash * 100.0) as i64);
        let committee = e.account_of("committee", c).unwrap();
        assert_eq!(e.holding(committee, c).unwrap().qty, 7500); // the rights holders are not touched
    }
    // the agents paid for their shares with money that came from the world account, and the market makers'
    // surplus cash went back: the world's balance changed by exactly the market makers' cash change
    let mm_cash_before = 6 * 3_000_000;
    let mm_cash_after = 6 * (m.cfg.mm_cash * 100.0) as i64;
    assert_eq!(e.account(world).unwrap().cash - world_before, mm_cash_before - mm_cash_after);
}

#[test]
fn fund_flows_are_bounded_and_zero_sum() {
    let cfg = cfg();
    let (mut e, mut m) = one_day(11);
    let world = e.special("world").unwrap();
    // drain one agent's cash to the world account
    let a = m.agents[0].clone();
    let cash = e.available_cash(a.account);
    e.transfer(a.account, world, cash, "test", DAY, "2026-11-06T06:30:00+08:00", None, None);
    let before = e.account(a.account).unwrap().cash;
    m.fund_flows(&mut e, DAY, "2026-11-06T06:30:00+08:00");
    let got = e.account(a.account).unwrap().cash - before;
    let cap = (a.start_cash as f64 * m.cfg.fund_weekly_cap / 7.0).round() as i64;
    assert!(got > 0 && got <= cap, "{got} > {cap}");
    e.check_invariants().unwrap();
    let _ = cfg;
}

#[test]
fn beliefs_are_pure_functions_of_seed_agent_company_time() {
    let cfg = cfg();
    let m1 = Market::new(small(42), &cfg);
    let m2 = Market::new(small(42), &cfg);
    let t = at_local(DAY, 15, 0);
    assert_eq!(m1.bias(3, 1001, t), m2.bias(3, 1001, t));
    assert_ne!(m1.bias(3, 1001, t), m1.bias(4, 1001, t));
    // the bias drifts slowly: within a few hours it barely moves
    let d = (m1.bias(3, 1001, t) - m1.bias(3, 1001, t + 3 * 3600)).abs();
    assert!(d < 0.02, "{d}");
    // stationary spread close to bias_sigma
    let xs: Vec<f64> = (0..2000).map(|i| m1.bias(i, 1001, t)).collect();
    let sd = (xs.iter().map(|x| x * x).sum::<f64>() / xs.len() as f64).sqrt();
    assert!((sd / m1.cfg.bias_sigma - 1.0).abs() < 0.1, "{sd}");
}

#[test]
fn a_restarted_population_keeps_watching_what_it_holds() {
    let cfg = cfg();
    let (e, m) = one_day(13);
    // a restart with no heat information (every stock equally hot): watch lists are redrawn
    let mut m2 = Market::new(small(13), &cfg);
    m2.attach_population(&e);
    assert_eq!(m2.agents.len(), m.agents.len());
    for (a, b) in m.agents.iter().zip(&m2.agents) {
        assert_eq!(a.account, b.account);
        assert_eq!(a.start_cash, b.start_cash);
        for h in e.holdings_of(b.account).filter(|h| h.qty > 0) {
            assert!(
                b.watch.contains(&h.company_id),
                "agent {} holds {} but does not watch it",
                b.account,
                h.company_id
            );
        }
    }
}

#[test]
fn migrating_a_v1_database_retires_the_v1_npcs_zero_sum() {
    let mut cfg = cfg();
    cfg.market = small(21);
    let mut e = world();
    let world_id = e.special("world").unwrap();
    let ts = "2026-11-04T20:00:00+08:00";
    // a v1 noise trader with cash, shares and an open order
    let n = e.create_account("noise", "散户1", None, None, None, ts);
    e.transfer(world_id, n, 500_000, "grant", "2026-11-04", ts, None, None);
    let mm = e.account_of("mm", 1000).unwrap();
    e.remove_shares(mm, 1000, 100, 0).unwrap();
    e.add_shares(n, 1000, 100, 500, "2026-10-01");
    e.place_order(&anistock::NewOrder {
        account_id: n, company_id: 1000, side: "sell", kind: "limit", qty: 50, price: Some(900), day: DAY,
        ts, match_: false,
    })
    .unwrap();
    let mut rt = anistock::market::runtime::MarketRuntime::new(&cfg);
    rt.migrate(&mut e, &cfg, at_local(DAY, 6, 40)).unwrap();
    e.check_invariants().unwrap();
    let a = e.account(n).unwrap();
    assert_eq!((a.kind.as_str(), a.cash, a.cash_frozen), ("retired", 0, 0));
    assert!(e.holding(n, 1000).is_none());
    assert_eq!(e.state("npc_market"), Some("1"));
    assert!(e.accounts().filter(|a| a.kind == "npc").count() > 50);
    // the market maker keeps its target; the rest went to the agents
    assert_eq!(e.holding(mm, 1000).unwrap().qty, 500);
}

#[test]
fn designation_places_the_loan_once_and_only_moves_committee_shares() {
    let cfg = cfg();
    let (mut e, _) = one_day(17);
    let mut mc = small(17);
    mc.mm_loan_frac = 0.2;
    mc.mm_cash = 50_000.0;
    let mut m = Market::new(mc, &cfg);
    m.attach_population(&e);
    let ts = "2026-11-06T08:00:00+08:00";
    let before: Vec<(i64, i64, i64, i64)> = e
        .companies()
        .map(|c| {
            let (com, mm) = (e.account_of("committee", c.id).unwrap(), e.account_of("mm", c.id).unwrap());
            (c.id, e.holding(com, c.id).map_or(0, |h| h.qty), e.holding(mm, c.id).map_or(0, |h| h.qty), c.last_price)
        })
        .collect();
    let npc_shares = |e: &Engine, m: &Market| -> i64 {
        m.agents.iter().map(|a| e.holdings_of(a.account).map(|h| h.qty).sum::<i64>()).sum()
    };
    let npc_before = npc_shares(&e, &m);
    assert!(m.designation_pending(&e));
    let moved = m.designate_market_makers(&mut e, DAY, ts);
    assert_eq!(moved, 6 * 2000);
    e.check_invariants().unwrap();
    for (c, com_q, mm_q, price) in before {
        let (com, mm) = (e.account_of("committee", c).unwrap(), e.account_of("mm", c).unwrap());
        assert_eq!(e.holding(com, c).unwrap().qty, com_q - 2000);
        assert_eq!(e.holding(mm, c).unwrap().qty, mm_q + 2000);
        assert_eq!(e.company(c).unwrap().last_price, price); // off the book: no trade, no price change
        assert!(e.account(mm).unwrap().cash >= 5_000_000);
    }
    assert_eq!(npc_shares(&e, &m), npc_before);
    // idempotent: a restart places nothing more
    assert!(!m.designation_pending(&e));
    assert_eq!(m.designate_market_makers(&mut e, DAY, ts), 0);
}

#[test]
fn a_larger_configured_population_is_appended_and_reattached() {
    let cfg = cfg();
    let (mut e, m) = one_day(19);
    let n0 = m.agents.len();
    let mut mc = small(19);
    mc.n_value = 70; // +30 value agents
    mc.grow_cash_scale = 0.33;
    let mut m2 = Market::new(mc.clone(), &cfg);
    m2.attach_population(&e);
    assert_eq!(m2.agents.len(), n0);
    let ts = "2026-11-06T08:00:00+08:00";
    assert_eq!(m2.grow_population(&mut e, DAY, ts), 30);
    e.check_invariants().unwrap();
    assert_eq!(m2.grow_population(&mut e, DAY, ts), 0);
    // after a restart the appended block is found again, accounts in the same order
    let mut mc = mc;
    mc.grow_cash_scale = 0.5; // a later config change does not alter the recorded block's funding
    let mut m3 = Market::new(mc, &cfg);
    m3.attach_population(&e);
    assert_eq!(m3.agents.len(), n0 + 30);
    for (a, b) in m2.agents.iter().zip(&m3.agents) {
        assert_eq!((a.account, a.kind, a.start_cash), (b.account, b.kind, b.start_cash));
    }
}

// ---------------------------------------------------------------- sentiment layer (docs/v2-sentiment.md)

fn layer_on(seed: u64) -> MarketConfig {
    let mut m = small(seed);
    m.sentiment_strength = 0.15;
    m.attention = 0.25;
    m.attention_rank_n = 6;
    m.reaction = true;
    m
}

#[test]
fn market_sentiment_is_a_seeded_stationary_ar1() {
    use anistock::market::{secret_salt, sentiment_band, sentiment_level};
    let mc = layer_on(3);
    let salt = secret_salt(b"a secret");
    let xs: Vec<f64> = (20_000..24_000).map(|d| sentiment_level(&mc, salt, d)).collect();
    // deterministic per (seed, secret, day)
    assert_eq!(xs[17], sentiment_level(&mc, salt, 20_017));
    assert_ne!(xs[17], sentiment_level(&mc, secret_salt(b"another"), 20_017));
    let mut other = mc.clone();
    other.seed = 4;
    assert_ne!(xs[17], sentiment_level(&other, salt, 20_017));
    // unit variance, lag-1 autocorrelation = 0.5^(1/half-life)
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    let ac = xs.windows(2).map(|w| (w[0] - mean) * (w[1] - mean)).sum::<f64>() / n / var;
    let phi = 0.5f64.powf(1.0 / mc.sentiment_half_life_days);
    assert!(mean.abs() < 0.3 && (var - 1.0).abs() < 0.3, "{mean} {var}");
    assert!((ac - phi).abs() < 0.03, "{ac} {phi}");
    // the bands split at ±sentiment_band; off: no band
    let hot = xs.iter().filter(|x| **x > mc.sentiment_band).count() as f64 / n;
    assert!((0.15..0.35).contains(&hot), "{hot}");
    assert_eq!(sentiment_band(&MarketConfig::default(), salt, DAY), None);
    assert!(sentiment_band(&mc, salt, DAY).is_some());
}

#[test]
fn a_day_with_the_sentiment_layer_keeps_every_constraint_and_is_deterministic() {
    let run = |seed: u64| {
        let cfg = cfg();
        let mut e = world();
        let mut m = Market::new(layer_on(seed), &cfg);
        let t0 = at_local(DAY, 8, 0);
        let ts0 = anistock::market::ts_of(t0);
        m.create_population(&mut e, "2026-11-04", &ts0);
        m.rebalance_float(&mut e, "2026-11-04", &ts0);
        m.refresh_universe(&e, t0);
        m.step_until(&mut e, at_local(DAY, 23, 0));
        (e, m)
    };
    let (a, ma) = run(7);
    a.check_invariants().unwrap();
    assert_eq!(ma.stats.rejected, 0);
    // the movers' lists drew attention, and those triggers are handed out for logging
    assert!(ma.events.iter().any(|e| e.kind == "attention" && e.source == "rank"));
    assert_eq!(ma.fresh.len(), ma.events.len());
    let (b, _) = run(7);
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn premiere_night_attention_is_random_but_reproducible() {
    let cfg = cfg();
    let e = world();
    let air = at_local("2026-10-01", 22, 0);
    let mk = || {
        let mut m = Market::new(layer_on(11), &cfg);
        m.set_air_times((0..6).map(|i| (1000 + i, vec![air, air + 7 * 86_400])).collect());
        m.refresh_universe(&e, air - 86_400);
        m
    };
    let (m1, m2) = (mk(), mk());
    let at: Vec<f64> = (0..6).map(|i| m1.attention(1000 + i, air + 1800)).collect();
    assert_eq!(at, (0..6).map(|i| m2.attention(1000 + i, air + 1800)).collect::<Vec<_>>());
    assert!(at.iter().all(|a| *a >= 0.0) && at.iter().any(|a| *a > 0.0), "{at:?}");
    // nothing before the premiere, and it fades
    assert!((0..6).all(|i| m1.attention(1000 + i, air - 60) == 0.0));
    assert!((0..6).all(|i| m1.attention(1000 + i, air + 3 * 86_400) < 1e-3));
}

#[test]
fn a_streak_of_same_sign_revisions_turns_under_reaction_into_over_reaction() {
    use anistock::market::{Credibility, Event, Nature};
    let cfg = cfg();
    let mut m = Market::new(layer_on(5), &cfg);
    let rev = |at: i64, change: f64| Event {
        company_id: 1000,
        kind: "revision".into(),
        direction: if change > 0.0 { 1 } else { -1 },
        magnitude: (change.abs() * 50.0).min(10.0),
        credibility: Credibility::Official,
        source: "估值修正".into(),
        at,
        nature: Nature::Sentiment,
        half_life_h: 2.0,
        title: String::new(),
        change,
    };
    let t = at_local(DAY, 3, 0);
    let w = 86_400 * 7;
    // up, up, up (a week apart), then down; then up again after a gap longer than the window
    for (k, ch) in [(0, 0.08), (1, 0.06), (2, 0.1), (3, -0.07)] {
        m.events.push(rev(t + k * w, ch));
    }
    let late = t + 3 * w + 9 * 86_400;
    m.events.push(rev(late, 0.05));
    m.index_reactions();
    let r = |k: i64| m.reaction(1000, t + k * w).unwrap();
    let mc = layer_on(5);
    let first = |x: (f64, f64)| (mc.reaction_first[0]..=mc.reaction_first[1]).contains(&x.0);
    assert!(first(r(0)), "{:?}", r(0));
    assert!(r(1).0 > 1.0 && r(2).0 > r(1).0.min(mc.reaction_max) - 1.0 && r(2).0 <= mc.reaction_max, "{:?} {:?}", r(1), r(2));
    assert!(first(r(3)), "the first fall after rises is a fresh surprise: {:?}", r(3));
    assert!(first(m.reaction(1000, late).unwrap()));
    for k in 0..4 {
        let hl = r(k).1;
        assert!((mc.reaction_half_life_h[0]..=mc.reaction_half_life_h[1]).contains(&hl), "{hl}");
    }
    assert_eq!(m.streak(1000), 1);
}

// ---------------------------------------------------------------- sentiment layer, batch 2

#[test]
fn a_run_in_a_hot_stock_starts_a_seeded_bubble_that_survives_a_restart() {
    let cfg = cfg();
    let mut mc = small(9);
    mc.bubble_prob = 1.0;
    let mut e = world();
    let close = |d: i64| at_local(DAY, 6, 30) + d * 86_400;
    // six flat closes, then +30% on the last: the run (5 closes) crosses 25% once
    let base: Vec<i64> = e.companies().map(|c| c.last_price).collect();
    let ids: Vec<i64> = e.companies().map(|c| c.id).collect();
    // no inputs: heat falls back to its default (under the average), so any heat qualifies here
    let mut mc_hot = mc.clone();
    mc_hot.bubble_heat = 0.0;
    let mut m = Market::new(mc_hot.clone(), &cfg);
    m.refresh_universe(&e, at_local(DAY, 8, 0));
    for d in 0..7 {
        if d == 6 {
            let id = ids[2];
            let mut co = e.company(id).unwrap().clone();
            co.last_price = base[2] * 13 / 10;
            e.insert_company(co);
        }
        m.note_closes(&mut e, close(d));
    }
    let bubbles: Vec<_> = m.events.iter().filter(|x| x.kind == "bubble").cloned().collect();
    assert_eq!(bubbles.len(), 1, "{bubbles:?}");
    assert_eq!(bubbles[0].company_id, ids[2]);
    let (s, burst, end) = m.bubble_draws(&bubbles[0]);
    assert!(s > 0.0 && burst > close(6) && end > burst && end - close(6) < 8 * 86_400);
    assert_eq!(m.bubble(ids[2], close(6) + 60).map(|b| b.0), Some(true));
    assert_eq!(m.bubble(ids[2], burst + 1).map(|b| b.0), Some(false));
    assert_eq!(m.bubble(ids[2], end + 1), None);
    // a restarted market reads the closes back from the state table and the bubble from the event log
    let mut m2 = Market::new(mc_hot, &cfg);
    m2.events.push(bubbles[0].clone());
    assert_eq!(m2.bubble_draws(&bubbles[0]), (s, burst, end));
    m2.note_closes(&mut e, close(7));
    assert!(m2.events.iter().filter(|x| x.kind == "bubble").count() == 1, "no second trigger while the run continues");
    // off: nothing
    let mut off = Market::new(small(9), &cfg);
    off.note_closes(&mut e, close(8));
    assert!(off.bubble(ids[2], close(6) + 60).is_none());
}
