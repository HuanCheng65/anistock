//! 破产重整 (src/restructure.rs): eligibility, the once-per-season limit, the atomic procedure, and the
//! chain-linked return on the 总榜.

mod common;

use anistock::api::v1::account::ranked;
use anistock::model::Company;
use anistock::restructure::{check, linked_return, Refusal};
use common::{Fx, DAY, TS};

const INITIAL: i64 = 10_000_000; // 100,000 币
const TH: f64 = 0.1;

fn ipo_company(fx: &mut Fx) {
    fx.e.insert_company(Company {
        id: 2,
        code: "100002".into(),
        title: "新股".into(),
        title_ja: None,
        title_en: None,
        studio: None,
        episodes: None,
        long_runner: 0,
        first_air: "2026-12-01".into(),
        listed_on: "2026-11-10".into(),
        status: "subscribing".into(),
        sub_start: Some("2026-11-01".into()),
        sub_end: Some("2026-11-09".into()),
        shares: 1000,
        ipo_price: 300,
        prev_close: 300,
        open_ref: None,
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 300,
        fair_value: 0,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
    });
}

#[test]
fn eligibility_threshold_counts_frozen_cash() {
    assert_eq!(check(999_999, INITIAL, TH, None, DAY), Ok(()));
    assert_eq!(check(1_000_000, INITIAL, TH, None, DAY), Err(Refusal::NotPoor(1_000_000)));
    assert_eq!(Refusal::NotPoor(1_000_000).message(), "总资产低于 10,000 币才能申请重整");

    let mut fx = Fx::new();
    // 9,000 币 available + 2,000 币 frozen in a resting buy = 11,000 币: not eligible
    let p = fx.player("frozen", 1_100_000, 0);
    // holdings count at the last price: 9,600 币 cash + 100 shares at 5 币 = 10,100 币, not eligible
    let q = fx.player("stock", 960_000, 100);
    fx.seed_rest();
    assert_eq!(fx.e.total_assets(q), 1_010_000);
    assert_eq!(fx.e.restructure(q, INITIAL, TH, DAY, TS), Err(Refusal::NotPoor(1_000_000)));
    fx.limit(p, "buy", 400, 500).unwrap();
    assert_eq!(fx.e.available_cash(p), 1_100_000 - fx.e.account(p).unwrap().cash_frozen);
    assert!(fx.e.account(p).unwrap().cash_frozen >= 200_000);
    assert_eq!(fx.e.total_assets(p), 1_100_000);
    assert_eq!(fx.e.restructure(p, INITIAL, TH, DAY, TS), Err(Refusal::NotPoor(1_000_000)));
    // nothing changed
    assert_eq!(fx.e.open_orders_of(p).len(), 1);
    // one more 币 out and it qualifies, the frozen cash included
    let world = fx.e.special("world").unwrap();
    fx.e.transfer(p, world, 100_001, "test", DAY, TS, None, None);
    assert!(fx.e.restructure(p, INITIAL, TH, DAY, TS).is_ok());
    fx.check();
}

#[test]
fn once_per_season_across_a_quarter_boundary() {
    assert_eq!(check(0, INITIAL, TH, Some("2026-10-01"), "2026-12-31"), Err(Refusal::ThisSeason));
    assert_eq!(check(0, INITIAL, TH, Some("2026-12-31"), "2027-01-01"), Ok(()));
    assert_eq!(check(0, INITIAL, TH, Some("2026-09-30"), "2026-10-01"), Ok(()));
    assert_eq!(Refusal::ThisSeason.message(), "本季已经重整过一次");

    let mut fx = Fx::new();
    let p = fx.player("p", 500_000, 0);
    fx.seed_rest();
    let world = fx.e.special("world").unwrap();
    let ts1 = "2026-12-31T10:00:00+08:00";
    fx.e.restructure(p, INITIAL, TH, "2026-12-31", ts1).unwrap();
    assert_eq!(fx.e.account(p).unwrap().cash, INITIAL);
    // poor again the same season
    fx.e.transfer(p, world, INITIAL - 300_000, "test", "2026-12-31", ts1, None, None);
    assert_eq!(fx.e.restructure(p, INITIAL, TH, "2026-12-31", ts1), Err(Refusal::ThisSeason));
    // the next season
    fx.e.restructure(p, INITIAL, TH, "2027-01-01", "2027-01-01T10:00:00+08:00").unwrap();
    assert_eq!(fx.e.player_return(p).unwrap().restructures, 2);
    // 5,000 / 100,000 then 3,000 / 100,000
    assert!((fx.e.player_return(p).unwrap().return_factor - 0.05 * 0.03).abs() < 1e-15);
    fx.check();
}

