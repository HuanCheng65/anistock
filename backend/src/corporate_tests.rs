//! Corporate actions on an in-memory engine (docs/v2-corporate.md §3): a split with lots and a short position,
//! a rights issue with a subscriber, a lapsed holder and a short seller, a buyback through the book; and the
//! public schedule's upcoming episodes.

use super::*;
use crate::margin::Mode;
use crate::model::Company;
use crate::vclose::WorkMeta;
use crate::{Fees, Rules};

const TS: &str = "2026-11-05T10:00:00+08:00";
const DAY: &str = "2026-11-05";

fn company(price: i64) -> Company {
    Company {
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
        ipo_price: price,
        prev_close: price,
        open_ref: None,
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: price,
        fair_value: price,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
        source: None,
        genres: String::new(),
    }
}

fn player(e: &mut Engine, name: &str, cash: i64) -> i64 {
    let id = e.create_account("player", name, Some(name), None, None, "2026-10-01T10:00:00+08:00");
    let world = e.special("world").unwrap();
    e.transfer(world, id, cash, "grant", DAY, TS, None, None);
    id
}

struct W {
    e: Engine,
    acct: i64,
    committee: i64,
    seller: i64,
    buyer: i64,
    p: i64,
}

/// Company 1, 1,000 shares at `price`: the rights holders 600, a seller 400 (two lots); a buyer with cash; player
/// `p` with credit, short 100 (borrowed from the rights holders, sold to the buyer).
fn world(price: i64) -> W {
    let rules = Rules { vcm_threshold: 1000.0, ..Rules::default() };
    let (mut e, _) = Engine::in_memory(Fees::default(), rules);
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    e.set_state("rate", "0.03");
    e.set_state("auction_day", DAY);
    e.insert_company(company(price));
    let acct = e.create_account("company", "company", None, None, Some(1), TS);
    let committee = e.create_account("committee", "committee", None, None, Some(1), TS);
    e.add_shares(committee, 1, 600, price, "2026-09-10");
    let seller = player(&mut e, "seller", 0);
    e.add_shares(seller, 1, 250, price, "2026-09-10");
    e.add_shares(seller, 1, 150, price, "2026-10-10");
    let buyer = player(&mut e, "buyer", 100_000_000);
    let p = player(&mut e, "p", 10_000_000);
    e.margin.stock_cap = 1.0;
    e.open_credit(p, 0, DAY, TS).unwrap_or_else(|err| panic!("{err}"));
    e.place_order(&NewOrder { account_id: buyer, company_id: 1, side: "buy", kind: "limit", qty: 100, price: Some(price), day: DAY, ts: TS, match_: true })
        .unwrap();
    e.place_credit(&NewOrder { account_id: p, company_id: 1, side: "sell", kind: "limit", qty: 100, price: Some(price), day: DAY, ts: TS, match_: true }, Mode::ShortSell)
        .unwrap();
    assert_eq!(e.short_qty(p, 1), 100);
    ok(&e);
    W { e, acct, committee, seller, buyer, p }
}

fn ok(e: &Engine) {
    e.check_invariants().unwrap_or_else(|err| panic!("{err}"));
}

fn day(n: i64) -> String {
    day_add(DAY, n)
}

