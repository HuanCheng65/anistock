//! Pure exchange state: books, accounts, holdings, orders, counters.
//! Owned and mutated by exactly one thread (the engine). No async in here.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

pub const NSYM: usize = 141;
pub const NACCT: usize = 11141; // 0..=11140
pub const START_CASH: i64 = 1_000_000_000_000;
pub const START_SHARES: i64 = 1_000_000;
pub const MAX_QTY: i64 = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Side {
    B,
    S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum OType {
    L,
    M,
}

impl Side {
    fn as_str(self) -> &'static str {
        match self {
            Side::B => "B",
            Side::S => "S",
        }
    }
}
impl OType {
    fn as_str(self) -> &'static str {
        match self {
            OType::L => "L",
            OType::M => "M",
        }
    }
}

pub fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

#[derive(Debug)]
struct Order {
    acct: u32,
    sym: u16,
    side: Side,
    px: i64, // limit px (0 for market; market orders never rest)
    qty: i64,
    filled: i64,
    p_reserve: i64, // buy: px (limit) or last_at_entry*2 (market)
}

#[derive(Default)]
struct Level {
    qty: i64,            // live aggregate qty at this level
    q: VecDeque<u64>,    // FIFO of order ids; may contain dead (cancelled) ids
}

#[derive(Default)]
struct Book {
    bids: BTreeMap<i64, Level>, // best = highest key
    asks: BTreeMap<i64, Level>, // best = lowest key
}

#[derive(Clone, Copy)]
struct Holding {
    shares: i64,
    frozen: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Filled,
    Open,
    Cancelled,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Filled => "filled",
            Status::Open => "open",
            Status::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub id: u64,
    pub filled: i64,
    pub status: Status,
}

// ---- log line shapes ----
#[derive(Serialize)]
struct LogO {
    e: &'static str,
    id: u64,
    acct: u32,
    sym: u16,
    side: &'static str,
    #[serde(rename = "type")]
    typ: &'static str,
    qty: i64,
    px: i64,
    ts: i64,
}
#[derive(Serialize)]
struct LogT {
    e: &'static str,
    sym: u16,
    px: i64,
    qty: i64,
    buy: u64,
    sell: u64,
    ts: i64,
}
#[derive(Serialize)]
struct LogC {
    e: &'static str,
    id: u64,
    ts: i64,
}

pub struct Exchange {
    pub base: Vec<i64>,
    pub last: Vec<i64>,
    pub vol: Vec<i64>,
    cash: Vec<i64>,
    cash_frozen: Vec<i64>,
    holdings: HashMap<(u32, u16), Holding>,
    orders: HashMap<u64, Order>,
    books: Vec<Book>,
    next_id: u64,
    pub trades: u64,
    log: Option<Sender<Vec<u8>>>,
}

fn holding(h: &mut HashMap<(u32, u16), Holding>, acct: u32, sym: u16) -> &mut Holding {
    h.entry((acct, sym)).or_insert(Holding {
        shares: START_SHARES,
        frozen: 0,
    })
}

impl Exchange {
    pub fn new(log: Option<Sender<Vec<u8>>>) -> Self {
        let base: Vec<i64> = (0..NSYM as i64).map(|s| 1000 + 100 * s).collect();
        Exchange {
            last: base.clone(),
            base,
            vol: vec![0; NSYM],
            cash: vec![START_CASH; NACCT],
            cash_frozen: vec![0; NACCT],
            holdings: HashMap::new(),
            orders: HashMap::new(),
            books: (0..NSYM).map(|_| Book::default()).collect(),
            next_id: 1,
            trades: 0,
            log,
        }
    }

    fn emit<T: Serialize>(&self, v: &T) {
        if let Some(tx) = &self.log {
            let mut line = serde_json::to_vec(v).expect("serialize log line");
            line.push(b'\n');
            let _ = tx.send(line);
        }
    }

