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
        shares: 0, // nobody holds it in these tests
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
        FuturesChange::Position(Position { account_id: 7, contract_id: 1, qty: -2, basis: -18000, margin: 0 }),
        FuturesChange::Position(Position { account_id: 8, contract_id: 1, qty: 2, basis: 18000, margin: 0 }),
        FuturesChange::Position(Position { account_id: 9, contract_id: 1, qty: 0, basis: 5, margin: 0 }),
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
    let a = Position { account_id: 1, contract_id: 1, qty: 2 - 1, basis: 2 * 9000 - 9200, margin: 0 };
    let b = Position { account_id: 2, contract_id: 1, qty: -2 + 1, basis: -2 * 9000 + 9200, margin: 0 };
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
    let forecast = |u: &Underlying, _: i64, _: NaiveDate, _: NaiveDate| Some(if u.work_id == 1 { 120.0 } else { 40.0 });
    // nothing at the premiere or during week 0
    assert!(e.futures_list(&rules, "2026-10-02", d("2026-10-03"), &works, forecast).is_empty());
    assert!(e.futures_list(&rules, "2026-10-03", d("2026-10-04"), &works, forecast).is_empty());
    // a week later: periods 1 and 2, for the hot work only
    let new = e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &works, forecast);
    assert_eq!(new.len(), 2);
    let c = &e.st.futures.contracts[&new[0]];
    assert_eq!((c.work_id, c.period, c.first_day.as_str(), c.last_day.as_str()), (1, 1, "2026-10-10", "2026-11-06"));
    assert_eq!((c.forecast, c.prev_settle), (12000, 12000));
    assert_eq!(e.st.futures.contracts[&new[1]].period, 2);
    // the same day again (a re-run close) lists nothing
    assert!(e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &works, forecast).is_empty());
    // period 3 on the first day of period 2
    let new = e.futures_list(&rules, "2026-11-06", d("2026-11-07"), &works, forecast);
    assert_eq!(new.iter().map(|i| e.st.futures.contracts[i].period).collect::<Vec<_>>(), [3]);
}

#[test]
fn no_listing_for_a_company_on_its_way_out() {
    let mut e = engine();
    let rules = FuturesRules::default();
    let u = [Underlying { company_id: 1, work_id: 1, premiere: d("2026-10-03") }];
    e.st.companies.get_mut(&1).unwrap().low_weeks = 1;
    assert!(e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &u, |_, _, _, _| Some(500.0)).is_empty());
    e.st.companies.get_mut(&1).unwrap().low_weeks = 0;
    e.st.penny_days.insert(1, 3);
    assert!(e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &u, |_, _, _, _| Some(500.0)).is_empty());
    e.st.penny_days.remove(&1);
    assert_eq!(e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &u, |_, _, _, _| Some(500.0)).len(), 2);
}

// ---------------------------------------------------------------- trading

const DAY: &str = "2026-10-12";
const T1: &str = "2026-10-12T10:00:00+08:00";

/// An engine with futures on, one listed contract at 90.00 (id 1) and three players with 100 000 币 each.
fn market() -> (Engine, [i64; 3]) {
    let mut e = engine();
    e.futures.enabled = true;
    let u = [Underlying { company_id: 1, work_id: 1, premiere: d("2026-10-03") }];
    let rules = e.futures.clone();
    assert_eq!(e.futures_list(&rules, "2026-10-09", d("2026-10-10"), &u, |_, _, _, _| Some(90.0)), [1, 2]);
    let ids = ["a", "b", "c"].map(|n| e.register_player(n, "x", 10_000_000, DAY, T1));
    for id in ids {
        e.open_credit_unchecked(id, T1);
    }
    (e, ids)
}

