//! One instrument's order book: the resting limit orders' ids by price, then time. It holds ids only;
//! how much an order has left is the venue's business, so every query that needs quantities takes a
//! `left(id)` lookup.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound::{Excluded, Unbounded};

use serde::Serialize;

use crate::model::Side;

/// Aggregated depth: (price, total remaining qty, number of orders).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Level {
    pub price: i64,
    pub qty: i64,
    pub orders: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BookView {
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct Book {
    /// (Reverse(price), id): best (highest) price first, then time
    bids: BTreeSet<(Reverse<i64>, i64)>,
    /// (price, id): best (lowest) price first, then time
    asks: BTreeSet<(i64, i64)>,
}

static EMPTY: Book = Book { bids: BTreeSet::new(), asks: BTreeSet::new() };

impl Book {
    /// The book of an instrument nobody has ordered yet.
    pub fn empty() -> &'static Book {
        &EMPTY
    }

    pub fn insert(&mut self, side: Side, price: i64, id: i64) {
        match side {
            Side::Buy => self.bids.insert((Reverse(price), id)),
            Side::Sell => self.asks.insert((price, id)),
        };
    }

    pub fn remove(&mut self, side: Side, price: i64, id: i64) {
        match side {
            Side::Buy => self.bids.remove(&(Reverse(price), id)),
            Side::Sell => self.asks.remove(&(price, id)),
        };
    }

    /// Resting buys as (price, id), best first.
    pub fn bids(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.bids.iter().map(|(Reverse(p), id)| (*p, *id))
    }

    /// Resting sells as (price, id), best first.
    pub fn asks(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.asks.iter().copied()
    }

    /// The next resting order after `after` on the side an incoming `side` order trades against, if its
    /// price still crosses `limit`.
    pub fn next_against(&self, side: Side, limit: i64, after: Option<(i64, i64)>) -> Option<(i64, i64)> {
        match side {
            Side::Buy => {
                let mut it = match after {
                    None => self.asks.range(..),
                    Some(k) => self.asks.range((Excluded(k), Unbounded)),
                };
                it.next().filter(|(p, _)| *p <= limit).copied()
            }
            Side::Sell => {
                let mut it = match after {
                    None => self.bids.range(..),
                    Some((p, id)) => self.bids.range((Excluded((Reverse(p), id)), Unbounded)),
                };
                it.next().filter(|(Reverse(p), _)| *p >= limit).map(|(Reverse(p), id)| (*p, *id))
            }
        }
    }

    /// Price levels, best first, at most `depth` per side.
    pub fn depth(&self, depth: usize, left: impl Fn(i64) -> i64) -> BookView {
        let levels = |orders: &mut dyn Iterator<Item = (i64, i64)>| {
            let mut out: Vec<Level> = Vec::new();
            for (price, id) in orders {
                if let Some(l) = out.last_mut().filter(|l| l.price == price) {
                    l.qty += left(id);
                    l.orders += 1;
                } else if out.len() == depth {
                    break;
                } else {
                    out.push(Level { price, qty: left(id), orders: 1 });
                }
            }
            out
        };
        BookView { bids: levels(&mut self.bids()), asks: levels(&mut self.asks()) }
    }

    /// Equilibrium price of the resting orders: most volume, then least imbalance, then closest to
    /// `reference`. `(None, 0)` if nothing crosses.
    pub fn auction(&self, left: impl Fn(i64) -> i64, reference: i64) -> (Option<i64>, i64) {
        let mut buys: BTreeMap<i64, i64> = BTreeMap::new();
        let mut sells: BTreeMap<i64, i64> = BTreeMap::new();
        for (p, id) in self.bids() {
            *buys.entry(p).or_default() += left(id);
        }
        for (p, id) in self.asks() {
            *sells.entry(p).or_default() += left(id);
        }
        if buys.is_empty() || sells.is_empty() {
            return (None, 0);
        }
        let prices: BTreeSet<i64> = buys.keys().chain(sells.keys()).copied().collect();
        let mut best: Option<((i64, i64, i64), i64, i64)> = None;
        for p in prices {
            let demand: i64 = buys.range(p..).map(|(_, q)| q).sum();
            let supply: i64 = sells.range(..=p).map(|(_, q)| q).sum();
            let vol = demand.min(supply);
            if vol <= 0 {
                continue;
            }
            let key = (vol, -(demand - supply).abs(), -(p - reference).abs());
            if best.as_ref().is_none_or(|b| key > b.0) {
                best = Some((key, p, vol));
            }
        }
        match best {
            Some((_, p, v)) => (Some(p), v),
            None => (None, 0),
        }
    }

    /// The ids of the buys at or above `price` and the sells at or below it, best first.
    pub fn crossing(&self, price: i64) -> (Vec<i64>, Vec<i64>) {
        let buys = self.bids().take_while(|(p, _)| *p >= price).map(|(_, id)| id).collect();
        let sells = self.asks().take_while(|(p, _)| *p <= price).map(|(_, id)| id).collect();
        (buys, sells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(bids: &[(i64, i64)], asks: &[(i64, i64)]) -> Book {
        let mut b = Book::default();
        for &(p, id) in bids {
            b.insert(Side::Buy, p, id);
        }
        for &(p, id) in asks {
            b.insert(Side::Sell, p, id);
        }
        b
    }

    #[test]
    fn price_then_time() {
        let b = book(&[(100, 2), (101, 3), (100, 1)], &[(105, 5), (104, 6), (105, 4)]);
        assert_eq!(b.bids().collect::<Vec<_>>(), [(101, 3), (100, 1), (100, 2)]);
        assert_eq!(b.asks().collect::<Vec<_>>(), [(104, 6), (105, 4), (105, 5)]);
        assert_eq!(b.next_against(Side::Buy, 105, None), Some((104, 6)));
        assert_eq!(b.next_against(Side::Buy, 105, Some((104, 6))), Some((105, 4)));
        assert_eq!(b.next_against(Side::Buy, 104, Some((104, 6))), None);
        assert_eq!(b.next_against(Side::Sell, 100, Some((101, 3))), Some((100, 1)));
    }

    #[test]
    fn depth_aggregates_levels() {
        let b = book(&[(100, 1), (100, 2), (99, 3), (98, 4)], &[]);
        let v = b.depth(2, |_| 10);
        assert_eq!(v.bids, [Level { price: 100, qty: 20, orders: 2 }, Level { price: 99, qty: 10, orders: 1 }]);
        assert!(v.asks.is_empty());
    }

    #[test]
    fn auction_and_crossing() {
        let b = book(&[(510, 1), (500, 2)], &[(490, 3), (505, 4)]);
        // every price clears 10 with the same imbalance: the one closest to the reference wins
        assert_eq!(b.auction(|_| 10, 500), (Some(500), 10));
        assert_eq!(b.auction(|_| 10, 508), (Some(510), 10));
        assert_eq!(b.crossing(500), (vec![1, 2], vec![3]));
        assert_eq!(Book::empty().auction(|_| 1, 500), (None, 0));
    }
}
