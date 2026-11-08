//! 指数基金 on an in-memory engine: a subscription executed in the next session and confirmed at its close, a
//! later subscription at a higher net value (no dilution of the units already there), a redemption paid.

use super::*;
use crate::model::Company;
use crate::{Fees, Rules};

const D1: &str = "2026-11-05";
const D2: &str = "2026-11-06";
const D3: &str = "2026-11-07";

fn ts(day: &str, hm: &str) -> String {
    format!("{day}T{hm}:00+08:00")
}

fn company(id: i64) -> Company {
    Company {
        id,
        code: format!("10000{id}"),
        title: format!("作品{id}"),
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
        shares: 10_000,
        ipo_price: 500,
        prev_close: 500,
        open_ref: Some(500),
        vcm_until: None,
        vcm_low: None,
        vcm_high: None,
        vcm_count: 0,
        last_price: 500,
        fair_value: 0,
        profit_week: 0,
        low_weeks: 0,
        delisted_on: None,
        source: None,
        genres: String::new(),
    }
}

struct W {
    e: Engine,
    a: i64,
    b: i64,
    dealer: i64,
}

fn player(e: &mut Engine, name: &str, cash: i64) -> i64 {
    let id = e.create_account("player", name, Some(name), None, None, &ts(D1, "10:00"));
    let world = e.special("world").unwrap();
    e.transfer(world, id, cash, "grant", D1, &ts(D1, "10:00"), None, None);
    id
}

/// Two stocks at 5 星辉 (company 2 twice the shares: two thirds of the index), a dealer holding plenty of both
/// with cash, players a and b with 1,000 星辉 each; the `all` fund, trading up to the whole volume.
fn world() -> W {
    let rules = Rules { vcm_threshold: 1000.0, ..Rules::default() };
    let (mut e, _) = Engine::in_memory(Fees::default(), rules);
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, &ts(D1, "10:00"));
    }
    e.set_state("auction_day", D2);
    e.insert_company(company(1));
    e.insert_company(Company { shares: 20_000, ..company(2) });
    for c in [1, 2] {
        e.create_account("company", "company", None, None, Some(c), &ts(D1, "10:00"));
    }
    let dealer = player(&mut e, "dealer", 10_000_000);
    e.add_shares(dealer, 1, 10_000, 500, "2026-09-10");
    e.add_shares(dealer, 2, 20_000, 500, "2026-09-10");
    let a = player(&mut e, "a", 100_000);
    let b = player(&mut e, "b", 100_000);
    e.funds = FundRules { enabled: true, keys: vec!["all".into()], batch_limit: 0.0, max_volume_share: 1.0, ..FundRules::default() };
    e.funds_setup(&ts(D1, "10:00"));
    // a volume history, so the fund may trade from its first session
    for c in [1, 2] {
        e.st.funds.vol_days.insert(c, [10_000].into_iter().collect());
    }
    W { e, a, b, dealer }
}

/// The dealer offers / bids `qty` of each stock at its last price.
fn dealer_quotes(w: &mut W, side: &str, day: &str) {
    for c in [1, 2] {
        let p = w.e.company(c).unwrap().last_price;
        w.e.place_order(&NewOrder {
            account_id: w.dealer,
            company_id: c,
            side,
            kind: "limit",
            qty: 1_000,
            price: Some(p),
            day,
            ts: &ts(day, "09:10"),
            match_: true,
        })
        .unwrap();
    }
}

/// One session of `day`: the requests of the days before start executing, one batch at 09:30 trades them
/// against the dealer's quotes, the close confirms them.
fn session(w: &mut W, day: &str, dealer_side: &str) {
    w.e.set_state("auction_day", day);
    w.e.funds_auction(day, &ts(day, "08:50"));
    for c in [1, 2] {
        w.e.cancel_all(w.dealer, c, &ts(day, "09:00"));
    }
    dealer_quotes(w, dealer_side, day);
    // the last batch of the day: the whole remaining difference
    w.e.funds_batch(day, &ts(day, "23:30"), 23 * 60 + 30);
    w.e.funds_close(day, &ts(day, "23:59"));
    w.e.check_invariants().unwrap();
}

fn set_price(w: &mut W, c: i64, p: i64) {
    let co = w.e.st.companies.get_mut(&c).unwrap();
    co.last_price = p;
    co.prev_close = p;
}

#[test]
fn subscription_buys_the_index_and_issues_units_at_the_close() {
    let mut w = world();
    let fund = w.e.fund("all").unwrap().account;
    w.e.fund_subscribe(w.a, "all", 90_000, D1, &ts(D1, "20:00")).unwrap();
    assert_eq!(w.e.account(w.a).unwrap().cash, 10_000);
    // the cash is in the fund but not the fund's: nothing executes until the next session
    assert_eq!(w.e.fund("all").unwrap().units, 0);
    assert_eq!(w.e.fund_value_of(w.a), 90_000);

    session(&mut w, D2, "sell");
    let f = w.e.fund("all").unwrap();
    // cap-weighted: one third in company 1, two thirds in company 2 (both at 5 星辉)
    let q1 = w.e.holding(fund, 1).map_or(0, |h| h.qty);
    let q2 = w.e.holding(fund, 2).map_or(0, |h| h.qty);
    assert!((59..=60).contains(&q1) && (119..=120).contains(&q2), "{q1} {q2}");
    // the first units at 1 星辉, the commission charged to the subscription
    let c = &f.confirmed[0];
    assert_eq!(c.nav, NAV0);
    assert!(c.cost > 0 && c.cost < 100, "{}", c.cost);
    assert_eq!(c.units, (90_000 - c.cost) * UNIT / 100);
    assert_eq!(f.units_of(w.a), c.units);
    assert_eq!(f.pending_cash, 0);
    // a's claim is the fund's assets, less the fee
    let v = w.e.fund_value_of(w.a);
    assert!((90_000 - c.cost - v).abs() <= 2 + f.fees_paid, "{v}");
}

