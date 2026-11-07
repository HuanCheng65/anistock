//! The futures market maker (docs/v3-futures.md §4.3): one per contract, quoting around the model's forecast
//! of the period average (leaning a little toward the last price), the spread widening with the forecast's
//! uncertainty, the centre skewed against its inventory, within a position limit. Its account (`fmm`) gets
//! capital from world for every contract listed and returns it when the contract settles; what it wins or
//! loses stays in the money stock's books. The market module schedules it (`Market::step_until`).

use super::trading::FutOrderReq;
use super::FuturesRules;
use crate::engine::Engine;
use crate::model::{Kind, OrderStatus, Side};

/// Price levels to quote: (price, lots), best first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quotes {
    pub mid: i64,
    pub bids: Vec<(i64, i64)>,
    pub asks: Vec<(i64, i64)>,
}

/// The quotes for a forecast `mean` with spread `sd` (ticks), the `last` price and the maker's `position`.
pub fn quotes(r: &FuturesRules, mean: i64, sd: i64, last: i64, position: i64) -> Quotes {
    let reference = (1.0 - r.mm_last_weight) * mean as f64 + r.mm_last_weight * last as f64;
    let half = (r.mm_spread_sd * sd as f64).max(r.mm_min_half as f64);
    let centre = reference - r.mm_skew * half * position as f64 / r.mm_limit.max(1) as f64;
    let (mut room_buy, mut room_sell) = ((r.mm_limit - position).max(0), (r.mm_limit + position).max(0));
    let mut q = Quotes { mid: centre.round() as i64, ..Quotes::default() };
    for i in 0..r.mm_levels {
        let off = half * (1 + i) as f64;
        let (bid, ask) = (((centre - off).floor() as i64).max(1), ((centre + off).ceil() as i64).max(2));
        let (b, a) = (r.mm_size.min(room_buy), r.mm_size.min(room_sell));
        if b > 0 {
            q.bids.push((bid, b));
            room_buy -= b;
        }
        if a > 0 {
            q.asks.push((ask, a));
            room_sell -= a;
        }
    }
    q
}

impl Engine {
    /// The market maker's account (created on first use).
    pub(crate) fn fmm_account(&mut self) -> i64 {
        match self.special("fmm") {
            Some(id) => id,
            None => self.do_create_account("fmm", "期货做市商", None, None, None, "1970-01-01T00:00:00"),
        }
    }

    /// Capital for a contract just listed, from world.
    pub(crate) fn fmm_fund(&mut self, contract_id: i64, day: &str, ts: &str) {
        let (acct, world) = (self.fmm_account(), self.special("world").expect("no world account"));
        let company = self.st.futures.contracts[&contract_id].company_id;
        let amount = self.futures.mm_capital;
        self.do_transfer(world, acct, amount, "futures_mm_fund", day, ts, Some(company), Some(contract_id));
        self.st.futures.mm_funded += amount;
    }

    /// A contract settled: its capital goes back to world, as far as the maker's free cash goes.
    pub(crate) fn fmm_return(&mut self, contract_id: i64, company_id: i64, day: &str, ts: &str) {
        let Some(acct) = self.special("fmm") else { return };
        let world = self.special("world").expect("no world account");
        let amount = self.futures.mm_capital.min(self.available_cash(acct)).max(0);
        self.do_transfer(acct, world, amount, "futures_mm_return", day, ts, Some(company_id), Some(contract_id));
        self.st.futures.mm_funded -= amount;
    }

    /// Requote one contract: cancel the maker's orders on it, place new ones, record the mid (the settlement
    /// price when nothing trades). Returns the quotes placed.
    pub fn futures_mm_requote(&mut self, contract_id: i64, day: &str, ts: &str) -> Quotes {
        let Some(c) = self.st.futures.contracts.get(&contract_id).filter(|c| c.trading()).cloned() else {
            return Quotes::default();
        };
        let acct = self.fmm_account();
        let mine: Vec<i64> = self.st.futures.open_orders(acct, contract_id).map(|o| o.id).collect();
        for id in mine {
            self.fut_close(id, OrderStatus::Cancelled, ts);
        }
        let f = &self.st.futures;
        let mean = f.mean.get(&contract_id).copied().unwrap_or(c.forecast);
        let sd = f.sd.get(&contract_id).copied().unwrap_or(0);
        let q = quotes(&self.futures, mean, sd, c.last_price, f.position(acct, contract_id));
        self.st.futures.last_mid.insert(contract_id, q.mid);
        let levels =
            q.bids.iter().map(|&(p, n)| (Side::Buy, p, n)).chain(q.asks.iter().map(|&(p, n)| (Side::Sell, p, n)));
        for (side, price, qty) in levels.collect::<Vec<_>>() {
            let req = FutOrderReq {
                account_id: acct,
                contract_id,
                side,
                kind: Kind::Limit,
                qty,
                price: Some(price),
                day,
                ts,
                match_: true,
            };
            if self.do_futures_place(&req).is_err() {
                break; // out of capital
            }
        }
        self.st.futures.reap();
        self.commit();
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_widen_with_uncertainty_and_lean_against_inventory() {
        let r = FuturesRules::default();
        let flat = quotes(&r, 9000, 400, 9000, 0);
        assert_eq!(flat.mid, 9000);
        assert_eq!(flat.bids, [(8900, 10), (8800, 10), (8700, 10)]);
        assert_eq!(flat.asks, [(9100, 10), (9200, 10), (9300, 10)]);
        let tight = quotes(&r, 9000, 40, 9000, 0);
        assert_eq!((tight.bids[0].0, tight.asks[0].0), (8980, 9020), "at least mm_min_half");
        // long 100 of a 200 limit: the centre moves half a half-spread down
        let long = quotes(&r, 9000, 400, 9000, 100);
        assert_eq!(long.mid, 8950);
        // at the limit: no more bids
        let full = quotes(&r, 9000, 400, 9000, 200);
        assert!(full.bids.is_empty() && full.asks.len() == 3);
        // the last price pulls the centre a fifth of the way
        assert_eq!(quotes(&r, 9000, 400, 10000, 0).mid, 9200);
    }
}
