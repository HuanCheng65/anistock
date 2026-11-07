//! The matching layer every instrument shares: price-time priority, continuous matching (`sweep`), the
//! opening auction (`uncross`), no self-trades. What a fill does to cash and positions is the venue's
//! business (stocks: `engine.rs`; futures: `futures/`); the price rules (volatility control) are in
//! `exchange.rs`.

mod book;

pub(crate) use book::Book;
pub use book::{BookView, Level};

use crate::model::Side;

/// A resting order as the matcher sees it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resting {
    pub account: i64,
    pub left: i64,
}

/// The order being matched: `limit` is its price, or the edge of the sanity band for a market order.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Incoming {
    pub id: i64,
    pub account: i64,
    pub side: Side,
    pub limit: i64,
    pub left: i64,
}

/// One instrument's market as the matcher needs it.
pub(crate) trait Venue {
    fn book(&self) -> &Book;
    /// The account and quantity left of a resting order; None once it is no longer open.
    fn resting(&self, id: i64) -> Option<Resting>;
    /// May a trade print at `price` now? (Volatility control: may start a cooling-off period.)
    fn allow(&mut self, price: i64) -> bool;
    /// How much of `want` the incoming order can still take at `price` (what its account can pay for).
    fn capacity(&mut self, price: i64, want: i64) -> i64;
    /// Settle one trade.
    fn fill(&mut self, buy: i64, sell: i64, qty: i64, price: i64);
}

/// Match an incoming order against the book, best price first. Stops at the first price the volatility
/// control will not let print. Returns (quantity filled, notional).
pub(crate) fn sweep(v: &mut impl Venue, o: Incoming) -> (i64, i64) {
    let (mut left, mut filled, mut notional) = (o.left, 0, 0);
    let mut after = None;
    while let Some((price, rid)) = v.book().next_against(o.side, o.limit, after) {
        after = Some((price, rid));
        if left <= 0 {
            break;
        }
        let Some(r) = v.resting(rid) else { continue };
        if r.account == o.account {
            continue; // no self-trades
        }
        if !v.allow(price) {
            break;
        }
        let q = v.capacity(price, left.min(r.left));
        if q <= 0 {
            break;
        }
        let (buy, sell) = match o.side {
            Side::Buy => (o.id, rid),
            Side::Sell => (rid, o.id),
        };
        v.fill(buy, sell, q, price);
        left -= q;
        filled += q;
        notional += q * price;
    }
    (filled, notional)
}

/// Clear the opening auction at `price` for up to `volume`: crossing buys in priority order, each against
/// the crossing sells in priority order. Returns the quantity traded.
pub(crate) fn uncross(v: &mut impl Venue, price: i64, volume: i64) -> i64 {
    let (buys, sells) = v.book().crossing(price);
    let mut left = volume;
    'buys: for &b in &buys {
        if left <= 0 {
            break;
        }
        for &s in &sells {
            if left <= 0 {
                break;
            }
            let Some(buy) = v.resting(b) else { continue 'buys };
            let Some(sell) = v.resting(s) else { continue };
            if sell.account == buy.account {
                continue;
            }
            let q = buy.left.min(sell.left).min(left);
            if q > 0 {
                v.fill(b, s, q, price);
                left -= q;
            }
        }
    }
    volume - left
}