#[test]
fn split_after_five_closes_rescales_everything() {
    let mut w = world(15_000); // 150 币
    // a financed buyer: 50 shares bought on margin from the seller
    let q = player(&mut w.e, "q", 10_000_000);
    w.e.open_credit(q, 0, DAY, TS).unwrap_or_else(|err| panic!("{err}"));
    w.e.place_order(&NewOrder { account_id: w.seller, company_id: 1, side: "sell", kind: "limit", qty: 50, price: Some(15_000), day: DAY, ts: TS, match_: true })
        .unwrap();
    w.e.place_credit(&NewOrder { account_id: q, company_id: 1, side: "buy", kind: "limit", qty: 50, price: Some(15_000), day: DAY, ts: TS, match_: true }, Mode::MarginBuy)
        .unwrap();
    assert_eq!(w.e.st.margin.debts[&(q, 1)].qty, 50);
    let principal = w.e.st.margin.debts[&(q, 1)].principal;
    let value = |e: &Engine, a: i64| e.holding(a, 1).map_or(0, |h| h.qty) * e.company(1).unwrap().prev_close;
    let before = (value(&w.e, w.seller), value(&w.e, w.committee), value(&w.e, w.buyer));
    for i in 0..4 {
        w.e.corporate_close(&day(i), TS, false, None).unwrap();
    }
    assert!(w.e.open_action(1, "split").is_none());
    w.e.corporate_close(&day(4), TS, false, None).unwrap();
    let a = w.e.open_action(1, "split").unwrap().clone();
    // one listed stock: the market's median is its own price, the smallest ratio is closest
    assert_eq!(a.params.k, 2);
    assert_eq!(a.effective, day(11));
    // applied at the close before its first day
    for i in 5..10 {
        w.e.corporate_close(&day(i), TS, false, None).unwrap();
        assert_eq!(w.e.company(1).unwrap().shares, 1000);
    }
    w.e.corporate_close(&day(10), TS, false, None).unwrap();
    w.e.commit();
    let c = w.e.company(1).unwrap().clone();
    assert_eq!((c.shares, c.prev_close, c.last_price, c.fair_value), (2000, 7500, 7500, 7500));
    assert_eq!(w.e.holding(w.seller, 1).unwrap().qty, 700);
    let lots: Vec<(i64, i64)> = w.e.lots_of(w.seller, 1).iter().map(|l| (l.qty, l.cost)).collect();
    assert_eq!(lots, vec![(400, 7500), (300, 7500)]);
    assert_eq!(w.e.short_qty(w.p, 1), 200);
    // the financing contract bought twice as many (new) shares for the same amount
    assert_eq!((w.e.st.margin.debts[&(q, 1)].qty, w.e.st.margin.debts[&(q, 1)].principal), (100, principal));
    assert_eq!(w.e.holding(q, 1).unwrap().qty, 100);
    assert_eq!((value(&w.e, w.seller), value(&w.e, w.committee), value(&w.e, w.buyer)), before);
    assert_eq!(w.e.adjust_factor(1, &day(10)), (0.5, 2.0));
    assert_eq!(w.e.adjust_factor(1, &day(11)), (1.0, 1.0));
    assert!(w.e.open_action(1, "split").is_none());
    ok(&w.e);
}

#[test]
fn rights_issue_subscribers_lapsed_holders_and_shorts() {
    let mut w = world(1000); // 10 币
    let world_acct = w.e.special("world").unwrap();
    // the company holds 2,000 币 and needs 5,000 币 for a sequel: 3,000 short, rights at 8 币
    w.e.transfer(world_acct, w.acct, 200_000, "grant", DAY, TS, Some(1), None);
    let id = w.e.new_action(1, "rights", DAY, &day(3), "pending", Params { ratio: 0.3, price: 800, budget: 500_000, sequel: 9, work: 1, ..Default::default() });
    w.e.commit();
    // the buyer holds 100 (bought from the short seller): 30 rights; subscribes for 50, capped to 30 at the record
    assert_eq!(w.e.rights_entitlement(id, w.buyer), 30);
    assert!(w.e.rights_subscribe(w.buyer, 1, 50, TS).is_err());
    assert_eq!(w.e.rights_subscribe(w.buyer, 1, 30, TS).unwrap(), 24_000);
    w.e.add_shares(w.buyer, 1, 10, 1000, DAY); // (bought more: still subscribed for 30)
    w.e.remove_shares(w.buyer, 1, 10, 0).unwrap();
    let cash = |e: &Engine, a: i64| e.account(a).unwrap().cash;
    let (b0, s0, p0) = (cash(&w.e, w.buyer), cash(&w.e, w.seller), cash(&w.e, w.p));
    for i in 0..3 {
        w.e.corporate_close(&day(i), TS, false, None).unwrap();
    }
    let f = w.e.corporate_close(&day(3), TS, false, None).unwrap();
    w.e.commit();
    ok(&w.e);
    let a = w.e.corporate_actions(1)[0].clone();
    assert_eq!(a.status, "done");
    // holders: committee 500 (600 less 100 lent) → 150, seller 400 → 120, buyer 100 → 30: 300 new shares
    assert_eq!(a.params.issued, 300);
    assert_eq!(w.e.company(1).unwrap().shares, 1300);
    // TERP = (1000 × 10 + 300 × 8) / 1300 = 9.54
    assert_eq!(a.params.terp, 954);
    assert_eq!(w.e.company(1).unwrap().prev_close, 954);
    assert_eq!(w.e.holding(w.buyer, 1).unwrap().qty, 130);
    assert_eq!(cash(&w.e, w.buyer), b0 - 30 * 800);
    // the seller lapsed: 120 × (9.54 − 8) in cash, no new shares
    assert_eq!(w.e.holding(w.seller, 1).unwrap().qty, 400);
    assert_eq!(cash(&w.e, w.seller), s0 + 120 * 154);
    // the short seller owes the lender 30 rights' worth
    assert_eq!(cash(&w.e, w.p), p0 - 30 * 154);
    // the rights holders took their 150 and the seller's 120
    assert_eq!(w.e.holding(w.committee, 1).unwrap().qty, 500 + 150 + 120);
    // production cost: the company's 2,000 + 300 × 8 raised = 4,400 < 5,000: all of it paid
    assert_eq!(a.params.paid, 440_000);
    assert_eq!(w.e.account(w.acct).unwrap().cash, 0);
    assert_eq!(f.exported, 440_000 + 30 * 154);
    assert!((w.e.adjust_factor(1, &day(3)).0 - 954.0 / 1000.0).abs() < 1e-12);
}

