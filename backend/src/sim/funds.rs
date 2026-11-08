//! 指数基金 in the replay (docs/v3-funds.md §9 step 1): the players' subscriptions and redemptions, the index the
//! fund tracks (computed the way `GET /market/indices` computes it, from the closes), and the report: tracking
//! error, what the fund's orders did to the opening auction, costs and fees.

use std::collections::{BTreeMap, HashMap};

use serde_json::json;

use crate::engine::Engine;
use crate::funds::{member, season_from, UNIT};

/// The idle players' flows into one fund (`--fund-flows KEY,SHARE,CHURN`): each subscribes `share` of its cash,
/// in five equal parts over the bots' first five days; then every day, with probability `churn` each, subscribes
/// 5% of its cash or redeems 10% of its units (equally likely).
#[derive(Debug, Clone)]
pub struct FundFlows {
    pub key: String,
    pub share: f64,
    pub churn: f64,
}

impl FundFlows {
    pub fn parse(v: &str) -> Result<FundFlows, String> {
        let x: Vec<&str> = v.split(',').collect();
        let [key, share, churn] = x.as_slice() else { return Err("KEY,SHARE,CHURN".into()) };
        Ok(FundFlows {
            key: key.to_string(),
            share: share.parse().map_err(|_| "share")?,
            churn: churn.parse().map_err(|_| "churn")?,
        })
    }
}

fn unit(seed: u64, a: i64, day: &str, salt: u32) -> f64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&(seed, a, day, salt), &mut h);
    (std::hash::Hasher::finish(&h) >> 11) as f64 / (1u64 << 53) as f64
}

/// The idle players' requests of one day (20:00), `k` days after the bots started.
pub fn idle_flows(e: &mut Engine, f: &FundFlows, players: &[(i64, i64)], k: usize, seed: u64, day: &str, ts: &str) {
    for &(a, start_cash) in players {
        if k < 5 {
            let amt = ((start_cash as f64 * f.share) / 5.0) as i64;
            let amt = amt.min(e.available_cash(a));
            if amt > 0 {
                let _ = e.fund_subscribe(a, &f.key, amt, day, ts);
            }
            continue;
        }
        let u = unit(seed, a, day, 1);
        if u < f.churn / 2.0 {
            let amt = e.available_cash(a) / 20;
            if amt > 0 {
                let _ = e.fund_subscribe(a, &f.key, amt, day, ts);
            }
        } else if u < f.churn {
            let units = e.fund(&f.key).map_or(0, |x| x.units_of(a)) / 10;
            if units > 0 {
                let _ = e.fund_redeem(a, &f.key, units, day);
            }
        }
    }
}

/// The tracked index, from the engine's closes: each day's members (as the fund sees them that morning) and their
/// market values at the last close; after the close, their values plus the dividends they paid that day.
#[derive(Default)]
pub struct Bench {
    /// key -> (members' market value at the morning reference, cents)
    start: HashMap<String, BTreeMap<i64, f64>>,
    /// key -> [(day, index return, fund return)]
    pub days: HashMap<String, Vec<(String, f64, f64)>>,
    /// key -> the fund's net value at the last close
    nav: HashMap<String, f64>,
    /// days on which a member was split / ex-rights / delisted (the cap-based return is approximate there)
    pub irregular: Vec<(String, i64, &'static str)>,
    /// the funds' fills against the day's close: key -> (notional, Σ signed (price − close) × qty), cents
    pub shortfall: HashMap<String, (i64, i64)>,
    /// key -> the fund's share of each (stock, day)'s volume it traded in
    pub volume_share: HashMap<String, Vec<f64>>,
    /// all trades against the day's close: (notional, Σ (price − close) × qty), cents
    pub market_vs_close: (i64, i64),
    /// key -> the morning's weights: the fund's (stock -> share of its assets) and the index's
    weights: HashMap<String, (BTreeMap<i64, f64>, BTreeMap<i64, f64>)>,
    /// key -> per day: the difference from the stocks listed in the last 14 days, from the others, and the rest
    /// (the day's trading against the close, costs, fees)
    pub parts: HashMap<String, Vec<[f64; 3]>>,
}

impl Bench {
    /// The morning (before the auction): the day's members and their reference values.
    pub fn morning(&mut self, e: &Engine, day: &str) {
        let qs = season_from(day);
        for f in e.funds_iter() {
            let m: BTreeMap<i64, f64> = e
                .companies()
                .filter(|c| member(&f.key, c, day, &qs) && c.prev_close > 0)
                .map(|c| (c.id, c.prev_close as f64 * c.shares as f64))
                .collect();
            let total: f64 = m.values().sum::<f64>().max(1.0);
            let wi: BTreeMap<i64, f64> = m.iter().map(|(c, v)| (*c, v / total)).collect();
            let held: BTreeMap<i64, f64> = e
                .holdings_of(f.account)
                .filter_map(|h| e.company(h.company_id).map(|c| (h.company_id, (h.qty * c.prev_close) as f64)))
                .collect();
            let cash = e.account(f.account).map_or(0, |a| a.cash) - f.pending_cash - f.owed.values().sum::<i64>();
            let assets = held.values().sum::<f64>() + cash as f64;
            let wf: BTreeMap<i64, f64> = if assets > 0.0 { held.iter().map(|(c, v)| (*c, v / assets)).collect() } else { BTreeMap::new() };
            self.weights.insert(f.key.clone(), (wf, wi));
            self.start.insert(f.key.clone(), m);
        }
    }