fn req(account_id: i64, side: Side, qty: i64, price: Option<i64>) -> trading::FutOrderReq<'static> {
    trading::FutOrderReq {
        account_id,
        contract_id: 1,
        side,
        kind: if price.is_some() { Kind::Limit } else { Kind::Market },
        qty,
        price,
        day: DAY,
        ts: T1,
        match_: true,
    }
}

/// cash_frozen of an account equals what its futures orders and positions hold.
fn frozen_matches(e: &Engine, a: i64) {
    let f = &e.st.futures;
    let held: i64 = f.positions.values().filter(|p| p.account_id == a).map(|p| p.margin).sum::<i64>()
        + f.orders.values().filter(|o| o.account_id == a).map(|o| o.frozen).sum::<i64>();
    assert_eq!(e.st.accounts[&a].cash_frozen, held, "account {a}");
}

#[test]
fn fills_open_and_close_positions() {
    let (mut e, [a, b, c]) = market();
    let im = e.fut_im_lot(&e.st.futures.contracts[&1].clone());
    assert_eq!(im, 13500); // 15% of 90.00 x 10 星辉 = 135 星辉
    let world = e.special("world").unwrap();
    let world_before = e.st.accounts[&world].cash;
    let sold = e.futures_place(&req(a, Side::Sell, 2, Some(9000))).unwrap();
    assert_eq!((sold.status, sold.filled), (OrderStatus::Open, 0));
    let bought = e.futures_place(&req(b, Side::Buy, 3, Some(9100))).unwrap();
    assert_eq!((bought.status, bought.filled, bought.avg_price), (OrderStatus::Open, 2, Some(9000)));
    let f = &e.st.futures;
    assert_eq!(
        (f.positions[&(a, 1)].qty, f.positions[&(a, 1)].basis, f.positions[&(a, 1)].margin),
        (-2, -18000, 2 * im)
    );
    assert_eq!((f.positions[&(b, 1)].qty, f.positions[&(b, 1)].margin), (2, 2 * im));
    // commission: 0.025% of 2 x 90.00 x 1000 cents, each side
    assert_eq!(e.st.accounts[&world].cash - world_before, 2 * 45);
    for x in [a, b, c] {
        frozen_matches(&e, x);
    }
    e.check_invariants().unwrap();

    // c sells into b's resting bid; then a buys 3 from b: closes its 2 short and opens 1 long, b goes flat
    e.futures_place(&req(c, Side::Sell, 1, Some(9100))).unwrap();
    e.futures_place(&req(b, Side::Sell, 3, Some(9050))).unwrap();
    let placed = e.futures_place(&req(a, Side::Buy, 3, Some(9050))).unwrap();
    assert_eq!(placed.filled, 3);
    let f = &e.st.futures;
    assert_eq!((f.positions[&(a, 1)].qty, f.positions[&(a, 1)].margin), (1, im));
    let pb = f.positions[&(b, 1)];
    assert_eq!(
        (pb.qty, pb.basis, pb.margin),
        (0, 2 * 9000 + 9100 - 3 * 9050, 0),
        "flat, its gain waits for the settlement"
    );
    for x in [a, b, c] {
        frozen_matches(&e, x);
    }
    e.check_invariants().unwrap();
}

#[test]
fn closing_needs_no_margin() {
    let (mut e, [a, b, _]) = market();
    e.futures_place(&req(a, Side::Sell, 1, Some(9000))).unwrap();
    e.futures_place(&req(b, Side::Buy, 1, Some(9000))).unwrap();
    // b spends its free cash elsewhere; closing still goes through, opening does not
    let world = e.special("world").unwrap();
    e.do_transfer(b, world, e.available_cash(b) - 1000, "test", DAY, T1, None, None);
    assert!(e.futures_place(&req(b, Side::Buy, 1, Some(9000))).is_err(), "opening needs margin");
    let placed = e.futures_place(&req(b, Side::Sell, 1, Some(9200))).unwrap();
    assert_eq!(placed.status, OrderStatus::Open);
    // a second closing order cannot close the same lot again
    assert!(e.futures_place(&req(b, Side::Sell, 1, Some(9200))).is_err());
    e.futures_cancel(b, placed.order_id, T1).unwrap();
    frozen_matches(&e, b);
    e.check_invariants().unwrap();
}

