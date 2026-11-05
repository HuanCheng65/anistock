//! Liquidity probe: about twenty ordinary players (100,000 币 each) trading the popular stocks at
//! realistic sizes (1k-20k 币) with limit orders at the best price, at the mid, and marketable orders.
//! It measures what a player sees: how long a resting limit order waits for its fill, how far an order of
//! a given size would move the price (walking the book, no trade), and how often a probe order sets off a
//! VCM cooling-off. Quoted spreads come from the recorder (`groups.*.spread_*`).

use std::collections::HashMap;

use serde_json::json;

use crate::engine::{Engine, NewOrder};
use crate::market::rng::Rng;
use crate::market::{day_of, local, ts_of};

/// orders per player per active hour
const RATE_PER_HOUR: f64 = 1.0;
/// stocks a player picks from: the most popular this many
const POPULAR: usize = 15;
/// a resting order still open after this long is cancelled (counted as not filled)
const TIMEOUT_MIN: i64 = 120;
/// order sizes for the static price-impact walk, 币
const IMPACT_SIZES: [i64; 3] = [1_000, 10_000, 50_000];
/// a coordinated group: this many players each buy this much (币) of one stock at once, once a week
const RAIDERS: usize = 5;
const RAID_YUAN: i64 = 50_000;
/// minutes after the raid at which its price deviation is measured
const RAID_HORIZONS_MIN: [i64; 6] = [5, 60, 240, 1440, 2880, 4320];

