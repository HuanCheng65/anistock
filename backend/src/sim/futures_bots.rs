//! The 热度期货 exploit tests (docs/v3-futures.md §4.5): a submodule of `bots`. Each runs as a player with the
//! starting cash, opens credit on its first day and trades futures (and, for the hedgers, stocks) through the
//! same engine. Their counterparty is the market maker; a bot beating doing nothing by more than the gate
//! means the maker's quotes (the forecast) leave a mechanical edge.
//!
//! Once a day (10:30; the front-runner at 21:00) each sets a target position per contract and trades toward it
//! with market orders:
//!
//! - `fut_long` / `fut_short`: always long / short, 30% of net assets spread over the contracts trading (at least
//!   one lot each);
//! - `fut_calendar`: long the first period (the premiere's month), short the third and later (the finale);
//! - `fut_momentum`: the direction of the index's last week against the week before (when over 5%);
//! - `fut_front_run`: knows the day's index during the session (a player reading the platforms before the
//!   index is published), long when it is over 10% above the last week's mean, short when under;
//! - `fut_hedge`: holds the stocks of the works with contracts, sells futures worth half of them;
//! - `fut_double`: the same stocks and buys the futures: the same direction twice (cascading liquidations?).

use super::{place, Bot, BotCtx, BotKind};
use crate::engine::Engine;
use crate::futures::trading::FutOrderReq;
use crate::market::{day_of, local, ts_of};
use crate::model::{Kind, Side};

/// Share of net assets the futures bots commit.
const SIZE: f64 = 0.3;

pub fn is_futures(k: BotKind) -> bool {
    matches!(
        k,
        BotKind::FutLong
            | BotKind::FutShort
            | BotKind::FutCalendar
            | BotKind::FutMomentum
            | BotKind::FutFrontRun
            | BotKind::FutHedge
            | BotKind::FutDouble
    )
}

pub fn on_minute(b: &mut Bot, e: &mut Engine, t: i64, ctx: &BotCtx) {
    let (_, s) = local(t);
    let at = if b.kind == BotKind::FutFrontRun { 21 * 3600 } else { 10 * 3600 + 1800 };
    if s != at || !e.futures.enabled {
        return;
    }
    let (day, ts) = (day_of(t), ts_of(t));
    if e.credit(b.account).is_none() && e.open_credit(b.account, b.start_cash, &day, &ts).is_err() {
        return;
    }
    let contracts: Vec<i64> = e.futures_trading().into_iter().collect();
    if contracts.is_empty() {
        return;
    }
    let net = e.net_assets(b.account).max(0) as f64;
    let per = net * SIZE / contracts.len() as f64;
    for k in contracts {
        let c = e.st.futures.contracts[&k].clone();
        let lot = (c.prev_settle * c.tick_value).max(1) as f64;
        let lots = ((per / lot).floor() as i64).max(1); // a lot is about 1% of the starting cash
        let direction = match b.kind {
            BotKind::FutLong => 1,
            BotKind::FutShort => -1,
            BotKind::FutCalendar => match c.period {
                1 => 1,
                p if p >= 3 => -1,
                _ => 0,
            },
            BotKind::FutMomentum => momentum(e, c.work_id, &day),
            BotKind::FutFrontRun => front_run(e, ctx, c.work_id, &day),
            BotKind::FutHedge | BotKind::FutDouble => {
                hold_stock(b, e, c.company_id, net * SIZE / 3.0, t);
                let stock = e.holding(b.account, c.company_id).map_or(0, |h| h.qty)
                    * e.company(c.company_id).map_or(0, |x| x.last_price);
                let lots = (stock as f64 * 0.5 / lot).floor() as i64;
                let target = if b.kind == BotKind::FutHedge { -lots } else { lots };
                trade_to(b, e, k, target, &day, &ts);
                continue;
            }
            _ => 0,
        };
        trade_to(b, e, k, direction * lots, &day, &ts);
    }
}

/// Market orders toward `target` lots.
fn trade_to(b: &mut Bot, e: &mut Engine, contract: i64, target: i64, day: &str, ts: &str) {
    let delta = target - e.st.futures.position(b.account, contract);
    if delta == 0 {
        return;
    }
    let side = if delta > 0 { Side::Buy } else { Side::Sell };
    let req = FutOrderReq {
        account_id: b.account,
        contract_id: contract,
        side,
        kind: Kind::Market,
        qty: delta.abs(),
        price: None,
        day,
        ts,
        match_: true,
    };
    if e.state("auction_day") != Some(day) {
        e.open_market(day, ts);
    }
    if e.futures_place(&req).is_ok() {
        b.trades_placed += 1;
    }
}

/// The published index of the last `n` days before `day` (most recent first).
fn last_days(e: &Engine, work: i64, day: &str, n: usize) -> Vec<f64> {
    e.st.futures.heat.range((work, String::new())..(work, day.to_string())).rev().take(n).map(|(_, v)| *v).collect()
}

fn momentum(e: &Engine, work: i64, day: &str) -> i64 {
    let v = last_days(e, work, day, 14);
    if v.len() < 14 {
        return 0;
    }
    let (now, before) = (v[..7].iter().sum::<f64>(), v[7..].iter().sum::<f64>());
    match now / before.max(1e-9) - 1.0 {
        x if x > 0.05 => 1,
        x if x < -0.05 => -1,
        _ => 0,
    }
}

fn front_run(e: &Engine, ctx: &BotCtx, work: i64, day: &str) -> i64 {
    let Some(today) = ctx.heat_today.get(&work).map(|h| h.idx) else { return 0 };
    let v = last_days(e, work, day, 7);
    if v.len() < 7 {
        return 0;
    }
    let mean = v.iter().sum::<f64>() / 7.0;
    match today / mean.max(1e-9) {
        r if r > 1.1 => 1,
        r if r < 0.9 => -1,
        _ => 0,
    }
}

/// Hold about `budget` cents of the company's stock (buying once).
fn hold_stock(b: &mut Bot, e: &mut Engine, company: i64, budget: f64, t: i64) {
    if e.holding(b.account, company).is_some_and(|h| h.qty > 0) {
        return;
    }
    let Some(p) = e.company(company).filter(|c| c.status == "listed").map(|c| c.last_price).filter(|p| *p > 0) else {
        return;
    };
    let lim = p * 1005 / 1000 + 1;
    let q = super::affordable(e, (budget as i64).min(e.available_cash(b.account)), lim);
    if place(e, b.account, company, "buy", "limit", q, Some(lim), t) {
        b.trades_placed += 1;
    }
}
