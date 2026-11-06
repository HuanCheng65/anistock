//! Market indices (docs/api-v1.md §4.1.4): point-in-time members (a delisted stock keeps its history), the
//! total-return basis, and splits taken out with the 前复权 factors.

use anistock::api::v1::market::{compute_indices, IndexBasis};
use anistock::store;

/// Two stocks of 1,000 shares on four days. A: 100, 110, then a 2-for-1 split effective on day 3 (55, 55; 2,000
/// shares now), a dividend of 11 cents per (new) share on day 4. B: 100, 100, 100, delisted on day 4 (close
/// 90) returning 80 cents per share.
fn world() -> (tempfile::TempDir, rusqlite::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("g.db");
    store::migrate(&path).unwrap();
    let c = store::connect(&path).unwrap();
    c.execute_batch(
        "INSERT INTO companies(id, code, title, first_air, listed_on, status, shares, ipo_price, prev_close, last_price, delisted_on)
           VALUES (1, '100001', 'A', '2026-07-01', '2026-06-20', 'listed', 2000, 100, 55, 55, NULL),
                  (2, '100002', 'B', '2026-07-01', '2026-06-20', 'delisted', 1000, 100, 100, 90, '2026-11-05');
         INSERT INTO candles(company_id, day, open, high, low, close) VALUES
           (1, '2026-11-02', 100, 100, 100, 100), (1, '2026-11-03', 110, 110, 110, 110),
           (1, '2026-11-04', 55, 55, 55, 55), (1, '2026-11-05', 55, 55, 55, 55),
           (2, '2026-11-02', 100, 100, 100, 100), (2, '2026-11-03', 100, 100, 100, 100),
           (2, '2026-11-04', 100, 100, 100, 100), (2, '2026-11-05', 90, 90, 90, 90);
         INSERT INTO price_adjustments(company_id, day, price_factor, qty_factor, action_id) VALUES (1, '2026-11-04', 0.5, 2.0, 1);
         INSERT INTO company_days(company_id, day, revenue, rev_us, rev_cn, rev_jp, aired, dividend_ps)
           VALUES (1, '2026-11-05', 0, 0, 0, 0, 12, 11);
         INSERT INTO accounts(id, kind, name, company_id, created_at) VALUES (50, 'company', 'B', 2, 't'), (51, 'player', 'p', NULL, 't');
         INSERT INTO ledger(ts, day, from_id, to_id, amount, reason, company_id) VALUES ('t', '2026-11-05', 50, 51, 80000, 'delisting', 2);",
    )
    .unwrap();
    (dir, c)
}

fn levels(c: &rusqlite::Connection, basis: IndexBasis) -> (Vec<f64>, i64) {
    let r = compute_indices(c, "2026-11-05", true, basis).unwrap();
    assert_eq!(r.items.len(), 1, "only the composite has members");
    let all = &r.items[0];
    (all.series.iter().map(|p| p.value).collect(), all.members)
}

#[test]
fn price_index_keeps_delisted_history_and_ignores_the_split() {
    let (_d, c) = world();
    let (v, members) = levels(&c, IndexBasis::Price);
    // day 2: A +10% on half the weight; day 3: the split moves nothing; day 4: B −10% on its 100k of 210k
    assert_eq!(v, vec![1000.0, 1050.0, 1050.0, 1000.0]);
    assert_eq!(members, 1, "listed today");
}

#[test]
fn total_return_adds_dividends_and_the_delisting_payout() {
    let (_d, c) = world();
    let (v, _) = levels(&c, IndexBasis::TotalReturn);
    // day 4: A (55 + 11) × 2,000 against 110,000; B returns 80 × 1,000 against 100,000
    let want = 1050.0 * (66.0 * 2000.0 + 80.0 * 1000.0) / (110_000.0 + 100_000.0);
    assert_eq!(&v[..3], &[1000.0, 1050.0, 1050.0]);
    assert!((v[3] - want).abs() < 1e-3, "{} vs {want}", v[3]);
}
