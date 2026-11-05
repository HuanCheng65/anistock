#![allow(dead_code)]
//! The fixture of tests/test_exchange.py: a fresh database with world/cb/gov and one listed company.

use anistock::engine::NewOrder;
use anistock::model::Company;
use anistock::{store, Engine, Fees, OrderError, Placed, Rules};
use rusqlite::Connection;

pub const DAY: &str = "2026-11-05";
pub const TS: &str = "2026-11-05T10:00:00+08:00";

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub e: Engine,
}

impl Fx {
    pub fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.db");
        store::migrate(&path).unwrap();
        let c = store::connect(&path).unwrap();
        for kind in ["world", "cb", "gov"] {
            c.execute("INSERT INTO accounts(kind, name, created_at) VALUES (?,?,?)", [kind, kind, TS]).unwrap();
        }
        drop(c);
        let mut e = Engine::open(&path, Fees::default(), Rules::default()).unwrap();
        // (inserted after loading: with no holders yet it would fail the start-up invariant check)
        e.insert_company(Company {
            id: 1,
            code: "100001".into(),
            title: "テスト".into(),
            title_ja: None,
            title_en: None,
            studio: None,
            episodes: None,
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
            profit_week: 0,
            low_weeks: 0,
            delisted_on: None,
        });
        Fx { dir, e }
    }

    pub fn db(&mut self) -> Connection {
        self.e.flush().unwrap();
        store::connect(&self.dir.path().join("game.db")).unwrap()
    }

    pub fn player(&mut self, name: &str, cash: i64, shares: i64) -> i64 {
        let pid = self.e.create_account("player", name, Some(name), None, None, TS);
        if cash != 0 {
            let world = self.e.special("world").unwrap();
            self.e.transfer(world, pid, cash, "grant", DAY, TS, None, None);
        }
        if shares != 0 {
            self.e.add_shares(pid, 1, shares, 500, "2026-10-01");
        }
        pid
    }

    /// Give the remaining shares to an inert holder so the share-count invariant holds.
    pub fn seed_rest(&mut self) {
        let held: i64 = self
            .e
            .accounts()
            .map(|a| a.id)
            .collect::<Vec<_>>()
            .iter()
            .filter_map(|a| self.e.holding(*a, 1))
            .map(|h| h.qty)
            .sum();
        let holder = self.player("holder", 0, 0);
        self.e.add_shares(holder, 1, 1000 - held, 500, "2026-09-10");
    }

    pub fn order_full(
        &mut self,
        acct: i64,
        side: &str,
        qty: i64,
        price: Option<i64>,
        kind: &str,
        ts: &str,
        m: bool,
    ) -> Result<Placed, OrderError> {
        self.e.place_order(&NewOrder {
            account_id: acct,
            company_id: 1,
            side,
            kind,
            qty,
            price,
            day: DAY,
            ts,
            match_: m,
        })
    }

    pub fn limit(&mut self, acct: i64, side: &str, qty: i64, price: i64) -> Result<Placed, OrderError> {
        self.order_full(acct, side, qty, Some(price), "limit", TS, true)
    }

    pub fn check(&mut self) {
        self.e.check_invariants().unwrap();
        let c = self.db();
        store::check_invariants(&c).unwrap();
    }
}
