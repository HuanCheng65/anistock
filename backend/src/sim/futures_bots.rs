//! The 热度期货 exploit tests (docs/design/futures.md §4.5): a submodule of `bots`. Each is a futures strategy
//! (crate::futures::agents) run once a day (10:30; the front-runner at 21:00) by a player with the starting
//! cash and an open credit account; the two hedgers also hold stocks. The counterparties are the market maker
//! and the futures NPCs; a bot beating doing nothing by more than the gate is a mechanical edge.
//!
//! - `fut_long` / `fut_short`: always long / short, 30% of net assets spread over the contracts trading (at least
//!   one lot each);
//! - `fut_calendar`: long the first period, short the third and later;
//! - `fut_momentum`: the direction of the index's last week against the week before (when over 5%);
//! - `fut_front_run`: knows the day's index during the session (a player reading the platforms before the
//!   index is published), long when it is over 10% above the last week's mean, short when under;
//! - `fut_hedge`: holds the stocks of the works with contracts, sells futures worth half of them;
//! - `fut_double`: the same stocks and buys the futures: the same direction twice.

use std::collections::BTreeMap;

use super::{place, Bot, BotCtx, BotKind};
use crate::engine::Engine;
use crate::futures::agents::{Intent, Moment, Strategy, Trader, View};
use crate::futures::Contract;
use crate::market::{day_of, local, ts_of};

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

/// A gate bot's futures leg.
struct Gate(BotKind);

impl Strategy for Gate {
    fn decide(&mut self, v: &View, account: i64, c: &Contract) -> Option<Intent> {
        let n = v.contracts().len().max(1) as f64;
        let lots = ((v.net_assets(account).max(0) as f64 * SIZE / n / v.lot_value(c)).floor() as i64).max(1);
        let direction = match self.0 {
            BotKind::FutLong => 1,
            BotKind::FutShort => -1,
            BotKind::FutCalendar => match c.period {
                1 => 1,
                p if p >= 3 => -1,
                _ => 0,
            },
            BotKind::FutMomentum => momentum(&v.index_before(c.work_id, 14)),
            BotKind::FutFrontRun => front_run(v.heat_today, c.work_id, &v.index_before(c.work_id, 7)),
            BotKind::FutHedge | BotKind::FutDouble => {
                let stock = v.e.holding(account, c.company_id).map_or(0, |h| h.qty)
                    * v.e.company(c.company_id).map_or(0, |x| x.last_price);
                let lots = (stock as f64 * 0.5 / v.lot_value(c)).floor() as i64;
                return Some(Intent { lots: if self.0 == BotKind::FutHedge { -lots } else { lots }, limit: None });
            }
            _ => return None,
        };
        Some(Intent { lots: direction * lots, limit: None })
    }
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
    if matches!(b.kind, BotKind::FutHedge | BotKind::FutDouble) {
        let companies: Vec<i64> = e.futures_trading().iter().map(|k| e.st.futures.contracts[k].company_id).collect();
        let budget = e.net_assets(b.account) as f64 * SIZE / 3.0;
        for c in companies {
            hold_stock(b, e, c, budget, t);
        }
    }
    let mut trader = Trader { label: b.kind.name(), account: b.account, strategy: Box::new(Gate(b.kind)) };
    let m = Moment { day: &day, t, flashes: &[], heat_today: Some(ctx.heat_today) };
    b.trades_placed += trader.act(e, m, &ts) as u64;
}

fn momentum(days: &[f64]) -> i64 {
    if days.len() < 14 {
        return 0;
    }
    let (now, before) = (days[..7].iter().sum::<f64>(), days[7..].iter().sum::<f64>());
    match now / before.max(1e-9) - 1.0 {
        x if x > 0.05 => 1,
        x if x < -0.05 => -1,
        _ => 0,
    }
}

fn front_run(today: Option<&BTreeMap<i64, crate::inputs::HeatIndex>>, work: i64, week: &[f64]) -> i64 {
    let Some(idx) = today.and_then(|h| h.get(&work)).map(|h| h.idx) else { return 0 };
    if week.len() < 7 {
        return 0;
    }
    match idx / (week.iter().sum::<f64>() / 7.0).max(1e-9) {
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
