//! 指数基金 in the replay (docs/design/funds.md §9 step 1): the players' subscriptions and redemptions, the index the
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

/// The labels the theme funds select on (source, genres plus the Isekai tag at rank 60, as engine/economy.py
/// `show_genres`), from the collector's latest AniList snapshots: replay inputs written before they carried them.
pub fn labels_from_collector(path: &std::path::Path) -> HashMap<i64, (Option<String>, Vec<String>)> {
    crate::funds::labels_from_collector(path).into_iter().collect()
}

fn unit(seed: u64, a: i64, day: &str, salt: u32) -> f64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&(seed, a, day, salt), &mut h);
    (std::hash::Hasher::finish(&h) >> 11) as f64 / (1u64 << 53) as f64
}

/// The idle players' requests of one day (20:00), `k` days after the bots started. Key `spread`: each player uses one
/// of the funds, drawn once.
pub fn idle_flows(e: &mut Engine, f: &FundFlows, players: &[(i64, i64)], k: usize, seed: u64, day: &str, ts: &str) {
    let keys = e.funds.keys.clone();
    for &(a, start_cash) in players {
        let key = if f.key == "spread" {
            keys[(unit(seed, a, "", 7) * keys.len() as f64) as usize % keys.len()].clone()
        } else {
            f.key.clone()
        };
        let f = &FundFlows { key, ..f.clone() };
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
                let _ = e.fund_redeem(a, &f.key, units, day, ts);
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
    /// the morning's reference prices (the last close): the evening's prev_close is already the new close
    refs: HashMap<i64, i64>,
    /// (key, "in:reason" / "out:reason") -> cents: every non-trade ledger row of a fund's account
    pub flows: BTreeMap<(String, String), i64>,
    /// the reconciliation (`morning` .. `reconcile`): per fund account, the morning's shares and cash; the day's
    /// fills (stock, signed qty, price) and non-trade ledger net
    recon: HashMap<i64, (BTreeMap<i64, i64>, i64)>,
    day_fills: HashMap<i64, Vec<(i64, i64, i64)>>,
    day_ledger: HashMap<i64, i64>,
    /// key -> per day (day, residual cents, its parts: price move, trading at the close, costs, ledger)
    pub residuals: HashMap<String, Vec<(String, i64, [i64; 4])>>,
    /// key -> per day: the index computed two ways (must be 0), the morning holdings against the index's weights, and
    /// the net value against those holdings (the day's trading, costs, fees)
    pub parts: HashMap<String, Vec<[f64; 3]>>,
    /// key -> per day: half the sum of |fund weight − index weight| (0: the same portfolio)
    pub active: HashMap<String, Vec<f64>>,
    /// (key, stock) -> (Σ fund weight − index weight, Σ that × the stock's return, days): where the holdings differ
    pub per_stock: HashMap<(String, i64), (f64, f64, u32)>,
}

impl Bench {
    /// The morning (before the auction): the day's members and their reference values.
    pub fn morning(&mut self, e: &Engine, day: &str) {
        let qs = season_from(day);
        self.refs = e.companies().map(|c| (c.id, c.prev_close)).collect();
        self.recon = e
            .funds_iter()
            .map(|f| {
                (
                    f.account,
                    (
                        e.holdings_of(f.account).map(|h| (h.company_id, h.qty)).collect(),
                        e.account(f.account).map_or(0, |a| a.cash),
                    ),
                )
            })
            .collect();
        self.day_fills.clear();
        self.day_ledger.clear();
        let _ = (&qs, member);
        for f in e.funds_iter() {
            // the index's weights by the shared rules (members, caps, 安易 20's choice, the month's float)
            let wi: BTreeMap<i64, f64> = e.index_weights(&f.key, day);
            let m: BTreeMap<i64, f64> = wi.clone();
            let held: BTreeMap<i64, f64> = e
                .holdings_of(f.account)
                .filter_map(|h| e.company(h.company_id).map(|c| (h.company_id, (h.qty * c.prev_close) as f64)))
                .collect();
            let cash = e.account(f.account).map_or(0, |a| a.cash) - f.pending_cash - f.owed.values().sum::<i64>();
            let assets = held.values().sum::<f64>() + cash as f64;
            let wf: BTreeMap<i64, f64> =
                if assets > 0.0 { held.iter().map(|(c, v)| (*c, v / assets)).collect() } else { BTreeMap::new() };
            self.weights.insert(f.key.clone(), (wf, wi));
            self.start.insert(f.key.clone(), m);
        }
    }

    /// The day's ledger rows other than trades (Tap keeps those): the funds' money in and out by reason.
    pub fn ledger(&mut self, e: &Engine, rows: &[(i64, i64, i64, String)]) {
        let funds: HashMap<i64, String> = e.funds_iter().map(|f| (f.account, f.key.clone())).collect();
        for (from, to, amount, reason) in rows {
            if let Some(k) = funds.get(from) {
                *self.flows.entry((k.clone(), format!("out:{reason}"))).or_default() += amount;
                *self.day_ledger.entry(*from).or_default() -= amount;
            }
            if let Some(k) = funds.get(to) {
                *self.flows.entry((k.clone(), format!("in:{reason}"))).or_default() += amount;
                *self.day_ledger.entry(*to).or_default() += amount;
            }
        }
    }

    /// After the day's trades and ledger are in: each fund's cash + shares at the close against the morning's, the
    /// difference explained by the price moves of the morning's shares, the fills against the close, commission and
    /// stamp duty, and the non-trade ledger rows. The residual must be zero (rounding of the costs aside).
    pub fn reconcile(&mut self, e: &Engine, day: &str) {
        let adj: std::collections::HashSet<i64> =
            e.price_adjustments().iter().filter(|a| a.day == day).map(|a| a.company_id).collect();
        for f in e.funds_iter() {
            let Some((h0, cash0)) = self.recon.get(&f.account) else { continue };
            if h0.keys().any(|c| adj.contains(c)) {
                continue;
            }
            let close = |c: i64| e.company(c).map_or(0, |x| x.last_price);
            let now: i64 = e.holdings_of(f.account).map(|h| h.qty * close(h.company_id)).sum::<i64>()
                + e.account(f.account).map_or(0, |a| a.cash);
            let before: i64 = h0.iter().map(|(c, q)| q * self.refs.get(c).copied().unwrap_or(0)).sum::<i64>() + cash0;
            let moved: i64 = h0.iter().map(|(c, q)| q * (close(*c) - self.refs.get(c).copied().unwrap_or(0))).sum();
            let fills = self.day_fills.get(&f.account).cloned().unwrap_or_default();
            let at_close: i64 = fills.iter().map(|(c, q, px)| q * (close(*c) - px)).sum();
            let costs: i64 = fills
                .iter()
                .map(|(_, q, px)| {
                    e.fees.commission_on(q.abs() * px) + if *q < 0 { e.fees.stamp_on(-q * px) } else { 0 }
                })
                .sum();
            let ledger = self.day_ledger.get(&f.account).copied().unwrap_or(0);
            let resid = (now - before) - (moved + at_close - costs + ledger);
            self.residuals.entry(f.key.clone()).or_default().push((
                day.to_string(),
                resid,
                [moved, at_close, -costs, ledger],
            ));
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
                self.day_fills.entry(acct).or_default().push((t.company, sign * t.qty, t.price));
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
            for (&c, &w) in m {
                let Some(co) = e.company(c) else { continue };
                // each member's return over the day (its close plus the dividend against the morning's price),
                // weighted as the index weighs it in the morning
                let p0 = self.refs.get(&c).copied().unwrap_or(0).max(1) as f64;
                let fl = w / p0;
                let v0 = w;
                let div = e.company_day(c, day).map_or(0, |d| d.dividend_ps) as f64 * fl;
                if co.status != "listed" {
                    self.irregular.push((day.into(), c, "delisted"));
                }
                if adj.contains(&c) {
                    self.irregular.push((day.into(), c, "adjusted"));
                }
                num += co.last_price as f64 * fl + div;
                den += v0;
            }
            let prev = self.nav.insert(f.key.clone(), f.nav);
            if f.units > 0 && den > 0.0 {
                if let Some(p) = prev.filter(|p| *p > 0.0) {
                    let (ri, rf) = (num / den - 1.0, f.nav / p - 1.0);
                    self.days.entry(f.key.clone()).or_default().push((day.into(), ri, rf));
                    // the difference in three independent parts: the index two ways (its value sums against its morning
                    // weights × each stock's return: must agree), the fund's morning holdings against the index's
                    // weights, and the net value against those holdings (the day's trading and costs)
                    if let Some((wf, wi)) = self.weights.get(&f.key) {
                        let ids: std::collections::BTreeSet<i64> = wf.keys().chain(wi.keys()).copied().collect();
                        let (mut idx_w, mut hold, mut active) = (0.0f64, 0.0f64, 0.0f64);
                        for c in ids {
                            let (a, b) = (wf.get(&c).copied().unwrap_or(0.0), wi.get(&c).copied().unwrap_or(0.0));
                            active += (a - b).abs() / 2.0;
                            let Some(co) = e.company(c) else { continue };
                            let p0 = self.refs.get(&c).copied().unwrap_or(0);
                            if p0 <= 0 || adj.contains(&c) {
                                continue;
                            }
                            let div = e.company_day(c, day).map_or(0, |d| d.dividend_ps) as f64;
                            let r = (co.last_price as f64 + div) / p0 as f64 - 1.0;
                            idx_w += b * r;
                            hold += a * r;
                            let x = self.per_stock.entry((f.key.clone(), c)).or_default();
                            x.0 += a - b;
                            x.1 += (a - b) * r;
                            x.2 += 1;
                        }
                        let part = [ri - idx_w, hold - idx_w, rf - hold];
                        self.active.entry(f.key.clone()).or_default().push(active);
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
        let col =
            |i: usize| -> Vec<f64> { bench.parts.get(&f.key).map_or(vec![], |p| p.iter().map(|x| x[i]).collect()) };
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let sdv = |v: &[f64]| {
            let m = avg(v);
            (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len().max(2) - 1) as f64).sqrt()
        };
        let mut worst: Vec<(String, f64, f64, [f64; 3])> = days
            .iter()
            .zip(bench.parts.get(&f.key).cloned().unwrap_or_default())
            .map(|(d, p)| (d.0.clone(), d.1, d.2, p))
            .collect();
        worst.sort_by(|a, b| (b.2 - b.1).abs().partial_cmp(&(a.2 - a.1).abs()).unwrap());
        let weekday = |d: &str| {
            chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map(|x| x.format("%a").to_string()).unwrap_or_default()
        };
        let ledger: BTreeMap<String, f64> = bench
            .flows
            .iter()
            .filter(|((k, _), _)| k == &f.key)
            .map(|((_, r), v)| (r.clone(), *v as f64 / 100.0))
            .collect();
        let res = bench.residuals.get(&f.key).cloned().unwrap_or_default();
        let mut worst_res = res.clone();
        worst_res.sort_by_key(|x| -x.1.abs());
        let mut stocks: Vec<(i64, f64, f64, u32)> =
            bench.per_stock.iter().filter(|((k, _), _)| k == &f.key).map(|((_, c), v)| (*c, v.0, v.1, v.2)).collect();
        stocks.sort_by(|a, b| b.2.abs().partial_cmp(&a.2.abs()).unwrap());
        let by_stock: Vec<serde_json::Value> = stocks
            .iter()
            .take(12)
            .map(|(c, act, contrib, n)| {
                let co = e.company(*c);
                json!({"stock": co.map(|x| x.title.clone()), "listed_on": co.map(|x| x.listed_on.clone()), "days": n,
                    "avg_active_pct": act / (*n).max(1) as f64 * 100.0, "contrib_bp": contrib * 1e4})
            })
            .collect();
        let parts_json = json!({
            "by_stock": by_stock,
            "reconcile": {"days": res.len(), "residual_total": res.iter().map(|x| x.1).sum::<i64>() as f64 / 100.0,
                "worst": worst_res.iter().take(8).map(|x| json!({"day": x.0, "residual": x.1 as f64 / 100.0, "moved": x.2[0] as f64 / 100.0, "at_close": x.2[1] as f64 / 100.0, "costs": x.2[2] as f64 / 100.0, "ledger": x.2[3] as f64 / 100.0})).collect::<Vec<_>>()},
            "ledger": ledger,
            "worst_days": worst.iter().take(10).map(|(d, i, fr, p)| json!({"day": d, "weekday": weekday(d), "index_bp": i * 1e4, "fund_bp": fr * 1e4,
                "index_check_bp": p[0] * 1e4, "holdings_bp": p[1] * 1e4, "trading_bp": p[2] * 1e4})).collect::<Vec<_>>(),
            "parts": "index two ways (must be 0), holdings vs the index's weights, net value vs the holdings",
            "active_share": {"mean": avg(&bench.active.get(&f.key).cloned().unwrap_or_default()), "max": bench.active.get(&f.key).map_or(0.0, |v| v.iter().cloned().fold(0.0, f64::max))},
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