    /// The day's trades (before the close): the funds' fills against the closing price, and their share of volume.
    pub fn trades(&mut self, e: &Engine, trades: &[super::metrics::TradeRec]) {
        let funds: HashMap<i64, String> = e.funds_iter().map(|f| (f.account, f.key.clone())).collect();
        let mut vol: HashMap<i64, i64> = HashMap::new();
        let mut fvol: HashMap<(String, i64), i64> = HashMap::new();
        for t in trades {
            *vol.entry(t.company).or_default() += t.qty;
            // the control: every trade of the day against the close (where the session's prices sit)
            if let Some(c) = e.company(t.company) {
                self.market_vs_close.0 += t.price * t.qty;
                self.market_vs_close.1 += (t.price - c.last_price) * t.qty;
            }
            for (acct, sign) in [(t.buyer, 1), (t.seller, -1)] {
                let Some(k) = funds.get(&acct) else { continue };
                let close = e.company(t.company).map_or(t.price, |c| c.last_price);
                let x = self.shortfall.entry(k.clone()).or_default();
                x.0 += t.price * t.qty;
                x.1 += sign * (t.price - close) * t.qty;
                *fvol.entry((k.clone(), t.company)).or_default() += t.qty;
            }
        }
        for ((k, c), q) in fvol {
            self.volume_share.entry(k).or_default().push(q as f64 / vol[&c].max(1) as f64);
        }
    }

