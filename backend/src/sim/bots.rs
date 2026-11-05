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
    /// hold the market, but sell each stock the evening before its episode airs and buy it back after
    AirCalendar,
    /// the same, only before a show's first two episodes (the premiere revision must not be predictable)
    AirEarly,
    /// "fan insight": knows each show's value two closes ahead (what the filter will make of the next
    /// observations) with a multiplicative error of 10% / 30%, and trades the gap to the price
    Fan10,
    Fan30,
    /// a fan's judgement of a show: from its first episode on, knows what the show will be worth over the
    /// next four weeks (the value 25 days ahead plus the dividends until then) with a 30% error fixed per
    /// show; rebalances weekly and holds
    FanLong,
    /// subscribe to every new issue (an equal budget each) and hold what is allotted
    IpoAll,
    /// hold the market, but sell everything on Sunday evening (before the dividend) and buy back on Monday
    ExDividend,
}

impl BotKind {
    pub fn all() -> [BotKind; 16] {
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
            BotKind::AirCalendar,
            BotKind::AirEarly,
            BotKind::ExDividend,
            BotKind::Fan10,
            BotKind::Fan30,
            BotKind::FanLong,
            BotKind::IpoAll,
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
            BotKind::AirCalendar => "air_calendar",
            BotKind::AirEarly => "air_early",
            BotKind::Fan10 => "fan_0.1",
            BotKind::Fan30 => "fan_0.3",
            BotKind::FanLong => "fan_long_0.3",
            BotKind::IpoAll => "ipo_all",
            BotKind::ExDividend => "ex_dividend",
        }
    }
    /// Error of the fan bot's foresight (None: not a fan bot).
    pub fn foresight_sigma(self) -> Option<f64> {
        match self {
            BotKind::Fan10 => Some(0.1),
            BotKind::Fan30 => Some(0.3),
            BotKind::FanLong => Some(0.3),
            _ => None,
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
    sold: Vec<i64>,
    pub trades_placed: u64,
    /// the fan bots' foresight: (company, day) -> the hidden value at that day's open, cents
    pub foresight: Option<std::sync::Arc<std::collections::HashMap<(i64, String), (i64, i64)>>>,
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
        Bot { kind, account, start_cash, bought_initial: false, target: None, state: 0, since: 0, sold: Vec::new(), trades_placed: 0, foresight: None }
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
            BotKind::AirCalendar | BotKind::AirEarly | BotKind::ExDividend if self.bought_initial => {
                let (d, _) = local(t);
                let today = crate::vclose::day_no(&day_of(t)).unwrap_or(0);
                let sunday = (d + 4).rem_euclid(7) == 0; // 1970-01-01 was a Thursday: (d + 4) % 7 is 0 on Sundays
                let monday = (d + 4).rem_euclid(7) == 1;
                if s == 22 * 3600 + 1800 {
                    // sell what is about to air tomorrow (or everything before the Sunday dividend)
                    let held: Vec<(i64, i64)> = e.holdings_of(acct).filter(|h| h.qty > h.qty_frozen).map(|h| (h.company_id, h.qty - h.qty_frozen)).collect();
                    for (c, q) in held {
                        let go = match self.kind {
                            BotKind::ExDividend => sunday,
                            BotKind::AirEarly => {
                                let air = e.air_days_of(c);
                                let first = e.company(c).and_then(|x| crate::vclose::day_no(&x.first_air));
                                (air.is_empty() && first == Some(today + 1)) || (air.len() == 1 && air[0] + 7 == today + 1)
                            }
                            _ => {
                                let air = e.air_days_of(c);
                                air.last().is_some_and(|a| a + 7 == today + 1)
                            }
                        };
                        if go {
                            let p = e.company(c).map_or(1, |x| x.last_price);
                            if place(e, acct, c, "sell", "limit", q, Some((p * 99 / 100).max(1)), t) {
                                self.trades_placed += 1;
                                self.sold.push(c);
                            }
                        }
                    }
                } else if s == 9 * 3600 + 1800 && !self.sold.is_empty() {
                    let ok_day = match self.kind {
                        BotKind::ExDividend => monday,
                        _ => true,
                    };
                    if ok_day {
                        let sold = std::mem::take(&mut self.sold);
                        let per = e.available_cash(acct) / sold.len().max(1) as i64;
                        for c in sold {
                            if e.company(c).is_none_or(|x| x.status != "listed") {
                                continue;
                            }
                            let p = e.company(c).unwrap().last_price;
                            let lim = p * 101 / 100 + 1;
                            let q = affordable(e, per, lim);
                            if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                                self.trades_placed += 1;
                            }
                        }
                    }
                }
            }
            BotKind::Fan10 | BotKind::Fan30 => {
                // every 30 minutes, 09:30-22:30
                if s % 1800 != 0 || !(9 * 3600 + 1800..=22 * 3600 + 1800).contains(&s) {
                    return;
                }
                let Some(fs) = self.foresight.clone() else { return };
                let sigma = self.kind.foresight_sigma().unwrap_or(0.0);
                let today = crate::vclose::day_no(&day_of(t)).unwrap_or(0);
                let ahead = chrono::NaiveDate::from_num_days_from_ce_opt((today + 2) as i32).map(|d| d.format("%Y-%m-%d").to_string());
                let Some(ahead) = ahead else { return };
                let wealth = self.wealth(e);
                let mut listed: Vec<(i64, i64)> = e.companies().filter(|c| c.status == "listed" && c.last_price > 0).map(|c| (c.id, c.last_price)).collect();
                listed.sort_unstable();
                for (c, p) in listed {
                    let Some(&(f, _)) = fs.get(&(c, ahead.clone())) else { continue };
                    if f <= 0 {
                        continue;
                    }
                    // the same error all day for a (show, day): a fan's read of the show, not fresh noise
                    let z = normal_of(c as u64 ^ ((today as u64) << 20) ^ 0x9e37_79b9_7f4a_7c15);
                    let est = f as f64 * (sigma * z - 0.5 * sigma * sigma).exp();
                    let gap = (est / p as f64).ln();
                    let held = e.holding(acct, c).map_or(0, |h| h.qty);
                    let free = e.available_shares(acct, c);
                    e.cancel_all(acct, c, &ts);
                    // a fan who knows how good their read is: acts only on gaps beyond its error
                    if gap > sigma.max(0.05) {
                        // up to 15% of wealth in one stock
                        let target = wealth * 15 / 100;
                        let have = held * p;
                        if have < target * 8 / 10 {
                            let lim = p * 1005 / 1000 + 1;
                            let q = affordable(e, (target - have).min(e.available_cash(acct)), lim);
                            if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                                self.trades_placed += 1;
                            }
                        }
                    } else if gap < -0.5 * sigma && free > 0 {
                        let lim = (p * 995 / 1000).max(1);
                        if place(e, acct, c, "sell", "limit", free, Some(lim), t) {
                            self.trades_placed += 1;
                        }
                    }
                }
            }
            BotKind::IpoAll => {
                if s != 12 * 3600 {
                    return;
                }
                let day = day_of(t);
                let open: Vec<(i64, i64)> = e
                    .companies()
                    .filter(|c| {
                        c.sub_start.as_deref().is_some_and(|x| x <= day.as_str()) && c.sub_end.as_deref().is_some_and(|x| day.as_str() <= x)
                    })
                    .map(|c| (c.id, c.ipo_price))
                    .collect();
                for (c, price) in open {
                    if self.sold.contains(&c) || price <= 0 {
                        continue;
                    }
                    let budget = (self.start_cash / 15).min(e.available_cash(acct));
                    let q = budget / price;
                    if q > 0 && e.subscribe(acct, c, q, &day, &ts).is_ok() {
                        self.trades_placed += 1;
                        self.sold.push(c);
                    }
                }
            }
            BotKind::FanLong => {
                // Mondays at 10:00 (and the first day it trades)
                let (d, _) = local(t);
                let monday = (d + 4).rem_euclid(7) == 1;
                if s != 10 * 3600 || !(monday || self.since == 0) {
                    return;
                }
                let Some(fs) = self.foresight.clone() else { return };
                let today = crate::vclose::day_no(&day_of(t)).unwrap_or(0);
                if self.since == 0 {
                    self.since = today;
                }
                let day_str = |n: i64| chrono::NaiveDate::from_num_days_from_ce_opt(n as i32).map(|d| d.format("%Y-%m-%d").to_string());
                let wealth = self.wealth(e);
                let mut listed: Vec<(i64, i64)> = e.companies().filter(|c| c.status == "listed" && c.last_price > 0).map(|c| (c.id, c.last_price)).collect();
                listed.sort_unstable();
                for (c, p) in listed {
                    // only shows that have aired (a fan has seen episode 1)
                    if e.air_days_of(c).is_empty() {
                        continue;
                    }
                    // the value 25 days ahead (or the last day known) plus the dividends paid until then
                    let mut divs = 0i64;
                    let mut f = None;
                    for j in 0..=25 {
                        let Some(ds) = day_str(today + j) else { break };
                        match fs.get(&(c, ds)) {
                            Some(&(fair, div)) => {
                                f = Some(fair);
                                if j < 25 {
                                    divs += div;
                                }
                            }
                            None => break,
                        }
                    }
                    let Some(f) = f.filter(|f| *f > 0) else { continue };
                    let z = normal_of((c as u64).wrapping_mul(0x2545_f491_4f6c_dd1d) ^ 0x5bd1_e995);
                    let est = (f + divs) as f64 * (0.3 * z - 0.045).exp();
                    let gap = (est / p as f64).ln();
                    let held = e.holding(acct, c).map_or(0, |h| h.qty);
                    let free = e.available_shares(acct, c);
                    e.cancel_all(acct, c, &ts);
                    if gap > 0.3 {
                        let target = wealth * 15 / 100;
                        let have = held * p;
                        if have < target * 8 / 10 {
                            let lim = p * 1005 / 1000 + 1;
                            let q = affordable(e, (target - have).min(e.available_cash(acct)), lim);
                            if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                                self.trades_placed += 1;
                            }
                        }
                    } else if gap < -0.15 && free > 0 {
                        let lim = (p * 995 / 1000).max(1);
                        if place(e, acct, c, "sell", "limit", free, Some(lim), t) {
                            self.trades_placed += 1;
                        }
                    }
                }
            }
            BotKind::BuyHold | BotKind::BuyHoldCap | BotKind::AirCalendar | BotKind::AirEarly | BotKind::ExDividend => {
                // the first five days, every 30 minutes: build the position in steps (a fifth of the budget more
                // each day, limit at the last price + 0.5%), so the benchmark doesn't pay for impatience
                if self.bought_initial || s % 1800 != 0 {
                    return;
                }
                let today = crate::vclose::day_no(&day_of(t)).unwrap_or(0);
                if self.since == 0 {
                    self.since = today;
                }
                let step = (today - self.since + 1).min(5);
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
                    let per = per * step / 5;
                    let p = e.company(c).unwrap().last_price;
                    let have = e.holding(acct, c).map_or(0, |h| h.qty) * p;
                    if have < per * 8 / 10 {
                        e.cancel_all(acct, c, &ts);
                        let lim = p * 1005 / 1000 + 1;
                        let q = affordable(e, (per - have).min(e.available_cash(acct)), lim);
                        if place(e, acct, c, "buy", "limit", q, Some(lim), t) {
                            self.trades_placed += 1;
                        }
                        pending = true;
                    }
                }
                if (step == 5 && !pending) || today - self.since >= 7 {
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

/// A standard normal draw from a seed (splitmix64 + Box-Muller): deterministic per (show, day).
fn normal_of(seed: u64) -> f64 {
    let mut x = seed;
    let mut next = || {
        x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        ((z >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    let (u1, u2) = (next(), next());
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}
