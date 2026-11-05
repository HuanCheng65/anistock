//! The night session: one trading day runs 09:00 to 04:00 the next morning and the trading day rolls over
//! at 04:00 (`game.day_rollover`). Orders, trades, candles, order expiry, the Sunday dividend's record date
//! and the dividend tax's holding days all follow the trading day, not the calendar date.

use anistock::config::prev_day;
use anistock::daily::CloseOutcome;
use anistock::inputs::Inputs;
use anistock::market::{at_local, day_of, Market, MarketConfig};
use anistock::model::Company;
use anistock::store::Change;
use anistock::{Clock, Config, Engine, Fees, NewOrder, Phase, Rules};

const SUNDAY: &str = "2026-11-08";
const CLOSE_TS: &str = "2026-11-09T06:30:00+08:00";

fn cfg() -> Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    Config::load_from(&root, |_| None).unwrap()
}

fn clock_at(cfg: &Config, iso: &str) -> Clock {
    Clock::fixed(cfg.tz, iso).with_rollover(&cfg.game.day_rollover)
}

#[test]
fn the_trading_day_rolls_over_at_four() {
    let cfg = cfg();
    assert_eq!((cfg.game.trading_open.as_str(), cfg.game.trading_close.as_str()), ("09:00", "04:00"));
    assert_eq!(cfg.game.day_rollover, "04:00");
    let cases = [
        ("2026-11-05T23:59:00+08:00", "2026-11-05", Phase::Continuous),
        ("2026-11-06T00:00:00+08:00", "2026-11-05", Phase::Continuous),
        ("2026-11-06T01:30:00+08:00", "2026-11-05", Phase::Continuous),
        ("2026-11-06T03:59:59+08:00", "2026-11-05", Phase::Continuous),
        ("2026-11-06T04:00:00+08:00", "2026-11-06", Phase::Closed),
        ("2026-11-06T06:30:00+08:00", "2026-11-06", Phase::Closed),
        ("2026-11-06T08:44:59+08:00", "2026-11-06", Phase::Closed),
        ("2026-11-06T08:45:00+08:00", "2026-11-06", Phase::Auction),
        ("2026-11-06T08:59:59+08:00", "2026-11-06", Phase::Auction),
        ("2026-11-06T09:00:00+08:00", "2026-11-06", Phase::Continuous),
        ("2026-11-06T12:00:00+08:00", "2026-11-06", Phase::Continuous),
    ];
    for (iso, day, phase) in cases {
        let c = clock_at(&cfg, iso);
        assert_eq!(c.today(), day, "{iso}");
        assert_eq!(c.phase(&cfg.game, false), phase, "{iso}");
        assert_eq!(c.trading(&cfg.game), phase == Phase::Continuous, "{iso}");
    }
    // without a rollover (v1, the difftests) the day is the calendar date
    assert_eq!(Clock::fixed(cfg.tz, "2026-11-06T01:30:00+08:00").today(), "2026-11-06");
    assert_eq!(cfg.game.session_text(), "09:00 至次日 04:00");
}

#[test]
fn the_daily_close_at_six_thirty_closes_the_previous_trading_day() {
    // the daily loop closes up to prev_day(today) once the time is past daily_step_at
    let cfg = cfg();
    assert_eq!(cfg.game.daily_step_at, "06:30");
    assert_eq!(prev_day(&clock_at(&cfg, "2026-11-06T06:30:00+08:00").today()), "2026-11-05");
    // between midnight and 04:00 yesterday is two calendar days back, and the loop is not due anyway
    assert_eq!(prev_day(&clock_at(&cfg, "2026-11-06T02:00:00+08:00").today()), "2026-11-04");
    // after a long downtime, the evening still closes up to the previous trading day
    assert_eq!(prev_day(&clock_at(&cfg, "2026-11-08T21:00:00+08:00").today()), "2026-11-07");
}

#[test]
fn the_market_npcs_trade_the_night_for_the_previous_day() {
    assert_eq!(day_of(at_local("2026-11-06", 1, 30)), "2026-11-05");
    assert_eq!(day_of(at_local("2026-11-06", 3, 59)), "2026-11-05");
    assert_eq!(day_of(at_local("2026-11-06", 4, 0)), "2026-11-06");
    assert_eq!(day_of(at_local("2026-11-06", 9, 0)), "2026-11-06");
    let cfg = cfg();
    let m = Market::new(MarketConfig::default(), &cfg);
    let s = |h: i64, mi: i64| h * 3600 + mi * 60;
    let night = MarketConfig::default().curve_night;
    assert_eq!(m.curve(s(23, 30)), night[0]);
    assert_eq!(m.curve(s(0, 30)), night[0]);
    assert_eq!(m.curve(s(2, 30)), night[1]);
    assert!(night[0] > night[1] && night[1] > 0.0);
    assert_eq!(m.curve(s(4, 0)), 0.0);
    assert_eq!(m.curve(s(8, 44)), 0.0);
    assert!(m.curve(s(8, 45)) > 0.0);
}

