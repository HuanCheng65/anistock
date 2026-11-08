//! The model's forecast of a contract's average index (docs/v3-futures.md §4.3): what listing, the initial
//! margin's floor and the market maker's quotes rest on. The same filter the stock values come from: each
//! market's expected attention path (`vfilter::att_path`), turned into index points (`FuturesRules::index_per_att`, by weeks since the premiere), shrunk by
//! `horizon_drift` per day ahead.
//!
//! mean = (published days + Σ expected days to come) / days. The spread of the days to come: the levels'
//! uncertainty (the filter's covariance plus its random walk to the middle of the remaining days, lognormal,
//! correlated across markets) and `model_cv` of their expectation for everything the filter does not model.

use chrono::Datelike;

use crate::engine::Engine;
use crate::valuation::Params;
use crate::vclose::day_no;
use crate::vfilter::{self as F, FilterParams};

/// A forecast of an average, in index points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Forecast {
    pub mean: f64,
    pub sd: f64,
}

impl Engine {
    /// The forecast of `work`'s average index over `first..=last` (day numbers) as of the close of `today`.
    /// None without a filter state for the work.
    pub(crate) fn fut_forecast(
        &self,
        fp: &FilterParams,
        p: &Params,
        company_id: i64,
        work: i64,
        first: i64,
        last: i64,
        today: i64,
    ) -> Option<Forecast> {
        let js = self.st.jstate.get(&work)?;
        let c = self.st.companies.get(&company_id)?;
        let (sched, first_air, long) = self.work_shape(c, work, p);
        let days = (last - first + 1) as f64;
        // published days of the period
        let mut known = 0.0;
        for t in first..=last.min(today) {
            let d = day_str(t);
            known += self.st.futures.heat.get(&(work, d)).copied().unwrap_or(0.0);
        }
        // the days to come, per market
        let from = first.max(today + 1);
        let mut g = [0.0f64; 3];
        if from <= last {
            let fpc = fp.for_class(js.heat_class);
            let h = (last - today) as usize;
            for (m, gm) in g.iter_mut().enumerate() {
                if !js.active[m] {
                    continue;
                }
                let path =
                    F::att_path(&fpc, p, m, js.mean[m], js.var(m), today, &sched, first_air, long, h, js.tail_mult);
                let rules = &self.futures;
                *gm = (from..=last)
                    .map(|t| {
                        let shrink = (-rules.horizon_drift * (t - today) as f64).exp();
                        rules.index_per_att(m, (t - first_air).div_euclid(7)) * path[(t - today - 1) as usize] * shrink
                    })
                    .sum();
            }
        }
        let expected: f64 = g.iter().sum();
        // the levels' uncertainty, to the middle of the remaining days
        let mid = if from <= last { ((from + last) as f64 / 2.0 - today as f64).max(0.0) } else { 0.0 };
        let mut var = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                if g[i] > 0.0 && g[j] > 0.0 {
                    let s = js.cov[i][j] + if i == j { fp.q[i] * mid } else { 0.0 };
                    var += g[i] * g[j] * (s.exp() - 1.0);
                }
            }
        }
        var += (self.futures.model_cv * expected).powi(2);
        Some(Forecast { mean: (known + expected) / days, sd: var.max(0.0).sqrt() / days })
    }
}

/// `YYYY-MM-DD` of a day number (`vclose::day_no`'s inverse).
pub fn day_str(n: i64) -> String {
    chrono::NaiveDate::from_num_days_from_ce_opt(n as i32).map(|d| d.to_string()).unwrap_or_default()
}

/// The day numbers of a `YYYY-MM-DD` range.
pub fn day_range(first: &str, last: &str) -> Option<(i64, i64)> {
    Some((day_no(first)?, day_no(last)?))
}

impl Engine {
    /// At startup: list what the last close would list today (`futures_after_close` again for the last closed
    /// day; periods already listed are skipped and the forecasts come out the same). With the listing week
    /// (calendar::listed_on) a deploy or a missed close lists the periods starting within 7 days at once,
    /// not at the next close. Returns the contracts listed.
    pub fn futures_catch_up(&mut self, cfg: &crate::config::Config) -> Result<usize, String> {
        if !self.futures.enabled {
            return Ok(0);
        }
        let Some(day) = self.state("last_closed").filter(|d| !d.is_empty()).map(str::to_string) else { return Ok(0) };
        let (fp, p) = (FilterParams::load(cfg)?, Params::from_config(cfg)?);
        let before = self.st.futures.contracts.len();
        self.futures_after_close(&fp, &p, &day);
        Ok(self.st.futures.contracts.len() - before)
    }

    /// After the day's valuation, in the close of `day`: refresh every listed contract's forecast (the market
    /// maker's reference, the initial margin's floor) and list the periods due tomorrow.
    pub(crate) fn futures_after_close(&mut self, fp: &FilterParams, p: &Params, day: &str) {
        if !self.futures.enabled {
            return;
        }
        let Some(today) = day_no(day) else { return };
        let ids: Vec<i64> = self.st.futures.contracts.values().filter(|c| c.trading()).map(|c| c.id).collect();
        for id in ids {
            let c = &self.st.futures.contracts[&id];
            let Some((first, last)) = day_range(&c.first_day, &c.last_day) else { continue };
            if let Some(f) = self.fut_forecast(fp, p, c.company_id, c.work_id, first, last, today) {
                self.st.futures.mean.insert(id, (f.mean * 100.0).round() as i64);
                self.st.futures.sd.insert(id, (f.sd * 100.0).round() as i64);
            }
        }
        let next = today + 1;
        let next_day = chrono::NaiveDate::from_num_days_from_ce_opt(next as i32).expect("day");
        let weeks = self.futures.weeks;
        let mut due = vec![];
        for c in self.st.companies.values().filter(|c| c.status == "listed") {
            for w in self.works_of(c.id) {
                let (_, first_air, _) = self.work_shape(c, w, p);
                let Some(premiere) = chrono::NaiveDate::from_num_days_from_ce_opt(first_air as i32) else { continue };
                if let Some(k) = super::calendar::listed_on(premiere, next_day, weeks) {
                    let (a, b) = super::calendar::period_days(premiere, k, weeks);
                    let (a, b) = (a.num_days_from_ce() as i64, b.num_days_from_ce() as i64);
                    if let Some(f) = self.fut_forecast(fp, p, c.id, w, a, b, today) {
                        due.push((super::listing::Underlying { company_id: c.id, work_id: w, premiere }, k, f));
                    }
                }
            }
        }
        let mut works: Vec<_> = due.iter().map(|(u, _, _)| *u).collect();
        works.dedup();
        let rules = self.futures.clone();
        let find = |work: i64, k: i64| due.iter().find(|(u, j, _)| u.work_id == work && *j == k).map(|(_, _, f)| *f);
        let new = self.futures_list(&rules, day, next_day, &works, |u, k, _, _| find(u.work_id, k).map(|f| f.mean));
        for id in new {
            let c = &self.st.futures.contracts[&id];
            let f = find(c.work_id, c.period).unwrap();
            self.st.futures.mean.insert(id, (f.mean * 100.0).round() as i64);
            self.st.futures.sd.insert(id, (f.sd * 100.0).round() as i64);
        }
    }
}
