//! 融资融券 scenarios on an in-memory engine: margin math, the order hooks, the close, forced liquidation,
//! default (穿仓) and restructuring.

use super::*;
use crate::model::{Company, CompanyDay};
use crate::{Fees, Rules};

const TS: &str = "2026-11-05T10:00:00+08:00";
const DAY: &str = "2026-11-05";

struct W {
    e: Engine,
    committee: i64,
    seller: i64,
    buyer: i64,
    p: i64,
}

fn company() -> Company {
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
    }
}

fn player(e: &mut Engine, name: &str, cash: i64) -> i64 {
    let id = e.create_account("player", name, Some(name), None, None, "2026-10-01T10:00:00+08:00");
    let world = e.special("world").unwrap();
    e.transfer(world, id, cash, "grant", DAY, TS, None, None);
    id
}

/// Company 1 (1,000 shares at 5 币): the rights holders hold 600, a seller 400; a buyer with cash; player `p`
/// with 2,000 币 and credit open. No volatility control (prices are moved by hand).
fn world() -> W {
    let rules = Rules { vcm_threshold: 1000.0, ..Rules::default() };
    let (mut e, _) = Engine::in_memory(Fees::default(), rules);
    for k in ["world", "cb", "gov"] {
        e.create_account(k, k, None, None, None, TS);
    }
    e.set_state("rate", "0.03");
    e.set_state("auction_day", DAY);
    e.insert_company(company());
    e.create_account("company", "company", None, None, Some(1), TS);
    let committee = e.create_account("committee", "committee", None, None, Some(1), TS);
    e.add_shares(committee, 1, 600, 500, "2026-09-10");
    let seller = player(&mut e, "seller", 0);
    e.add_shares(seller, 1, 400, 500, "2026-09-10");
    let buyer = player(&mut e, "buyer", 10_000_000);
    let p = player(&mut e, "p", 200_000);
    e.margin.stock_cap = 1.0;
    e.open_credit(p, 0, DAY, TS).unwrap_or_else(|err| panic!("{err}"));
    W { e, committee, seller, buyer, p }
}

fn order(e: &mut Engine, acct: i64, side: &str, qty: i64, price: i64) -> crate::engine::Placed {
    e.place_order(&NewOrder { account_id: acct, company_id: 1, side, kind: "limit", qty, price: Some(price), day: DAY, ts: TS, match_: true })
        .unwrap_or_else(|err| panic!("{err}"))
}

fn credit(e: &mut Engine, acct: i64, mode: Mode, qty: i64, price: i64) -> Result<crate::engine::Placed, OrderError> {
    let side = mode.side().as_str();
    e.place_credit(&NewOrder { account_id: acct, company_id: 1, side, kind: "limit", qty, price: Some(price), day: DAY, ts: TS, match_: true }, mode)
}

fn set_price(e: &mut Engine, p: i64) {
    let c = e.st.companies.get_mut(&1).unwrap();
    c.last_price = p;
}

