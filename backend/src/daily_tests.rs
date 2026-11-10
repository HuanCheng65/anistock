//! Port of tests/test_daily.py, plus the close's rollback.

use super::*;
use crate::{Fees, Rules};

const TS: &str = "2026-11-08T06:30:00+08:00";
const DAY: &str = "2026-11-08"; // a Sunday

fn cfg() -> Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let root = if root.join("server/game.toml").exists() { root } else { crate::config::find_root() };
    let mut c = Config::load_from(&root, |_| None).unwrap();
    // no inputs on disk: a close reads only the inputs it is given (the switch's week, `inputs_week`)
    c.inputs_dir = std::env::temp_dir().join("anistock-tests-no-inputs");
    c
}

/// The config with the absolute revenue rule on (off by default until the platform history allows it).
fn cfg_absolute() -> Config {
    let mut c = cfg();
    let rv = c.params.get_mut("revenue").and_then(|v| v.as_table_mut()).unwrap();
    rv.insert("absolute_revenue".into(), toml::Value::Integer(1));
    // every market's total kept on the switch day (the cn coverage factor is tested on its own)
    rv.insert("absolute_cn_coverage".into(), toml::Value::Float(1.0));
    c
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
        source: None,
        genres: String::new(),
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
fn an_index_fund_takes_its_dividends_untaxed() {
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let world = e.special("world").unwrap();
    let (company_acct, committee) = (e.account_of("company", 1).unwrap(), e.account_of("committee", 1).unwrap());
    let fund = e.create_account("fund", "fund:all", None, None, None, TS);
    e.add_shares(committee, 1, 750, 500, "2026-06-10");
    e.add_shares(fund, 1, 250, 500, "2026-11-08"); // bought today: a player would pay 20%
    e.transfer(world, company_acct, 10_000, "revenue", DAY, TS, Some(1), None);
    e.st.companies.get_mut(&1).unwrap().profit_week = 10_000;
    let mut cx = ctx(&e, &cfg, &inp);
    e.pay_dividends(&mut cx).unwrap();
    assert_eq!(cash(&e, fund), 9 * 250);
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
    assert_eq!(err.0, "可用资金不足：认购 1,000 股需要冻结 5,011.25 星辉");
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
    // a player who is not eligible (crate::kyc) gets nothing; the households that are share it
    let q = e.create_account("player", "q", Some("q"), None, None, TS);
    let mut cx = ctx(&e, &cfg, &inp);
    e.pay_transfers_to(&mut cx, 50_000, Some(&std::collections::HashSet::from([p])));
    assert_eq!(cash(&e, p), 22_500); // 50,000 over p and the 3 NPCs; q left out
    assert_eq!(cash(&e, q), 0);
    // the v1 rule: players only
    let mut cfg1 = cfg.clone();
    cfg1.economy_v1 = true;
    let mut cx1 = ctx(&e, &cfg1, &inp);
    e.pay_transfers(&mut cx1, 40_000);
    assert_eq!(cash(&e, p), 42_500); // 40,000 over p and q
    assert_eq!(cash(&e, q), 20_000);
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

#[test]
fn a_delisting_buys_the_works_rights_out_at_their_appraisal() {
    // docs/design/series.md §13.5: with asset_injection, the holders get the cash plus the rights' appraisal (the last
    // 8 weeks' revenue capitalised), the world account paying for the rights
    let mut cfg = cfg();
    cfg.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(1));
    let inp = empty_inputs();
    let payout = |cfg: &Config| {
        let mut e = fixture();
        e.set_state("rate", "0.03");
        let committee = e.account_of("committee", 1).unwrap();
        let acct = e.account_of("company", 1).unwrap();
        let world = e.special("world").unwrap();
        e.add_shares(committee, 1, 900, 500, "2026-09-10");
        let player = e.create_account("player", "p", None, None, None, TS);
        e.add_shares(player, 1, 100, 500, "2026-09-10");
        e.do_transfer(world, acct, 50_000, "revenue", "2026-11-01", TS, Some(1), None);
        for k in 0..56 {
            let d = iso(date(DAY).unwrap() - Duration::days(k));
            e.st.company_days.insert((1, d.clone()), CompanyDay { company_id: 1, day: d, revenue: 700, rev_us: 400, rev_cn: 200, rev_jp: 100, aired: 12, dividend_ps: 0 });
        }
        e.st.companies.get_mut(&1).unwrap().prev_close = 99;
        let before = cash(&e, player);
        let book = cash(&e, acct);
        let mut cx = ctx(&e, cfg, &inp);
        for _ in 0..20 {
            e.penny_rule(&mut cx, &[1]).unwrap();
        }
        assert_eq!(e.company(1).unwrap().status, "delisted");
        e.flush().unwrap();
        e.check_invariants().unwrap();
        (cash(&e, player) - before, book)
    };
    let mut off = cfg.clone();
    off.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(0));
    let (plain, book) = payout(&off);
    let (with, _) = payout(&cfg);
    assert_eq!(plain, book / 1000 * 100);
    let p = Params::from_config(&cfg).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    // 7 days a week of 4/2/1 星辉 per day
    let ip = pround(ip_appraisal(&p, &fp, &[28.0, 14.0, 7.0], 0.03) * 100.0);
    assert!(ip > 0);
    assert_eq!(with, (book + ip) / 1000 * 100);
}

