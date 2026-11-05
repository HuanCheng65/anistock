//! The one-time `player_days` backfill (migration V6): written when the replay reproduces the accounts,
//! skipped (nothing written) when it does not.

mod common;

use anistock::store::{self, player_days::{backfill, Backfill}};
use common::{Fx, DAY, TS};

const INITIAL: i64 = 10_000_000;

#[test]
fn backfill_replays_cash_and_trades() {
    let mut fx = Fx::new();
    let a = fx.player("a", INITIAL, 0);
    let b = fx.player("b", INITIAL, 0);
    // the float sits with a non-player holder (players only get shares through trades and allotments)
    let holder = fx.e.create_account("committee", "committee", None, None, Some(1), TS);
    fx.e.add_shares(holder, 1, 1000, 500, "2026-09-10");
    // b buys 10 from the holder: one trade on DAY
    fx.limit(holder, "sell", 10, 500).unwrap();
    fx.limit(b, "buy", 10, 500).unwrap();
    fx.e.set_state("last_closed", DAY);
    let mut c = fx.db();
    c.execute("INSERT INTO candles(company_id, day, open, high, low, close, volume, source) VALUES (1, ?, 500, 500, 500, 520, 10, 'live')", [DAY])
        .unwrap();
    match backfill(&mut c, INITIAL, "04:00").unwrap() {
        Backfill::Written { players, rows, .. } => {
            assert_eq!(players, 2);
            assert!(rows >= 2, "{rows}");
        }
        Backfill::Skipped(why) => panic!("skipped: {why}"),
    }
    let row = |id: i64| -> (i64, i64, f64) {
        c.query_row("SELECT cash, stock_value, return_index FROM player_days WHERE account_id=? AND day=?", rusqlite::params![id, DAY], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap()
    };
    let (cash_a, sv_a, idx_a) = row(a);
    assert_eq!((cash_a, sv_a), (INITIAL, 0));
    assert!((idx_a - 1.0).abs() < 1e-12);
    let (cash_b, sv_b, _) = row(b);
    let now_b: i64 = c.query_row("SELECT cash FROM accounts WHERE id=?", [b], |r| r.get(0)).unwrap();
    assert_eq!(cash_b, now_b);
    assert_eq!(sv_b, 10 * 520, "valued at the day's close");
    // a second run does nothing
    assert!(matches!(backfill(&mut c, INITIAL, "04:00").unwrap(), Backfill::Skipped(_)));
}

#[test]
fn backfill_gives_up_when_the_replay_does_not_match() {
    let mut fx = Fx::new();
    // shares without a trade, allotment or any ledger row: the replay cannot explain them
    fx.player("a", INITIAL, 50);
    fx.seed_rest();
    fx.e.set_state("last_closed", DAY);
    let mut c = fx.db();
    match backfill(&mut c, INITIAL, "04:00").unwrap() {
        Backfill::Skipped(why) => assert!(why.contains("holdings"), "{why}"),
        other => panic!("{other:?}"),
    }
    let n: i64 = c.query_row("SELECT COUNT(*) FROM player_days", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    let _ = store::migrate;
}
