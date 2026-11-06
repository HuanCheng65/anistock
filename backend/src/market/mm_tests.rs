//! The designated market maker selling borrowed stock (docs/v2-npc-market.md §11, docs/v2-margin.md §8):
//! it keeps offering at zero inventory, covers first on the way down, stays within its bound, and pays the
//! lending fee and manufactured dividends like any short seller.

use super::*;
use crate::margin::Mode;
use crate::model::{Company, CompanyDay};
use crate::store::persist::{Change, MemorySink};
use crate::{Fees, Rules};

const DAY: &str = "2026-11-05";
const C: i64 = 1;

struct W {
    e: Engine,
    sink: MemorySink,
    m: Market,
    mm: i64,
    committee: i64,
    holder: i64,
    buyer: i64,
}

fn game() -> crate::config::Config {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    crate::config::Config::load_from(&root, |_| None).unwrap()
}

/// The live incident's shape: 10,000 shares at 9 币 (value 9 币), the rights holders 5,500, NPC holders 4,500,
/// the market maker none (it sold its inventory) with 50,000 币. The game's `[market]` and `[margin]` settings.
fn world() -> W {
    let rules = Rules { vcm_threshold: 1000.0, ..Rules::default() };
    let (mut e, sink) = Engine::in_memory(Fees::default(), rules);
    let ts0 = "2026-09-10T08:00:00+08:00";
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, ts0);
    }
    let game = game();
    e.margin = game.margin.clone();
    e.set_state("rate", "0.03");
    e.set_state("auction_day", DAY);
    e.insert_company(Company {
        id: C,
        code: "100010".into(),
        title: "侦探已经死了。第二季".into(),
        title_ja: None,
        title_en: None,
        studio: None,
        episodes: Some(12),
        long_runner: 0,
        first_air: "2026-07-01".into(),
        listed_on: "2026-06-10".into(),
        status: "listed".into(),
        sub_start: None,
        sub_end: None,
        shares: 10_000,
        ipo_price: 400,
        prev_close: 900,
        open_ref: Some(900),
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 900,
        fair_value: 900,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
    });
    let world = e.special("world").unwrap();
    e.create_account("company", "company", None, None, Some(C), ts0);
    let committee = e.create_account("committee", "committee", None, None, Some(C), ts0);
    e.add_shares(committee, C, 5_500, 400, "2026-06-10");
    let mm = e.create_account("mm", "mm", None, None, Some(C), ts0);
    e.transfer(world, mm, 5_000_000, "grant", DAY, ts0, None, None);
    let holder = e.create_account("npc", "holder", None, None, None, ts0);
    e.add_shares(holder, C, 4_500, 400, "2026-06-10");
    let buyer = e.create_account("player", "buyer", Some("buyer"), None, None, ts0);
    e.transfer(world, buyer, 100_000_000, "grant", DAY, ts0, None, None);
    let mut mc = game.market.clone();
    mc.n_value = 0;
    mc.n_trend = 0;
    mc.n_news = 0;
    mc.n_retail = 0;
    let mut m = Market::new(mc, &game);
    m.refresh_universe(&e, at_local(DAY, 9, 0));
    W { e, sink, m, mm, committee, holder, buyer }
}

fn t(h: i64, min: i64) -> i64 {
    at_local(DAY, h, min)
}

fn ok(e: &Engine) {
    e.check_invariants().unwrap_or_else(|err| panic!("{err}"));
}

/// The market maker's open orders: (side is buy, mode, qty left, price).
fn mm_orders(w: &W) -> Vec<(bool, Option<Mode>, i64, i64)> {
    w.e.open_orders_of(w.mm).iter().map(|o| (o.side == crate::model::Side::Buy, w.e.margin_mode(o.id), o.left(), o.price.unwrap_or(0))).collect()
}