fn show_with_sources(cn: bool, jp: bool) -> crate::inputs::ShowInputs {
    serde_json::from_value(serde_json::json!({
        "id": 1, "title_ja": "テスト", "title_en": null, "studio": null, "first_air": "2026-07-01", "episodes": 12,
        "episodes_known": true, "long_runner": false, "popularity_end": 0, "aired": 5, "has_real_data": true,
        "att": {"us": 1.0, "cn": 1.0, "jp": 1.0}, "expected": {"us": 1.0, "cn": 1.0, "jp": 1.0},
        "sources": {"us": true, "cn": cn, "jp": jp}
    }))
    .unwrap()
}

#[test]
fn a_newly_mapped_market_posts_one_notice() {
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
    e.insert_company(company(1, "listed", 1000, "2026-07-01", "2026-06-10"));
    let c = e.company(1).unwrap().clone();
    let notices = |sink: &crate::store::persist::MemorySink| -> Vec<(String, String, String, Option<i64>)> {
        sink.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|ch| match ch {
                Change::NewsInsert(n) => Some((n.kind.clone(), n.title.clone(), n.body.clone().unwrap_or_default(), n.company_id)),
                _ => None,
            })
            .collect()
    };
    // first sighting: recorded, no notice (a running game does not announce every show at once)
    e.note_new_sources(TS, DAY, &c, &show_with_sources(true, false));
    e.commit();
    assert!(notices(&sink).is_empty());
    assert_eq!(e.state("sources:1"), Some("110"));
    // unchanged: nothing
    e.note_new_sources(TS, DAY, &c, &show_with_sources(true, false));
    e.commit();
    assert!(notices(&sink).is_empty());
    // the Japanese source is mapped: one notice
    e.note_new_sources(TS, "2026-11-09", &c, &show_with_sources(true, true));
    e.commit();
    assert_eq!(
        notices(&sink),
        vec![("system".to_string(), "《テスト》接入日本市场数据".to_string(), "日本市场的热度数据从今天起计入营收和估值。".to_string(), Some(1))]
    );
    // the next closes: no repeat
    e.note_new_sources(TS, "2026-11-10", &c, &show_with_sources(true, true));
    e.commit();
    assert_eq!(notices(&sink).len(), 1);
    // inputs without the flags (before v0.3.2) change nothing
    let mut old = show_with_sources(true, true);
    old.sources = None;
    e.note_new_sources(TS, "2026-11-11", &c, &old);
    e.commit();
    assert_eq!(notices(&sink).len(), 1);
    assert_eq!(e.state("sources:1"), Some("111"));
}

/// Two listed shows and the day's inputs with a platform size averaging `days` days.
fn platform_case(days: i64) -> (Engine, Inputs) {
    let (e, inp, _) = platform_case_sink(days);
    (e, inp)
}

fn platform_case_sink(days: i64) -> (Engine, Inputs, crate::store::persist::MemorySink) {
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
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
    (e, inp, sink)
}

#[test]
fn the_switch_to_absolute_revenue_keeps_the_days_total() {
    // docs/design/series.md §6: on the first day the platform size averages enough history, each market's scale is set
    // so that the day's revenue in that market is what the relative rule gives
    let cfg = cfg_absolute();
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
    assert!(scale.iter().all(|s| *s > 0.0));
    assert!((ra - rb).abs() <= 2, "relative {ra} vs absolute {rb} (cents, per-company rounding)");
    // one scale per market: on the switch day each company earns what the relative rule gives (the rules part only
    // as attention and the platform size move afterwards)
    let rev = |e: &Engine, id: i64| e.st.company_days.get(&(id, DAY.to_string())).unwrap().revenue;
    assert!((rev(&a, 1) - rev(&b, 1)).abs() <= 1);
}

