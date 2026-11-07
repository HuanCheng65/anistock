//! A replay's account of the futures (docs/v3-futures.md §4.5): every contract's forecast as the closes saw
//! it, how often the final price fell inside the forecast's 80% interval (at listing, at the middle of the
//! period, in its last week), trading, and the market maker's result.

use std::collections::BTreeMap;

use serde_json::json;

use crate::engine::Engine;

/// z of a central 80% interval
const Z80: f64 = 1.281_551_565_545;

#[derive(Debug, Clone, Default)]
pub struct FuturesLog {
    /// contract -> (work, period, first day, last day, the forecasts in order: (day, mean, sd) in ticks)
    seen: BTreeMap<i64, (i64, i64, String, String, Vec<(String, i64, i64)>)>,
    /// contract -> final price
    finals: BTreeMap<i64, i64>,
    volume: i64,
}

impl FuturesLog {
    /// After the close of `day`.
    pub fn observe(&mut self, e: &Engine, day: &str) {
        let f = &e.st.futures;
        for c in f.contracts.values() {
            let row = self
                .seen
                .entry(c.id)
                .or_insert_with(|| (c.work_id, c.period, c.first_day.clone(), c.last_day.clone(), vec![]));
            if let (Some(&m), Some(&s)) = (f.mean.get(&c.id), f.sd.get(&c.id)) {
                row.4.push((day.to_string(), m, s));
            }
            if let Some((d, ticks)) = f.today.get(&c.id) {
                if d == day {
                    self.volume += ticks.iter().map(|t| t.qty).sum::<i64>();
                }
            }
        }
        for (id, price) in &f.settled {
            self.finals.insert(*id, *price);
        }
    }

    pub fn report(&self, e: &Engine) -> serde_json::Value {
        let mut hits = [(0, 0); 3];
        let mut ratios: [Vec<f64>; 3] = Default::default();
        let mut rows = vec![];
        for (id, (work, period, first, last, fc)) in &self.seen {
            let Some(&fin) = self.finals.get(id) else { continue };
            let Some((a, b)) = crate::futures::forecast::day_range(first, last) else { continue };
            // the first forecast at or after a day of the period
            let from = |d: i64| {
                let d = crate::futures::forecast::day_str(d);
                fc.iter().find(|x| x.0 >= d).cloned()
            };
            let listing = fc.first().cloned();
            let (mid, late) = (from((a + b) / 2), from(b - 6));
            for (k, f) in [&listing, &mid, &late].into_iter().enumerate() {
                if let Some((_, m, s)) = f {
                    ratios[k].push(fin as f64 / (*m).max(1) as f64);
                    hits[k].1 += 1;
                    hits[k].0 += ((fin - m).abs() as f64 <= Z80 * *s as f64) as i64;
                }
            }
            rows.push(json!({"contract": id, "work": work, "period": period, "final": fin as f64 / 100.0,
                "listing": listing.as_ref().map(|x| [x.1 as f64 / 100.0, x.2 as f64 / 100.0])}));
        }
        // the forecast's day-to-day revisions against what the maker's spread assumes (sd / sqrt(days left)),
        // root mean square of both relative to the forecast
        let (mut actual, mut assumed, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for (_, _, _, last, fc) in self.seen.values() {
            let Some(end) = crate::vclose::day_no(last) else { continue };
            for w in fc.windows(2) {
                let (Some(d0), m0, s0, m1) =
                    (crate::vclose::day_no(&w[0].0), w[0].1 as f64, w[0].2 as f64, w[1].1 as f64)
                else {
                    continue;
                };
                if m0 <= 0.0 || d0 >= end {
                    continue;
                }
                actual += ((m1 - m0) / m0).powi(2);
                assumed += (s0 / ((end - d0) as f64).sqrt() / m0).powi(2);
                n += 1.0;
            }
        }
        let share = |(h, n): (i64, i64)| if n > 0 { h as f64 / n as f64 } else { f64::NAN };
        let mm = e.special("fmm");
        let funded = e.st.futures.mm_funded;
        let mm_cash = mm.map_or(0, |a| e.st.accounts[&a].cash);
        let mm_marks: i64 = mm.map_or(0, |a| {
            let f = &e.st.futures;
            f.positions
                .range((a, i64::MIN)..=(a, i64::MAX))
                .map(|(_, p)| f.contracts.get(&p.contract_id).map_or(0, |c| p.variation(c.last_price) * c.tick_value))
                .sum()
        });
        json!({
            "contracts": self.seen.len(),
            "settled": self.finals.len(),
            "volume_lots": self.volume,
            "coverage80": {"listing": share(hits[0]), "mid": share(hits[1]), "last_week": share(hits[2]), "n": hits[0].1},
            "final_over_forecast_median": {"listing": median(&mut ratios[0]), "mid": median(&mut ratios[1]), "last_week": median(&mut ratios[2])},
            "mm_pnl": (mm_cash + mm_marks - funded) as f64 / 100.0,
            "daily_revision_rms": {"actual": (actual / n.max(1.0)).sqrt(), "assumed": (assumed / n.max(1.0)).sqrt(), "n": n},
            "settled_contracts": rows,
        })
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}
