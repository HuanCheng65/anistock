//! Simulated players running the exploit strategies of docs/v2-npc-market.md §6, each with a real
//! player's starting cash, trading through the same engine as everyone else.

use serde::Serialize;

use crate::engine::{Engine, NewOrder};
use crate::exchange::cooling;
use crate::market::{day_of, local, ts_of};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BotKind {
    /// buy every listed stock equally on day one, hold (the baseline)
    BuyHold,
    /// buy every listed stock in proportion to its market value on day one, hold (second baseline)
    BuyHoldCap,
    /// repeatedly buy at the market maker's bid +1 tick and sell at its ask -1 tick
    Scalper,
    /// the same, crossing the spread (buy the ask, sell the bid)
    ScalperTaker,
    /// buy below the hidden value (known: the formula is public), sell above it
    Value,
    /// buy below the hidden value and hold to the end of the season
    ValueHold,
    /// push a stock up just short of the VCM, wait for trend followers, sell
    Pump,
    /// place and cancel large bids to lift the market maker's quotes, sell into them
    Spoof,
    /// push a stock into a VCM cooling-off, then trade against the move
    VcmFade,
}

impl BotKind {
    pub fn all() -> [BotKind; 9] {
        [
            BotKind::BuyHold,
            BotKind::BuyHoldCap,
            BotKind::Scalper,
            BotKind::ScalperTaker,
            BotKind::Value,
            BotKind::ValueHold,
            BotKind::Pump,
            BotKind::Spoof,
            BotKind::VcmFade,
        ]
    }
    pub fn name(self) -> &'static str {
        match self {
            BotKind::BuyHold => "buy_hold",
            BotKind::BuyHoldCap => "buy_hold_cap",
            BotKind::Scalper => "scalper",
            BotKind::ScalperTaker => "scalper_taker",
            BotKind::Value => "value",
            BotKind::ValueHold => "value_hold",
            BotKind::Pump => "pump",
            BotKind::Spoof => "spoof",
            BotKind::VcmFade => "vcm_fade",
        }
    }
    pub fn parse(s: &str) -> Option<BotKind> {
        BotKind::all().into_iter().find(|b| b.name() == s)
    }
}

pub struct Bot {
    pub kind: BotKind,
    pub account: i64,
    pub start_cash: i64,
    bought_initial: bool,
    target: Option<i64>,
    state: u32,
    since: i64,
    pub trades_placed: u64,
}

fn place(e: &mut Engine, acct: i64, c: i64, side: &str, kind: &str, qty: i64, price: Option<i64>, t: i64) -> bool {
    if qty <= 0 {
        return false;
    }
    let (day, ts) = (day_of(t), ts_of(t));
    if e.state("auction_day") != Some(day.as_str()) {
        e.open_market(&day, &ts);
    }
    e.place_order(&NewOrder {
        account_id: acct,
        company_id: c,
        side,
        kind,
        qty,
        price,
        day: &day,
        ts: &ts,
        match_: true,
    })
    .is_ok()
}

fn hottest(e: &Engine, heat: &dyn Fn(i64) -> f64, n: usize) -> Vec<i64> {
    let mut v: Vec<(f64, i64)> = e.companies().filter(|c| c.status == "listed").map(|c| (heat(c.id), c.id)).collect();
    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
    v.into_iter().take(n).map(|x| x.1).collect()
}

fn affordable(e: &Engine, budget: i64, price: i64) -> i64 {
    if price <= 0 {
        0
    } else {
        budget / e.fees.buy_reserve(price, 1)
    }
}

impl Bot {
    pub fn new(kind: BotKind, account: i64, start_cash: i64) -> Bot {
        Bot { kind, account, start_cash, bought_initial: false, target: None, state: 0, since: 0, trades_placed: 0 }
    }

    /// Wealth at last prices, cents.
    pub fn wealth(&self, e: &Engine) -> i64 {
        let cash = e.account(self.account).map_or(0, |a| a.cash);
        cash + e
            .holdings_of(self.account)
            .map(|h| h.qty * e.company(h.company_id).map_or(0, |c| c.last_price))
            .sum::<i64>()
    }