#[test]
fn a_valuation_rebuild_sets_the_rule_the_next_close_would() {
    // a rebuild on inputs whose platform size qualifies switches as that close would (the same scales); on shorter
    // history it records the switch day, as the close does
    let cfg = cfg_absolute();
    let (mut a, inp_long) = platform_case(200);
    let (mut b, _) = platform_case(200);
    let (mut c, inp_short) = platform_case(30);
    assert!(a.platform_switch_at_rebuild(&cfg, std::slice::from_ref(&inp_long)).unwrap());
    assert!(matches!(b.run_day(&cfg, DAY, TS, &inp_long).unwrap(), CloseOutcome::Closed(_)));
    assert_eq!(a.platform_rule(), b.platform_rule());
    assert!(!c.platform_switch_at_rebuild(&cfg, std::slice::from_ref(&inp_short)).unwrap());
    assert!(c.platform_rule().is_none());
    let pending = c.state("platform_pending").expect("pending");
    let today = crate::vclose::day_no(DAY).unwrap();
    assert!(pending.ends_with(&format!(",{}", today + 182 - 30)), "{pending}");
}

#[test]
fn a_new_season_of_a_listed_series_joins_its_company() {
    // docs/design/series.md §3.3: no new shares; a company notice; its attention enters the company's revenue
    let cfg = cfg_absolute();
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
    // docs/design/series.md §13.7: the current work is the latest-premiered one on air (4 over 1; 3 not yet aired)
    let cur = e.st.company_current.get(&1).unwrap();
    assert_eq!((cur.work_id, cur.first_air.as_str(), cur.aired), (4, "2026-10-15", 3));
    assert_eq!(e.st.company_current.get(&2).unwrap().work_id, 2);
    // the next day the new season's pre-air attention earns for company 1
    let day2 = "2026-11-09";
    assert!(matches!(e.run_day(&cfg, day2, "2026-11-09T06:30:00+08:00", &inp(day2, 500.0)).unwrap(), CloseOutcome::Closed(_)));
    let (pl, sc) = e.platform_rule().unwrap();
    let p = Params::from_config(&cfg).unwrap();
    let one = |us: f64| crate::vfilter::absolute_parts(&[us, 0.0, 0.0], &pl, &sc, &p.market_weights, &p)[0];
    let want = pround((one(3000.0) + one(500.0) + one(800.0)) * 100.0);
    let got = e.st.company_days.get(&(1, day2.to_string())).unwrap().revenue;
    assert!((got - want).abs() <= 1, "{got} vs {want}");
}

#[test]
fn an_earlier_work_joins_by_an_asset_injection_at_its_appraisal() {
    // docs/design/series.md §13: a work of the series premiered long before joins once its appraisal data is there;
    // the committee is paid in new shares, its appraisal at the 20-day average close
    let mut cfg = cfg_absolute();
    cfg.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(1));
    let (mut e, _) = platform_case(200);
    e.st.corp.closes.insert(1, (0..30).map(|k| if k < 10 { 9999 } else { 600 }).collect());
    let show = |id: i64, first_air: &str, aired: i64, us: f64, appraisal: Option<f64>| {
        let mut v = serde_json::json!({"id": id, "title_ja": format!("作品{id}"), "title_en": null, "studio": null,
            "first_air": first_air, "episodes": 12, "episodes_known": true, "long_runner": false,
            "popularity_end": 9000, "aired": aired, "has_real_data": true, "series": 1,
            "att": {"us": us, "cn": 0.0, "jp": 0.0}, "expected": {"us": us, "cn": 0.0, "jp": 0.0}});
        if let Some(a) = appraisal {
            v["appraisal_att"] = serde_json::json!({"us": a, "cn": 2.0, "jp": 1.0, "basis": "window", "from": "2026-04-13", "to": "2026-06-07"});
        }
        v
    };
    let inp = |day: &str, appraisal: Option<f64>| {
        Inputs::parse(
            &serde_json::to_string(&serde_json::json!({
                "day": day, "cn_titles": {"5": "前作"}, "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": 200,
                "shows": [show(1, "2026-10-01", 6, 3000.0, None), show(2, "2026-10-01", 6, 1200.0, None),
                          show(5, "2025-01-05", 12, 100.0, appraisal)]
            }))
            .unwrap(),
        )
        .unwrap()
    };
    // no appraisal data yet: it waits
    assert!(matches!(e.run_day(&cfg, DAY, TS, &inp(DAY, None)).unwrap(), CloseOutcome::Closed(_)));
    assert!(e.st.company_works.get(&5).is_none());
    let shares0 = e.company(1).unwrap().shares;
    let committee = e.account_of("committee", 1).unwrap();
    let held0 = e.holding(committee, 1).unwrap().qty;
    let day2 = "2026-11-09";
    assert!(matches!(e.run_day(&cfg, day2, "2026-11-09T06:30:00+08:00", &inp(day2, Some(150.0))).unwrap(), CloseOutcome::Closed(_)));
    assert_eq!(e.st.company_works.get(&5), Some(&1));
    let p = Params::from_config(&cfg).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let (pl, sc) = e.platform_rule().unwrap();
    let day = crate::vfilter::absolute_parts(&[150.0, 2.0, 1.0], &pl, &sc, &p.market_weights, &p);
    let week: crate::valuation::M3 = std::array::from_fn(|m| day[m] * 7.0);
    let value = pround(ip_appraisal(&p, &fp, &week, 0.03) * 100.0);
    // the last 20 closes: the two days' closes appended after the 30 seeded ones (all 600 but the oldest 10, gone)
    let avg = {
        let v: Vec<i64> = e.st.corp.closes[&1].iter().rev().take(20).copied().collect();
        (v.iter().sum::<i64>() as f64 / v.len() as f64).round() as i64
    };
    let issued = e.company(1).unwrap().shares - shares0;
    assert!(issued > 0);
    assert!((issued - value / avg).abs() <= 1, "{issued} vs {value}/{avg}");
    assert_eq!(e.holding(committee, 1).unwrap().qty - held0, issued);
    e.flush().unwrap();
    e.check_invariants().unwrap();
    // a premise of the number: the yearly decline enters the capitalisation per market
    let flat = ip_appraisal(&p, &fp, &[1.0, 0.0, 0.0], 0.03);
    assert!((flat - 52.0 * (1.0 - p.operating_cost_ratio) * (1.0 - p.corp_tax) / (0.03 + p.equity_premium + 1.0 - fp.tail[0].as_ref().map_or(1.0, |t| t.yearly))).abs() < 1e-9);
}