fn company(id: i64) -> Company {
    Company {
        id,
        code: format!("{}", 100000 + id),
        title: "テスト".into(),
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
        shares: 1000,
        ipo_price: 500,
        prev_close: 500,
        open_ref: None,
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 500,
        fair_value: 0,
        profit_week: 50_000,
        low_weeks: 0,
        delisted_on: None,
    }
}

fn order<'a>(acct: i64, side: &'a str, qty: i64, price: i64, day: &'a str, ts: &'a str) -> NewOrder<'a> {
    NewOrder { account_id: acct, company_id: 1, side, kind: "limit", qty, price: Some(price), day, ts, match_: true }
}

/// Sunday's trading day, partly after midnight, then its close at 06:30 Monday: the 01:30 trade is in
/// Sunday's candle, the 01:30 buyer is a Sunday record holder taxed as a same-day holder, and open orders
/// placed before 04:00 expire while one placed after the rollover stays.
#[test]
fn sunday_night_trades_belong_to_sunday() {
    let mut cfg = cfg();
    cfg.economy_v1 = true; // the v1 close needs no valuation state; expiry, candles and dividends are shared
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
    let ts0 = "2026-11-07T10:00:00+08:00";
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, ts0);
    }
    e.insert_company(company(1));
    for k in ["company", "committee"] {
        e.create_account(k, k, None, None, Some(1), ts0);
    }
    let world = e.special("world").unwrap();
    let committee = e.account_of("committee", 1).unwrap();
    let company_acct = e.account_of("company", 1).unwrap();
    e.add_shares(committee, 1, 800, 500, "2026-09-10");
    e.transfer(world, company_acct, 100_000, "revenue", "2026-11-07", ts0, Some(1), None);
    let seller = e.create_account("player", "s", Some("s"), None, None, ts0);
    e.add_shares(seller, 1, 200, 500, "2026-09-10");
    let p = e.create_account("player", "p", Some("p"), None, None, ts0);
    e.transfer(world, p, 1_000_000, "grant", "2026-11-07", ts0, None, None);
    for (k, v) in [("rate", "0.03"), ("a_ref", "100000.0"), ("last_closed", "2026-11-07")] {
        e.set_state(k, v);
    }

    // every order is stamped with the trading day the API takes from the clock
    let at = |iso: &str| clock_at(&cfg, iso).today();
    let place = |e: &mut Engine, o: NewOrder| e.open_and_place(&o).unwrap();
    let t1 = "2026-11-08T22:00:00+08:00";
    let sell = place(&mut e, order(seller, "sell", 150, 500, &at(t1), t1));
    let t2 = "2026-11-09T01:30:00+08:00";
    assert_eq!(at(t2), SUNDAY);
    let buy = place(&mut e, order(p, "buy", 100, 500, &at(t2), t2));
    assert_eq!(buy.filled, 100);
    let t3 = "2026-11-09T03:59:00+08:00";
    let late = place(&mut e, order(p, "buy", 10, 450, &at(t3), t3));
    // a resting order from Monday's opening auction (the close may run late, after it)
    let t4 = "2026-11-09T08:50:00+08:00";
    let monday = e
        .place_order(&NewOrder { match_: false, ..order(p, "buy", 10, 440, &at(t4), t4) })
        .unwrap();
    assert_eq!(at(t4), "2026-11-09");
    assert_eq!(e.lots_of(p, 1).iter().map(|l| l.acquired_on.as_str()).collect::<Vec<_>>(), [SUNDAY]);
    e.flush().unwrap();
    sink.0.lock().unwrap().clear();

    let inp = Inputs::parse(
        &serde_json::to_string(&serde_json::json!({
            "day": SUNDAY, "cn_titles": {}, "shows": [{"id": 1, "title_ja": "テスト", "title_en": null, "studio": null,
            "first_air": "2026-10-01", "episodes": 12, "episodes_known": true, "long_runner": false,
            "popularity_end": 9000, "aired": 6, "has_real_data": true,
            "att": {"us": 100.0, "cn": 50.0, "jp": 10.0}, "expected": {"us": 100.0, "cn": 50.0, "jp": 10.0}}]
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(e.run_day(&cfg, SUNDAY, CLOSE_TS, &inp).unwrap(), CloseOutcome::Closed(_)));
    e.flush().unwrap();
    let changes = sink.0.lock().unwrap().clone();

    // Sunday's candle has the 22:00 and the 01:30 trades (one fill of 100)
    let candle = changes
        .iter()
        .find_map(|c| match c {
            Change::CandleReplace(k) if k.company_id == 1 => Some(k.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!((candle.day.as_str(), candle.volume), (SUNDAY, 100));

    // the 01:30 buyer gets Sunday's dividend, taxed at the same-day rate
    let paid = |reason: &str, to: i64| {
        changes
            .iter()
            .filter_map(|c| match c {
                Change::LedgerInsert(l) if l.reason == reason && l.from_id == company_acct && (to == 0 || l.to_id == to) => Some(l.amount),
                _ => None,
            })
            .sum::<i64>()
    };
    let net = paid("dividend", p);
    assert!(net > 0, "the night buyer is a record holder");
    let dps = changes
        .iter()
        .find_map(|c| match c {
            Change::CompanyDayDividend { company_id: 1, day, dividend_ps } if day == SUNDAY => Some(*dividend_ps),
            _ => None,
        })
        .expect("Sunday's dividend");
    assert_eq!(net, dps * 100 - (dps as f64 * 100.0 * cfg.game.dividend_tax[0].1).floor() as i64);

    // orders placed up to 03:59 expire with Sunday; Monday's auction order stays
    assert!(e.order(sell.order_id).is_none(), "expired"); // 50 left unfilled
    assert!(e.order(late.order_id).is_none(), "expired");
    assert!(e.order(monday.order_id).is_some(), "still open");
    e.check_invariants().unwrap();
}

#[test]
fn an_order_at_four_is_after_the_session() {
    // the expiry cutoff is the rollover: created_at 04:00:00 the next morning is the next day's order
    let mut cfg = cfg();
    cfg.economy_v1 = true;
    let (mut e, _sink) = Engine::in_memory(Fees::default(), Rules::default());
    let ts0 = "2026-11-04T10:00:00+08:00";
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, ts0);
    }
    let world = e.special("world").unwrap();
    let p = e.create_account("player", "p", Some("p"), None, None, ts0);
    e.transfer(world, p, 1_000_000, "grant", "2026-11-04", ts0, None, None);
    e.insert_company(company(1));
    for k in ["company", "committee"] {
        e.create_account(k, k, None, None, Some(1), ts0);
    }
    let committee = e.account_of("committee", 1).unwrap();
    e.add_shares(committee, 1, 1000, 500, "2026-09-10");
    for (k, v) in [("rate", "0.03"), ("a_ref", "100000.0"), ("last_closed", "2026-11-04")] {
        e.set_state(k, v);
    }
    let day = "2026-11-05"; // a Thursday
    let a = e.open_and_place(&order(p, "buy", 1, 400, day, "2026-11-06T03:59:59+08:00")).unwrap();
    let b = e.place_order(&order(p, "buy", 1, 400, "2026-11-06", "2026-11-06T04:00:00+08:00")).unwrap();
    let inp = Inputs::parse(&format!(r#"{{"day":"{day}","cn_titles":{{}},"shows":[]}}"#)).unwrap();
    assert!(matches!(e.run_day(&cfg, day, "2026-11-06T06:30:00+08:00", &inp).unwrap(), CloseOutcome::Closed(_)));
    assert!(e.order(a.order_id).is_none(), "expired");
    assert!(e.order(b.order_id).is_some(), "still open");
}

/// The v2 NPCs keep trading after midnight, in continuous trading (not the opening auction) and for the
/// previous trading day, and stop at 04:00 until the 08:45 auction.
#[test]
fn the_npc_market_trades_the_night_session() {
    let cfg = cfg();
    let (mut e, sink) = Engine::in_memory(Fees::default(), Rules::default());
    let ts = "2026-11-04T20:00:00+08:00";
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, ts);
    }
    let world = e.special("world").unwrap();
    for i in 0..3i64 {
        let id = 1 + i;
        let mut c = company(id);
        c.shares = 10_000;
        c.open_ref = Some(500);
        c.fair_value = 520;
        e.insert_company(c);
        let committee = e.create_account("committee", "c", None, None, Some(id), ts);
        let mm = e.create_account("mm", "mm", None, None, Some(id), ts);
        e.transfer(world, mm, 3_000_000, "grant", "2026-11-04", ts, None, None);
        e.add_shares(committee, id, 7500, 500, "2026-09-10");
        e.add_shares(mm, id, 2500, 500, "2026-09-10");
    }
    e.set_state("auction_day", "2026-11-05");
    let mut mc = MarketConfig::default();
    (mc.n_value, mc.n_trend, mc.n_news, mc.n_retail, mc.population_reference) = (40, 20, 10, 30, 0.0);
    let mut m = Market::new(mc, &cfg);
    let t0 = at_local("2026-11-05", 22, 0);
    let ts0 = anistock::market::ts_of(t0);
    m.create_population(&mut e, "2026-11-05", &ts0);
    m.rebalance_float(&mut e, "2026-11-05", &ts0);
    m.refresh_universe(&e, t0);
    m.reschedule(t0);
    e.flush().unwrap();
    sink.0.lock().unwrap().clear();
    m.step_until(&mut e, at_local("2026-11-06", 8, 44));
    e.flush().unwrap();
    let trades: Vec<_> = sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|c| match c {
            Change::TradeInsert(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    let after_midnight = trades.iter().filter(|t| t.ts.as_str() >= "2026-11-06T00:00").count();
    assert!(after_midnight > 20, "{after_midnight} night trades");
    assert!(trades.iter().all(|t| t.day == "2026-11-05"), "every trade belongs to the 5th's session");
    assert!(trades.iter().all(|t| t.ts.as_str() < "2026-11-06T04:00"), "nothing after the session ends");
    // no second opening auction for the 6th
    assert_eq!(e.state("auction_day"), Some("2026-11-05"));
    e.check_invariants().unwrap();
}
