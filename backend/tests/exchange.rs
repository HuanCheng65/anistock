//! Port of tests/test_exchange.py.

mod common;

use anistock::exchange::cooling;
use anistock::model::OrderStatus;
use anistock::Fees;
use common::*;

const FEES: Fees = Fees { commission: 0.00025, stamp_duty: 0.0005 };

#[test]
fn limit_orders_cross_at_resting_price() {
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 0);
    let b = f.player("b", 0, 100);
    f.seed_rest();
    let sell = f.limit(b, "sell", 40, 520).unwrap();
    assert!(sell.status == OrderStatus::Open && sell.filled == 0);
    let buy = f.limit(a, "buy", 40, 540).unwrap();
    assert_eq!(buy.status, OrderStatus::Filled);
    assert_eq!(buy.avg_price, Some(520)); // resting price wins
    let notional = 40 * 520;
    let acc = f.e.account(a).unwrap();
    assert_eq!(acc.cash, 100_000 - notional - FEES.commission_on(notional));
    assert_eq!(acc.cash_frozen, 0);
    assert_eq!(f.e.account(b).unwrap().cash, notional - FEES.commission_on(notional) - FEES.stamp_on(notional));
    assert_eq!(f.e.available_shares(a, 1), 40);
    assert_eq!(f.e.available_shares(b, 1), 60);
    f.check();
}

#[test]
fn partial_fill_then_cancel_releases_frozen() {
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 0);
    let b = f.player("b", 0, 100);
    f.seed_rest();
    f.limit(b, "sell", 10, 500).unwrap();
    let r = f.limit(a, "buy", 30, 510).unwrap();
    assert!(r.filled == 10 && r.status == OrderStatus::Open);
    assert!(f.e.account(a).unwrap().cash_frozen > 0);
    f.e.cancel_order(a, r.order_id, TS).unwrap();
    assert_eq!(f.e.account(a).unwrap().cash_frozen, 0);
    let err = f.e.cancel_order(a, r.order_id, TS).unwrap_err();
    assert_eq!(err.0, "这笔委托已经结束，不能撤单");
    assert_eq!(f.e.cancel_order(b, r.order_id, TS).unwrap_err().0, "找不到这笔委托");
    assert_eq!(f.e.cancel_order(a, 9999, TS).unwrap_err().0, "找不到这笔委托");
    f.check();
}

#[test]
fn price_time_priority() {
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 0);
    let s1 = f.player("s1", 0, 50);
    let s2 = f.player("s2", 0, 50);
    f.seed_rest();
    f.limit(s1, "sell", 10, 505).unwrap();
    f.limit(s2, "sell", 10, 500).unwrap(); // better price, later
    f.limit(s1, "sell", 10, 500).unwrap(); // same price, even later
    f.limit(a, "buy", 15, 505).unwrap();
    let c = f.db();
    let trades: Vec<(i64, i64, i64)> = c
        .prepare("SELECT seller_id, price, qty FROM trades ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(trades, vec![(s2, 500, 10), (s1, 500, 5)]);
    f.check();
}

#[test]
fn market_buy_sweeps_and_cancels_rest() {
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 0);
    let b = f.player("b", 0, 100);
    f.seed_rest();
    f.limit(b, "sell", 5, 500).unwrap();
    f.limit(b, "sell", 5, 530).unwrap();
    f.limit(b, "sell", 5, 560).unwrap(); // 12% above the reference: the volatility control stops here
    let r = f.order_full(a, "buy", 20, None, "market", TS, true).unwrap();
    assert!(r.filled == 10 && r.status == OrderStatus::Cancelled);
    let c = f.e.company(1).unwrap();
    assert_eq!(cooling(c, TS), Some((450, 550)));
    assert_eq!(c.vcm_count, 1);
    f.check();
}

#[test]
fn rejects_absurd_price_and_overspend() {
    let mut f = Fx::new();
    let a = f.player("a", 1_000, 0);
    let b = f.player("b", 0, 10);
    f.seed_rest();
    assert!(f.limit(a, "buy", 1, 500 * 20 + 1).unwrap_err().0.contains("离谱"));
    assert!(f.limit(a, "buy", 10, 500).unwrap_err().0.contains("资金"));
    assert!(f.limit(b, "sell", 11, 500).unwrap_err().0.contains("股数"));
    f.limit(b, "sell", 10, 500).unwrap();
    assert!(f.limit(b, "sell", 1, 500).unwrap_err().0.contains("股数")); // already frozen by the open sell
}