#[test]
fn a_missing_episode_marker_is_not_a_finish_when_the_schedule_has_more() {
    // 185874: AniList stopped recording episodes at 8 of 10; Bangumi lists episodes 9 and 10
    let aired: Vec<i64> = (0..8).map(|k| 1000 + 7 * k).collect();
    let today = 1049 + 14;
    assert_eq!(Engine::planned_episodes(Some(10), &aired, today, &[]), Some(8));
    let schedule: Vec<i64> = (0..10).map(|k| 1000 + 7 * k).collect();
    assert_eq!(Engine::planned_episodes(Some(10), &aired, today, &schedule), Some(10));
    // a season released at once: the schedule has nothing after the last episode seen
    assert_eq!(Engine::planned_episodes(Some(10), &aired, today, &aired), Some(8));
}

#[test]
fn an_offer_priced_over_its_value_is_cut_before_allotment() {
    // a show listed late from a pre-air estimate that its data have since brought down: the price is cut (value less
    // the discount for the uncertainty left, a notice posted; at startup and at every close), the allotment charges it
    let cfg = cfg();
    let inp = empty_inputs();
    let mut e = fixture();
    let committee = subscribing(&mut e);
    let mut js = crate::vclose::JState::empty(DAY);
    js.active = [true, false, false];
    js.cov[0][0] = 0.04;
    e.st.jstate.insert(2, js);
    e.st.companies.get_mut(&2).unwrap().fair_value = 200;
    let a = apply(&mut e, "a", 600);
    let cash0 = e.account(a).unwrap().cash;
    assert_eq!(e.reprice_offers(&cfg, DAY, TS).unwrap(), 1);
    // (done: a second pass changes nothing)
    assert_eq!(e.reprice_offers(&cfg, DAY, TS).unwrap(), 0);
    let mut cx = ctx(&e, &cfg, &inp);
    e.allot(&mut cx, 2).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let want = (200.0 * (-fp.ipo_risk * 0.2f64).exp()).round() as i64;
    let c = e.company(2).unwrap();
    assert_eq!(c.ipo_price, want);
    assert_eq!(c.prev_close, want);
    assert_eq!(e.available_shares(a, 2), 600);
    assert_eq!(cash0 - e.account(a).unwrap().cash, 600 * want + Fees::default().commission_on(600 * want));
    assert_eq!(frozen_players(&e), 0);
    assert_eq!(e.available_shares(committee, 2), 7500 + 1900);
    e.check_invariants().unwrap();

    // an offer within 5% of the supported price is left alone
    let mut e = fixture();
    subscribing(&mut e);
    let mut js = crate::vclose::JState::empty(DAY);
    js.active = [true, false, false];
    e.st.jstate.insert(2, js);
    e.st.companies.get_mut(&2).unwrap().fair_value = 510;
    apply(&mut e, "a", 600);
    assert_eq!(e.reprice_offers(&cfg, DAY, TS).unwrap(), 0);
    assert_eq!(e.company(2).unwrap().ipo_price, 500);
}

