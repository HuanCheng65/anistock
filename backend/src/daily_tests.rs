//! Port of tests/test_daily.py, plus the close's rollback.

use super::*;
use crate::{Fees, Rules};

const TS: &str = "2026-11-08T06:30:00+08:00";
const DAY: &str = "2026-11-08"; // a Sunday

fn cfg() -> Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = if root.join("server/game.toml").exists() { root } else { crate::config::find_root() };
    Config::load_from(&root, |_| None).unwrap()
}

fn company(id: i64, status: &str, shares: i64, first_air: &str, listed_on: &str) -> Company {
    Company {
        id,
        code: format!("{}", 100000 + id),
        title: if id == 1 { "テスト".into() } else { "新番".into() },
        title_ja: None,
        title_en: None,
        studio: None,
        episodes: None,
        long_runner: 0,
        first_air: first_air.into(),
        listed_on: listed_on.into(),
        status: status.into(),
        sub_start: None,
        sub_end: None,
        shares,
        ipo_price: 500,
        prev_close: 500,
        open_ref: None,
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 500,
        fair_value: 0,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
    }
}

/// The fixture of test_daily.py: world/cb/gov, company 1 with its company and committee accounts.
fn fixture() -> Engine {
    let (mut e, _sink) = Engine::in_memory(Fees::default(), Rules::default());
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    e.insert_company(company(1, "listed", 1000, "2026-07-01", "2026-06-10"));
    for k in ["company", "committee"] {
        e.create_account(k, k, None, None, Some(1), TS);
    }
    e
}

fn empty_inputs() -> Inputs {
    Inputs::parse(&format!(r#"{{"day":"{DAY}","cn_titles":{{}},"shows":[]}}"#)).unwrap()
}

fn ctx<'a>(e: &Engine, cfg: &'a Config, inp: &'a Inputs) -> Ctx<'a> {
    Ctx {
        cfg,
        p: Params::from_config(cfg).unwrap(),
        day: DAY.into(),
        d: date(DAY).unwrap(),
        ts: TS.into(),
        inp,
        companies: e.st.companies.values().cloned().collect(),
        listed: vec![],
        world: e.special("world").unwrap(),
        cb: e.special("cb").unwrap(),
        gov: e.special("gov").unwrap(),
        sum: Summary { day: DAY.into(), injected: 0, exported: 0, taxes: 0, interest: 0, rate: 0.03 },
        vtotals: None,
    }
}

fn cash(e: &Engine, id: i64) -> i64 {
    e.account(id).unwrap().cash
}

#[test]
fn dividend_tax_by_lot_age_and_founders_leave() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let (world, gov) = (e.special("world").unwrap(), e.special("gov").unwrap());
    let (company_acct, committee) = (e.account_of("company", 1).unwrap(), e.account_of("committee", 1).unwrap());
    let p = e.create_account("player", "p", Some("p"), None, None, TS);
    e.add_shares(committee, 1, 750, 500, "2026-06-10");
    e.add_shares(p, 1, 100, 500, "2026-06-20"); // held over 4 weeks: tax free
    e.add_shares(p, 1, 150, 500, "2026-11-05"); // held 3 days: 10%
    e.transfer(world, company_acct, 10_000, "revenue", DAY, TS, Some(1), None);
    e.st.companies.get_mut(&1).unwrap().profit_week = 10_000;
    let world_before = cash(&e, world);
    let mut cx = ctx(&e, &cfg, &inp);
    e.pay_dividends(&mut cx).unwrap();

    let dps = 9;
    let tax = (dps as f64 * 150.0 * 0.10) as i64;
    assert_eq!(cash(&e, p), dps * 250 - tax);
    assert_eq!(cash(&e, gov), tax);
    assert_eq!(cash(&e, world), world_before + dps * 750);
    assert_eq!(cx.sum.exported, dps * 750);
    assert_eq!(e.st.company_days[&(1, DAY.to_string())].dividend_ps, dps);
    e.check_invariants().unwrap();
}

#[test]
fn holding_tax_schedule() {
    let cfg = cfg();
    assert_eq!(holding_tax_rate(&cfg.game, "2026-11-08", "2026-11-08").unwrap(), 0.20);
    assert_eq!(holding_tax_rate(&cfg.game, "2026-11-05", "2026-11-08").unwrap(), 0.10);
    assert_eq!(holding_tax_rate(&cfg.game, "2026-10-12", "2026-11-08").unwrap(), 0.10);
    assert_eq!(holding_tax_rate(&cfg.game, "2026-10-11", "2026-11-08").unwrap(), 0.0);
}

fn subscribing(e: &mut Engine) -> i64 {
    e.st.companies.get_mut(&1).unwrap().status = "delisted".into(); // the fixture's other company is not used
    let mut c = company(2, "subscribing", 10_000, "2026-12-01", "2026-11-10");
    c.sub_start = Some("2026-11-06".into());
    c.sub_end = Some("2026-11-09".into());
    e.insert_company(c);
    for k in ["company", "committee"] {
        e.create_account(k, k, None, None, Some(2), TS);
    }
    let committee = e.account_of("committee", 2).unwrap();
    e.add_shares(committee, 2, 7500, 500, "2026-11-10");
    committee
}

fn apply(e: &mut Engine, name: &str, qty: i64) -> i64 {
    let world = e.special("world").unwrap();
    let pid = e.create_account("player", name, Some(name), None, None, TS);
    e.transfer(world, pid, 10_000_000, "grant", DAY, TS, None, None);
    let frozen = Fees::default().buy_reserve(500, qty);
    e.add_cash(pid, 0, frozen);
    e.st.ipo_apps.insert(
        (2, pid),
        IpoApplication {
            company_id: 2,
            account_id: pid,
            qty,
            frozen,
            allotted: None,
            created_at: format!("{TS}{name}"),
        },
    );
    pid
}

fn frozen_players(e: &Engine) -> i64 {
    e.accounts().filter(|a| a.kind == "player").map(|a| a.cash_frozen).sum()
}

#[test]
fn undersubscribed_ipo_fills_everyone_and_founders_keep_the_rest() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let committee = subscribing(&mut e);
    let (a, b) = (apply(&mut e, "a", 600), apply(&mut e, "b", 400));
    let mut cx = ctx(&e, &cfg, &inp);
    e.allot(&mut cx, 2).unwrap();
    assert_eq!((e.available_shares(a, 2), e.available_shares(b, 2)), (600, 400));
    assert_eq!(e.available_shares(committee, 2), 7500 + 1500);
    assert_eq!(frozen_players(&e), 0);
    assert_eq!(e.company(2).unwrap().status, "listed");
    e.check_invariants().unwrap();
}

