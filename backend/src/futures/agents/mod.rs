//! Who trades the futures besides players and the market maker (docs/design/futures.md §4.3), in three layers:
//!
//! - [`View`]: what a trader can see: the contracts, the model's forecast, prices, its position, the day's
//!   热度快讯. Read-only.
//! - [`Strategy`]: what it wants: for one contract, a target position and the worst price it accepts. No engine
//!   access beyond the view, so each strategy is a small struct tested on its own.
//! - [`execute`]: getting there: cancel its orders on the contract, order the difference.
//!
//! The strategies: [`Hedger`] (a production committee selling the heat its revenue depends on), [`Value`]
//! (a view of its own on the forecast), [`News`] (riding a 热度快讯). The gate bots (sim/futures_bots.rs) are
//! strategies too.

mod hedger;
mod news;
mod value;

pub use hedger::Hedger;
pub use news::News;
pub use value::Value;

use std::collections::BTreeMap;

use super::trading::FutOrderReq;
use super::Contract;
use crate::engine::Engine;
use crate::inputs::HeatIndex;
use crate::model::{Kind, OrderStatus, Side};

/// A published 热度快讯: the company, when, and how far the discussion ran above or below the previous episodes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flash {
    pub company: i64,
    pub t: i64,
    pub up: bool,
    /// the episode's comments relative to the previous episodes' at the same hour, minus 1
    pub surprise: f64,
}

/// The moment a trader looks: time `t` (unix seconds) of trading day `day`, and what has been published.
#[derive(Clone, Copy)]
pub struct Moment<'a> {
    pub day: &'a str,
    pub t: i64,
    /// 热度快讯 published so far
    pub flashes: &'a [Flash],
    /// the day's index as the platforms show it during the session (only the front-running gate bot looks)
    pub heat_today: Option<&'a BTreeMap<i64, HeatIndex>>,
}

/// What a futures trader sees.
pub struct View<'a> {
    pub e: &'a Engine,
    pub day: &'a str,
    pub t: i64,
    pub flashes: &'a [Flash],
    pub heat_today: Option<&'a BTreeMap<i64, HeatIndex>>,
}

impl<'a> View<'a> {
    pub fn new(e: &'a Engine, m: Moment<'a>) -> View<'a> {
        View { e, day: m.day, t: m.t, flashes: m.flashes, heat_today: m.heat_today }
    }
}

impl View<'_> {
    /// Contracts trading now, in id order.
    pub fn contracts(&self) -> Vec<&Contract> {
        self.e.st.futures.contracts.values().filter(|c| c.trading()).collect()
    }
    /// The model's forecast of the contract's average and its standard deviation, ticks.
    pub fn forecast(&self, c: &Contract) -> (f64, f64) {
        let f = &self.e.st.futures;
        (f.mean.get(&c.id).copied().unwrap_or(c.forecast) as f64, f.sd.get(&c.id).copied().unwrap_or(0) as f64)
    }
    pub fn position(&self, account: i64, c: &Contract) -> i64 {
        self.e.st.futures.position(account, c.id)
    }
    pub fn net_assets(&self, account: i64) -> i64 {
        self.e.net_assets(account)
    }
    /// One lot's notional, cents.
    pub fn lot_value(&self, c: &Contract) -> f64 {
        (c.last_price.max(1) * c.tick_value) as f64
    }
    /// The company's revenue over its last `days` closed days, cents.
    pub fn revenue(&self, company: i64, days: usize) -> i64 {
        let rows = self.e.st.company_days.range((company, String::new())..(company, self.day.to_string()));
        rows.rev().take(days).map(|(_, d)| d.revenue).sum()
    }
    /// The published index of the work over the `n` days before today, most recent first.
    pub fn index_before(&self, work: i64, n: usize) -> Vec<f64> {
        let f = &self.e.st.futures;
        f.heat.range((work, String::new())..(work, self.day.to_string())).rev().take(n).map(|(_, v)| *v).collect()
    }
}

/// A target: the position (lots, signed) and the worst price to trade toward it at (None: any price within
/// the band, a market order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intent {
    pub lots: i64,
    pub limit: Option<i64>,
}

pub trait Strategy: Send {
    /// What `account` wants in contract `c` now (None: leave it alone).
    fn decide(&mut self, v: &View, account: i64, c: &Contract) -> Option<Intent>;
}

/// A trader: an account and how it decides.
pub struct Trader {
    pub label: &'static str,
    pub account: i64,
    pub strategy: Box<dyn Strategy>,
}

impl Trader {
    /// Look at every contract trading and move toward what the strategy wants. Returns the orders placed.
    pub fn act(&mut self, e: &mut Engine, m: Moment, ts: &str) -> usize {
        let wants: Vec<(i64, Intent)> = {
            let v = View::new(e, m);
            let contracts: Vec<Contract> = v.contracts().into_iter().cloned().collect();
            contracts.iter().filter_map(|c| self.strategy.decide(&v, self.account, c).map(|i| (c.id, i))).collect()
        };
        wants.into_iter().filter(|(k, i)| execute(e, self.account, *k, *i, m.day, ts)).count()
    }
}

impl Engine {
    /// A futures trader's own account (kind `fnpc`, outside the money stock like the market maker's), with
    /// `capital` from world.
    pub fn futures_npc_account(&mut self, name: &str, capital: i64, day: &str, ts: &str) -> i64 {
        let id = self.do_create_account("fnpc", name, None, None, None, ts);
        let world = self.special("world").expect("no world account");
        self.do_transfer(world, id, capital, "futures_npc_fund", day, ts, None, None);
        self.commit();
        id
    }
}

/// Move `account`'s position in `contract` toward `intent`: cancel its open orders there, then order the
/// difference (a limit order rests at its price; a market order takes what is there). True if it ordered.
pub fn execute(e: &mut Engine, account: i64, contract: i64, intent: Intent, day: &str, ts: &str) -> bool {
    let mine: Vec<i64> = e.st.futures.open_orders(account, contract).map(|o| o.id).collect();
    for id in mine {
        e.fut_close(id, OrderStatus::Cancelled, ts);
    }
    e.st.futures.reap();
    let delta = intent.lots - e.st.futures.position(account, contract);
    if delta == 0 {
        e.commit();
        return false;
    }
    if e.state("auction_day") != Some(day) {
        e.open_market(day, ts);
    }
    let req = FutOrderReq {
        account_id: account,
        contract_id: contract,
        side: if delta > 0 { Side::Buy } else { Side::Sell },
        kind: if intent.limit.is_some() { Kind::Limit } else { Kind::Market },
        qty: delta.abs(),
        price: intent.limit,
        day,
        ts,
        match_: true,
    };
    let ok = e.futures_place(&req).is_ok();
    e.commit();
    ok
}

/// A deterministic draw in (0, 1) from a key (each trader's private view of each work stays the same all season).
pub(crate) fn unit(key: u64) -> f64 {
    let mut z = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
}

/// A standard normal draw from a key.
pub(crate) fn normal(key: u64) -> f64 {
    let (u1, u2) = (unit(key), unit(key ^ 0xA5A5_A5A5_A5A5_A5A5));
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}