#[test]
fn a_sequels_prior_takes_the_sequel_bias() {
    // docs/design/series.md §5.3: a sequel's pre-air list entries include the earlier seasons' audience
    let cfg = cfg();
    let mut fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    fp.prior_bias = [0.5, -0.3, -0.5];
    fp.prior_bias_sequel = [-0.2, -1.2, -1.2];
    let mut s = show_with_sources(true, true);
    s.first_air = "2026-07-01".into();
    s.prior = Some(serde_json::from_value(serde_json::json!({"us": 100.0, "cn": 0.0, "jp": 0.0})).unwrap());
    let today = crate::vclose::day_no("2026-07-01").unwrap();
    let first = Engine::prior_mean(&fp, &s, today).unwrap();
    s.prequel = true;
    let seq = Engine::prior_mean(&fp, &s, today).unwrap();
    for m in 0..3 {
        assert!((first[m] - seq[m] - (fp.prior_bias[m] - fp.prior_bias_sequel[m])).abs() < 1e-12, "market {m}");
    }
}

#[test]
fn a_rebuild_takes_a_sequel_it_meets_as_financed_and_a_close_announces_one() {
    // docs/design/series.md §5.2: a sequel announced before the state existed (met in a replay of the history) posts
    // no notice and its production counts as paid; one a live close meets is announced
    let cfg = cfg();
    let p = Params::from_config(&cfg).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let mut s = show_with_sources(true, true);
    s.prior = Some(serde_json::from_value(serde_json::json!({"us": 100.0, "cn": 0.0, "jp": 0.0})).unwrap());
    s.relations_known = true;
    s.sequel = Some(serde_json::from_value(serde_json::json!(
        {"id": 900, "announced": "2026-09-20", "premieres": ["2027-04-15"], "exact": false})).unwrap());
    let sequel_notices = |sink: &crate::store::persist::MemorySink| {
        sink.0.lock().unwrap().iter().filter(|ch| matches!(ch, Change::NewsInsert(n) if n.kind == "sequel")).count()
    };
    for replay in [true, false] {
        let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
        e.insert_company(company(1, "listed", 1000, "2026-07-01", "2026-06-10"));
        e.valuation_replay = replay;
        e.valuation_step(&fp, &p, &s, DAY, TS);
        e.commit();
        let j = e.valuation_state(1).unwrap();
        assert_eq!(j.sequel.as_ref().map(|q| q.0), Some(900));
        assert_eq!(j.sequel_paid, if replay { Some(900) } else { None }, "replay {replay}");
        assert_eq!(sequel_notices(&sink), if replay { 0 } else { 1 }, "replay {replay}");
    }
}

#[test]
fn an_official_no_sequel_is_announced_once_and_ends_the_option() {
    // docs/design/series.md §5.4
    let cfg = cfg();
    let p = Params::from_config(&cfg).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let mut s = show_with_sources(true, true);
    s.relations_known = true;
    s.sequel_cancelled = Some("https://example.org/c".into());
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
    e.insert_company(company(1, "listed", 1000, "2026-07-01", "2026-06-10"));
    let notices = |sink: &crate::store::persist::MemorySink| -> Vec<String> {
        sink.0.lock().unwrap().iter().filter_map(|ch| match ch {
            Change::NewsInsert(n) if n.kind == "sequel" => Some(format!("{}|{}", n.title, n.body.clone().unwrap_or_default())),
            _ => None,
        }).collect()
    };
    e.valuation_step(&fp, &p, &s, DAY, TS);
    e.valuation_step(&fp, &p, &s, "2026-11-09", TS);
    e.commit();
    assert!(e.valuation_state(1).unwrap().sequel_cancelled);
    assert_eq!(notices(&sink), vec!["《テスト》官方表示不再制作续作|来源：https://example.org/c".to_string()]);
}

