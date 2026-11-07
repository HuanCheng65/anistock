use super::store::{apply, load};
use super::*;
use crate::model::{Kind, Side};

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