    /// Called once per simulated minute during continuous trading.
    pub fn on_minute(&mut self, e: &mut Engine, t: i64, heat: &dyn Fn(i64) -> f64) {
        let (_, s) = local(t);
        let acct = self.account;
        let ts = ts_of(t);
        match self.kind {
            BotKind::BuyHold | BotKind::BuyHoldCap => {
                // day one, every 30 minutes until invested: equal budget per listed stock
                if self.bought_initial || s % 1800 != 0 {
                    return;
                }
                let listed: Vec<(i64, i64)> =
                    e.companies().filter(|c| c.status == "listed").map(|c| (c.id, c.shares * c.last_price)).collect();
                let cap_total: i64 = listed.iter().map(|x| x.1).sum::<i64>().max(1);
                let n = listed.len().max(1) as i64;
                let mut pending = false;
                for (c, cap) in listed {
                    let per = if self.kind == BotKind::BuyHoldCap {
                        (self.start_cash as i128 * cap as i128 / cap_total as i128) as i64
                    } else {
                        self.start_cash / n
                    };
                    let p = e.company(c).unwrap().last_price;
                    let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
                    if have < per * 8 / 10 {
                        e.cancel_all(acct, c, &ts);
                        let q = affordable(e, (per - have).min(e.available_cash(acct)), p * 102 / 100 + 1);
                        if place(e, acct, c, "buy", "limit", q, Some(p * 102 / 100 + 1), t) {
                            self.trades_placed += 1;
                        }
                        pending = true;
                    }
                }
                if !pending || s >= 22 * 3600 {
                    self.bought_initial = true;
                }
            }
            BotKind::Scalper | BotKind::ScalperTaker => {
                for c in hottest(e, heat, 5) {
                    let b = e.book(c, 1);
                    let (Some(bid), Some(ask)) = (b.bids.first().map(|l| l.price), b.asks.first().map(|l| l.price))
                    else {
                        continue;
                    };
                    let held = e.available_shares(acct, c) + e.holding(acct, c).map_or(0, |h| h.qty_frozen);
                    e.cancel_all(acct, c, &ts);
                    let held_now = e.available_shares(acct, c);
                    let budget = e.available_cash(acct) / 5;
                    if self.kind == BotKind::Scalper {
                        if ask - bid >= 3 {
                            if held_now == 0 {
                                let q = affordable(e, budget, bid + 1);
                                place(e, acct, c, "buy", "limit", q, Some(bid + 1), t);
                            } else {
                                place(e, acct, c, "sell", "limit", held_now, Some(ask - 1), t);
                            }
                            self.trades_placed += 1;
                        }
                    } else if held == 0 {
                        let q = affordable(e, budget, ask);
                        place(e, acct, c, "buy", "limit", q, Some(ask), t);
                        self.trades_placed += 1;
                    } else if held_now > 0 {
                        place(e, acct, c, "sell", "limit", held_now, Some(bid), t);
                        self.trades_placed += 1;
                    }
                }
            }
            BotKind::ValueHold => {
                // every hour: put up to 1/15 of wealth into each stock trading 10% or more under its value
                if s % 3600 != 0 || e.available_cash(acct) <= 0 {
                    return;
                }
                let listed: Vec<(i64, i64, i64)> = e
                    .companies()
                    .filter(|c| c.status == "listed" && c.fair_value > 0)
                    .map(|c| (c.id, c.last_price, c.fair_value))
                    .collect();
                let w = self.wealth(e);
                for (c, p, fair) in listed {
                    if (p as f64) < fair as f64 * 0.9 {
                        let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
                        e.cancel_all(acct, c, &ts);
                        let budget = (w / 15 - have).min(e.available_cash(acct));
                        let q = affordable(e, budget, p + 1);
                        if place(e, acct, c, "buy", "limit", q, Some(p + 1), t) {
                            self.trades_placed += 1;
                        }
                    }
                }
            }
            BotKind::Value => {
                if s % 3600 != 0 {
                    return;
                }
                let listed: Vec<(i64, i64, i64)> = e
                    .companies()
                    .filter(|c| c.status == "listed" && c.fair_value > 0)
                    .map(|c| (c.id, c.last_price, c.fair_value))
                    .collect();
                let w = self.wealth(e);
                for (c, p, fair) in listed {
                    let held = e.available_shares(acct, c);
                    e.cancel_all(acct, c, &ts);
                    if (p as f64) < fair as f64 * 0.85 {
                        let have = held * p;
                        let budget = (w / 15 - have).min(e.available_cash(acct));
                        let q = affordable(e, budget, p + 1);
                        if place(e, acct, c, "buy", "limit", q, Some(p + 1), t) {
                            self.trades_placed += 1;
                        }
                    } else if held > 0 && p as f64 > fair as f64 {
                        if place(e, acct, c, "sell", "limit", held, Some((p - 1).max(1)), t) {
                            self.trades_placed += 1;
                        }
                    }
                }
            }
            BotKind::Pump => {
                // 20:00 pump a mid-heat stock (rotating), hold 45 minutes, dump
                let mids = {
                    let mut v: Vec<(f64, i64)> =
                        e.companies().filter(|c| c.status == "listed").map(|c| (heat(c.id), c.id)).collect();
                    v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
                    let n = v.len();
                    v.into_iter().skip(n / 3).take((n / 3).max(1)).map(|x| x.1).collect::<Vec<_>>()
                };
                if mids.is_empty() {
                    return;
                }
                if s == 20 * 3600 && self.state == 0 {
                    let (d, _) = local(t);
                    let c = mids[(d as usize) % mids.len()];
                    let co = e.company(c).unwrap().clone();
                    let limit = co.last_price * 109 / 100;
                    let q = affordable(e, e.available_cash(acct) * 3 / 10, limit);
                    place(e, acct, c, "buy", "limit", q, Some(limit), t);
                    self.trades_placed += 1;
                    self.target = Some(c);
                    self.state = 1;
                    self.since = t;
                } else if self.state == 1 && t - self.since >= 45 * 60 {
                    let c = self.target.unwrap();
                    e.cancel_all(acct, c, &ts);
                    let held = e.available_shares(acct, c);
                    let p = e.company(c).unwrap().last_price;
                    place(e, acct, c, "sell", "limit", held, Some((p * 95 / 100).max(1)), t);
                    self.trades_placed += 1;
                    self.state = 2;
                    self.since = t;
                } else if self.state == 2 && t - self.since >= 15 * 60 {
                    // anything left: sell at the market, then idle until tomorrow
                    let c = self.target.unwrap();
                    e.cancel_all(acct, c, &ts);
                    let held = e.available_shares(acct, c);
                    place(e, acct, c, "sell", "market", held, None, t);
                    self.state = 0;
                }
                if s >= 22 * 3600 + 3000 {
                    self.state = 0;
                }
            }
            BotKind::Spoof => {
                // every 10 minutes on the hottest stock: buy a little at the ask, put a big bid just under the
                // best bid, wait 3 minutes, cancel it and sell at the bid
                let Some(&c) = hottest(e, heat, 1).first() else { return };
                let phase = (s / 60) % 10;
                let b = e.book(c, 1);
                let (Some(bid), Some(ask)) = (b.bids.first().map(|l| l.price), b.asks.first().map(|l| l.price)) else {
                    return;
                };
                if phase == 0 {
                    e.cancel_all(acct, c, &ts);
                    let q = affordable(e, e.available_cash(acct) / 20, ask);
                    place(e, acct, c, "buy", "limit", q, Some(ask), t);
                    let big = affordable(e, e.available_cash(acct) / 2, (bid - 1).max(1));
                    place(e, acct, c, "buy", "limit", big, Some((bid - 1).max(1)), t);
                    self.trades_placed += 2;
                } else if phase == 3 {
                    e.cancel_all(acct, c, &ts);
                    let held = e.available_shares(acct, c);
                    place(e, acct, c, "sell", "limit", held, Some(bid), t);
                    self.trades_placed += 1;
                }
            }
            BotKind::VcmFade => {
                // 19:00 buy the hottest stock up into a VCM cooling-off; sell when the cooling ends
                let Some(&c) = hottest(e, heat, 1).first() else { return };
                if s == 19 * 3600 && self.state == 0 {
                    let q =
                        affordable(e, e.available_cash(acct) * 4 / 10, e.company(c).unwrap().last_price * 115 / 100);
                    place(e, acct, c, "buy", "market", q, None, t);
                    self.trades_placed += 1;
                    self.state = 1;
                    self.since = t;
                    self.target = Some(c);
                } else if self.state == 1 {
                    let c = self.target.unwrap();
                    let co = e.company(c).unwrap().clone();
                    if cooling(&co, &ts).is_none() && t - self.since >= 60 {
                        let held = e.available_shares(acct, c);
                        place(e, acct, c, "sell", "limit", held, Some((co.last_price * 99 / 100).max(1)), t);
                        self.trades_placed += 1;
                        self.state = 2;
                        self.since = t;
                    }
                } else if self.state == 2 && t - self.since >= 30 * 60 {
                    let c = self.target.unwrap();
                    e.cancel_all(acct, c, &ts);
                    let held = e.available_shares(acct, c);
                    place(e, acct, c, "sell", "market", held, None, t);
                    self.state = 0;
                }
                if s >= 22 * 3600 + 3000 {
                    self.state = 0;
                }
            }
        }
    }
}