#[test]
fn a_new_listing_takes_the_earlier_works_of_its_series_at_their_appraisal() {
    // docs/design/series.md §13.8: listed as a series; no shares issued for them after the listing
    let mut cfg = cfg_absolute();
    cfg.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(1));
    let p = Params::from_config(&cfg).unwrap();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let price = |with_earlier: bool| {
        let (mut e, _) = platform_case(200);
        let show = |id: i64, first_air: &str, aired: i64, us: f64, series: i64, appraisal: bool| {
            let mut v = serde_json::json!({"id": id, "title_ja": format!("作品{id}"), "title_en": null, "studio": null,
                "first_air": first_air, "episodes": 12, "episodes_known": true, "long_runner": false,
                "popularity_end": 9000, "aired": aired, "has_real_data": aired > 0, "series": series,
                "prior": {"us": 2000.0, "cn": 0.0, "jp": 0.0},
                "att": {"us": us, "cn": 0.0, "jp": 0.0}, "expected": {"us": us, "cn": 0.0, "jp": 0.0}});
            if appraisal {
                v["appraisal_att"] = serde_json::json!({"us": 150.0, "cn": 2.0, "jp": 1.0, "basis": "window", "from": "2026-04-13", "to": "2026-06-07"});
            }
            v
        };
        let mut shows = vec![show(1, "2026-10-01", 6, 3000.0, 1, false), show(2, "2026-10-01", 6, 1200.0, 2, false),
                             show(7, "2026-12-01", 0, 300.0, 7, false)];
        if with_earlier {
            shows.push(show(6, "2024-04-05", 12, 80.0, 7, true));
        }
        let inp = Inputs::parse(&serde_json::to_string(&serde_json::json!({
            "day": DAY, "cn_titles": {"6": "前作", "7": "新作"}, "series_titles": {"7": "系列名"},
            "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": 200, "shows": shows})).unwrap()).unwrap();
        assert!(matches!(e.run_day(&cfg, DAY, TS, &inp).unwrap(), CloseOutcome::Closed(_)));
        let c = e.company(7).expect("offer").clone();
        e.flush().unwrap();
        e.check_invariants().unwrap();
        (c, e)
    };
    let (alone, _) = price(false);
    let (series, e) = price(true);
    assert_eq!(e.st.company_works.get(&6), Some(&7));
    assert_eq!(series.title, "系列名");
    assert_eq!(alone.title, "新作");
    assert_eq!(series.shares, alone.shares, "no shares for the earlier work");
    let (pl, sc) = e.platform_rule().unwrap();
    let day = crate::vfilter::absolute_parts(&[150.0, 2.0, 1.0], &pl, &sc, &p.market_weights, &p);
    let week: crate::valuation::M3 = std::array::from_fn(|m| day[m] * 7.0);
    let per_share = ip_appraisal(&p, &fp, &week, 0.03) * 100.0 / series.shares as f64;
    // the offer's discount applies to the whole line
    let lift = (series.ipo_price - alone.ipo_price) as f64;
    assert!(lift > 0.5 * per_share && lift <= per_share + 1.0, "{lift} vs {per_share}");
}

#[test]
fn the_switch_scale_comes_from_the_week_not_its_last_day() {
    // docs/design/series.md §12 v0.8.5: us attention is far higher at the weekend; one day's scale would depend on
    // the weekday of the switch
    let cfg = cfg_absolute();
    let scale = |week: &[Inputs]| {
        let (mut e, _) = platform_case(200);
        assert!(e.platform_switch_at_rebuild(&cfg, week).unwrap());
        e.platform_rule().unwrap().1
    };
    let (_, sunday) = platform_case(200);
    let mut monday = sunday.clone();
    monday.day = "2026-11-09".into();
    for s in monday.shows.iter_mut() {
        s.att.us *= 0.6;
    }
    let one = scale(std::slice::from_ref(&monday));
    let both = scale(&[sunday.clone(), monday.clone()]);
    let other = scale(std::slice::from_ref(&sunday));
    for m in 0..3 {
        let (lo, hi) = (one[m].min(other[m]), one[m].max(other[m]));
        assert!(lo < both[m] && both[m] < hi, "market {m}: {} {} {}", one[m], both[m], other[m]);
    }
    // more than a week back is left out
    let mut long = vec![sunday.clone(); 8];
    long.push(monday);
    let week = scale(&long);
    let seven = scale(&long[long.len() - 7..]);
    assert_eq!(week, seven);
}

