//! The filtered valuation inside the engine: per-show filter state (table `valuation_state`), its daily update
//! at the close, the 热度快讯 update during the day, and the values of every listed company from it.
//! Formulas: `vfilter.rs` (= engine/valuation.py).

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};

use crate::config::Config;
use crate::engine::Engine;
use crate::inputs::ShowInputs;
use crate::model::Company;
use crate::store::persist::Change;
use crate::valuation::Params;
use crate::vfilter::{self as F, FilterParams};

/// One market's filter state for one show.
#[derive(Debug, Clone, PartialEq)]
pub struct VState {
    pub mean: f64,
    pub var: f64,
    pub last_obs: Option<f64>,
    pub day: String,
    /// us: this episode week's daily log values so far (sum, count), observed together when it ends
    pub acc: f64,
    pub acc_n: i64,
}

/// Day number of the first day of the next anime season (1 Jan/Apr/Jul/Oct) after `today`.
pub fn next_season_start(today: i64) -> i64 {
    use chrono::Datelike;
    let Some(d) = NaiveDate::from_num_days_from_ce_opt(today as i32) else { return today + 91 };
    let (y, q) = (d.year(), (d.month0() / 3 + 1) * 3 + 1);
    let n = if q > 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, q, 1) };
    n.map_or(today + 91, |n| n.num_days_from_ce() as i64)
}

/// Day number of `YYYY-MM-DD` (= Python `date.toordinal()`).
pub fn day_no(s: &str) -> Option<i64> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| d.num_days_from_ce() as i64)
}

/// What a 热度快讯 did to a show's value.
#[derive(Debug, Clone)]
pub struct FlashOutcome {
    /// comments / expected - 1
    pub surprise: f64,
    pub old_value: i64,
    pub new_value: i64,
}

impl Engine {
    pub fn valuation_state(&self, company: i64, market: usize) -> Option<&VState> {
        self.st.vstate.get(&(company, market))
    }

    fn save_vstate(&mut self, company: i64, market: usize, s: VState, ts: &str) {
        self.out.push(Change::ValuationState {
            company_id: company,
            market: F::MARKETS[market].to_string(),
            mean: s.mean,
            var: s.var,
            last_obs: s.last_obs,
            day: s.day.clone(),
            acc: s.acc,
            acc_n: s.acc_n,
            updated_at: ts.to_string(),
        });
        self.st.vstate.insert((company, market), s);
    }

    /// Air days (day numbers), one entry per episode, from the company's daily `aired` counts.
    pub fn air_days_of(&self, company: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut prev = 0;
        for (_, r) in self.st.company_days.range((company, String::new())..).take_while(|(k, _)| k.0 == company) {
            if r.aired > prev {
                if let Some(d) = day_no(&r.day) {
                    for _ in prev..r.aired {
                        out.push(d);
                    }
                }
            }
            prev = prev.max(r.aired);
        }
        out
    }

    fn shape(&self, c: &Company, p: &Params) -> (Vec<i64>, i64, bool) {
        let long = c.long_runner != 0;
        let episodes = c.episodes.or(if long { None } else { Some(p.default_episodes) });
        let first_air = day_no(&c.first_air).unwrap_or(0);
        let mut aired = self.air_days_of(c.id);
        if aired.is_empty() && !long {
            // not aired yet: the first episode on its announced date, then weekly
            aired.push(first_air);
        }
        (F::schedule(&aired, episodes, long), first_air, long)
    }

    /// A state started from a first observation (no usable prior): its level, with the observation's own
    /// noise as the uncertainty (a single us day is noisier than the weekly average the filter is fitted on:
    /// seven times its variance), capped at the prior's. The us day also opens the week's average.
    fn state_from_observation(fp: &FilterParams, m: usize, att: f64, k: Option<i64>, day: &str) -> VState {
        let y = (att / F::curve(fp, m, k)).ln();
        if m == 0 {
            VState { mean: y, var: (7.0 * fp.r[m]).min(fp.p0[m]), last_obs: Some(att), day: day.into(), acc: y, acc_n: 1 }
        } else {
            VState { mean: y, var: fp.r[m].min(fp.p0[m]), last_obs: Some(att), day: day.into(), acc: 0.0, acc_n: 0 }
        }
    }