    /// Validate, accept, match and (maybe) rest an order.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &mut self,
        acct: i64,
        sym: i64,
        side: Side,
        typ: OType,
        qty: i64,
        px: Option<i64>,
    ) -> Result<Placed, &'static str> {
        if !(0..NACCT as i64).contains(&acct) {
            return Err("bad acct");
        }
        if !(0..NSYM as i64).contains(&sym) {
            return Err("bad sym");
        }
        if !(1..=MAX_QTY).contains(&qty) {
            return Err("bad qty");
        }
        let a = acct as usize;
        let s = sym as usize;
        let last = self.last[s];
        let px = match typ {
            OType::L => {
                let p = px.ok_or("px required")?;
                if p < last / 2 || p > last * 2 {
                    return Err("px out of band");
                }
                p
            }
            OType::M => 0,
        };
        let p_reserve = match (side, typ) {
            (Side::B, OType::L) => px,
            (Side::B, OType::M) => last * 2,
            (Side::S, _) => 0,
        };
        match side {
            Side::B => {
                let reserve = qty * p_reserve;
                if self.cash[a] - self.cash_frozen[a] < reserve {
                    return Err("insufficient cash");
                }
                self.cash_frozen[a] += reserve;
            }
            Side::S => {
                let h = holding(&mut self.holdings, acct as u32, sym as u16);
                if h.shares - h.frozen < qty {
                    return Err("insufficient shares");
                }
                h.frozen += qty;
            }
        }

        let id = self.next_id;
        self.next_id += 1;
        let ts = now_us();
        self.emit(&LogO {
            e: "o",
            id,
            acct: acct as u32,
            sym: sym as u16,
            side: side.as_str(),
            typ: typ.as_str(),
            qty,
            px,
            ts,
        });

        let acct = acct as u32;
        let sym16 = sym as u16;
        let mut filled = 0i64;
        let mut stp = false;

        // ---- matching ----
        loop {
            if filled == qty {
                break;
            }
            let book = &mut self.books[s];
            let opp = match side {
                Side::B => &mut book.asks,
                Side::S => &mut book.bids,
            };
            let best = match side {
                Side::B => opp.keys().next().copied(),
                Side::S => opp.keys().next_back().copied(),
            };
            let Some(bpx) = best else { break };
            if typ == OType::L {
                let crosses = match side {
                    Side::B => bpx <= px,
                    Side::S => bpx >= px,
                };
                if !crosses {
                    break;
                }
            }
            let level = opp.get_mut(&bpx).expect("level");
            // skip dead ids at the front
            let rid = loop {
                let id = *level.q.front().expect("live level has live order");
                if self.orders.contains_key(&id) {
                    break id;
                }
                level.q.pop_front();
            };
            let resting = self.orders.get_mut(&rid).expect("resting");
            if resting.acct == acct {
                stp = true;
                break;
            }
            let q = (qty - filled).min(resting.qty - resting.filled);
            let p = bpx;
            resting.filled += q;
            filled += q;
            level.qty -= q;
            let resting_acct = resting.acct;
            let resting_reserve = resting.p_reserve;
            let resting_done = resting.filled == resting.qty;
            if resting_done {
                level.q.pop_front();
                self.orders.remove(&rid);
            }
            if level.qty == 0 {
                opp.remove(&bpx);
            }

            // settlement
            let (buyer, seller, buy_reserve, buy_id, sell_id) = match side {
                Side::B => (acct, resting_acct, p_reserve, id, rid),
                Side::S => (resting_acct, acct, resting_reserve, rid, id),
            };
            self.cash[buyer as usize] -= q * p;
            self.cash_frozen[buyer as usize] -= q * buy_reserve;
            self.cash[seller as usize] += q * p;
            {
                let hs = holding(&mut self.holdings, seller, sym16);
                hs.shares -= q;
                hs.frozen -= q;
            }
            holding(&mut self.holdings, buyer, sym16).shares += q;
            self.last[s] = p;
            self.vol[s] += q;
            self.trades += 1;
            self.emit(&LogT {
                e: "t",
                sym: sym16,
                px: p,
                qty: q,
                buy: buy_id,
                sell: sell_id,
                ts: now_us(),
            });
        }

        let status = if filled == qty {
            Status::Filled
        } else if stp || typ == OType::M {
            // remainder cancelled: release what is still frozen
            let rem = qty - filled;
            match side {
                Side::B => self.cash_frozen[acct as usize] -= rem * p_reserve,
                Side::S => holding(&mut self.holdings, acct, sym16).frozen -= rem,
            }
            Status::Cancelled
        } else {
            let rem = qty - filled;
            let book = &mut self.books[s];
            let own = match side {
                Side::B => &mut book.bids,
                Side::S => &mut book.asks,
            };
            let level = own.entry(px).or_default();
            level.qty += rem;
            level.q.push_back(id);
            self.orders.insert(
                id,
                Order {
                    acct,
                    sym: sym16,
                    side,
                    px,
                    qty,
                    filled,
                    p_reserve,
                },
            );
            Status::Open
        };
        Ok(Placed { id, filled, status })
    }

    /// Cancel an open order owned by `acct`. Returns the symbol on success.
    pub fn cancel(&mut self, acct: i64, id: u64) -> Option<usize> {
        match self.orders.get(&id) {
            Some(o) if o.acct as i64 == acct => {}
            _ => return None,
        }
        let o = self.orders.remove(&id).expect("order");
        let s = o.sym as usize;
        let rem = o.qty - o.filled;
        let book = &mut self.books[s];
        let side_book = match o.side {
            Side::B => &mut book.bids,
            Side::S => &mut book.asks,
        };
        let level = side_book.get_mut(&o.px).expect("level of open order");
        level.qty -= rem;
        // id stays in the FIFO as a dead entry (skipped lazily by matching);
        // drop the whole level once nothing live remains.
        if level.qty == 0 {
            side_book.remove(&o.px);
        }
        match o.side {
            Side::B => self.cash_frozen[o.acct as usize] -= rem * o.p_reserve,
            Side::S => holding(&mut self.holdings, o.acct, o.sym).frozen -= rem,
        }
        self.emit(&LogC {
            e: "c",
            id,
            ts: now_us(),
        });
        Some(s)
    }

    /// Top 5 levels per side, best first, qty aggregated.
    pub fn depth(&self, sym: usize) -> (Vec<[i64; 2]>, Vec<[i64; 2]>) {
        let b = &self.books[sym];
        let bids = b.bids.iter().rev().take(5).map(|(p, l)| [*p, l.qty]).collect();
        let asks = b.asks.iter().take(5).map(|(p, l)| [*p, l.qty]).collect();
        (bids, asks)
    }

    pub fn ref_px(&self, sym: usize) -> i64 {
        (8 * self.last[sym] + 2 * self.base[sym]) / 10
    }

    pub fn is_open(&self, id: u64) -> bool {
        self.orders.contains_key(&id)
    }

    #[cfg(test)]
    fn cash_of(&self, a: usize) -> (i64, i64) {
        (self.cash[a], self.cash_frozen[a])
    }
    #[cfg(test)]
    fn shares_of(&self, a: u32, s: u16) -> (i64, i64) {
        self.holdings
            .get(&(a, s))
            .map(|h| (h.shares, h.frozen))
            .unwrap_or((START_SHARES, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_time_priority() {
        let mut ex = Exchange::new(None);
        // sym 5: last = 1500
        let a = ex.place(1, 5, Side::S, OType::L, 10, Some(1510)).unwrap();
        let b = ex.place(2, 5, Side::S, OType::L, 10, Some(1500)).unwrap();
        let c = ex.place(3, 5, Side::S, OType::L, 10, Some(1500)).unwrap();
        // buy 15 @ 1510: should take b (best price, earliest) fully, then 5 of c
        let r = ex.place(4, 5, Side::B, OType::L, 15, Some(1510)).unwrap();
        assert_eq!(r.status, Status::Filled);
        assert_eq!(r.filled, 15);
        assert!(!ex.is_open(b.id));
        assert!(ex.is_open(c.id));
        assert!(ex.is_open(a.id));
        assert_eq!(ex.last[5], 1500);
        assert_eq!(ex.vol[5], 15);
        let (_, asks) = ex.depth(5);
        assert_eq!(asks, vec![[1500, 5], [1510, 10]]);
        // trade at resting price: buyer paid 15*1500, all frozen released
        assert_eq!(ex.cash_of(4), (START_CASH - 15 * 1500, 0));
        assert_eq!(ex.shares_of(4, 5), (START_SHARES + 15, 0));
        assert_eq!(ex.shares_of(2, 5), (START_SHARES - 10, 0));
        assert_eq!(ex.shares_of(3, 5), (START_SHARES - 5, 5));
    }

    #[test]
    fn self_trade_cancels_incoming_remainder() {
        let mut ex = Exchange::new(None);
        ex.place(1, 0, Side::S, OType::L, 5, Some(1000)).unwrap();
        let own = ex.place(7, 0, Side::S, OType::L, 5, Some(1001)).unwrap();
        // acct 7 buys 10 @ 1001: fills 5 from acct 1, then hits own ask -> stop
        let r = ex.place(7, 0, Side::B, OType::L, 10, Some(1001)).unwrap();
        assert_eq!(r.filled, 5);
        assert_eq!(r.status, Status::Cancelled);
        assert!(!ex.is_open(r.id));
        assert!(ex.is_open(own.id));
        // remaining frozen cash released; only the trade is charged
        assert_eq!(ex.cash_of(7), (START_CASH - 5 * 1000, 0));
        let (bids, asks) = ex.depth(0);
        assert!(bids.is_empty());
        assert_eq!(asks, vec![[1001, 5]]);
        // non-crossing limit next to own order is not affected by STP
        let r2 = ex.place(7, 0, Side::B, OType::L, 3, Some(990)).unwrap();
        assert_eq!(r2.status, Status::Open);
    }

    #[test]
    fn frozen_released_on_cancel_and_market() {
        let mut ex = Exchange::new(None);
        let b = ex.place(1, 0, Side::B, OType::L, 10, Some(1100)).unwrap();
        assert_eq!(ex.cash_of(1), (START_CASH, 11000));
        assert_eq!(ex.cancel(2, b.id), None); // wrong account
        assert_eq!(ex.cancel(1, b.id), Some(0));
        assert_eq!(ex.cancel(1, b.id), None); // already closed
        assert_eq!(ex.cash_of(1), (START_CASH, 0));
        assert!(ex.depth(0).0.is_empty());

        // market buy with partial liquidity: remainder cancelled, reserve released
        ex.place(2, 0, Side::S, OType::L, 4, Some(1200)).unwrap();
        let m = ex.place(3, 0, Side::B, OType::M, 10, None).unwrap();
        assert_eq!(m.filled, 4);
        assert_eq!(m.status, Status::Cancelled);
        assert_eq!(ex.cash_of(3), (START_CASH - 4 * 1200, 0));
        assert_eq!(ex.last[0], 1200);

        // sell cancel releases frozen shares
        let s = ex.place(4, 1, Side::S, OType::L, 30, Some(1100)).unwrap();
        assert_eq!(ex.shares_of(4, 1), (START_SHARES, 30));
        ex.cancel(4, s.id).unwrap();
        assert_eq!(ex.shares_of(4, 1), (START_SHARES, 0));
    }

    #[test]
    fn validation() {
        let mut ex = Exchange::new(None);
        assert!(ex.place(1, 0, Side::B, OType::L, 1, Some(499)).is_err());
        assert!(ex.place(1, 0, Side::B, OType::L, 1, Some(2001)).is_err());
        assert!(ex.place(1, 0, Side::B, OType::L, 1, Some(500)).is_ok());
        assert!(ex.place(1, 141, Side::B, OType::L, 1, Some(1000)).is_err());
        assert!(ex.place(11141, 0, Side::B, OType::L, 1, Some(1000)).is_err());
        assert!(ex.place(1, 0, Side::B, OType::L, 100_001, Some(1000)).is_err());
        assert!(ex.place(1, 0, Side::S, OType::L, 100_000, Some(1000)).is_ok());
        // lazily-skipped dead entry ahead of a live one
        let mut ex = Exchange::new(None);
        let a = ex.place(1, 0, Side::S, OType::L, 5, Some(1000)).unwrap();
        let b = ex.place(2, 0, Side::S, OType::L, 5, Some(1000)).unwrap();
        ex.cancel(1, a.id).unwrap();
        assert_eq!(ex.depth(0).1, vec![[1000, 5]]);
        let r = ex.place(3, 0, Side::B, OType::L, 5, Some(1000)).unwrap();
        assert_eq!(r.status, Status::Filled);
        assert!(!ex.is_open(b.id));
        assert!(ex.depth(0).1.is_empty());
    }
}