#[test]
fn after_a_reprice_the_volatility_control_ignores_the_old_prices() {
    // 2026-11-10: a trade at the old price earlier that day made the first trade at the new price start a cooling-off
    // period around the old one
    let (mut e, _) = Engine::in_memory(Fees::default(), Rules::default());
    e.insert_company(company(1, "listed", 1000, "2026-07-01", "2026-06-10"));
    let t = |ts: &str| crate::engine::TradeRef { day: "2026-11-10".into(), ts: ts.into(), price: 7000, id: 1, qty: 1 };
    e.st.recent_trades.insert(1, vec![t("2026-11-10T15:00:00+08:00")]);
    let at = "2026-11-10T18:40:00+08:00";
    assert_eq!(e.vcm_lookback(1, "2026-11-10", at), Some(7000));
    e.reprice(&std::collections::HashMap::from([(1, 16000)]), "2026-11-10T18:30:00+08:00");
    assert_eq!(e.vcm_lookback(1, "2026-11-10", at), None);
    // trades after it count again
    e.st.recent_trades.get_mut(&1).unwrap().push(crate::engine::TradeRef { price: 16100, ..t("2026-11-10T18:31:00+08:00") });
    assert_eq!(e.vcm_lookback(1, "2026-11-10", at), Some(16100));
}

#[test]
fn a_sequel_notice_says_what_it_is() {
    let q = |kind: &str, before: bool| crate::inputs::SequelInputs {
        id: 5,
        announced: "2026-11-06".into(),
        premieres: vec!["2027-04-15".into()],
        exact: false,
        source: None,
        title: Some("薬屋のひとりごと 第3期 第2クール".into()),
        kind: Some(kind.into()),
        part: (kind == "part").then_some(2),
        when: None,
        known_before: before,
    };
    let d = crate::vclose::day_no("2027-04-15").unwrap();
    let (t, b) = Engine::sequel_notice("药屋少女的呢喃 第三季", &q("part", true), &[d]);
    assert_eq!(t, "《药屋少女的呢喃 第三季》第 2 部分已公布");
    assert_eq!(b, "《薬屋のひとりごと 第3期 第2クール》，播出时间未定。此前已经公布，估值从今天起按已公布计算。");
    let (t, _) = Engine::sequel_notice("猫与龙", &q("season", false), &[d]);
    assert_eq!(t, "《猫与龙》续作公布");
    let mut unnumbered = q("part", false);
    unnumbered.part = None;
    assert_eq!(Engine::sequel_notice("罗梅莉亚战记", &unnumbered, &[d]).0, "《罗梅莉亚战记》下一部分公布");
    let mut month = q("part", false);
    month.when = Some("2027 年 4 月".into());
    assert_eq!(Engine::sequel_notice("药屋", &month, &[d]).1, "《薬屋のひとりごと 第3期 第2クール》，预计 2027 年 4 月播出。");
    let mut exact = q("film", false);
    exact.exact = true;
    let (t, b) = Engine::sequel_notice("斗罗大陆", &exact, &[d]);
    assert_eq!((t.as_str(), b.as_str()), ("《斗罗大陆》剧场版公布", "《薬屋のひとりごと 第3期 第2クール》，定于 2027 年 4 月 15 日播出。"));
}