    /// The close's filter step for one show (docs/v2-valuation.md §3.2): before the premiere the prior
    /// (pre-air estimate); then each new observation (us daily, cn/jp when their weekly value changes).
    pub(crate) fn valuation_step(&mut self, fp: &FilterParams, s: &ShowInputs, day: &str, ts: &str) {
        let Some(today) = day_no(day) else { return };
        let sched = self.air_days_of(s.id);
        let k = F::last_air_before(&sched, today).map(|a| today - a);
        let att = s.att.m3();
        let prior = s.preair.as_ref().map_or(s.expected.m3(), |p| p.m3());
        let premiered = s.aired >= 1;
        for m in 0..3 {
            // a show that has aired for four weeks and has never had anything in
            // this market, earns nothing there: no state (its revenue share there is 0). Once a market has
            // reported, a missing day is just a gap.
            let never_seen = self.st.vstate.get(&(s.id, m)).is_none_or(|v| v.last_obs.is_none());
            // (some markets' series start a few weeks late: four weeks of silence settles it)
            let long_aired = day_no(&s.first_air).is_some_and(|f| today - f >= 28);
            // (also a "show" listed years after its first date that never airs: a teaser, a stalled project)
            if long_aired && att[m] <= 0.0 && never_seen {
                if self.st.vstate.remove(&(s.id, m)).is_some() {
                    self.out.push(Change::ValuationStateDelete { company_id: s.id, market: F::MARKETS[m].to_string() });
                }
                continue;
            }
            let cur = self.st.vstate.get(&(s.id, m)).cloned();
            let next = match cur {
                None => {
                    // a show seen for the first time starts from its pre-air prior, unless it already has real
                    // data in this market (a market whose series starts late, a long-runner): then from today's
                    // value. Data then arrive through the filter.
                    if premiered && s.has_real_data && att[m] > 0.0 && (long_aired || prior[m] <= 0.0) {
                        Some(Self::state_from_observation(fp, m, att[m], k, day))
                    } else if prior[m] > 0.0 {
                        Some(VState { mean: prior[m].ln() + fp.prior_bias[m], var: fp.p0[m], last_obs: None, day: day.into(), acc: 0.0, acc_n: 0 })
                    } else if premiered && att[m] > 0.0 {
                        Some(Self::state_from_observation(fp, m, att[m], k, day))
                    } else {
                        None
                    }
                }
                Some(_) if !premiered => (prior[m] > 0.0).then(|| VState {
                    mean: prior[m].ln() + fp.prior_bias[m],
                    var: fp.p0[m],
                    last_obs: None,
                    day: day.into(),
                    acc: 0.0,
                    acc_n: 0,
                }),
                Some(st) => {
                    let days = day_no(&st.day).map_or(1, |d| (today - d).max(0));
                    let drift = fp.drift_for(m, s.long_runner);
                    let (mut mean, mut var) = F::kf_predict(st.mean, st.var, days, drift, fp.q[m]);
                    let mut last = st.last_obs;
                    let (mut acc, mut acc_n) = (st.acc, st.acc_n);
                    let mut y = None;
                    if m == 0 {
                        // us: the episode week's average, observed when the next episode airs (or after 7 days)
                        if acc_n > 0 && (k == Some(0) || acc_n >= 7) {
                            y = Some(F::week_observation(acc / acc_n as f64, acc_n, drift));
                            acc = 0.0;
                            acc_n = 0;
                        }
                        if att[m] > 0.0 {
                            acc += (att[m] / F::curve(fp, m, k)).ln();
                            acc_n += 1;
                            last = Some(att[m]);
                        }
                    } else if att[m] > 0.0 && last != Some(att[m]) {
                        last = Some(att[m]);
                        y = Some(att[m].ln());
                    }
                    if let Some(y) = y {
                        let (m2, v2, _) = F::robust_update(mean, var, y, fp.r[m], fp.outlier_p[m], fp.outlier_r[m], fp.clip);
                        mean = m2;
                        var = v2;
                    }
                    Some(VState { mean, var, last_obs: last, day: day.into(), acc, acc_n })
                }
            };
            if let Some(n) = next {
                self.save_vstate(s.id, m, n, ts);
            }
        }
    }