#[test]
fn vcm_cools_then_anchors_on_the_open() {
    let mut f = Fx::new();
    let a = f.player("a", 1_000_000, 0);
    let b = f.player("b", 0, 200);
    f.seed_rest();
    f.limit(b, "sell", 10, 540).unwrap();
    f.limit(b, "sell", 10, 600).unwrap();
    let r = f.limit(a, "buy", 20, 650).unwrap(); // 5.40 fills, 6.00 is +20% from 5.00: cooling starts
    assert_eq!(r.filled, 10);
    let during = "2026-11-05T10:03:00+08:00";
    assert_eq!(f.order_full(a, "buy", 5, Some(650), "limit", during, true).unwrap().filled, 0);
    let after = "2026-11-05T10:06:00+08:00";
    // second trigger anchors on the day's open (5.00) at ±20%: 6.00 is exactly +20%, allowed
    assert_eq!(f.order_full(a, "buy", 10, Some(650), "limit", after, true).unwrap().filled, 10);
    f.limit(b, "sell", 10, 610).unwrap();
    assert_eq!(f.order_full(a, "buy", 10, Some(650), "limit", after, true).unwrap().filled, 0); // cools again
    let c = f.e.company(1).unwrap();
    assert_eq!(c.vcm_count, 2);
    assert_eq!(cooling(c, after), Some((400, 600)));
    f.check();
    let db = f.db();
    let news: Vec<(String, String)> = db
        .prepare("SELECT title, body FROM news WHERE kind='vcm' ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(news.len(), 2);
    assert_eq!(news[0].0, "《テスト》触发冷静期");
    assert_eq!(news[1].1, "成交价将偏离参考价 5.00 超过 20%，5 分钟内只能在 4.00 – 6.00 之间成交。");
}

#[test]
fn opening_auction_clears_at_one_price() {
    let mut f = Fx::new();
    let a = f.player("a", 1_000_000, 0);
    let c_ = f.player("c", 1_000_000, 0);
    let b = f.player("b", 0, 100);
    let d = f.player("d", 0, 100);
    f.seed_rest();
    f.order_full(a, "buy", 30, Some(520), "limit", TS, false).unwrap();
    f.order_full(c_, "buy", 20, Some(500), "limit", TS, false).unwrap();
    f.order_full(b, "sell", 25, Some(490), "limit", TS, false).unwrap();
    f.order_full(d, "sell", 30, Some(510), "limit", TS, false).unwrap();
    let n: i64 = f.db().query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0); // nothing matches while collecting
    assert_eq!(f.e.auction_price(1, 500), (Some(510), 30)); // 5.10: demand 30, supply 55 -> 30
    assert_eq!(f.e.run_auction(1, DAY, TS), Some(510));
    let db = f.db();
    let rows: Vec<(i64, i64)> = db
        .prepare("SELECT price, qty FROM trades")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(rows.iter().all(|r| r.0 == 510));
    assert_eq!(rows.iter().map(|r| r.1).sum::<i64>(), 30);
    let err = f.order_full(a, "buy", 1, None, "market", TS, false).unwrap_err();
    assert!(err.0.contains("集合竞价"));
    f.check();
}

#[test]
fn fifo_lots() {
    let mut f = Fx::new();
    let a = f.player("a", 0, 0);
    f.e.add_shares(a, 1, 10, 400, "2026-10-01");
    f.e.add_shares(a, 1, 10, 600, "2026-10-20");
    let taken = f.e.remove_shares(a, 1, 15, 0).unwrap();
    assert_eq!(taken, vec![(10, 400, "2026-10-01".to_string()), (5, 600, "2026-10-20".to_string())]);
    let left: Vec<(i64, i64)> = f.e.lots_of(a, 1).iter().map(|l| (l.qty, l.cost)).collect();
    assert_eq!(left, vec![(5, 600)]);
    let db = f.db();
    let rows: Vec<(i64, i64)> = db
        .prepare("SELECT qty, cost FROM lots WHERE account_id=?")
        .unwrap()
        .query_map([a], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows, vec![(5, 600)]);
}

