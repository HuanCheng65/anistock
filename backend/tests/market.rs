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
    let (_, m) = one_day(3);
    let cfgm = small(3);
    // ∫ intraday curve over 08:45-23:00, in hours
    let c = cfgm.curve;
    let area = 0.25 * c[0] + 3.0 * c[1] + 1.5 * c[2] + 4.5 * c[3] + 2.0 * c[4] + 3.0 * c[5];
    for (kind, n, rate) in [
        ("Value", cfgm.n_value, cfgm.rate_value),
        ("Retail", cfgm.n_retail, cfgm.rate_retail),
        ("Trend", cfgm.n_trend, cfgm.rate_trend),
    ] {
        let expected = n as f64 * rate * area;
        let got = *m.stats.actions.get(kind).unwrap_or(&0) as f64;
        assert!((got / expected - 1.0).abs() < 0.1, "{kind}: {got} actions, expected {expected:.0}");
    }
    // the curve is respected: nothing outside 08:45-23:00
    assert_eq!(m.curve(8 * 3600), 0.0);
    assert_eq!(m.curve(23 * 3600 + 1), 0.0);
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