    /// Expected attention paths of every listed company with a filter state, from `today`.
    fn paths(&self, fp: &FilterParams, p: &Params, today: i64) -> HashMap<i64, [Vec<f64>; 3]> {
        let h = fp.horizon_days;
        let mut out = HashMap::new();
        for c in self.st.companies.values().filter(|c| c.status == "listed") {
            if !(0..3).any(|m| self.st.vstate.contains_key(&(c.id, m))) {
                continue;
            }
            let (sched, first_air, long) = self.shape(c, p);
            let path: [Vec<f64>; 3] = std::array::from_fn(|m| match self.st.vstate.get(&(c.id, m)) {
                Some(s) => F::att_path(fp, p, m, s.mean, s.var, today, &sched, first_air, long, h),
                None => vec![0.0; h],
            });
            out.insert(c.id, path);
        }
        out
    }

    /// Today's total attention per market: the expected levels of the listed shows on air (and the
    /// long-runners).
    fn current_total(&self, today: i64) -> [f64; 3] {
        let mut cs: Vec<&Company> = self.st.companies.values().filter(|c| c.status == "listed" && Self::in_totals(c, today)).collect();
        cs.sort_unstable_by_key(|c| c.id);
        let mut t = [0.0f64; 3];
        for c in cs {
            for (m, tm) in t.iter_mut().enumerate() {
                if let Some(s) = self.st.vstate.get(&(c.id, m)) {
                    *tm += (s.mean + 0.5 * s.var).exp();
                }
            }
        }
        t
    }

    /// Record today's total (state `valuation_totals`: "day:us,cn,jp" for the last 100 days). Once per close
    /// (and per history day of the bootstrap).
    pub(crate) fn record_total(&mut self, today: i64) {
        let cur = self.current_total(today);
        let mut hist: Vec<(i64, [f64; 3])> = self.totals_history().into_iter().filter(|(d, _)| *d < today && *d > today - 100).collect();
        hist.push((today, cur));
        let v: Vec<String> = hist
            .iter()
            .map(|(d, t)| format!("{d}:{},{},{}", crate::pyfmt::repr(t[0]), crate::pyfmt::repr(t[1]), crate::pyfmt::repr(t[2])))
            .collect();
        self.set_state("valuation_totals", &v.join(";"));
    }

    fn totals_history(&self) -> Vec<(i64, [f64; 3])> {
        let Some(s) = self.state("valuation_totals") else { return Vec::new() };
        s.split(';')
            .filter_map(|e| {
                let (d, t) = e.split_once(':')?;
                let v: Vec<f64> = t.split(',').filter_map(|x| x.parse().ok()).collect();
                Some((d.parse().ok()?, [*v.first()?, *v.get(1)?, *v.get(2)?]))
            })
            .collect()
    }