#[test]
fn rules_on_orders() {
    let (mut e, [a, _, _]) = market();
    assert_eq!(e.futures_place(&req(a, Side::Buy, 0, Some(9000))).unwrap_err().0, "数量必须是正整数");
    assert_eq!(
        e.futures_place(&req(a, Side::Buy, 1, Some(9000 * 21))).unwrap_err().0,
        "价格太离谱了：超出了允许的范围"
    );
    let m = e.futures_place(&req(a, Side::Buy, 1, None)).unwrap();
    assert_eq!((m.status, m.filled), (OrderStatus::Cancelled, 0), "a market order with nothing to take is cancelled");
    frozen_matches(&e, a);
    e.futures.enabled = false;
    assert_eq!(e.futures_place(&req(a, Side::Buy, 1, Some(9000))).unwrap_err().0, "期货交易还没有开放");
}

#[test]
fn opening_auction_and_cooling_off() {
    let (mut e, [a, b, c]) = market();
    let rest = |acct, side, qty, p| trading::FutOrderReq { match_: false, ..req(acct, side, qty, Some(p)) };
    e.futures_place(&rest(a, Side::Buy, 2, 9200)).unwrap();
    e.futures_place(&rest(b, Side::Sell, 1, 9100)).unwrap();
    e.futures_place(&rest(c, Side::Sell, 2, 9300)).unwrap();
    e.open_market(DAY, T1);
    let k = &e.st.futures.contracts[&1];
    assert_eq!((k.open_ref, k.last_price), (Some(9100), 9100));
    assert_eq!((e.st.futures.position(a, 1), e.st.futures.position(b, 1)), (1, -1));
    // a trade 11% above the 91.00 anchor starts a cooling-off period instead of printing
    e.futures_place(&req(b, Side::Sell, 1, Some(10100))).unwrap();
    let far = e.futures_place(&req(c, Side::Buy, 1, Some(10100))).unwrap();
    assert_eq!((far.status, far.filled), (OrderStatus::Open, 0));
    let k = &e.st.futures.contracts[&1];
    assert_eq!((k.vcm_count, k.vcm_low, k.vcm_high), (1, Some(8190), Some(10010)));
    for x in [a, b, c] {
        frozen_matches(&e, x);
    }
    e.check_invariants().unwrap();
}

// ---------------------------------------------------------------- settlement

const END: &str = "2026-10-13T04:00:00"; // the end of trading day DAY's session

fn heat(work: i64, idx: f64) -> std::collections::BTreeMap<i64, crate::inputs::HeatIndex> {
    let h =
        crate::inputs::HeatIndex { idx, us: None, cn: None, jp: None, flags: Default::default(), version: "t".into() };
    std::collections::BTreeMap::from([(work, h)])
}

fn cash(e: &Engine, a: i64) -> i64 {
    e.st.accounts[&a].cash
}

/// a short 2 and b long 2 at 90.00.
fn open_pair() -> (Engine, [i64; 3]) {
    let (mut e, ids) = market();
    e.futures_place(&req(ids[0], Side::Sell, 2, Some(9000))).unwrap();
    e.futures_place(&req(ids[1], Side::Buy, 2, Some(9000))).unwrap();
    (e, ids)
}