fn ok(e: &Engine) {
    e.check_invariants().unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn opening_conditions() {
    let mut w = world();
    assert_eq!(w.e.open_credit(w.p, 10_000_000, DAY, TS).unwrap_err().0, "信用账户已经开通");
    // 1,000 币 of net assets is under half the starting cash
    w.e.margin.min_days = 7;
    let poor = player(&mut w.e, "poor", 100_000);
    assert!(w.e.open_credit(poor, 10_000_000, DAY, TS).unwrap_err().0.contains("净资产不低于 50,000 币"));
    let young = w.e.create_account("player", "young", Some("young"), None, None, TS);
    assert!(w.e.open_credit(young, 0, DAY, TS).unwrap_err().0.contains("注册满 7 天"));
    // no credit: no credit orders
    assert_eq!(credit(&mut w.e, poor, Mode::MarginBuy, 1, 500).unwrap_err().0, "尚未开通信用账户");
}

#[test]
fn margin_buy_draws_and_repays_the_loan() {
    let mut w = world();
    order(&mut w.e, w.seller, "sell", 150, 500);
    // 200 at 5 币: 100,225 drawn (notional, commission, a cent a share), 150 fill, the rest is cancelled
    let r = credit(&mut w.e, w.p, Mode::MarginBuy, 200, 500).unwrap();
    assert_eq!(r.filled, 150);
    let debt = w.e.debts_of(w.p).next().unwrap().principal;
    assert_eq!(debt, 100_225);
    assert_eq!(w.e.available_cash(w.p), 200_000, "the loan is frozen in the order, own cash untouched");
    w.e.cancel_order(w.p, r.order_id, TS).unwrap();
    // spent: 75,000 + 19 commission
    assert_eq!(w.e.debts_of(w.p).next().unwrap().principal, 75_019);
    assert_eq!(w.e.account(w.p).unwrap().cash, 200_000);
    assert_eq!(w.e.holding(w.p, 1).unwrap().qty, 150);
    let n = w.e.credit_numbers(w.p, DAY);
    assert_eq!(n.liabilities, 75_019);
    assert_eq!(n.net(), 200_000 + 75_000 - 75_019);
    // available margin: cash + (financed value − financing, a loss of the commission in full) − 100% × financing
    assert_eq!(w.e.debts_of(w.p).next().unwrap().qty, 150);
    assert_eq!(n.available_margin, 200_000 + (75_000 - 75_019) - 75_019);
    ok(&w.e);

    // too much: 249 more need 124,781 (inside the stock's financing room) but leave the margin at −100
    assert_eq!(credit(&mut w.e, w.p, Mode::MarginBuy, 250, 500).unwrap_err().0, "这支股票的融资额度已满");
    assert_eq!(credit(&mut w.e, w.p, Mode::MarginBuy, 249, 500).unwrap_err().0, "可用保证金不足");
    // 卖券还款: proceeds repay the debt (no commission/stamp left over to keep)
    order(&mut w.e, w.buyer, "buy", 100, 500);
    let r = credit(&mut w.e, w.p, Mode::SellRepay, 100, 500).unwrap();
    assert_eq!(r.filled, 100);
    let net = 50_000 - 13 - 25;
    assert_eq!(w.e.debts_of(w.p).next().unwrap().principal, 75_019 - net);
    assert_eq!(w.e.debts_of(w.p).next().unwrap().qty, 50, "the sale takes the financed shares first");
    // 直接还款
    let paid = w.e.repay_cash(w.p, None, DAY, TS).unwrap();
    assert_eq!(paid, 75_019 - net);
    assert!(w.e.debts_of(w.p).next().is_none());
    ok(&w.e);
}

#[test]
fn short_sale_borrows_locks_and_returns() {
    let mut w = world();
    // price rule: not under the last trade
    assert!(credit(&mut w.e, w.p, Mode::ShortSell, 100, 499).unwrap_err().0.starts_with("融券卖出价格不能低于最新价"));
    // lending pool: half of the rights holders' 600
    assert_eq!(w.e.lendable(1), 300);
    assert!(credit(&mut w.e, w.p, Mode::ShortSell, 301, 500).unwrap_err().0.starts_with("可融券数量不足"));
    order(&mut w.e, w.buyer, "buy", 60, 500);
    let r = credit(&mut w.e, w.p, Mode::ShortSell, 100, 500).unwrap();
    assert_eq!(r.filled, 60);
    assert_eq!(w.e.holding(w.committee, 1).unwrap().qty, 500);
    let s = w.e.short_of(w.p, 1).unwrap().clone();
    assert_eq!(s.qty, 100);
    let net = 30_000 - 8 - 15;
    assert_eq!(s.proceeds, net);
    assert_eq!(w.e.available_cash(w.p), 200_000, "proceeds are locked");
    // an ordinary buy of a stock the account is short is refused
    let e = w.e.place_order(&NewOrder { account_id: w.p, company_id: 1, side: "buy", kind: "limit", qty: 1, price: Some(500), day: DAY, ts: TS, match_: true });
    assert!(e.unwrap_err().0.contains("买券还券"));
    ok(&w.e);
    // the unsold 40 go back to the lender when the order ends
    w.e.cancel_order(w.p, r.order_id, TS).unwrap();
    assert_eq!(w.e.short_qty(w.p, 1), 60);
    assert_eq!(w.e.holding(w.committee, 1).unwrap().qty, 540);
    assert!(w.e.holding(w.p, 1).is_none());
    ok(&w.e);

    // buy back below: locked proceeds pay first, the rest of the reserve comes from free cash
    order(&mut w.e, w.seller, "sell", 60, 400);
    assert!(credit(&mut w.e, w.p, Mode::BuyReturn, 61, 400).unwrap_err().0.starts_with("还券数量超过欠券"));
    let r = credit(&mut w.e, w.p, Mode::BuyReturn, 60, 400).unwrap();
    assert_eq!(r.filled, 60);
    assert!(w.e.short_of(w.p, 1).is_none(), "covered: position closed");
    assert_eq!(w.e.holding(w.committee, 1).unwrap().qty, 600);
    // gained 100 a share less fees, all unlocked
    let a = w.e.account(w.p).unwrap();
    assert_eq!(a.cash_frozen, 0);
    assert_eq!(a.cash, 200_000 + net - 24_000 - 6);
    ok(&w.e);
}

#[test]
fn close_charges_interest_fees_and_manufactured_dividends() {
    let mut w = world();
    order(&mut w.e, w.seller, "sell", 100, 500);
    credit(&mut w.e, w.p, Mode::MarginBuy, 100, 500).unwrap();
    let other = player(&mut w.e, "q", 200_000);
    w.e.open_credit(other, 0, DAY, TS).unwrap();
    order(&mut w.e, w.buyer, "buy", 100, 500);
    let r = credit(&mut w.e, other, Mode::ShortSell, 100, 500).unwrap();
    assert_eq!(r.filled, 100);
    // a resting margin buy is cancelled at the start of the close (its loan repaid)
    credit(&mut w.e, w.p, Mode::MarginBuy, 10, 400).unwrap();
    w.e.margin_pre_close("2026-11-06T06:30:00+08:00");
    assert_eq!(w.e.debts_of(w.p).next().unwrap().principal, 50_013);
    // a Sunday dividend of 0.10 币 a share
    w.e.st.company_days.insert((1, DAY.into()), CompanyDay { company_id: 1, day: DAY.into(), revenue: 0, rev_us: 0, rev_cn: 0, rev_jp: 0, aired: 0, dividend_ps: 10 });
    let world_before = w.e.account(w.e.special("world").unwrap()).unwrap().cash;
    w.e.margin_post_close(DAY, "2026-11-06T06:30:00+08:00");
    let interest = (50_013f64 * 0.06 / 365.0).ceil() as i64;
    let fee = (50_000f64 * 0.08 / 365.0).ceil() as i64;
    assert_eq!(w.e.account(w.p).unwrap().cash, 200_000 - interest);
    assert_eq!(w.e.debts_of(w.p).next().unwrap().interest, 0, "paid from free cash");
    let q = w.e.account(other).unwrap();
    assert_eq!(q.cash, 200_000 + 50_000 - 13 - 25 - 1_000 - fee);
    assert_eq!(w.e.account(w.e.special("world").unwrap()).unwrap().cash - world_before, interest + 1_000 + fee);
    ok(&w.e);
}

#[test]
fn liquidation_then_default_then_restructuring() {
    let mut w = world();
    // 300 shares with own cash, then 200 more on margin
    order(&mut w.e, w.seller, "sell", 400, 500);
    order(&mut w.e, w.p, "buy", 300, 500);
    credit(&mut w.e, w.p, Mode::MarginBuy, 100, 500).unwrap();
    // all the free cash spent elsewhere
    let free = w.e.available_cash(w.p);
    let world_id = w.e.special("world").unwrap();
    w.e.transfer(w.p, world_id, free, "test", DAY, TS, None, None);
    // the stock collapses to 1 币: ratio far under 110%
    set_price(&mut w.e, 100);
    w.e.st.companies.get_mut(&1).unwrap().prev_close = 100;
    // a buyer at 0.96 for everything
    order(&mut w.e, w.buyer, "buy", 1000, 96);
    let changed = w.e.margin_sweep(DAY, TS);
    assert_eq!(changed, vec![w.p]);
    // free cash repaid first, then all 400 shares sold at 0.96 (protection: not under 0.95)
    assert!(w.e.holding(w.p, 1).is_none());
    let left: i64 = w.e.debts_of(w.p).map(|d| d.principal + d.interest).sum();
    assert!(left > 0, "still owes after selling everything");
    assert!(w.e.net_assets(w.p) < 0);
    assert_eq!(w.e.credit(w.p).unwrap().status, LIQUIDATING);
    ok(&w.e);
    // the next round: nothing left to sell, equity negative: default
    w.e.margin_sweep(DAY, TS);
    let c = w.e.credit(w.p).unwrap().clone();
    assert_eq!(c.status, DEFAULTED);
    assert_eq!(c.bad_debt, left);
    assert_eq!(w.e.net_assets(w.p), -left);
    assert!(credit(&mut w.e, w.p, Mode::MarginBuy, 1, 100).unwrap_err().0.contains("穿仓"));
    ok(&w.e);
    // restructuring writes it off (season limit does not apply) and counts the segment as −100%
    w.e.st.player_returns.insert(
        w.p,
        crate::model::PlayerReturn { account_id: w.p, return_factor: 1.0, segment_base: 10_000_000, restructures: 1, last_day: DAY.into() },
    );
    let r = w.e.restructure(w.p, 10_000_000, 0.1, DAY, TS).unwrap();
    assert_eq!(r.assets_before, -left);
    assert_eq!(w.e.player_return(w.p).unwrap().return_factor, 0.0);
    assert!(w.e.credit(w.p).is_none());
    assert_eq!(w.e.net_assets(w.p), 10_000_000);
    ok(&w.e);
}

#[test]
fn short_squeeze_is_bought_in() {
    let mut w = world();
    order(&mut w.e, w.buyer, "buy", 200, 500);
    credit(&mut w.e, w.p, Mode::ShortSell, 200, 500).unwrap();
    // 4 币 higher: owed 180,000 against 200,000 + 99,9xx locked: ratio 1.67, fine
    set_price(&mut w.e, 900);
    assert!(w.e.margin_sweep(DAY, TS).is_empty());
    // the close: at 12 币 the ratio is 1.25: a margin call; restructuring is refused while it owes
    set_price(&mut w.e, 1200);
    w.e.margin_post_close(DAY, "2026-11-06T06:30:00+08:00");
    assert_eq!(w.e.credit(w.p).unwrap().status, CALL);
    assert_eq!(w.e.restructure(w.p, 10_000_000, 0.1, DAY, TS).unwrap_err(), crate::restructure::Refusal::Margin);
    // still under at the next close: liquidation
    w.e.margin_post_close("2026-11-06", "2026-11-07T06:30:00+08:00");
    assert_eq!(w.e.credit(w.p).unwrap().status, LIQUIDATING);
    // offers at 12.50 (inside the 5% protection): bought in until the ratio is back to 150%
    order(&mut w.e, w.seller, "sell", 400, 1250);
    for _ in 0..5 {
        w.e.margin_sweep("2026-11-06", "2026-11-06T10:00:00+08:00");
    }
    let s = w.e.short_qty(w.p, 1);
    assert!(s < 200 && s > 0, "partly bought in: {s}");
    assert_eq!(w.e.credit(w.p).unwrap().status, NORMAL);
    let n = w.e.credit_numbers(w.p, DAY);
    assert!(n.ratio().unwrap() >= 1.5, "{:?}", n.ratio());
    ok(&w.e);
}

#[test]
fn delisting_settles_shorts_in_cash() {
    let mut w = world();
    order(&mut w.e, w.buyer, "buy", 100, 500);
    credit(&mut w.e, w.p, Mode::ShortSell, 100, 500).unwrap();
    let world_id = w.e.special("world").unwrap();
    let before = w.e.account(world_id).unwrap().cash;
    w.e.margin_on_delist(1, 120, DAY, TS);
    assert!(w.e.short_of(w.p, 1).is_none());
    assert_eq!(w.e.account(world_id).unwrap().cash - before, 12_000);
    assert_eq!(w.e.account(w.p).unwrap().cash_frozen, 0);
    let _ = w.seller;
    ok(&w.e);
}

// ---- 保证金可用余额 (the exchanges' formula; docs/v2-margin.md §4)

/// A player with `cash` and credit open, outside the order flow (positions are set by hand).
fn bare(cash: i64) -> (W, i64) {
    let mut w = world();
    let q = player(&mut w.e, "q", cash);
    w.e.open_credit(q, 0, DAY, TS).unwrap();
    (w, q)
}

fn set_debt(e: &mut Engine, acct: i64, principal: i64, qty: i64) {
    e.st.margin.debts.insert((acct, 1), Debt { account_id: acct, company_id: 1, principal, interest: 0, opened_on: DAY.into(), qty });
}

#[test]
fn all_cash_finances_its_own_amount() {
    let (w, q) = bare(10_000_000);
    // 100,000 币 of cash: 100,000 币 of new financing (it was 100,000 / 1.3 = 76,923 before)
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000);
    // in practice: a stock with room for it, and its float on offer at 5 币
    // a 2,000-币 account: 390 shares at 5 币 fit, 399 (199,949 cents) do not
    let (mut w2, q2) = bare(200_000);
    w2.e.margin.stock_cap = 100.0;
    order(&mut w2.e, w2.seller, "sell", 400, 500);
    assert!(credit(&mut w2.e, q2, Mode::MarginBuy, 390, 500).is_ok());
    let n = w2.e.credit_numbers(q2, DAY);
    // 195,439 drawn (195,000 + 49 commission + a cent a share); the unused 390 repaid when it filled
    assert_eq!(n.debt_principal, 195_049);
    let (mut w3, q3) = bare(200_000);
    w3.e.margin.stock_cap = 100.0;
    order(&mut w3.e, w3.seller, "sell", 400, 500);
    assert_eq!(credit(&mut w3.e, q3, Mode::MarginBuy, 399, 500).unwrap_err().0, "可用保证金不足");
}