    /// The market's total attention per market and future day (docs/v2-valuation.md §3.3). The shows on
    /// air now fade as the filter expects each of them to (long-runners at their own rate) until the next
    /// season starts (1 January / April / July / October); listed shows not yet airing join from their first
    /// air date. From the next season on, the total is back to at least its peak of the last 100 days (next
    /// season's shows replace this season's). So a show's share does not drift up just because every show
    /// fades, and neither the season change nor a premiere is a predictable surprise.
    fn totals(&self, fp: &FilterParams, today: i64) -> [Vec<f64>; 3] {
        let h = fp.horizon_days;
        let mut cs: Vec<&Company> = self.st.companies.values().filter(|c| c.status == "listed" && !Self::in_totals(c, today)).collect();
        cs.sort_unstable_by_key(|c| c.id);
        let mut later: [Vec<f64>; 3] = std::array::from_fn(|_| vec![0.0; h]);
        for c in cs {
            let start = day_no(&c.first_air).map_or(h as i64, |d| d - today).max(0);
            for m in 0..3 {
                if let Some(s) = self.st.vstate.get(&(c.id, m)) {
                    let level = (s.mean + 0.5 * s.var).exp();
                    if (start as usize) < h {
                        for x in &mut later[m][start as usize..] {
                            *x += level;
                        }
                    }
                }
            }
        }
        // the shows on air now, split by how their level is expected to move: seasonal shows at the filter's
        // drift (level × exp(drift + q/2) per day, as the filter itself predicts), long-runners at their decay
        let (mut seasonal, mut long) = ([0.0f64; 3], [0.0f64; 3]);
        let mut on_air: Vec<&Company> = self.st.companies.values().filter(|c| c.status == "listed" && Self::in_totals(c, today)).collect();
        on_air.sort_unstable_by_key(|c| c.id);
        for c in on_air {
            for m in 0..3 {
                if let Some(s) = self.st.vstate.get(&(c.id, m)) {
                    let level = (s.mean + 0.5 * s.var).exp();
                    if c.long_runner != 0 { long[m] += level } else { seasonal[m] += level }
                }
            }
        }
        let cur = self.current_total(today);
        let mut peak = cur;
        for (_, t) in self.totals_history() {
            for m in 0..3 {
                peak[m] = peak[m].max(t[m]);
            }
        }
        let season = next_season_start(today) - today;
        std::array::from_fn(|m| {
            (0..h)
                .map(|t| {
                    // path index t is day today + t + 1
                    let j = t as i64 + 1;
                    let x = seasonal[m] * ((fp.drift[m] + 0.5 * fp.q[m]) * j as f64).exp()
                        + long[m] * fp.long_runner_decay.powf(j as f64 / 7.0)
                        + later[m][t];
                    if j < season { x } else { x.max(peak[m]) }
                })
                .collect()
        })
    }

    fn in_totals(c: &Company, today: i64) -> bool {
        c.long_runner != 0 || day_no(&c.first_air).is_some_and(|d| d <= today)
    }

    fn with_own(totals: &[Vec<f64>; 3], own: &[Vec<f64>; 3]) -> [Vec<f64>; 3] {
        std::array::from_fn(|m| totals[m].iter().zip(&own[m]).map(|(t, o)| t + o).collect())
    }

    fn value_of(&self, c: &Company, own: &[Vec<f64>; 3], totals: &[Vec<f64>; 3], p: &Params, rate: f64, a_ref: f64) -> i64 {
        let book = self.account_of("company", c.id).map_or(0, |a| self.st.accounts[&a].cash);
        let v = F::value_from_paths(own, totals, &p.market_weights, a_ref, p, c.shares, book, rate, c.long_runner != 0);
        ((v * 100.0).round_ties_even() as i64).max(1)
    }

    /// Hidden values of every listed company with a filter state (cents), and the market totals used.
    pub(crate) fn valuation_values(&self, fp: &FilterParams, p: &Params, today: i64, rate: f64, a_ref: f64) -> (HashMap<i64, i64>, [Vec<f64>; 3]) {
        let paths = self.paths(fp, p, today);
        let totals = self.totals(fp, today);
        let mut vals = HashMap::new();
        let trace_all = std::env::var("SIM_TRACE").is_ok_and(|v| v == "all");
        let trace: Option<i64> = std::env::var("SIM_TRACE").ok().and_then(|v| v.parse().ok());
        for (id, own) in &paths {
            let c = &self.st.companies[id];
            let v = self.value_of(c, own, &totals, p, rate, a_ref);
            if trace_all || trace == Some(*id) {
                let st: Vec<String> = (0..3)
                    .map(|m| self.st.vstate.get(&(*id, m)).map_or("-".into(), |s| format!("{:.3}±{:.3}", s.mean, s.var.sqrt())))
                    .collect();
                let own_sum: Vec<f64> = (0..3).map(|m| own[m].iter().take(28).sum::<f64>()).collect();
                let tot_sum: Vec<f64> = (0..3).map(|m| totals[m].iter().take(28).sum::<f64>()).collect();
                eprintln!("TRACE {today} {id} value {v} states {st:?} own28 {own_sum:.0?} tot28 {tot_sum:.0?}");
            }
            vals.insert(*id, v);
        }
        (vals, totals)
    }

