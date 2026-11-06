//! Port of tests/test_migrations.py, plus the v1 (yoyo) baseline.

use anistock::store;

#[test]
fn fresh_database_gets_every_migration_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    let applied = store::migrate(&path).unwrap();
    assert_eq!(applied.first().map(String::as_str), Some("V1__initial"));
    assert!(store::migrate(&path).unwrap().is_empty());
    let c = store::connect(&path).unwrap();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type='table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for t in ["accounts", "companies", "orders", "trades", "ledger", "lots", "ipo_applications"] {
        assert!(tables.iter().any(|x| x == t), "missing {t}");
    }
}

#[test]
fn backup_is_a_usable_copy() {
    let dir = tempfile::tempdir().unwrap();
    store::migrate(&dir.path().join("g.db")).unwrap();
    let c = store::connect(&dir.path().join("g.db")).unwrap();
    c.execute("INSERT INTO state(key, value) VALUES ('x', '1')", []).unwrap();
    let copy = store::backup(&c, &dir.path().join("backups").join("copy.db")).unwrap();
    let c2 = store::connect(&copy).unwrap();
    let v: String = c2.query_row("SELECT value FROM state WHERE key='x'", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "1");
}

#[test]
fn migration_file_is_v1s() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let ours = std::fs::read(root.join("migrations/V1__initial.sql")).unwrap();
    let v1 = std::fs::read(root.join("../server/migrations/001_initial.sql")).unwrap();
    assert_eq!(ours, v1, "backend/migrations/V1__initial.sql must stay identical to server/migrations/001_initial.sql");
}

#[test]
fn yoyo_database_is_baselined_not_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    {
        // what v1's yoyo leaves behind
        let c = store::connect(&path).unwrap();
        c.execute_batch(include_str!("../migrations/V1__initial.sql")).unwrap();
        c.execute_batch(
            "CREATE TABLE _yoyo_migration (migration_hash VARCHAR(64), migration_id VARCHAR(255),
                                           applied_at_utc TIMESTAMP, PRIMARY KEY (migration_hash));
             INSERT INTO _yoyo_migration VALUES ('abc', '001_initial', '2026-10-01 00:00:00');
             INSERT INTO state(key, value) VALUES ('keep', 'me');",
        )
        .unwrap();
    }
    // V1 is baselined (not run); the later migrations run, after a backup
    assert_eq!(
        store::migrate(&path).unwrap(),
        ["V2__valuation_state", "V3__valuation_joint", "V4__player_restructure", "V5__bindings", "V6__player_days", "V7__margin", "V8__margin_collateral", "V9__company_works", "V10__corporate"]
            .map(String::from)
            .to_vec()
    );
    assert!(store::migrate(&path).unwrap().is_empty());
    let c = store::connect(&path).unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM refinery_schema_history WHERE version=1", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    let v: String = c.query_row("SELECT value FROM state WHERE key='keep'", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "me");
    assert!(dir.path().join("backups").exists());
}

#[test]
fn yoyo_table_without_001_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    let c = store::connect(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE _yoyo_migration (migration_hash VARCHAR(64), migration_id VARCHAR(255),
                                                   applied_at_utc TIMESTAMP)",
    )
    .unwrap();
    drop(c);
    assert!(store::migrate(&path).is_err());
}

#[test]
fn refuses_to_load_broken_invariants() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.db");
    store::migrate(&path).unwrap();
    let c = store::connect(&path).unwrap();
    c.execute("INSERT INTO accounts(kind, name, cash, created_at) VALUES ('player', 'x', 5, 't')", []).unwrap();
    drop(c);
    let r = anistock::Engine::open(&path, anistock::Fees::default(), anistock::Rules::default());
    assert!(matches!(r, Err(store::StoreError::Invariant(_))));
}
