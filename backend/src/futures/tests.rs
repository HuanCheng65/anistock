use chrono::NaiveDate;

use super::listing::Underlying;
use super::store::{apply, load};
use super::*;
use crate::engine::Engine;
use crate::model::{Company, Kind, Side};
use crate::{Fees, Rules};

const TS: &str = "2026-10-03T06:30:00+08:00";

fn d(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

fn listed_company(id: i64) -> Company {
    Company {
        id,
        code: format!("{}", 100000 + id),
        title: format!("作品{id}"),
        title_ja: None,
        title_en: None,
        studio: None,
        episodes: Some(12),
        long_runner: 0,
        first_air: "2026-10-03".into(),
        listed_on: "2026-09-20".into(),
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
        fair_value: 500,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
    }
}

/// world, cb, gov and listed companies 1 and 2.
fn engine() -> Engine {
    let (mut e, _sink) = Engine::in_memory(Fees::default(), Rules::default());
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    e.insert_company(listed_company(1));
    e.insert_company(listed_company(2));
    e
}

fn contract(id: i64) -> Contract {
    Contract {
        id,
        code: format!("F{id}"),
        company_id: 1,
        work_id: 101,
        period: 1,
        first_day: "2026-10-08".into(),
        last_day: "2026-11-04".into(),
        listed_on: "2026-10-01".into(),
        status: ContractStatus::Listed,
        tick_value: 10,
        forecast: 9000,
        prev_settle: 9000,
        open_ref: None,
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 9000,
        final_price: None,
        settled_on: None,
    }
}

fn order(id: i64, side: Side, price: i64, status: OrderStatus) -> FutOrder {
    FutOrder {
        id,
        account_id: 7,
        contract_id: 1,
        side,
        kind: Kind::Limit,
        price: Some(price),
        qty: 3,
        filled: 1,
        frozen: 2700,
        status,
        created_at: "t".into(),
        updated_at: "t".into(),
    }
}

#[test]
fn rows_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    crate::store::migrate(&path).unwrap();
    let conn = crate::store::connect(&path).unwrap();
    // the contract table references companies
    conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    let mut settled = contract(2);
    settled.status = ContractStatus::Settled;
    let changes = [
        FuturesChange::Contract(contract(1)),
        FuturesChange::Contract(settled),
        FuturesChange::Order(order(5, Side::Buy, 8900, OrderStatus::Open)),
        FuturesChange::Order(order(6, Side::Sell, 9100, OrderStatus::Open)),
        FuturesChange::Order(order(7, Side::Sell, 9200, OrderStatus::Cancelled)),
        FuturesChange::Position(Position { account_id: 7, contract_id: 1, qty: -2, basis: -18000 }),
        FuturesChange::Position(Position { account_id: 8, contract_id: 1, qty: 2, basis: 18000 }),
        FuturesChange::Position(Position { account_id: 9, contract_id: 1, qty: 0, basis: 5 }),
        FuturesChange::PositionDelete(9, 1),
        FuturesChange::Trade(FutTrade {
            id: 3,
            contract_id: 1,
            price: 9000,
            qty: 2,
            buyer_id: 8,
            seller_id: 7,
            buy_order: 4,
            sell_order: 5,
            ts: "t".into(),
            day: "2026-10-08".into(),
        }),
        FuturesChange::Day(FutDay {
            contract_id: 1,
            day: "2026-10-08".into(),
            ohlc: Some((9000, 9000, 9000, 9000)),
            volume: 2,
            settle: 9000,
            source: SettleSource::Vwap,
            open_interest: 2,
        }),
    ];
    for c in &changes {
        apply(&conn, c).unwrap();
    }
    let s = load(&conn).unwrap();
    assert_eq!(s.contracts.keys().copied().collect::<Vec<_>>(), [1], "settled contracts are not loaded");
    assert_eq!(s.contracts[&1], contract(1));
    assert_eq!(s.orders.keys().copied().collect::<BTreeSet<_>>(), BTreeSet::from([5, 6]), "only open orders");
    assert_eq!(s.book(1).bids().collect::<Vec<_>>(), [(8900, 5)]);
    assert_eq!(s.book(1).asks().collect::<Vec<_>>(), [(9100, 6)]);
    assert_eq!(s.position(7, 1), -2);
    assert_eq!(s.open_interest(1), 2);
    assert!(!s.positions.contains_key(&(9, 1)));
    assert_eq!((s.last_contract_id, s.last_order_id, s.last_trade_id), (2, 7, 3));
}

