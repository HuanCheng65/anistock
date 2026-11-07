//! 热度期货: cash-settled futures on the AniEx heat index (docs/v3-futures.md; how the code is organised:
//! docs/v3-futures-impl.md). Matching is the shared layer (`crate::matching`); everything a fill does to
//! margin and positions, the daily mark-to-market and the final settlement live here.
//!
//! - `model`: the rows (contracts, orders, trades, positions, days, the frozen index)
//! - `store`: loading and saving them (migration V17)
//! - `rules`: `[futures]` of game.toml
//! - `calendar`, `listing`: the periods and listing them at the close

pub mod calendar;
pub mod listing;
pub mod model;
pub mod rules;
pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::matching::Book;
use crate::model::OrderStatus;
pub use model::*;
pub use rules::FuturesRules;
pub use store::FuturesChange;

/// A trade of the current trading day, kept per contract for the volatility control's look-back and the
/// settlement price's closing window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub ts: String,
    pub price: i64,
    pub qty: i64,
}

/// Everything futures-related in the engine's state (cloned with it for rollback).
#[derive(Debug, Clone, Default)]
pub struct FuturesState {
    /// contracts not settled yet
    pub contracts: BTreeMap<i64, Contract>,
    /// open orders, plus orders closed during the current operation
    pub orders: HashMap<i64, FutOrder>,
    pub(crate) books: HashMap<i64, Book>,
    pub by_account: HashMap<i64, BTreeSet<i64>>,
    /// (account, contract) -> net position
    pub positions: BTreeMap<(i64, i64), Position>,
    /// contract -> the current trading day and its trades so far
    pub today: HashMap<i64, (String, Vec<Tick>)>,
    /// contract -> the market maker's latest mid quote (the settlement price without trades)
    pub last_mid: HashMap<i64, i64>,
    pub last_contract_id: i64,
    pub last_order_id: i64,
    pub last_trade_id: i64,
}

impl FuturesState {
    /// Track an order: by account, and on its contract's book while it is an open limit order.
    pub fn insert_order(&mut self, o: FutOrder) {
        if let (Some(p), OrderStatus::Open) = (o.price, o.status) {
            self.books.entry(o.contract_id).or_default().insert(o.side, p, o.id);
        }
        if o.status == OrderStatus::Open {
            self.by_account.entry(o.account_id).or_default().insert(o.id);
        }
        self.orders.insert(o.id, o);
    }

    /// Take an order that is no longer open off its book and its account's list (the row stays in `orders`
    /// until the operation ends).
    pub fn unlist_order(&mut self, id: i64) {
        let Some(o) = self.orders.get(&id) else { return };
        if let (Some(p), Some(b)) = (o.price, self.books.get_mut(&o.contract_id)) {
            b.remove(o.side, p, o.id);
        }
        if let Some(ids) = self.by_account.get_mut(&o.account_id) {
            ids.remove(&id);
        }
    }

    pub(crate) fn book(&self, contract_id: i64) -> &Book {
        self.books.get(&contract_id).unwrap_or(Book::empty())
    }

    pub fn position(&self, account_id: i64, contract_id: i64) -> i64 {
        self.positions.get(&(account_id, contract_id)).map_or(0, |p| p.qty)
    }

    /// Open interest: the long side of every position.
    pub fn open_interest(&self, contract_id: i64) -> i64 {
        self.positions.values().filter(|p| p.contract_id == contract_id && p.qty > 0).map(|p| p.qty).sum()
    }

    /// The listed contracts on `work_id`, earliest period first.
    pub fn on_work(&self, work_id: i64) -> impl Iterator<Item = &Contract> {
        self.contracts.values().filter(move |c| c.work_id == work_id)
    }
}

#[cfg(test)]
mod tests;