#[test]
fn daily_settlement_marks_to_market() {
    let (mut e, [a, b, c]) = open_pair();
    let (ca, cb) = (cash(&e, a), cash(&e, b));
    e.st.futures.last_mid.insert(1, 9100);
    let out = e.futures_close(DAY, END, END, &heat(1, 95.0));
    assert_eq!((out.variation, out.shortfall), (2 * 100 * 10, 0));
    assert_eq!((cash(&e, a), cash(&e, b)), (ca - 2000, cb + 2000));
    assert_eq!(cash(&e, e.special("clearing").unwrap()), 0);
    let k = &e.st.futures.contracts[&1];
    assert_eq!((k.prev_settle, k.status), (9100, ContractStatus::Listed));
    let im = e.fut_im_lot(&k.clone());
    assert_eq!(im, 13650);
    let pa = e.st.futures.positions[&(a, 1)];
    assert_eq!((pa.qty, pa.basis, pa.margin), (-2, -2 * 9100, 2 * im));
    assert_eq!(e.st.futures.heat[&(1, DAY.to_string())], 95.0);
    for x in [a, b, c] {
        frozen_matches(&e, x);
    }
    e.check_invariants().unwrap();
}

#[test]
fn settlement_price_sources() {
    let (mut e, [a, b, _]) = open_pair();
    // trades in the last half hour: their volume-weighted average
    let late =
        |acct, side, qty, p| trading::FutOrderReq { ts: "2026-10-13T03:45:00+08:00", ..req(acct, side, qty, Some(p)) };
    e.futures_place(&late(a, Side::Sell, 1, 9200)).unwrap();
    e.futures_place(&late(b, Side::Buy, 1, 9200)).unwrap();
    e.futures_place(&late(a, Side::Sell, 3, 9300)).unwrap();
    e.futures_place(&late(b, Side::Buy, 3, 9300)).unwrap();
    e.futures_close(DAY, END, END, &Default::default());
    assert_eq!(e.st.futures.contracts[&1].prev_settle, (9200 + 3 * 9300 + 2) / 4);
    // the next day without trades or quotes: the previous settlement
    let before = e.st.futures.contracts[&1].prev_settle;
    e.futures_close("2026-10-13", "2026-10-14T04:00:00", "2026-10-14T04:00:00", &Default::default());
    assert_eq!(e.st.futures.contracts[&1].prev_settle, before);
    e.check_invariants().unwrap();
}

#[test]
fn final_settlement_on_the_period_average() {
    let (mut e, [a, b, c]) = open_pair();
    let k = e.st.futures.contracts[&1].clone();
    let first = d(&k.first_day);
    let (ca, cb) = (cash(&e, a), cash(&e, b));
    // the last day is published but one day in the middle is missing: trading stops, the contract waits
    for i in 0..28 {
        if i != 10 {
            e.st.futures.heat.insert((1, (first + chrono::Duration::days(i)).to_string()), 80.0 + i as f64);
        }
    }
    e.futures_close(&k.last_day, END, END, &Default::default());
    assert_eq!(e.st.futures.contracts[&1].status, ContractStatus::Expiring);
    assert!(e.futures_place(&req(a, Side::Buy, 1, Some(9000))).is_err());
    // the missing day arrives: settled on the average, 80 + 13.5 = 93.50
    e.st.futures.heat.insert((1, (first + chrono::Duration::days(10)).to_string()), 90.0);
    let out = e.futures_close("2026-11-07", END, END, &Default::default());
    assert_eq!(out.settled, [1]);
    assert!(!e.st.futures.contracts.contains_key(&1) && e.st.futures.positions.is_empty());
    assert_eq!((cash(&e, a), cash(&e, b)), (ca - 2 * 350 * 10, cb + 2 * 350 * 10));
    for x in [a, b, c] {
        assert_eq!(e.st.accounts[&x].cash_frozen, 0);
    }
    e.check_invariants().unwrap();
}