    /// At go-live (no filter state yet, the v2 economy): bootstrap from the daily inputs files on disk, up
    /// to 120 days back to the last close (`$ANISTOCK_INPUTS_DIR` or data/derived/inputs). Returns the number
    /// of companies valued, None when there was nothing to do. Without history files the filter simply
    /// starts at the next close (from each show's pre-air estimate, or its first observation).
    pub fn valuation_go_live(&mut self, cfg: &Config, ts: &str) -> Result<Option<usize>, String> {
        if cfg.economy_v1 || !self.st.vstate.is_empty() {
            return Ok(None);
        }
        let Some(last) = self.state("last_closed").map(str::to_string) else { return Ok(None) };
        let Some(last_no) = day_no(&last) else { return Ok(None) };
        let dir = crate::inputs::default_path(cfg, &last).parent().map(|p| p.to_path_buf()).ok_or("inputs dir")?;
        let mut days: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|f| f.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
            .filter(|d| day_no(d).is_some_and(|n| n <= last_no && n > last_no - 120))
            .collect();
        days.sort();
        if days.is_empty() {
            return Ok(None);
        }
        let mut history = Vec::new();
        for d in &days {
            match crate::inputs::Inputs::read(&dir.join(format!("{d}.json"))) {
                Ok(i) => history.push(i),
                Err(e) => eprintln!("valuation bootstrap: skipping {d}: {e}"),
            }
        }
        self.valuation_bootstrap(cfg, &history, ts).map(Some)
    }