#[test]
fn rights_issue_refunds_on_delisting() {
    let mut w = world(1000);
    let id = w.e.new_action(1, "rights", DAY, &day(3), "pending", Params { ratio: 0.3, price: 800, budget: 100, ..Default::default() });
    w.e.commit();
    w.e.rights_subscribe(w.buyer, 1, 30, TS).unwrap();
    assert_eq!(w.e.rights_sub(id, w.buyer).unwrap().frozen, 24_000);
    w.e.corporate_on_delist(1, TS);
    w.e.commit();
    assert!(w.e.rights_sub(id, w.buyer).is_none());
    assert_eq!(w.e.account(w.buyer).unwrap().cash_frozen, 0);
    assert_eq!(w.e.corporate_actions(1)[0].status, "cancelled");
    ok(&w.e);
}

#[test]
fn buyback_bids_and_cancels_bought_shares() {
    let mut w = world(1000);
    let world_acct = w.e.special("world").unwrap();
    w.e.transfer(world_acct, w.acct, 50_000, "grant", DAY, TS, Some(1), None);
    w.e.st.corp.volumes.insert(1, std::collections::VecDeque::from(vec![400; 20]));
    w.e.st.companies.get_mut(&1).unwrap().fair_value = 1200;
    w.e.new_action(1, "buyback", DAY, &day(91), "active", Params { budget: 50_000, cap: 1200, ..Default::default() });
    // a bid from the buyer at 9.90: the desk joins at the best bid, at most 25% of 400 a day over the slots left
    w.e.place_order(&NewOrder { account_id: w.buyer, company_id: 1, side: "buy", kind: "limit", qty: 10, price: Some(990), day: DAY, ts: TS, match_: true }).unwrap();
    w.e.corporate_minute(DAY, TS, 10 * 3600);
    let mine: Vec<(i64, i64)> = w.e.open_orders_of(w.acct).iter().map(|o| (o.price.unwrap(), o.qty)).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].0, 990);
    // the same slot does nothing more
    w.e.corporate_minute(DAY, TS, 10 * 3600 + 60);
    assert_eq!(w.e.open_orders_of(w.acct).len(), 1);
    // the seller hits it
    let q = mine[0].1;
    w.e.place_order(&NewOrder { account_id: w.seller, company_id: 1, side: "sell", kind: "limit", qty: 10 + q, price: Some(990), day: DAY, ts: TS, match_: true }).unwrap();
    assert_eq!(w.e.holding(w.acct, 1).unwrap().qty, q);
    // cancelled at the close
    w.e.corporate_pre_close(TS);
    w.e.commit();
    assert_eq!(w.e.company(1).unwrap().shares, 1000 - q);
    assert!(w.e.holding(w.acct, 1).is_none());
    let a = w.e.open_action(1, "buyback").unwrap();
    assert_eq!((a.params.bought, a.params.spent), (q, q * 990));
    ok(&w.e);
}

fn meta(schedule: &[i64], next: Option<(i64, i64)>, hiatus: Option<i64>) -> WorkMeta {
    WorkMeta { first_air: 0, episodes: Some(6), long: false, air: vec![], schedule: schedule.to_vec(), next_airing: next, hiatus }
}

#[test]
fn upcoming_episodes_follow_the_public_schedule() {
    let aired = [0, 7, 14];
    // weekly without a schedule
    assert_eq!(meta(&[], None, None).upcoming(&aired, Some(6)), vec![21, 28, 35]);
    // a break known in advance
    assert_eq!(meta(&[0, 7, 14, 28, 35], None, None).upcoming(&aired, Some(6)), vec![28, 35, 42]);
    // AniList dates the next episode a week later than Bangumi: the rest moves with it
    assert_eq!(meta(&[0, 7, 14, 21, 28], Some((4, 28)), None).upcoming(&aired, Some(6)), vec![28, 35, 42]);
    // a one-day difference is the late-night date convention, not a delay
    assert_eq!(meta(&[0, 7, 14, 21, 28], Some((4, 22)), None).upcoming(&aired, Some(6)), vec![21, 28, 35]);
    // a halt: resumes at its day
    assert_eq!(meta(&[0, 7, 14, 21], None, Some(300)).upcoming(&aired, Some(6)), vec![300, 307, 314]);
}

#[test]
fn per_ten_copy() {
    assert_eq!(per_ten(0.3), "3");
    assert_eq!(per_ten(0.25), "2.5");
    assert_eq!(per_ten(0.07), "0.7");
}