fn lift_offers(w: &mut W, ts: &str) -> i64 {
    let asks: i64 = mm_orders(w).iter().filter(|o| !o.0).map(|o| o.2).sum();
    if asks == 0 {
        return 0;
    }
    let r = w
        .e
        .place_order(&NewOrder { account_id: w.buyer, company_id: C, side: "buy", kind: "limit", qty: asks, price: Some(2_000), day: DAY, ts, match_: true })
        .unwrap();
    w.e.cancel_all(w.buyer, C, ts);
    r.filled
}

fn hit_bids(w: &mut W, ts: &str) -> i64 {
    let bids: i64 = mm_orders(w).iter().filter(|o| o.0).map(|o| o.2).sum();
    let have = w.e.available_shares(w.holder, C);
    let r = w
        .e
        .place_order(&NewOrder { account_id: w.holder, company_id: C, side: "sell", kind: "limit", qty: bids.min(have), price: Some(1), day: DAY, ts, match_: true })
        .unwrap();
    w.e.cancel_all(w.holder, C, ts);
    r.filled
}

#[test]
fn at_zero_inventory_it_borrows_and_keeps_offering() {
    let mut w = world();
    w.m.act_mm(&mut w.e, C, t(10, 0));
    let orders = mm_orders(&w);
    let asks: Vec<_> = orders.iter().filter(|o| !o.0).collect();
    assert!(!asks.is_empty(), "no offer at zero inventory: {orders:?}");
    assert!(asks.iter().all(|o| o.1 == Some(Mode::ShortSell)), "{orders:?}");
    // bids are ordinary (it owed nothing when they went in), all below the offers
    let bids: Vec<_> = orders.iter().filter(|o| o.0).collect();
    assert!(!bids.is_empty() && bids.iter().all(|o| o.1.is_none()));
    assert!(bids.iter().map(|o| o.3).max() < asks.iter().map(|o| o.3).min());
    // the borrowed shares came from the rights holders and rest in its sell orders
    let borrowed: i64 = asks.iter().map(|o| o.2).sum();
    assert_eq!(w.e.short_qty(w.mm, C), borrowed);
    assert_eq!(w.e.holding(w.committee, C).unwrap().qty, 5_500 - borrowed);
    ok(&w.e);

    // a buyer takes the offers; the market maker is short and offers again at the next quote
    let filled = lift_offers(&mut w, "2026-11-05T10:01:00+08:00");
    assert_eq!(filled, borrowed);
    w.m.act_mm(&mut w.e, C, t(10, 5));
    // its position is signed: the borrowed shares resting in its offers are held and owed
    assert_eq!(w.e.holding(w.mm, C).map_or(0, |h| h.qty) - w.e.short_qty(w.mm, C), -filled);
    let asks2: i64 = mm_orders(&w).iter().filter(|o| !o.0).map(|o| o.2).sum();
    assert!(asks2 > 0, "stopped offering once short");
    // the size skew runs on through zero: shorter, it offers less
    assert!(asks2 < borrowed, "{asks2} vs {borrowed}");
    // every bid is now a buy to cover, at most what it owes
    let cover: i64 = mm_orders(&w).iter().filter(|o| o.0).map(|o| { assert_eq!(o.1, Some(Mode::BuyReturn)); o.2 }).sum();
    assert!(cover > 0 && cover <= filled);
    ok(&w.e);
}