#[test]
fn no_self_trade() {
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 50);
    f.seed_rest();
    f.limit(a, "sell", 10, 500).unwrap();
    let r = f.limit(a, "buy", 10, 500).unwrap();
    assert_eq!(r.filled, 0);
    f.check();
}

#[test]
fn engine_reload_matches_memory() {
    // state written through the persister loads back identically (books rebuilt in price-time order)
    let mut f = Fx::new();
    let a = f.player("a", 100_000, 0);
    let b = f.player("b", 0, 100);
    f.seed_rest();
    f.limit(b, "sell", 10, 505).unwrap();
    f.limit(b, "sell", 10, 500).unwrap();
    f.limit(a, "buy", 5, 490).unwrap();
    f.limit(a, "buy", 5, 495).unwrap();
    let before = f.e.book(1, 5);
    f.e.flush().unwrap();
    let path = f.dir.path().join("game.db");
    let e2 = anistock::Engine::open(&path, Fees::default(), anistock::Rules::default()).unwrap();
    assert_eq!(e2.book(1, 5), before);
    assert_eq!(e2.account(a), f.e.account(a));
    assert_eq!(e2.lots_of(b, 1), f.e.lots_of(b, 1));
}

#[test]
fn register_player_grants_in_one_operation() {
    let mut f = Fx::new();
    let id = f.e.register_player("新玩家", "salt:hash", 10_000_000, DAY, TS);
    let a = f.e.account(id).unwrap().clone();
    assert_eq!(
        (a.kind.as_str(), a.username.as_deref(), a.pw_hash.as_deref()),
        ("player", Some("新玩家"), Some("salt:hash"))
    );
    assert_eq!(a.cash, 10_000_000);
    let db = f.db();
    let (reason, amount): (String, i64) =
        db.query_row("SELECT reason, amount FROM ledger WHERE to_id=?", [id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((reason.as_str(), amount), ("grant", 10_000_000));
}

#[test]
fn a_rejected_order_rolls_back_the_opening_auction() {
    let mut f = Fx::new();
    let a = f.player("a", 1_000_000, 0);
    let b = f.player("b", 0, 100);
    f.seed_rest();
    f.order_full(a, "buy", 30, Some(520), "limit", TS, false).unwrap();
    f.order_full(b, "sell", 25, Some(490), "limit", TS, false).unwrap();
    let new = |qty| anistock::NewOrder {
        account_id: a,
        company_id: 1,
        side: "buy",
        kind: "limit",
        qty,
        price: Some(500),
        day: DAY,
        ts: "2026-11-05T09:00:05+08:00",
        match_: true,
    };
    // v1: ensure_opening_auction and the order share one transaction, so a rejected order undoes the auction
    assert_eq!(f.e.open_and_place(&new(0)).unwrap_err().0, "数量必须是正整数");
    assert_eq!(f.e.state("auction_day"), None);
    assert_eq!(f.e.open_orders_of(a).len(), 1);
    let n: i64 = f.db().query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    // the next valid order opens the market at its own timestamp
    f.e.open_and_place(&new(1)).unwrap();
    assert_eq!(f.e.state("auction_day"), Some(DAY));
    let db = f.db();
    let (n, ts): (i64, String) =
        db.query_row("SELECT COUNT(*), MIN(ts) FROM trades", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((n, ts.as_str()), (1, "2026-11-05T09:00:05+08:00"));
    assert_eq!(f.e.company(1).unwrap().open_ref, Some(490));
    f.check();
}

/// A sink whose database stopped accepting writes.
struct BrokenDisk;

impl anistock::store::persist::ChangeSink for BrokenDisk {
    fn send(&mut self, _changes: Vec<anistock::store::persist::Change>) {}
    fn flush(&mut self) -> Result<(), String> {
        Err("disk I/O error".into())
    }
    fn failure(&self) -> Option<String> {
        Some("disk I/O error".into())
    }
}

#[test]
#[should_panic(expected = "persistence failed")]
fn a_failed_write_stops_the_engine() {
    // memory must not run ahead of the database: the next operation panics (the actor then exits and
    // systemd restarts the server from the database)
    let mut e = anistock::Engine::from_snapshot(
        anistock::store::Snapshot::default(),
        Fees::default(),
        anistock::Rules::default(),
        Box::new(BrokenDisk),
    );
    e.create_account("world", "world", None, None, None, TS);
}