#[test]
fn own_stock_counts_at_the_haircut() {
    let (mut w, q) = bare(10_000_000);
    w.e.add_shares(q, 1, 100, 500, DAY);
    // 100,000 + 100 × 5 × 0.7
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000 + 35_000);
}

#[test]
fn financed_gain_at_the_haircut_loss_in_full() {
    let (mut w, q) = bare(10_000_000);
    w.e.add_shares(q, 1, 100, 500, DAY);
    set_debt(&mut w.e, q, 50_000, 100);
    // at 6 币: + (60,000 − 50,000) × 0.7 − 50,000 × 100%
    set_price(&mut w.e, 600);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000 + 7_000 - 50_000);
    // at 4 币: the 10,000 loss counts in full
    set_price(&mut w.e, 400);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000 - 10_000 - 50_000);
    // own shares beside the financed ones count as collateral
    w.e.add_shares(q, 1, 50, 400, DAY);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000 + 14_000 - 10_000 - 50_000);
}

#[test]
fn short_gain_at_the_haircut_loss_in_full() {
    let (mut w, q) = bare(10_000_000);
    // 100 shares sold short for 50,000, the proceeds locked in the account's cash
    let world_id = w.e.special("world").unwrap();
    w.e.transfer(world_id, q, 50_000, "test", DAY, TS, None, None);
    w.e.st.margin.shorts.insert(
        (q, 1),
        Short { account_id: q, company_id: 1, lender_id: w.committee, qty: 100, proceeds: 50_000, fee: 0, opened_on: DAY.into(), amount: 50_000 },
    );
    // at 4 币: + (50,000 − 40,000) × 0.7 − 50,000 − 40,000 × 100%
    set_price(&mut w.e, 400);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_050_000 + 7_000 - 50_000 - 40_000);
    // at 6 币: the 10,000 loss in full
    set_price(&mut w.e, 600);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_050_000 - 10_000 - 50_000 - 60_000);
    // at 5 币 the short takes exactly its own value of margin: C − Y
    set_price(&mut w.e, 500);
    assert_eq!(w.e.credit_numbers(q, DAY).available_margin, 10_000_000 - 50_000);
}