struct Raid {
    company: i64,
    group: &'static str,
    t0: i64,
    /// log(price / value) just before
    dev0: f64,
    spent: i64,
    /// log(price / value) - dev0 at each horizon
    after: Vec<f64>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Style {
    Best,
    Mid,
}

struct Pending {
    id: i64,
    account: i64,
    style: Style,
    placed: i64,
    first_fill: Option<i64>,
}

#[derive(Default)]
struct Waits {
    /// minutes to the complete fill (None: not complete within the timeout)
    full: Vec<Option<f64>>,
    /// minutes to the first fill
    first: Vec<Option<f64>>,
}

pub struct Probe {
    pub accounts: Vec<i64>,
    rng: Rng,
    pending: Vec<Pending>,
    waits: HashMap<Style, Waits>,
    marketable: u64,
    marketable_filled_share: Vec<f64>,
    orders: u64,
    rejected: u64,
    vcm_triggers: u64,
    /// (heat tercile, size index) -> samples of |last price touched / mid - 1| (None: the book ran out)
    impact: HashMap<(&'static str, usize), Vec<Option<f64>>>,
    /// same, volume-weighted average price vs mid
    slippage: HashMap<(&'static str, usize), Vec<Option<f64>>>,
    /// impact samples with no bid, with no ask, all samples
    one_sided: [u64; 3],
    raiders: Vec<i64>,
    raids: Vec<Raid>,
    /// resting-order minutes checked, with the market maker quoting a better price, with it ahead at ours
    queue: [u64; 3],
    last_raid_day: i64,
}

impl Probe {
    pub fn new(e: &mut Engine, players: usize, start_cash: i64, day: &str, ts: &str, seed: u64) -> Probe {
        let world = e.special("world").unwrap();
        let accounts = (0..players)
            .map(|k| {
                let name = format!("probe{k:02}");
                let a = e.create_account("player", &name, Some(&name), None, None, ts);
                e.transfer(world, a, start_cash, "grant", day, ts, None, None);
                a
            })
            .collect();
        Probe {
            accounts,
            rng: Rng::new(seed ^ 0x9e0be),
            pending: Vec::new(),
            waits: HashMap::new(),
            marketable: 0,
            marketable_filled_share: Vec::new(),
            orders: 0,
            rejected: 0,
            vcm_triggers: 0,
            impact: HashMap::new(),
            slippage: HashMap::new(),
            one_sided: [0; 3],
            raiders: (0..RAIDERS)
                .map(|k| {
                    let name = format!("raid{k:02}");
                    let a = e.create_account("player", &name, Some(&name), None, None, ts);
                    e.transfer(world, a, start_cash, "grant", day, ts, None, None);
                    a
                })
                .collect(),
            raids: Vec::new(),
            queue: [0; 3],
            last_raid_day: i64::MIN,
        }
    }

    fn place(
        &mut self,
        e: &mut Engine,
        acct: i64,
        c: i64,
        side: &str,
        kind: &str,
        qty: i64,
        price: Option<i64>,
        t: i64,
    ) -> Option<(i64, i64)> {
        let (day, ts) = (day_of(t), ts_of(t));
        if e.state("auction_day") != Some(day.as_str()) {
            e.open_market(&day, &ts);
        }
        let vcm_before = e.company(c).map_or(0, |x| x.vcm_count);
        let r = e.place_order(&NewOrder {
            account_id: acct,
            company_id: c,
            side,
            kind,
            qty,
            price,
            day: &day,
            ts: &ts,
            match_: true,
        });
        self.orders += 1;
        if e.company(c).map_or(0, |x| x.vcm_count) > vcm_before {
            self.vcm_triggers += 1;
        }
        match r {
            Ok(p) => Some((p.order_id, p.filled)),
            Err(_) => {
                self.rejected += 1;
                None
            }
        }
    }

    fn dev(e: &Engine, c: i64) -> Option<f64> {
        let co = e.company(c)?;
        (co.status == "listed" && co.fair_value > 0 && co.last_price > 0)
            .then(|| (co.last_price as f64 / co.fair_value as f64).ln())
    }

    /// The coordinated group: every 7th day at 20:00 the raiders each market-buy `RAID_YUAN` of one stock
    /// (alternately the hottest and the median-heat stock) and hold; the deviation is followed for 3 days.
    fn raid(&mut self, e: &mut Engine, t: i64, heat: &dyn Fn(i64) -> f64) {
        for r in &mut self.raids {
            let k = r.after.len();
            if k < RAID_HORIZONS_MIN.len() && t >= r.t0 + RAID_HORIZONS_MIN[k] * 60 {
                match Self::dev(e, r.company) {
                    Some(d) => r.after.push(d - r.dev0),
                    None => r.after.push(f64::NAN),
                }
            }
        }
        let (d, s) = local(t);
        if s != 20 * 3600 || d < self.last_raid_day + 7 {
            return;
        }
        self.last_raid_day = d;
        let mut v: Vec<(f64, i64)> =
            e.companies().filter(|c| c.status == "listed" && c.last_price >= 100).map(|c| (heat(c.id), c.id)).collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        if v.is_empty() {
            return;
        }
        let (group, c) = if self.raids.len() % 2 == 0 { ("hot", v[0].1) } else { ("mid", v[v.len() / 2].1) };
        let Some(dev0) = Self::dev(e, c) else { return };
        let mut spent = 0;
        for k in 0..self.raiders.len() {
            let a = self.raiders[k];
            let ask = e.book(c, 1).asks.first().map(|l| l.price).unwrap_or(e.company(c).unwrap().last_price);
            let qty = (RAID_YUAN * 100 / ask.max(1)).min(e.available_cash(a) / (ask * 101 / 100 + 1).max(1));
            let before = e.available_cash(a);
            self.place(e, a, c, "buy", "market", qty, None, t);
            spent += before - e.available_cash(a);
        }
        self.raids.push(Raid { company: c, group, t0: t, dev0, spent, after: Vec::new() });
    }

    /// One minute of the trading day (09:00-23:00).
    pub fn on_minute(&mut self, e: &mut Engine, t: i64, heat: &dyn Fn(i64) -> f64) {
        self.track(e, t);
        self.raid(e, t, heat);
        let popular = {
            let mut v: Vec<(f64, i64)> = e
                .companies()
                .filter(|c| c.status == "listed" && c.last_price >= 100)
                .map(|c| (heat(c.id), c.id))
                .collect();
            v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
            v.into_iter().take(POPULAR).collect::<Vec<_>>()
        };
        if popular.is_empty() {
            return;
        }
        let (_, s) = local(t);
        for k in 0..self.accounts.len() {
            if self.rng.f64() >= RATE_PER_HOUR / 60.0 || s >= 22 * 3600 + 50 * 60 {
                continue;
            }
            let acct = self.accounts[k];
            let w: Vec<f64> = popular.iter().map(|x| x.0.max(1e-9)).collect();
            let c = popular[self.rng.weighted(&w).unwrap()].1;
            let book = e.book(c, 1);
            let (Some(bid), Some(ask)) = (book.bids.first().map(|l| l.price), book.asks.first().map(|l| l.price))
            else {
                continue;
            };
            let size = (self.rng.range((1_000f64).ln(), (20_000f64).ln()).exp() * 100.0) as i64; // cents
            let held = e.available_shares(acct, c);
            let sell = held > 0 && self.rng.f64() < 0.5;
            let u = self.rng.f64();
            let style = if u < 0.4 {
                Some(Style::Best)
            } else if u < 0.7 {
                Some(Style::Mid)
            } else {
                None
            };
            let price = match (style, sell) {
                (Some(Style::Best), false) => bid,
                (Some(Style::Best), true) => ask,
                (Some(Style::Mid), false) => ((bid + ask) / 2).max(bid),
                (Some(Style::Mid), true) => ((bid + ask + 1) / 2).min(ask),
                (None, false) => ask,
                (None, true) => bid,
            };
            let mut qty = size / price.max(1);
            if sell {
                qty = qty.min(held);
            } else {
                qty = qty.min(e.available_cash(acct) / (price * 101 / 100 + 1).max(1));
            }
            if qty <= 0 {
                continue;
            }
            let side = if sell { "sell" } else { "buy" };
            match style {
                Some(st) => {
                    if let Some((id, filled)) = self.place(e, acct, c, side, "limit", qty, Some(price), t) {
                        let first_fill = if filled > 0 { Some(t) } else { None };
                        if e.order(id).is_none() {
                            // filled on arrival (the book moved since the snapshot): a zero wait
                            let w = self.waits.entry(st).or_default();
                            w.full.push(Some(0.0));
                            w.first.push(Some(0.0));
                        } else {
                            self.pending.push(Pending { id, account: acct, style: st, placed: t, first_fill });
                        }
                    }
                }
                None => {
                    self.marketable += 1;
                    if let Some((_, filled)) = self.place(e, acct, c, side, "market", qty, None, t) {
                        self.marketable_filled_share.push(filled as f64 / qty as f64);
                    }
                }
            }
        }
    }

    /// Check the resting orders: record fills, cancel what waited too long.
    fn track(&mut self, e: &mut Engine, t: i64) {
        let (_, s) = local(t);
        let eod = s >= 22 * 3600 + 59 * 60;
        let mut keep = Vec::new();
        for mut p in std::mem::take(&mut self.pending) {
            let open = e.order(p.id).cloned();
            let mins = (t - p.placed) as f64 / 60.0;
            match open {
                None => {
                    let w = self.waits.entry(p.style).or_default();
                    w.full.push(Some(mins));
                    w.first.push(Some(p.first_fill.map_or(mins, |f| (f - p.placed) as f64 / 60.0)));
                }
                Some(o) => {
                    // queue position against the market maker: its orders on the same side that are at a
                    // better price, or at the same price and older (ahead in time priority)
                    if let Some(mm) = e.account_of("mm", o.company_id) {
                        let mine = o.price.unwrap_or(0);
                        let buy = o.side == crate::model::Side::Buy;
                        let (mut better, mut same_ahead) = (false, false);
                        for m in e.open_orders_of(mm) {
                            if m.company_id != o.company_id || m.side != o.side {
                                continue;
                            }
                            let mp = m.price.unwrap_or(0);
                            if (buy && mp > mine) || (!buy && mp < mine) {
                                better = true;
                            } else if mp == mine && m.id < o.id {
                                same_ahead = true;
                            }
                        }
                        self.queue[0] += 1;
                        self.queue[1] += better as u64;
                        self.queue[2] += same_ahead as u64;
                    }
                    if p.first_fill.is_none() && o.filled > 0 {
                        p.first_fill = Some(t);
                    }
                    if t - p.placed >= TIMEOUT_MIN * 60 || eod {
                        let _ = e.cancel_order(p.account, p.id, &ts_of(t));
                        let w = self.waits.entry(p.style).or_default();
                        w.full.push(None);
                        w.first.push(p.first_fill.map(|f| (f - p.placed) as f64 / 60.0));
                    } else {
                        keep.push(p);
                    }
                }
            }
        }
        self.pending = keep;
    }

    /// Price impact of buys and sells of each size, walking the whole book (no trade).
    pub fn sample_impact(&mut self, e: &Engine, heat: &dyn Fn(i64) -> f64) {
        let mut v: Vec<(f64, i64)> =
            e.companies().filter(|c| c.status == "listed" && c.last_price >= 100).map(|c| (heat(c.id), c.id)).collect();
        v.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap().then(a.1.cmp(&b.1)));
        let n = v.len();
        for (k, (_, c)) in v.iter().enumerate() {
            let g = if k < n / 3 {
                "hot"
            } else if k < 2 * n / 3 {
                "mid"
            } else {
                "cold"
            };
            let b = e.book(*c, 1000);
            self.one_sided[0] += b.bids.is_empty() as u64;
            self.one_sided[1] += b.asks.is_empty() as u64;
            self.one_sided[2] += 1;
            let (Some(bid), Some(ask)) = (b.bids.first(), b.asks.first()) else { continue };
            let mid = (bid.price + ask.price) as f64 / 2.0;
            for (si, &size) in IMPACT_SIZES.iter().enumerate() {
                for levels in [&b.asks, &b.bids] {
                    let mut left = (size * 100) as f64;
                    let (mut cost, mut qty, mut last) = (0.0, 0.0, mid);
                    for l in levels.iter() {
                        if left <= 0.0 {
                            break;
                        }
                        let take = (left / l.price as f64).min(l.qty as f64);
                        cost += take * l.price as f64;
                        qty += take;
                        left -= take * l.price as f64;
                        last = l.price as f64;
                    }
                    let done = left <= l_eps(size);
                    let imp = done.then(|| (last / mid - 1.0).abs());
                    let slip = (done && qty > 0.0).then(|| (cost / qty / mid - 1.0).abs());
                    for gg in [g, "all"] {
                        self.impact.entry((gg, si)).or_default().push(imp);
                        self.slippage.entry((gg, si)).or_default().push(slip);
                    }
                }
            }
        }
    }

    pub fn report(&self) -> serde_json::Value {
        fn q(v: &[Option<f64>], p: f64) -> Option<f64> {
            // missing values (not filled / book ran out) count as larger than everything
            if v.is_empty() {
                return None;
            }
            let mut x: Vec<f64> = v.iter().map(|a| a.unwrap_or(f64::INFINITY)).collect();
            x.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let y = x[((x.len() - 1) as f64 * p).round() as usize];
            y.is_finite().then_some(y)
        }
        let share = |v: &[Option<f64>]| v.iter().filter(|a| a.is_some()).count() as f64 / v.len().max(1) as f64;
        let mut waits = serde_json::Map::new();
        for (name, st) in [("best", Style::Best), ("mid", Style::Mid)] {
            let Some(w) = self.waits.get(&st) else { continue };
            waits.insert(
                name.into(),
                json!({
                    "orders": w.full.len(),
                    "filled_share": share(&w.full),
                    "full_fill_min_median": q(&w.full, 0.5), "full_fill_min_p90": q(&w.full, 0.9),
                    "first_fill_min_median": q(&w.first, 0.5), "first_fill_min_p90": q(&w.first, 0.9),
                    "within_5min": w.full.iter().filter(|a| a.is_some_and(|m| m <= 5.0)).count() as f64 / w.full.len().max(1) as f64,
                }),
            );
        }
        let mut impact = serde_json::Map::new();
        for g in ["hot", "mid", "cold", "all"] {
            let mut m = serde_json::Map::new();
            for (si, size) in IMPACT_SIZES.iter().enumerate() {
                let im = self.impact.get(&(g, si)).map(Vec::as_slice).unwrap_or(&[]);
                let sl = self.slippage.get(&(g, si)).map(Vec::as_slice).unwrap_or(&[]);
                m.insert(
                    size.to_string(),
                    json!({
                        "impact_median": q(im, 0.5), "impact_p90": q(im, 0.9),
                        "slippage_median": q(sl, 0.5),
                        "book_ran_out_share": 1.0 - share(im),
                        "over_vcm_share": im.iter().filter(|a| a.is_none_or(|x| x > 0.10)).count() as f64 / im.len().max(1) as f64,
                    }),
                );
            }
            impact.insert(g.into(), serde_json::Value::Object(m));
        }
        let mut mf = self.marketable_filled_share.clone();
        mf.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut raids = serde_json::Map::new();
        for g in ["hot", "mid"] {
            let rs: Vec<&Raid> = self.raids.iter().filter(|r| r.group == g).collect();
            let at = |k: usize| {
                let mut x: Vec<f64> =
                    rs.iter().filter_map(|r| r.after.get(k).copied()).filter(|x| x.is_finite()).collect();
                x.sort_by(|a, b| a.partial_cmp(b).unwrap());
                x.get(x.len() / 2).copied()
            };
            let mut sp: Vec<i64> = rs.iter().map(|r| r.spent).collect();
            sp.sort();
            let spent = sp.get(sp.len() / 2).map(|v| *v as f64 / 100.0);
            raids.insert(
                g.into(),
                json!({
                    "n": rs.len(),
                    "spent_median": spent,
                    "horizons_min": RAID_HORIZONS_MIN,
                    "dev_change_median": (0..RAID_HORIZONS_MIN.len()).map(at).collect::<Vec<_>>(),
                }),
            );
        }
        json!({
            "raids": raids,
            "resting_minutes": self.queue[0],
            "mm_better_price_share": self.queue[1] as f64 / self.queue[0].max(1) as f64,
            "mm_ahead_same_price_share": self.queue[2] as f64 / self.queue[0].max(1) as f64,
            "players": self.accounts.len(),
            "orders": self.orders,
            "rejected": self.rejected,
            "marketable": self.marketable,
            "marketable_filled_share_median": mf.get(mf.len() / 2),
            "vcm_triggers": self.vcm_triggers,
            "no_bid_share": self.one_sided[0] as f64 / self.one_sided[2].max(1) as f64,
            "no_ask_share": self.one_sided[1] as f64 / self.one_sided[2].max(1) as f64,
            "limit_waits": waits,
            "impact": impact,
        })
    }
}

/// Rounding slack of the book walk: a cent's worth per 币 of size is close enough to "done".
fn l_eps(size: i64) -> f64 {
    size as f64 * 0.01 + 100.0
}