#[test]
fn the_procedure_is_atomic_and_zero_sum() {
    let mut fx = Fx::new();
    ipo_company(&mut fx);
    // 2,000 币 cash, 300 shares at 5 币 = 1,500 币; total 3,500 币
    let p = fx.player("poor", 200_000, 300);
    fx.seed_rest();
    let world = fx.e.special("world").unwrap();
    fx.limit(p, "buy", 10, 400).unwrap(); // freezes cash
    fx.limit(p, "sell", 100, 900).unwrap(); // freezes shares
    fx.e.subscribe(p, 2, 100, DAY, TS).unwrap(); // freezes cash
    assert!(fx.e.account(p).unwrap().cash_frozen > 0);
    let world_cash = fx.e.account(world).unwrap().cash;
    let world_shares = fx.e.holding(world, 1).map_or(0, |h| h.qty);
    fx.e.check_invariants().unwrap();

    let rec = fx.e.restructure(p, INITIAL, TH, DAY, TS).unwrap();
    assert_eq!(rec.assets_before, 350_000);
    assert_eq!(rec.sold, 150_000);
    assert_eq!(rec.injected, INITIAL - 350_000);

    let a = fx.e.account(p).unwrap();
    assert_eq!((a.cash, a.cash_frozen), (INITIAL, 0));
    assert!(fx.e.open_orders_of(p).is_empty());
    assert!(fx.e.holdings_of(p).next().is_none());
    assert!(fx.e.lots_of(p, 1).is_empty());
    assert!(fx.e.ipo_application(2, p).is_none());
    assert_eq!(fx.e.holding(world, 1).unwrap().qty, world_shares + 300);
    assert_eq!(fx.e.account(world).unwrap().cash, world_cash - (INITIAL - 200_000));
    fx.check(); // cash sums to zero, shares and lots match, in memory and in the database

    let c = fx.db();
    let (n, sum): (i64, i64) = c
        .query_row("SELECT COUNT(*), SUM(amount) FROM ledger WHERE reason='restructure' AND to_id=?", [p], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!((n, sum), (2, INITIAL - 200_000));
    let row: (i64, i64, i64, i64) = c
        .query_row("SELECT account_id, assets_before, sold, injected FROM restructures", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!(row, (p, 350_000, 150_000, INITIAL - 350_000));
    let pr: (f64, i64, i64, String) = c
        .query_row("SELECT return_factor, segment_base, restructures, last_day FROM player_returns", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!(pr, (0.035, INITIAL, 1, DAY.to_string()));
    let orders: i64 =
        c.query_row("SELECT COUNT(*) FROM orders WHERE account_id=? AND status='open'", [p], |r| r.get(0)).unwrap();
    assert_eq!(orders, 0);

    // reloaded from the database, the engine knows the restructuring
    let snap = anistock::store::load(&c).unwrap();
    assert_eq!(snap.player_returns[&p].restructures, 1);
}

#[test]
fn the_linked_return_keeps_the_loss() {
    // lose to 10k (-90%), restructure, then gain 10%: 0.1 * 1.1 - 1 = -89%
    assert!((linked_return(0.1, INITIAL, 11_000_000) - -0.89).abs() < 1e-12);

    let mut fx = Fx::new();
    let a = fx.player("a", 900_000, 0); // 9,000 币: -91%
    fx.player("b", 5_000_000, 0); // 50,000 币: -50%, never restructured
    fx.seed_rest();
    let world = fx.e.special("world").unwrap();
    fx.e.restructure(a, INITIAL, TH, DAY, TS).unwrap();
    // then +10% on the new segment
    fx.e.transfer(world, a, 1_000_000, "test", DAY, TS, None, None);
    let c = fx.db();
    let rows = ranked(&c, INITIAL, None).unwrap_or_else(|e| panic!("{}", e.message));
    let names: Vec<&str> = rows.iter().map(|r| r.user.username.as_str()).collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    // a has more assets (110,000 vs 50,000) but ranks below b
    assert!(pos("b") < pos("a"), "{names:?}");
    assert_eq!(rows[pos("b")].rank, pos("b") as i64 + 1);
    let ra = &rows[pos("a")];
    // the API rounds ratios to 6 decimals
    assert!((ra.r#return - (0.09 * 1.1 - 1.0)).abs() < 1e-6, "{}", ra.r#return);
    assert_eq!(ra.restructures, 1);
    assert_eq!(rows[pos("b")].restructures, 0);
    assert_eq!(rows[pos("b")].r#return, -0.5);
}