#[test]
fn heat_is_published_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    crate::store::migrate(&path).unwrap();
    let conn = crate::store::connect(&path).unwrap();
    let row = |idx: f64| HeatRow {
        work_id: 101,
        day: "2026-10-08".into(),
        idx,
        us: Some(1.0),
        cn: None,
        jp: Some(2.0),
        flags: "{}".into(),
        version: "v".into(),
    };
    apply(&conn, &FuturesChange::Heat(row(80.0))).unwrap();
    apply(&conn, &FuturesChange::Heat(row(99.0))).unwrap();
    let idx: f64 = conn.query_row("SELECT idx FROM heat_index WHERE work_id=101", [], |r| r.get(0)).unwrap();
    assert_eq!(idx, 80.0);
}

#[test]
fn variation_margin_sums_to_zero() {
    // A is long 2 and B short 2 from yesterday's settlement at 90.00; A sells B one lot at 92.00; today settles
    // at 91.00. A earns 1.00 on the lot kept and 2.00 on the lot sold; B pays the same.
    let a = Position { account_id: 1, contract_id: 1, qty: 2 - 1, basis: 2 * 9000 - 9200 };
    let b = Position { account_id: 2, contract_id: 1, qty: -2 + 1, basis: -2 * 9000 + 9200 };
    assert_eq!(a.variation(9100), 100 + 200);
    assert_eq!(a.variation(9100) + b.variation(9100), 0);
}

#[test]
fn listing_once_per_period_above_the_floor() {
    let mut e = engine();
    let rules = FuturesRules::default();
    let works = [
        Underlying { company_id: 1, work_id: 1, premiere: d("2026-10-03") },
        Underlying { company_id: 2, work_id: 2, premiere: d("2026-10-03") },
    ];
    let forecast = |u: &Underlying, _: NaiveDate, _: NaiveDate| Some(if u.work_id == 1 { 120.0 } else { 40.0 });
    // the premiere: period 1 is listed for the hot work only
    let new = e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &works, forecast);
    assert_eq!(new.len(), 1);
    let c = &e.st.futures.contracts[&new[0]];
    assert_eq!((c.work_id, c.period, c.first_day.as_str(), c.last_day.as_str()), (1, 1, "2026-10-10", "2026-11-06"));
    assert_eq!((c.forecast, c.prev_settle), (12000, 12000));
    // the same day again (a re-run close) lists nothing; nor does a day that is no listing day
    assert!(e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &works, forecast).is_empty());
    assert!(e.futures_list(&rules, "2026-10-03", d("2026-10-04"), &works, forecast).is_empty());
    // period 2 on the first day of period 1; the cold work still stays below the floor
    let new = e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &works, forecast);
    assert_eq!(new.iter().map(|i| e.st.futures.contracts[i].period).collect::<Vec<_>>(), [2]);
}

#[test]
fn no_listing_for_a_company_on_its_way_out() {
    let mut e = engine();
    let rules = FuturesRules::default();
    let u = [Underlying { company_id: 1, work_id: 1, premiere: d("2026-10-03") }];
    e.st.companies.get_mut(&1).unwrap().low_weeks = 1;
    assert!(e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &u, |_, _, _| Some(500.0)).is_empty());
    e.st.companies.get_mut(&1).unwrap().low_weeks = 0;
    e.st.penny_days.insert(1, 3);
    assert!(e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &u, |_, _, _| Some(500.0)).is_empty());
    e.st.penny_days.remove(&1);
    assert_eq!(e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &u, |_, _, _| Some(500.0)).len(), 1);
}