#[test]
fn a_later_subscription_does_not_dilute_the_units_there() {
    let mut w = world();
    w.e.fund_subscribe(w.a, "all", 90_000, D1, &ts(D1, "20:00")).unwrap();
    session(&mut w, D2, "sell");
    // the stocks rise 20%: a's units are worth 20% more (on what was invested)
    set_price(&mut w, 1, 600);
    set_price(&mut w, 2, 600);
    let fund = w.e.fund("all").unwrap().account;
    let a_before = w.e.holdings_of(fund).map(|h| h.qty * 600).sum::<i64>() + w.e.account(fund).unwrap().cash;
    w.e.fund_subscribe(w.b, "all", 60_000, D2, &ts(D2, "20:00")).unwrap();
    session(&mut w, D3, "sell");
    let f = w.e.fund("all").unwrap();
    let cb = f.confirmed.iter().find(|c| c.account == w.b).unwrap();
    assert!(cb.nav > 115.0, "{}", cb.nav);
    // a keeps its value (minus a day's fee); b pays its own commission
    let a_after = w.e.fund_value_of(w.a);
    assert!((a_before - a_after).abs() <= a_before / 3000 + 2, "{a_before} {a_after}");
    assert!((w.e.fund_value_of(w.b) - (60_000 - cb.cost)).abs() <= 60_000 / 3000 + 2);
}

#[test]
fn a_redemption_sells_and_pays_the_holder() {
    let mut w = world();
    w.e.fund_subscribe(w.a, "all", 90_000, D1, &ts(D1, "20:00")).unwrap();
    session(&mut w, D2, "sell");
    let units = w.e.fund("all").unwrap().units_of(w.a);
    w.e.fund_redeem(w.a, "all", units, D2).unwrap();
    assert!(w.e.fund_redeem(w.a, "all", 1, D2).is_err());
    let cash0 = w.e.account(w.a).unwrap().cash;
    session(&mut w, D3, "buy");
    let f = w.e.fund("all").unwrap();
    let c = f.confirmed.iter().find(|c| c.units < 0).unwrap();
    assert_eq!(f.units, 0);
    assert!(f.owed.is_empty(), "{:?}", f.owed);
    // paid the net value less the sells' commission and stamp duty
    let got = w.e.account(w.a).unwrap().cash - cash0;
    assert_eq!(got, -c.cash);
    assert!(got > 89_500 && got < 90_000, "{got}");
}

#[test]
fn the_requests_pay_for_the_prices_they_moved() {
    // the fund buys at 5 星辉 for a subscription and the stocks close at 4.90: the 0.10 a share it paid over the
    // close is the subscription's cost, not the units' already there
    let mut w = world();
    w.e.fund_subscribe(w.a, "all", 90_000, D1, &ts(D1, "20:00")).unwrap();
    session(&mut w, D2, "sell");
    let a_units = w.e.fund("all").unwrap().units_of(w.a);
    w.e.fund_subscribe(w.b, "all", 60_000, D2, &ts(D2, "20:00")).unwrap();
    w.e.set_state("auction_day", D3);
    w.e.funds_auction(D3, &ts(D3, "08:50"));
    for c in [1, 2] {
        w.e.cancel_all(w.dealer, c, &ts(D3, "09:00"));
    }
    dealer_quotes(&mut w, "sell", D3);
    w.e.funds_batch(D3, &ts(D3, "23:30"), 23 * 60 + 30);
    set_price(&mut w, 1, 490);
    set_price(&mut w, 2, 490);
    w.e.funds_close(D3, &ts(D3, "23:59"));
    w.e.check_invariants().unwrap();
    let f = w.e.fund("all").unwrap();
    let cb = f.confirmed.iter().find(|c| c.account == w.b).unwrap();
    let bought: i64 = f.confirmed.iter().filter(|c| c.account == w.b).count() as i64;
    assert_eq!(bought, 1);
    // about 120 shares bought at 10 cents over the close, plus the commission
    assert!(cb.cost > 1_100 && cb.cost < 1_300, "{}", cb.cost);
    // a's units: worth what the stocks are worth at the close (2% down), nothing more lost
    let fund = f.account;
    let held: i64 = w.e.holdings_of(fund).map(|h| h.qty * 490).sum();
    let nav_a = (held + w.e.account(fund).unwrap().cash - f.pending_cash) as f64 - (60_000 - cb.cost) as f64;
    let a_value = a_units as f64 / UNIT as f64 * cb.nav;
    assert!((a_value - nav_a).abs() < 50.0, "{a_value} {nav_a}");
}