#[test]
fn on_the_way_down_it_covers_first() {
    let mut w = world();
    w.m.act_mm(&mut w.e, C, t(10, 0));
    let sold = lift_offers(&mut w, "2026-11-05T10:01:00+08:00");
    w.m.act_mm(&mut w.e, C, t(10, 5));
    let committee_before = w.e.holding(w.committee, C).unwrap().qty;
    let unsold: i64 = mm_orders(&w).iter().filter(|o| !o.0).map(|o| o.2).sum();
    let hit = hit_bids(&mut w, "2026-11-05T10:06:00+08:00");
    assert!(hit > 0);
    // what it bought went straight back to the lender; it holds none of its own
    assert_eq!(w.e.short_qty(w.mm, C), sold + unsold - hit);
    assert_eq!(w.e.holding(w.committee, C).unwrap().qty, committee_before + hit);
    assert_eq!(w.e.holding(w.mm, C).map_or(0, |h| h.qty), unsold);
    ok(&w.e);
    // keep selling to it until it has covered everything (its offers are not taken, so the short only shrinks)
    for k in 0..50 {
        w.e.cancel_all(w.mm, C, &ts_of(t(11, k)));
        if w.e.short_qty(w.mm, C) == 0 {
            break;
        }
        w.m.act_mm(&mut w.e, C, t(11, k));
        hit_bids(&mut w, &ts_of(t(11, k)));
        ok(&w.e);
    }
    assert_eq!(w.e.short_qty(w.mm, C), 0, "never covered");
    w.m.act_mm(&mut w.e, C, t(12, 0));
    // owing nothing it holds what it bought beyond the cover and bids ordinarily
    assert!(mm_orders(&w).iter().filter(|o| o.0).all(|o| o.1.is_none()));
    ok(&w.e);
}

#[test]
fn the_short_stays_within_its_bound() {
    let mut w = world();
    // a small bound: 1% of the shares (100 shares)
    w.m.cfg.mm_loan_frac = 0.01;
    let mut most = 0;
    for k in 0..40 {
        w.m.act_mm(&mut w.e, C, t(10, k));
        most = most.max(w.e.short_qty(w.mm, C));
        lift_offers(&mut w, &ts_of(t(10, k)));
        assert!(w.e.short_qty(w.mm, C) <= 100);
    }
    assert_eq!(most, 100, "the bound was reached");
    // at the bound it offers nothing more
    w.m.act_mm(&mut w.e, C, t(11, 0));
    assert_eq!(mm_orders(&w).iter().filter(|o| !o.0).count(), 0);
    ok(&w.e);

    // the game's bound (20% of the shares) is above the lending pool here: the pool binds first
    let mut w = world();
    for k in 0..200 {
        w.m.act_mm(&mut w.e, C, t(10, 0) + k * 60);
        lift_offers(&mut w, &ts_of(t(10, 0) + k * 60));
    }
    let pool = (w.e.margin.stock_cap * 4_500.0) as i64;
    let s = w.e.short_qty(w.mm, C);
    assert!(s > 0 && s <= pool && s <= 2_000, "{s}");
    ok(&w.e);
}

#[test]
fn it_pays_the_lending_fee_and_manufactured_dividends() {
    let mut w = world();
    w.m.act_mm(&mut w.e, C, t(10, 0));
    let sold = lift_offers(&mut w, "2026-11-05T10:01:00+08:00");
    assert!(sold > 0);
    let close = "2026-11-06T06:30:00+08:00";
    w.e.margin_pre_close(close);
    assert_eq!(w.e.short_qty(w.mm, C), sold, "unsold borrowed shares go back at the close");
    w.e.st.company_days.insert((C, DAY.into()), CompanyDay { company_id: C, day: DAY.into(), revenue: 0, rev_us: 0, rev_cn: 0, rev_jp: 0, aired: 0, dividend_ps: 10 });
    w.sink.0.lock().unwrap().clear();
    let before = w.e.account(w.mm).unwrap().cash;
    w.e.margin_post_close(DAY, close);
    w.e.commit();
    let paid = |reason: &str| -> i64 {
        w.sink.0.lock().unwrap().iter().filter_map(|c| match c {
            Change::LedgerInsert(l) if l.reason == reason && l.from_id == w.mm => Some(l.amount),
            _ => None,
        }).sum()
    };
    let last = w.e.company(C).unwrap().last_price;
    let fee = ((sold * last) as f64 * w.e.lending_rate() / 365.0).ceil() as i64;
    assert_eq!(paid("short_dividend"), sold * 10);
    assert_eq!(paid("lending_fee"), fee);
    assert_eq!(w.e.account(w.mm).unwrap().cash, before - sold * 10 - fee);
    ok(&w.e);
}