#[test]
fn an_unpaid_loss_is_covered_and_owed() {
    let (mut e, [a, b, _]) = open_pair();
    let world = e.special("world").unwrap();
    // a keeps only its margin
    let spare = e.available_cash(a);
    e.do_transfer(a, world, spare, "test", DAY, T1, None, None);
    let margin = e.st.accounts[&a].cash;
    e.st.futures.last_mid.insert(1, 9000 + 5000); // +50 points: a owes 2 x 5000 x 10 cents
    let cb = cash(&e, b);
    let out = e.futures_close(DAY, END, END, &Default::default());
    assert_eq!(out.shortfall, 100_000 - margin);
    assert_eq!(cash(&e, b), cb + 100_000, "the winner is paid in full");
    assert_eq!((cash(&e, a), cash(&e, e.special("clearing").unwrap())), (0, 0));
    e.check_invariants().unwrap();
}

// ---------------------------------------------------------------- the credit account

#[test]
fn players_need_credit_and_respect_the_caps() {
    let (mut e, [a, _, _]) = market();
    let d = e.register_player("d", "x", 10_000_000, DAY, T1);
    assert_eq!(e.futures_place(&req(d, Side::Buy, 1, Some(9000))).unwrap_err().0, "期货交易需要先开通信用账户");
    // 100 000 币 of net assets: one contract's notional at most 50 000 币 = 55 lots at 90.00
    assert_eq!(
        e.futures_place(&req(a, Side::Buy, 56, Some(9000))).unwrap_err().0,
        "单个合约的持仓不能超过净资产的 50%"
    );
    assert!(e.futures_place(&req(a, Side::Buy, 55, Some(9000))).is_ok());
    assert!(e.futures_place(&req(a, Side::Buy, 1, Some(9000))).is_err(), "resting orders count toward the cap");
}

#[test]
fn stocks_back_the_margin_through_financing() {
    let (mut e, [a, _, c]) = market();
    // c holds 1000 shares of company 2 at 5.00 and only 10 币 of cash
    e.st.companies.get_mut(&2).unwrap().shares = 1000;
    e.do_add_shares(c, 2, 1000, 500, DAY);
    let world = e.special("world").unwrap();
    e.do_transfer(c, world, e.available_cash(c) - 1000, "test", DAY, T1, None, None);
    e.futures_place(&req(a, Side::Sell, 2, Some(9000))).unwrap();
    let placed = e.futures_place(&req(c, Side::Buy, 2, Some(9000))).unwrap();
    assert_eq!(placed.filled, 2);
    let debt = e.st.margin.debts[&(c, 1)].principal;
    let fee_lot = e.fees.commission_on(9000 * 10) + 1;
    assert_eq!(debt, 2 * 13500 + 2 * fee_lot - 1000, "what free cash did not cover is borrowed");
    assert_eq!(e.st.futures.positions[&(c, 1)].margin, 2 * 13500);
    let n = e.credit_numbers(c, DAY);
    assert_eq!(n.futures_margin, 2 * 13500);
    assert!(n.available_margin >= 0, "{n:?}");
    e.check_invariants().unwrap();
}

#[test]
fn a_futures_loss_triggers_maintenance_and_liquidation_closes_futures_first() {
    let (mut e, [a, b, _]) = open_pair();
    let world = e.special("world").unwrap();
    e.do_transfer(a, world, e.available_cash(a), "test", DAY, T1, None, None);
    // the index jumps: a (short 2) owes more than it has; the loss is financed and the account is called
    e.st.futures.last_mid.insert(1, 11000);
    let out = e.futures_close(DAY, END, END, &Default::default());
    assert!(out.shortfall > 0);
    assert!(e.st.margin.debts[&(a, 1)].principal > 0);
    e.margin_post_close(DAY, END);
    assert_ne!(e.credit(a).unwrap().status, crate::margin::NORMAL);
    assert_eq!(e.margin_restructure_check(a), Err(()), "no restructuring with futures open");
    // liquidation: b offers to sell back; the sweep closes a's short first
    e.st.margin.credit.get_mut(&a).unwrap().status = crate::margin::LIQUIDATING.into();
    let t = "2026-10-13T10:00:00+08:00";
    e.futures_place(&trading::FutOrderReq { day: "2026-10-13", ts: t, ..req(b, Side::Sell, 2, Some(11000)) }).unwrap();
    e.margin_sweep("2026-10-13", t);
    assert_eq!(e.st.futures.position(a, 1), 0);
    e.check_invariants().unwrap();
}