    /// After the close: the index's return of the day against the fund's.
    pub fn evening(&mut self, e: &Engine, day: &str) {
        let adj: Vec<i64> = e.price_adjustments().iter().filter(|a| a.day == day).map(|a| a.company_id).collect();
        for f in e.funds_iter() {
            let Some(m) = self.start.get(&f.key) else { continue };
            let (mut num, mut den) = (0.0, 0.0);
            for (&c, &v0) in m {
                let Some(co) = e.company(c) else { continue };
                let div = e.company_day(c, day).map_or(0, |d| d.dividend_ps) as f64 * co.shares as f64;
                if co.status != "listed" {
                    self.irregular.push((day.into(), c, "delisted"));
                }
                if adj.contains(&c) {
                    self.irregular.push((day.into(), c, "adjusted"));
                }
                num += co.last_price as f64 * co.shares as f64 + div;
                den += v0;
            }
            let prev = self.nav.insert(f.key.clone(), f.nav);
            if f.units > 0 && den > 0.0 {
                if let Some(p) = prev.filter(|p| *p > 0.0) {
                    let (ri, rf) = (num / den - 1.0, f.nav / p - 1.0);
                    self.days.entry(f.key.clone()).or_default().push((day.into(), ri, rf));
                    // the difference by where it comes from: Σ (fund weight − index weight) × the stock's return
                    if let Some((wf, wi)) = self.weights.get(&f.key) {
                        let recent = crate::vclose::day_no(day).unwrap_or(0) - 14;
                        let mut part = [0.0f64; 3];
                        let ids: std::collections::BTreeSet<i64> = wf.keys().chain(wi.keys()).copied().collect();
                        for c in ids {
                            let Some(co) = e.company(c) else { continue };
                            if co.prev_close <= 0 || adj.contains(&c) {
                                continue;
                            }
                            let div = e.company_day(c, day).map_or(0, |d| d.dividend_ps) as f64;
                            let r = (co.last_price as f64 + div) / co.prev_close as f64 - 1.0;
                            let d = (wf.get(&c).copied().unwrap_or(0.0) - wi.get(&c).copied().unwrap_or(0.0)) * r;
                            let new = crate::vclose::day_no(&co.listed_on).is_some_and(|l| l >= recent);
                            part[if new { 0 } else { 1 }] += d;
                        }
                        part[2] = (rf - ri) - part[0] - part[1];
                        self.parts.entry(f.key.clone()).or_default().push(part);
                    }
                }
            }
        }
    }
}

fn quantile(mut v: Vec<f64>, q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// The replay's account of the funds.
pub fn report(e: &Engine, bench: &Bench) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for f in e.funds_iter() {
        let days = bench.days.get(&f.key).cloned().unwrap_or_default();
        let irregular: std::collections::HashSet<&str> = bench.irregular.iter().map(|x| x.0.as_str()).collect();
        let diffs: Vec<f64> = days.iter().filter(|d| !irregular.contains(d.0.as_str())).map(|d| d.2 - d.1).collect();
        let n = diffs.len().max(1) as f64;
        let mean = diffs.iter().sum::<f64>() / n;
        let sd = (diffs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
        let cum = |i: usize| days.iter().fold(1.0, |acc, d| acc * (1.0 + if i == 1 { d.1 } else { d.2 })) - 1.0;
        // the opening auction: the indicative price before and after the fund's orders, in basis points of the
        // reference (the last close), by the side of the order
        let mut moves: Vec<f64> = Vec::new();
        let mut open_vs_ref: Vec<f64> = Vec::new();
        let mut no_indicative = 0;
        let mut big = Vec::new();
        for m in &f.marks {
            match (m.before, m.after) {
                (Some(b), Some(a)) => {
                    let bp = (a - b) as f64 / m.reference.max(1) as f64 * 1e4 * m.qty.signum() as f64;
                    moves.push(bp);
                    if bp.abs() >= 200.0 {
                        big.push(json!({"day": m.day, "company": m.company, "qty": m.qty, "reference": m.reference, "before": b, "after": a}));
                    }
                }
                _ => no_indicative += 1,
            }
            if let Some(o) = m.open {
                open_vs_ref.push((o - m.reference) as f64 / m.reference.max(1) as f64 * 1e4 * m.qty.signum() as f64);
            }
        }
        let flow_days: std::collections::BTreeSet<&str> = f.confirmed.iter().map(|c| c.day.as_str()).collect();
        let subs: i64 = f.confirmed.iter().filter(|c| c.units > 0).map(|c| c.amount).sum();
        let reds: i64 = f.confirmed.iter().filter(|c| c.units < 0).map(|c| -c.cash).sum();
        let req_costs: i64 = f.confirmed.iter().map(|c| c.cost).sum();
        let last = f.history.last();
        let col = |i: usize| -> Vec<f64> { bench.parts.get(&f.key).map_or(vec![], |p| p.iter().map(|x| x[i]).collect()) };
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let sdv = |v: &[f64]| {
            let m = avg(v);
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len().max(2) - 1) as f64).sqrt()
        };
        let parts_json = json!({
            "parts": "new listings (last 14 days), other stocks, the rest (trading vs the close, costs, fees)",
            "sd_bp": (0..3).map(|i| sdv(&col(i)) * 1e4).collect::<Vec<_>>(),
            "mean_bp": (0..3).map(|i| avg(&col(i)) * 1e4).collect::<Vec<_>>(),
        });
        let (sf_n, sf_x) = bench.shortfall.get(&f.key).copied().unwrap_or((0, 0));
        let vs = bench.volume_share.get(&f.key).cloned().unwrap_or_default();
        let peak = f.history.iter().map(|h| h.aum).max().unwrap_or(0);
        out.insert(
            f.key.clone(),
            json!({
                "aum_end": last.map_or(0.0, |h| h.aum as f64 / 100.0), "aum_peak": peak as f64 / 100.0,
                "units_end": f.units as f64 / UNIT as f64, "nav_end": f.nav / 100.0, "holders": f.holders.values().filter(|u| **u > 0).count(),
                "cash_share_end": last.map_or(0.0, |h| h.cash as f64 / h.aum.max(1) as f64),
                "subscribed": subs as f64 / 100.0, "redeemed": reds as f64 / 100.0, "flow_days": flow_days.len(),
                "request_costs": req_costs as f64 / 100.0, "impact_charged": f.impact_charged as f64 / 100.0, "fund_costs": f.own_costs as f64 / 100.0, "fees": f.fees_paid as f64 / 100.0,
                "owed_end": f.owed.values().sum::<i64>() as f64 / 100.0,
                "tracking_parts": parts_json,
                "tracking": {
                    "days": diffs.len(), "irregular_days_left_out": days.len() - diffs.len(),
                    "mean_daily_diff_bp": mean * 1e4, "sd_daily_bp": sd * 1e4, "te_annual": sd * 365f64.sqrt(),
                    "fund_return": cum(2), "index_return": cum(1),
                },
                "fills": {
                    "notional": sf_n as f64 / 100.0, "vs_close_bp": sf_x as f64 / sf_n.max(1) as f64 * 1e4,
                    "market_vs_close_bp": bench.market_vs_close.1 as f64 / bench.market_vs_close.0.max(1) as f64 * 1e4,
                    "volume_share": {"p50": quantile(vs.clone(), 0.5), "p90": quantile(vs.clone(), 0.9), "max": quantile(vs.clone(), 1.0)},
                },
                "auction": {
                    "orders": f.marks.len(), "no_indicative": no_indicative,
                    "indicative_move_bp": {"p50": quantile(moves.clone(), 0.5), "p90": quantile(moves.clone(), 0.9), "p99": quantile(moves.clone(), 0.99), "max": quantile(moves.clone(), 1.0)},
                    "open_vs_close_bp": {"p10": quantile(open_vs_ref.clone(), 0.1), "p50": quantile(open_vs_ref.clone(), 0.5), "p90": quantile(open_vs_ref.clone(), 0.9)},
                    "moves_over_200bp": big.len(), "largest": big.into_iter().take(10).collect::<Vec<_>>(),
                },
            }),
        );
    }
    serde_json::Value::Object(out)
}