#[test]
fn a_companys_earlier_works_join_together_in_one_notice_with_its_new_name() {
    // one issue and one notice a company a day; it waits for every earlier work's appraisal data (at most
    // INJECT_WAIT_DAYS), and the day's rename is said in the same notice
    let mut cfg = cfg_absolute();
    cfg.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(1));
    let (mut e, _, sink) = platform_case_sink(200);
    e.st.corp.closes.insert(1, vec![600; 20].into());
    let show = |id: i64, first_air: &str, fmt: &str, appraisal: bool| {
        let mut v = serde_json::json!({"id": id, "title_ja": format!("作品{id}"), "title_en": null, "studio": null,
            "first_air": first_air, "episodes": 12, "episodes_known": true, "long_runner": false, "format": fmt,
            "popularity_end": 9000, "aired": 12, "has_real_data": true, "series": 1,
            "att": {"us": 100.0, "cn": 0.0, "jp": 0.0}, "expected": {"us": 100.0, "cn": 0.0, "jp": 0.0}});
        if appraisal {
            v["appraisal_att"] = serde_json::json!({"us": 150.0, "cn": 2.0, "jp": 1.0, "basis": "window", "from": "2026-04-13", "to": "2026-06-07"});
        }
        v
    };
    let inp = |day: &str, b_ready: bool| {
        Inputs::parse(&serde_json::to_string(&serde_json::json!({
            "day": day, "cn_titles": {"1": "テスト", "5": "第一季", "6": "第二季", "7": "剧场版"}, "series_titles": {"1": "系列名"},
            "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": 200,
            "shows": [show(1, "2026-10-01", "TV", false), show(2, "2026-10-01", "TV", false),
                      show(5, "2024-01-05", "TV", true), show(6, "2025-01-05", "TV", b_ready), show(7, "2025-06-01", "MOVIE", true)]
        })).unwrap()).unwrap()
    };
    let notices = |sink: &crate::store::persist::MemorySink| -> Vec<(String, String, String)> {
        sink.0.lock().unwrap().iter().filter_map(|ch| match ch {
            Change::NewsInsert(n) if n.kind == "sequel" || n.kind == "system" => Some((n.kind.clone(), n.title.clone(), n.body.clone().unwrap_or_default())),
            _ => None,
        }).collect()
    };
    let close = |e: &mut Engine, day: &str, ready: bool| {
        assert!(matches!(e.run_day(&cfg, day, &format!("{day}T06:30:00+08:00"), &inp(day, ready)).unwrap(), CloseOutcome::Closed(_)));
        e.commit();
    };
    let shares0 = e.company(1).unwrap().shares;
    // one work not ready: nobody joins
    close(&mut e, "2026-11-08", false);
    close(&mut e, "2026-11-09", false);
    assert!(e.st.company_works.get(&5).is_none() && notices(&sink).is_empty());
    // all ready: the three join at once, one issue, one notice with the rename
    close(&mut e, "2026-11-10", true);
    assert_eq!([5, 6, 7].map(|w| e.st.company_works.get(&w).copied()), [Some(1); 3]);
    let n = notices(&sink);
    assert_eq!(n.len(), 1, "{n:?}");
    assert_eq!(n[0].0, "sequel");
    assert_eq!(n[0].1, "《テスト》并入前作 3 部，更名为《系列名》");
    assert!(n[0].2.contains("《第一季》《第二季》，以及剧场版、OVA 和特别篇 1 部"), "{}", n[0].2);
    assert!(n[0].2.contains("改用系列名"), "{}", n[0].2);
    assert_eq!(e.company(1).unwrap().title, "系列名");
    let committee = e.account_of("committee", 1).unwrap();
    let issued = e.company(1).unwrap().shares - shares0;
    assert!(issued > 0 && e.holding(committee, 1).unwrap().qty >= issued);
    e.flush().unwrap();
    e.check_invariants().unwrap();
}

#[test]
fn a_company_stops_waiting_for_a_late_work_after_a_week() {
    let mut cfg = cfg_absolute();
    cfg.params.get_mut("company").and_then(|v| v.as_table_mut()).unwrap().insert("asset_injection".into(), toml::Value::Integer(1));
    let (mut e, _) = platform_case(200);
    let show = |id: i64, appraisal: bool| {
        let mut v = serde_json::json!({"id": id, "title_ja": format!("作品{id}"), "title_en": null, "studio": null,
            "first_air": "2025-01-05", "episodes": 12, "episodes_known": true, "long_runner": false, "format": "TV",
            "popularity_end": 9000, "aired": 12, "has_real_data": true, "series": 1,
            "att": {"us": 100.0, "cn": 0.0, "jp": 0.0}, "expected": {"us": 100.0, "cn": 0.0, "jp": 0.0}});
        if appraisal {
            v["appraisal_att"] = serde_json::json!({"us": 150.0, "cn": 2.0, "jp": 1.0, "basis": "window", "from": "2026-04-13", "to": "2026-06-07"});
        }
        v
    };
    let mut first = serde_json::json!({"id": 1, "title_ja": "x", "title_en": null, "studio": null, "first_air": "2026-10-01",
        "episodes": 12, "episodes_known": true, "long_runner": false, "popularity_end": 9000, "aired": 6, "has_real_data": true,
        "series": 1, "att": {"us": 3000.0, "cn": 0.0, "jp": 0.0}, "expected": {"us": 3000.0, "cn": 0.0, "jp": 0.0}});
    first["format"] = "TV".into();
    let mut d = chrono::NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
    for k in 0..=crate::daily::INJECT_WAIT_DAYS {
        let day = d.format("%Y-%m-%d").to_string();
        let inp = Inputs::parse(&serde_json::to_string(&serde_json::json!({
            "day": day, "cn_titles": {}, "platform": {"us": 90000.0, "cn": 400.0, "jp": 1500.0}, "platform_days": 200,
            "shows": [first.clone(), show(5, true), show(6, false)]})).unwrap()).unwrap();
        assert!(matches!(e.run_day(&cfg, &day, &format!("{day}T06:30:00+08:00"), &inp).unwrap(), CloseOutcome::Closed(_)));
        let joined = e.st.company_works.get(&5).is_some();
        assert_eq!(joined, k == crate::daily::INJECT_WAIT_DAYS, "day {k}");
        d = d.succ_opt().unwrap();
    }
    assert!(e.st.company_works.get(&6).is_none());
}