#[test]
fn oversubscribed_ipo_claws_back_and_ballots() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    subscribing(&mut e);
    // base public tranche 1250; 25,007 applied = 20x -> clawback to 70% of the 2,500 float = 1,750; the
    // odd shares go by ballot. Expected numbers printed by v1 (server/daily.allot) for the same input.
    let pids: Vec<i64> =
        [9_999, 10_003, 3_997, 1_001, 7].iter().enumerate().map(|(i, q)| apply(&mut e, &format!("p{i}"), *q)).collect();
    let mut cx = ctx(&e, &cfg, &inp);
    e.allot(&mut cx, 2).unwrap();
    let got: Vec<i64> = pids.iter().map(|p| e.available_shares(*p, 2)).collect();
    assert_eq!(got, vec![699, 700, 281, 70, 0]);
    assert_eq!(cx.sum.exported, 700_000);
    assert_eq!(frozen_players(&e), 0);
    e.flush().unwrap();
    e.check_invariants().unwrap();
}

#[test]
fn a_failed_close_changes_nothing() {
    // the v1 valuation raises on a non-long-running show without an episode count, after revenue and the
    // Sunday dividends are booked: a convenient mid-close failure
    let mut cfg = cfg();
    cfg.economy_v1 = true;
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    let mut c = company(1, "listed", 1000, "2026-10-01", "2026-09-10");
    c.profit_week = 50_000;
    e.insert_company(c);
    for k in ["company", "committee"] {
        e.create_account(k, k, None, None, Some(1), TS);
    }
    let committee = e.account_of("committee", 1).unwrap();
    let company_acct = e.account_of("company", 1).unwrap();
    let world = e.special("world").unwrap();
    e.add_shares(committee, 1, 1000, 500, "2026-09-10");
    e.transfer(world, company_acct, 100_000, "revenue", "2026-11-07", TS, Some(1), None);
    let p = e.create_account("player", "p", Some("p"), None, None, TS);
    e.transfer(world, p, 1_000_000, "grant", "2026-11-07", TS, None, None);
    for (k, v) in [("rate", "0.03"), ("a_ref", "100000.0"), ("last_closed", "2026-11-07")] {
        e.set_state(k, v);
    }
    e.flush().unwrap();
    let before_changes = sink.0.lock().unwrap().len();
    let before: Vec<Account> = e.accounts().cloned().collect();
    // a listed, not long-running show without an episode count: v1 raises TypeError in the valuation,
    // after revenue and the Sunday dividends were already booked
    let inp = Inputs::parse(&format!(
        r#"{{"day":"{DAY}","cn_titles":{{}},"shows":[{{"id":1,"title_ja":"テスト","title_en":null,"studio":null,
        "first_air":"2026-10-01","episodes":null,"episodes_known":false,"long_runner":false,"popularity_end":9000,
        "aired":6,"has_real_data":true,"att":{{"us":100.0,"cn":50.0,"jp":10.0}},
        "expected":{{"us":100.0,"cn":50.0,"jp":10.0}}}}]}}"#
    ))
    .unwrap();
    let err = e.run_day(&cfg, DAY, TS, &inp).unwrap_err();
    assert!(err.contains("episodes"), "{err}");
    e.flush().unwrap();
    assert_eq!(sink.0.lock().unwrap().len(), before_changes, "a failed close must not write anything");
    assert_eq!(e.accounts().cloned().collect::<Vec<_>>(), before);
    assert_eq!(e.state("last_closed"), Some("2026-11-07"));
    assert_eq!(e.company(1).unwrap().profit_week, 50_000);
    // and the same day closes once the inputs are fixed
    let fixed = Inputs::parse(
        &serde_json::to_string(&serde_json::json!({
            "day": DAY, "cn_titles": {}, "shows": [{"id": 1, "title_ja": "テスト", "title_en": null, "studio": null,
            "first_air": "2026-10-01", "episodes": 12, "episodes_known": true, "long_runner": false,
            "popularity_end": 9000, "aired": 6, "has_real_data": true,
            "att": {"us": 100.0, "cn": 50.0, "jp": 10.0}, "expected": {"us": 100.0, "cn": 50.0, "jp": 10.0}}]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(e.run_day(&cfg, DAY, TS, &fixed).unwrap(), CloseOutcome::Closed(_)));
    assert!(matches!(e.run_day(&cfg, DAY, TS, &fixed).unwrap(), CloseOutcome::Skipped));
    assert_eq!(e.state("last_closed"), Some(DAY));
    e.check_invariants().unwrap();
    // the close wrote the player's asset snapshot (V6) in its own batch, once
    e.flush().unwrap();
    let days: Vec<PlayerDay> = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            Change::PlayerDayReplace(d) => Some(d.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(days.len(), 1, "{days:?}");
    let d = &days[0];
    assert_eq!((d.account_id, d.day.as_str(), d.cash, d.stock_value, d.restructured), (p, DAY, cash(&e, p), 0, false));
    let initial = (cfg.game.initial_cash * 100.0).round_ties_even();
    assert!((d.return_index - cash(&e, p) as f64 / initial).abs() < 1e-12, "{d:?}");
}

/// Company 2 open for subscription from 2026-11-06 to 2026-11-09, and a player with `cash` cents.
fn ipo_fixture(cash: i64) -> (Engine, i64) {
    let mut e = fixture();
    subscribing(&mut e);
    let world = e.special("world").unwrap();
    let p = e.create_account("player", "p", Some("p"), None, None, TS);
    e.transfer(world, p, cash, "grant", DAY, TS, None, None);
    (e, p)
}

#[test]
fn subscribe_replaces_and_withdraws() {
    let (mut e, p) = ipo_fixture(1_000_000);
    let frozen = e.subscribe(p, 2, 100, "2026-11-07", TS).unwrap();
    assert_eq!(frozen, Fees::default().buy_reserve(500, 100));
    assert_eq!(e.account(p).unwrap().cash_frozen, frozen);
    let f2 = e.subscribe(p, 2, 300, "2026-11-08", TS).unwrap(); // replaces the first application
    assert_eq!(e.account(p).unwrap().cash_frozen, f2);
    assert_eq!(e.ipo_application(2, p).unwrap().qty, 300);
    assert_eq!(e.subscribe(p, 2, 0, "2026-11-08", TS).unwrap(), 0); // qty 0 just withdraws
    assert!(e.ipo_application(2, p).is_none());
    e.subscribe(p, 2, 10, "2026-11-08", TS).unwrap();
    e.unsubscribe(p, 2).unwrap();
    assert_eq!(e.account(p).unwrap().cash_frozen, 0);
    assert_eq!(e.unsubscribe(p, 1).unwrap_err().0, "认购已经截止，不能撤回");
    assert_eq!(e.subscribe(p, 1, 10, "2026-11-08", TS).unwrap_err().0, "这支新股不在认购期");
}

#[test]
fn rejected_subscription_keeps_the_old_one() {
    // v1 withdraws the old application before checking the new one, inside the transaction it then rolls
    // back: on rejection the old application and its frozen cash stay exactly as they were
    let (mut e, p) = ipo_fixture(60_000);
    let frozen = e.subscribe(p, 2, 100, "2026-11-07", TS).unwrap();
    let err = e.subscribe(p, 2, 1000, "2026-11-07", TS).unwrap_err();
    assert_eq!(err.0, "可用资金不足：认购 1,000 股需要冻结 5,011.25 币");
    assert_eq!(e.ipo_application(2, p).unwrap().qty, 100);
    assert_eq!(e.account(p).unwrap().cash_frozen, frozen);
    // the old application's frozen cash counts as available for the new one
    assert!(e.subscribe(p, 2, 119, "2026-11-07", TS).is_ok());
    let err = e.subscribe(p, 2, 50, "2026-11-10", TS).unwrap_err();
    assert_eq!(err.0, "认购时间是 2026-11-06 至 2026-11-09");
    assert_eq!(e.ipo_application(2, p).unwrap().qty, 119);
    e.check_invariants().unwrap();
}

#[test]
fn transfers_go_to_every_household() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let world = e.special("world").unwrap();
    let gov = e.special("gov").unwrap();
    e.transfer(world, gov, 1_000_000, "corp_tax", DAY, TS, None, None);
    let p = e.create_account("player", "p", Some("p"), None, None, TS);
    let npcs: Vec<i64> = (0..3).map(|i| e.create_account("npc", &format!("n{i}"), None, None, None, TS)).collect();
    let mut cx = ctx(&e, &cfg, &inp);
    e.pay_transfers(&mut cx, 40_000);
    assert_eq!(cash(&e, p), 10_000); // 40,000 split over 4 households
    for n in npcs {
        assert_eq!(cash(&e, n), 10_000);
    }
    // the v1 rule: players only
    let mut cfg1 = cfg.clone();
    cfg1.economy_v1 = true;
    let mut cx1 = ctx(&e, &cfg1, &inp);
    e.pay_transfers(&mut cx1, 40_000);
    assert_eq!(cash(&e, p), 50_000);
}

#[test]
fn penny_stocks_are_warned_then_delisted() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let committee = e.account_of("committee", 1).unwrap();
    e.add_shares(committee, 1, 1000, 500, "2026-09-10");
    e.st.companies.get_mut(&1).unwrap().prev_close = 99;
    let mut cx = ctx(&e, &cfg, &inp);
    for day in 1..=19 {
        e.penny_rule(&mut cx, &[1]).unwrap();
        assert_eq!(e.st.penny_days[&1], day);
        assert_eq!(e.company(1).unwrap().status, "listed");
    }
    e.penny_rule(&mut cx, &[1]).unwrap();
    assert_eq!(e.company(1).unwrap().status, "delisted");
    assert!(e.holding(committee, 1).is_none());
    e.flush().unwrap();
    e.check_invariants().unwrap();
    // one day at or above 1 币 resets the count
    let mut e = fixture();
    e.st.companies.get_mut(&1).unwrap().prev_close = 50;
    let mut cx = ctx(&e, &cfg, &inp);
    for _ in 0..15 {
        e.penny_rule(&mut cx, &[1]).unwrap();
    }
    e.st.companies.get_mut(&1).unwrap().prev_close = 100;
    e.penny_rule(&mut cx, &[1]).unwrap();
    assert_eq!(e.st.penny_days[&1], 0);
}

/// Two listed shows and the day's inputs with a platform size averaging `days` days.
fn platform_case(days: i64) -> (Engine, Inputs) {
    let (mut e, _sink) = Engine::in_memory(Fees::default(), Rules::default());
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    for id in [1, 2] {
        let mut c = company(id, "listed", 10000, "2026-10-01", "2026-09-10");
        c.episodes = Some(12);
        e.insert_company(c);
        for k in ["company", "committee"] {
            e.create_account(k, k, None, None, Some(id), TS);
        }
        let committee = e.account_of("committee", id).unwrap();
        e.add_shares(committee, id, 10000, 500, "2026-09-10");
    }
    for (k, v) in [("rate", "0.03"), ("a_ref", "100000.0"), ("last_closed", "2026-11-07")] {
        e.set_state(k, v);
    }
    let show = |id: i64, us: f64, cn: f64, jp: f64| {
        serde_json::json!({"id": id, "title_ja": "x", "title_en": null, "studio": null, "first_air": "2026-10-01",
            "episodes": 12, "episodes_known": true, "long_runner": false, "popularity_end": 9000, "aired": 6,
            "has_real_data": true, "att": {"us": us, "cn": cn, "jp": jp}, "expected": {"us": us, "cn": cn, "jp": jp}})
    };
    let inp = Inputs::parse(
        &serde_json::to_string(&serde_json::json!({
            "day": DAY, "cn_titles": {}, "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": days,
            "shows": [show(1, 3000.0, 30.0, 40.0), show(2, 1200.0, 5.0, 25.0)]
        }))
        .unwrap(),
    )
    .unwrap();
    (e, inp)
}

#[test]
fn the_switch_to_absolute_revenue_keeps_the_days_total() {
    // docs/v2-series.md §6: on the first day the platform size averages enough history, the scale is set so that
    // the day's total revenue is what the relative rule gives
    let cfg = cfg();
    let (mut a, inp_short) = platform_case(30);
    let (mut b, inp_long) = platform_case(200);
    let ra = match a.run_day(&cfg, DAY, TS, &inp_short).unwrap() {
        CloseOutcome::Closed(s) => s.injected,
        _ => panic!(),
    };
    let rb = match b.run_day(&cfg, DAY, TS, &inp_long).unwrap() {
        CloseOutcome::Closed(s) => s.injected,
        _ => panic!(),
    };
    assert!(a.platform_rule().is_none());
    let (pl, scale) = b.platform_rule().expect("switched");
    assert_eq!(pl, [90000.0, 400.0, 1500.0]);
    assert!(scale > 0.0);
    assert!((ra - rb).abs() <= 2, "relative {ra} vs absolute {rb} (cents, per-company rounding)");
    // the shares differ: absolute revenue is not renormalised to the listed shows
    let rev = |e: &Engine, id: i64| e.st.company_days.get(&(id, DAY.to_string())).unwrap().revenue;
    assert_ne!(rev(&a, 1), rev(&b, 1));
}

#[test]
fn a_new_season_of_a_listed_series_joins_its_company() {
    // docs/v2-series.md §3.3: no new shares; a company notice; its attention enters the company's revenue
    let cfg = cfg();
    let (mut e, _) = platform_case(200);
    let show = |id: i64, first_air: &str, aired: i64, us: f64, series: i64| {
        serde_json::json!({"id": id, "title_ja": format!("作品{id}"), "title_en": null, "studio": null,
            "first_air": first_air, "episodes": 12, "episodes_known": true, "long_runner": false,
            "popularity_end": 9000, "aired": aired, "has_real_data": aired > 0, "series": series,
            "att": {"us": us, "cn": 0.0, "jp": 0.0}, "expected": {"us": us, "cn": 0.0, "jp": 0.0}})
    };
    let inp = |day: &str, s3_us: f64| {
        Inputs::parse(
            &serde_json::to_string(&serde_json::json!({
                "day": day, "cn_titles": {}, "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": 200,
                "shows": [show(1, "2026-10-01", 6, 3000.0, 1), show(2, "2026-10-01", 6, 1200.0, 2),
                          show(3, "2026-11-20", 0, s3_us, 1), show(4, "2026-10-15", 3, 800.0, 1),
                          show(5, "2026-07-01", 12, 100.0, 1)]
            }))
            .unwrap(),
        )
        .unwrap()
    };
    assert!(matches!(e.run_day(&cfg, DAY, TS, &inp(DAY, 0.0)).unwrap(), CloseOutcome::Closed(_)));
    assert_eq!(e.st.company_works.get(&3), Some(&1));
    // (show 2 premiered 2026-10-01, within 60 days, but is of another series: untouched)
    assert!(e.st.company_works.get(&2).is_none());
    assert!(e.company(3).is_none(), "no new listing");
    // an earlier work of the series on air (within 60 days of its premiere) joins too; one long past does not
    assert_eq!(e.works_of(1), vec![1, 3, 4]);
    assert!(e.company(5).is_none() && e.st.company_works.get(&5).is_none());
    // the next day the new season's pre-air attention earns for company 1
    let day2 = "2026-11-09";
    assert!(matches!(e.run_day(&cfg, day2, "2026-11-09T06:30:00+08:00", &inp(day2, 500.0)).unwrap(), CloseOutcome::Closed(_)));
    let (pl, sc) = e.platform_rule().unwrap();
    let p = Params::from_config(&cfg).unwrap();
    let one = |us: f64| crate::vfilter::absolute_parts(&[us, 0.0, 0.0], &pl, sc, &p.market_weights, &p)[0];
    let want = pround((one(3000.0) + one(500.0) + one(800.0)) * 100.0);
    let got = e.st.company_days.get(&(1, day2.to_string())).unwrap().revenue;
    assert!((got - want).abs() <= 1, "{got} vs {want}");
}