    /// Start the filter from history: run the daily step over past inputs (oldest first), then value every
    /// listed company. For a database that has never had the filtered valuation (go-live, a new world).
    pub fn valuation_bootstrap(&mut self, cfg: &Config, history: &[crate::inputs::Inputs], ts: &str) -> Result<usize, String> {
        let fp = FilterParams::load(cfg)?;
        let p = Params::from_config(cfg)?;
        let mut last = None;
        for inp in history {
            let listed: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
            for id in listed {
                if let Some(s) = inp.show(id) {
                    self.valuation_step(&fp, s, &inp.day, ts);
                }
            }
            if let Some(d) = day_no(&inp.day) {
                self.record_total(d);
            }
            last = Some(inp.day.clone());
        }
        let Some(day) = last else { return Ok(0) };
        // the history ends mid-week: observe the us days collected so far (at least three), so the first
        // live close doesn't revise every show at once
        let mut part: Vec<((i64, usize), VState)> =
            self.st.vstate.iter().filter(|(k, v)| k.1 == 0 && v.acc_n >= 3).map(|(k, v)| (*k, v.clone())).collect();
        part.sort_by_key(|(k, _)| *k);
        for ((id, m), mut st) in part {
            let long = self.st.companies.get(&id).is_some_and(|c| c.long_runner != 0);
            let y = F::week_observation(st.acc / st.acc_n as f64, st.acc_n, fp.drift_for(m, long));
            let (m2, v2, _) = F::robust_update(st.mean, st.var, y, fp.r[m], fp.outlier_p[m], fp.outlier_r[m], fp.clip);
            st.mean = m2;
            st.var = v2;
            st.acc = 0.0;
            st.acc_n = 0;
            self.save_vstate(id, m, st, ts);
        }
        let today = day_no(&day).ok_or("bad day")?;
        let rate: f64 = self.state("rate").and_then(crate::pyfmt::parse_float).unwrap_or(0.03);
        let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).unwrap_or(1.0);
        let (vals, _) = self.valuation_values(&fp, &p, today, rate, a_ref);
        for (id, v) in &vals {
            self.st.companies.get_mut(id).unwrap().fair_value = *v;
            self.mark_company(*id);
        }
        self.commit();
        Ok(vals.len())
    }

    /// IPO price of a new show from the same model's pre-air prior, less a discount for its uncertainty.
    pub(crate) fn ipo_price_v2(&self, fp: &FilterParams, p: &Params, s: &ShowInputs, shares: i64, today: i64, totals: &[Vec<f64>; 3], rate: f64, a_ref: f64) -> i64 {
        let prior = s.preair.as_ref().map_or(s.expected.m3(), |x| x.m3());
        let long = s.long_runner;
        let episodes = s.episodes.or(if long { None } else { Some(p.default_episodes) });
        let first = day_no(&s.first_air).unwrap_or(today);
        let sched = F::schedule(&[first], episodes, long);
        let own: [Vec<f64>; 3] = std::array::from_fn(|m| {
            if prior[m] > 0.0 {
                F::att_path(fp, p, m, prior[m].ln() + fp.prior_bias[m], fp.p0[m], today, &sched, first, long, fp.horizon_days)
            } else {
                vec![0.0; fp.horizon_days]
            }
        });
        // (the new show is not in today's totals yet: it joins them)
        let v = F::value_from_paths(&own, &Self::with_own(totals, &own), &p.market_weights, a_ref, p, shares, 0, rate, long);
        let used: Vec<f64> = (0..3).filter(|m| prior[*m] > 0.0).map(|m| fp.p0[m]).collect();
        let sd = if used.is_empty() { 0.0 } else { (used.iter().sum::<f64>() / used.len() as f64).sqrt() };
        let discount = 1.0 - (-fp.ipo_risk * sd).exp();
        let min = (p.ipo_price_min * 100.0).round_ties_even() as i64;
        min.max((v * (1.0 - discount) * 100.0).round_ties_even() as i64)
    }

    /// A 热度快讯: the episode's comments `hours` after airing, as a noisy observation of the show's cn level
    /// (docs/v2-valuation.md §3.4). Updates the filter and the hidden value at once. None when the
    /// observation carries no information (no comparison base, too early, unknown company).
    ///
    /// The hourly observations of one episode are not independent: each replaces the previous one. The
    /// state remembers (`flash:<company>` in the game state) the level before the episode's first flash and
    /// where the last one put it; a new hour's observation moves the level by the difference only (so a
    /// close in between keeps what it learned).
    #[allow(clippy::too_many_arguments)]
    pub fn valuation_flash(&mut self, cfg: &Config, company: i64, episode: f64, hours: f64, comments: i64, prev_final: &[i64], ts: &str) -> Result<Option<FlashOutcome>, String> {
        let fp = FilterParams::load(cfg)?;
        let p = Params::from_config(cfg)?;
        let Some(c) = self.st.companies.get(&company).filter(|c| c.status == "listed").cloned() else { return Ok(None) };
        let Some((y_rel, r)) = F::flash_observation(&fp, hours, comments, prev_final) else { return Ok(None) };
        let Some(st) = self.st.vstate.get(&(company, 1)).cloned() else { return Ok(None) };
        let rate: f64 = self.state("rate").and_then(crate::pyfmt::parse_float).unwrap_or(0.03);
        let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).unwrap_or(1.0);
        let today = day_no(&st.day).unwrap_or(0);
        // observation of this episode's level: the level before the episode's first flash, moved by the surprise
        let key = format!("flash:{company}");
        let prior: Option<(f64, f64, f64)> = self.state(&key).and_then(|v| {
            let x: Vec<&str> = v.split(';').collect();
            (x.len() == 4 && x[0].parse::<f64>().ok() == Some(episode))
                .then(|| Some((x[1].parse().ok()?, x[2].parse().ok()?, x[3].parse().ok()?)))
                .flatten()
        });
        let (base_mean, base_var, last) = prior.unwrap_or((st.mean, st.var, st.mean));
        let (post_mean, post_var, _) = F::kf_update(base_mean, base_var, base_mean + y_rel, r, fp.clip);
        let mean = st.mean + (post_mean - last);
        // the uncertainty shrinks once per episode (a later hour refines the same observation)
        let var = if prior.is_some() { st.var } else { post_var };
        let v = format!("{episode};{};{};{}", crate::pyfmt::repr(base_mean), crate::pyfmt::repr(base_var), crate::pyfmt::repr(post_mean));
        self.set_state(&key, &v);
        let old = c.fair_value;
        self.save_vstate(company, 1, VState { mean, var, ..st.clone() }, ts);
        let paths = self.paths(&fp, &p, today);
        let totals = self.totals(&fp, today);
        let new = match paths.get(&company) {
            Some(own) => self.value_of(&c, own, &totals, &p, rate, a_ref),
            None => old,
        };
        self.st.companies.get_mut(&company).unwrap().fair_value = new;
        self.mark_company(company);
        self.commit();
        Ok(Some(FlashOutcome { surprise: y_rel.exp() - 1.0, old_value: old, new_value: new }))
    }
}