// ---------------------------------------------------------------- the forecast

fn game_cfg() -> crate::config::Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    crate::config::Config::load_from(&root, |_| None).unwrap()
}

/// Work 1 in its fifth week, with a filter state of log level `level` (us only) and log variance `var`.
fn with_state(e: &mut Engine, level: f64, var: f64) {
    let mut js = crate::vclose::JState::empty("2026-11-07");
    js.active = [true, false, false];
    js.mean = [level, 0.0, 0.0];
    js.cov[0][0] = var;
    e.st.jstate.insert(1, js);
}

#[test]
fn forecast_of_a_period() {
    let cfg = game_cfg();
    let fp = crate::vfilter::FilterParams::load(&cfg).unwrap();
    let p = crate::valuation::Params::from_config(&cfg).unwrap();
    let mut e = engine();
    e.futures.horizon_drift = 0.0; // compared with the raw path below
    with_state(&mut e, 7.0, 0.04);
    let day = |s: &str| crate::vclose::day_no(s).unwrap();
    let (first, last, today) = (day("2026-11-07"), day("2026-12-04"), day("2026-11-06"));
    let f = e.fut_forecast(&fp, &p, 1, 1, first, last, today).unwrap();
    // the mean is the expected path in index points
    let js = e.st.jstate[&1].clone();
    let (sched, fa, long) = e.work_shape(&e.st.companies[&1].clone(), 1, &p);
    let path =
        crate::vfilter::att_path(&fp.for_class(None), &p, 0, 7.0, 0.04, today, &sched, fa, long, 28, js.tail_mult);
    let k = e.futures.index_per_att(0, 6); // us: the same in every block
    assert!((f.mean - k * path.iter().sum::<f64>() / 28.0).abs() < 1e-9);
    assert!(f.sd > 0.15 * f.mean * 0.99, "at least model_cv of the mean: {f:?}");
    // more certain level, smaller spread; a period entirely published: its average, no spread
    with_state(&mut e, 7.0, 0.0001);
    assert!(e.fut_forecast(&fp, &p, 1, 1, first, last, today).unwrap().sd < f.sd);
    for t in first..=last {
        e.st.futures.heat.insert((1, forecast::day_str(t)), 50.0 + (t - first) as f64);
    }
    let done = e.fut_forecast(&fp, &p, 1, 1, first, last, last).unwrap();
    assert_eq!((done.mean, done.sd), (63.5, 0.0));
}

#[test]
fn the_market_maker_quotes_and_takes_the_other_side() {
    let (mut e, [a, _, _]) = market();
    e.st.futures.mean.insert(1, 9000);
    e.st.futures.sd.insert(1, 0);
    e.futures.mm_min_half = 100; // a half-spread of 1.00
    let q = e.futures_mm_requote(1, DAY, T1);
    assert_eq!((q.mid, q.bids[0], q.asks[0]), (9000, (8900, 10), (9100, 10)));
    assert_eq!(e.st.futures.last_mid[&1], 9000);
    let bought = e.futures_place(&req(a, Side::Buy, 15, None)).unwrap();
    assert_eq!((bought.filled, bought.avg_price), (15, Some((10 * 9100 + 5 * 9200 + 7) / 15)));
    let mm = e.special("fmm").unwrap();
    assert_eq!(e.st.futures.position(mm, 1), -15);
    // requoting replaces its orders and leans against the short
    let q = e.futures_mm_requote(1, DAY, T1);
    assert!(q.mid > 9000 && e.st.futures.open_orders(mm, 1).count() == 6);
    frozen_matches(&e, mm);
    e.check_invariants().unwrap();
}
