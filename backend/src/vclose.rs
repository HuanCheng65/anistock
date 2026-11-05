//! The filtered valuation inside the engine: per-show joint filter state (table `valuation_joint`), its daily
//! update at the close, the 热度快讯 update during the day, and the values of every listed company from it.
//! Formulas: `vfilter.rs` (= engine/valuation.py).

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::engine::Engine;
use crate::inputs::ShowInputs;
use crate::model::Company;
use crate::store::persist::Change;
use crate::valuation::{Params, M3};
use crate::vfilter::{self as F, Cov, FilterParams};

/// The joint filter's state for one show (docs/v2-valuation.md §3.2): the log levels of the three markets
/// (us, cn, jp) and their covariance. A market whose data source the show is not mapped to (no Bangumi /
/// Annict entry), or that reports zero for four weeks after the premiere, is inactive: it earns nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JState {
    pub mean: M3,
    pub cov: Cov,
    pub active: [bool; 3],
    /// a positive observation has arrived
    pub seen: [bool; 3],
    /// the last raw value (cn/jp weekly series: an update happens when it changes)
    pub last_obs: [Option<f64>; 3],
    /// days the market reported zero (cn: a week without comments)
    pub zero_days: [i64; 3],
    /// us: this episode week's daily log values so far (sum, count), observed together when it ends
    pub acc: f64,
    pub acc_n: i64,
    pub day: String,
    /// the post-finale factor on `day` (1 before the finale's week): the levels above are pre-finale
    /// equivalents, the show draws level × post now
    #[serde(default = "ones")]
    pub post: M3,
    /// the show's long-tail prior (score and format, `vfilter::tail_mult`): follows the inputs until the finale's
    /// week ends, then fixed (the post-finale observations are read through it)
    #[serde(default = "one")]
    pub tail_mult: f64,
    /// the sequel option (docs/v2-series.md §5): the announcement hazard's log-odds shift of the show and of its
    /// sequel (fixed at the finale's week, like `tail_mult`)
    #[serde(default)]
    pub seq_z: f64,
    #[serde(default)]
    pub seq_zs: f64,
    /// an announced sequel: its id, the announcement day and its premiere day(s) (day numbers)
    #[serde(default)]
    pub sequel: Option<(i64, i64, Vec<i64>)>,
}

fn one() -> f64 {
    1.0
}

fn ones() -> M3 {
    [1.0; 3]
}

impl JState {
    pub fn empty(day: &str) -> JState {
        JState {
            mean: [0.0; 3],
            cov: [[0.0; 3]; 3],
            active: [false; 3],
            seen: [false; 3],
            last_obs: [None; 3],
            zero_days: [0; 3],
            acc: 0.0,
            acc_n: 0,
            day: day.to_string(),
            post: [1.0; 3],
            tail_mult: 1.0,
            seq_z: 0.0,
            seq_zs: 0.0,
            sequel: None,
        }
    }
    fn var(&self, m: usize) -> f64 {
        self.cov[m][m]
    }
}

/// Day number of the start of the anime season (1 Jan/Apr/Jul/Oct) containing `day`.
pub fn prev_season_start(day: i64) -> i64 {
    let Some(d) = NaiveDate::from_num_days_from_ce_opt(day as i32) else { return day };
    let q = d.month0() / 3 * 3 + 1;
    NaiveDate::from_ymd_opt(d.year(), q, 1).map_or(day, |n| n.num_days_from_ce() as i64)
}

/// Day number of the first day of the next anime season (1 Jan/Apr/Jul/Oct) after `today`.
pub fn next_season_start(today: i64) -> i64 {
    use chrono::Datelike;
    let Some(d) = NaiveDate::from_num_days_from_ce_opt(today as i32) else { return today + 91 };
    let (y, q) = (d.year(), (d.month0() / 3 + 1) * 3 + 1);
    let n = if q > 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, q, 1) };
    n.map_or(today + 91, |n| n.num_days_from_ce() as i64)
}

/// Why the inputs cannot feed the fitted filter properly, if they can't: inputs written before v0.3.2 have no
/// point-in-time prior and no observed/source flags, and every show's filter then starts from its first data
/// (a much worse valuation that silently follows public heat with a delay). None when they are fine.
pub fn inputs_schema_problem(fp: &FilterParams, inp: &crate::inputs::Inputs) -> Option<String> {
    if !fp.prior_from_us || inp.shows.is_empty() {
        return None;
    }
    let missing = inp.shows.iter().filter(|s| s.prior.is_none() || s.observed.is_none() || s.sources.is_none()).count();
    (missing > 0).then(|| {
        format!(
            "{missing} of {} shows in the inputs for {} lack prior/observed/sources (inputs written before v0.3.2?): \
             regenerate them with engine/daily_inputs.py (--from … --force)",
            inp.shows.len(),
            inp.day
        )
    })
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
    pub fn valuation_state(&self, company: i64) -> Option<&JState> {
        self.st.jstate.get(&company)
    }

    fn save_jstate(&mut self, company: i64, s: JState, ts: &str) {
        self.out.push(Change::ValuationJoint {
            company_id: company,
            state: serde_json::to_string(&s).unwrap_or_default(),
            day: s.day.clone(),
            updated_at: ts.to_string(),
        });
        self.st.jstate.insert(company, s);
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

    /// The episode count to plan with: the announced one, unless the show has aired nothing for two weeks
    /// since its last episode (a whole season released at once, or a stop): then what has aired.
    pub(crate) fn planned_episodes(episodes: Option<i64>, aired: &[i64], today: i64) -> Option<i64> {
        match (episodes, aired.last()) {
            (Some(n), Some(&last)) if (aired.len() as i64) < n && today - last >= 14 => Some(aired.len() as i64),
            _ => episodes,
        }
    }

    fn shape(&self, c: &Company, p: &Params) -> (Vec<i64>, i64, bool) {
        let long = c.long_runner != 0;
        let first_air = day_no(&c.first_air).unwrap_or(0);
        let aired0 = self.air_days_of(c.id);
        let today = self.st.jstate.get(&c.id).and_then(|j| day_no(&j.day)).unwrap_or(first_air);
        let episodes = Self::planned_episodes(c.episodes.or(if long { None } else { Some(p.default_episodes) }), &aired0, today);
        let mut aired = aired0;
        if aired.is_empty() && !long {
            // not aired yet: the first episode on its announced date, then weekly
            aired.push(first_air);
        }
        (F::schedule(&aired, episodes, long), first_air, long)
    }

    /// The prior log levels of a show's three markets (point-in-time, frozen at the premiere by the inputs):
    /// from the us pre-air estimate and the fitted per-market bias; a market with its own pre-air snapshot
    /// estimate uses that instead (not yet calibrated: bias 0). None without any estimate.
    fn prior_mean(fp: &FilterParams, s: &ShowInputs, today: i64) -> Option<M3> {
        if fp.prior_from_us {
            let p = s.prior.as_ref()?.m3();
            if p[0] <= 0.0 {
                return None;
            }
            // the estimate keeps growing until it freezes the day before the premiere: its expected growth
            let k = day_no(&s.first_air).map_or(0, |f| f - 1 - today);
            let (g, _) = fp.growth(k);
            let b = fp.prior_beta;
            Some(std::array::from_fn(|m| if m > 0 && p[m] > 0.0 { p[m].ln() + g } else { fp.prior_bias[m] + b[m] * (p[0].ln() + g) }))
        } else {
            // v0.3.0 parameters: each market from its own (look-ahead) estimate
            let p = s.preair.as_ref().map_or(s.expected.m3(), |x| x.m3());
            (p[0] > 0.0).then(|| std::array::from_fn(|m| if p[m] > 0.0 { p[m].ln() + fp.prior_bias[m] } else { p[0].ln() + fp.prior_bias[m] }))
        }
    }

    /// Which markets a show can earn in: those whose data source it is mapped to (older inputs: all).
    /// The prior covariance, plus the common uncertainty of the pre-air estimate's growth until it freezes.
    fn prior_cov_at(fp: &FilterParams, s: &ShowInputs, today: i64) -> Cov {
        let k = day_no(&s.first_air).map_or(0, |f| f - 1 - today);
        let (_, v) = fp.growth(k);
        let c = F::prior_cov(&fp.p0, fp.rho);
        let b = fp.prior_beta;
        std::array::from_fn(|i| std::array::from_fn(|j| c[i][j] + b[i] * b[j] * v))
    }

    /// The absolute revenue rule in force (docs/v2-series.md §6): the platform sizes of the last close and the
    /// scale set at the switch (state `platform_rule`: "us,cn,jp,scale"); None while the relative rule applies.
    pub fn platform_rule(&self) -> F::PlatformRule {
        let v: Vec<f64> = self.state("platform_rule")?.split(',').filter_map(crate::pyfmt::parse_float).collect();
        (v.len() == 4).then(|| ([v[0], v[1], v[2]], v[3]))
    }

    /// The sequel option per market in attention-days discounted to today (docs/v2-series.md §5), while the show's
    /// next season is not valued on its own (an announced sequel that has a filter state of its own carries its
    /// value from then on).
    #[allow(clippy::too_many_arguments)]
    fn sequel_extra(&self, fp: &FilterParams, p: &Params, js: &JState, sched: &[i64], long: bool, first_air: i64, today: i64, rate: f64) -> Option<M3> {
        fp.sequel.as_ref()?;
        if long {
            return None;
        }
        let fin = *sched.last()?;
        if js.sequel.as_ref().is_some_and(|q| self.st.jstate.contains_key(&q.0)) {
            return None;
        }
        let known: &[i64] = js.sequel.as_ref().map_or(&[], |q| &q.2);
        let disc = F::discount_day(rate, p);
        Some(std::array::from_fn(|m| {
            if js.active[m] {
                F::sequel_extra(fp, m, js.mean[m], js.var(m), today, fin, first_air, js.seq_z, js.seq_zs, disc, known)
            } else {
                0.0
            }
        }))
    }

    /// The show's long-tail prior from its score and format (`vfilter::tail_mult`).
    fn tail_mult_of(fp: &FilterParams, s: &ShowInputs) -> f64 {
        F::tail_mult(fp, s.score, s.format.as_deref() == Some("ONA"))
    }

    /// For each market whose expected path ends in the long tail: (its tail, the show's prior, days after the
    /// finale's week on the path's last day), for the value beyond the horizon.
    fn tail_ends<'a>(fp: &'a FilterParams, sched: &[i64], long: bool, today: i64, mult: f64) -> [F::TailEnd<'a>; 3] {
        let fin = if long { None } else { sched.last().copied() };
        std::array::from_fn(|m| match (fin, &fp.tail[m]) {
            (Some(f), Some(tl)) => {
                let t_last = today + fp.horizon_days as i64 - f - 7;
                (t_last >= 0).then_some((tl, mult, t_last))
            }
            _ => None,
        })
    }

    fn sources(s: &ShowInputs) -> [bool; 3] {
        s.sources.as_ref().map_or([true; 3], |x| [true, x.cn, x.jp])
    }

    /// Whether each market reported on the day (older inputs: a positive value).
    fn observed(s: &ShowInputs) -> [bool; 3] {
        let a = s.att.m3();
        s.observed.as_ref().map_or([true, a[1] > 0.0, a[2] > 0.0], |x| [x.us, x.cn, x.jp])
    }

    /// The close's filter step for one show (docs/v2-valuation.md §3.2): before the premiere the prior; then
    /// each new observation (us: an episode week's average; cn/jp: when their weekly value changes), each
    /// moving all three markets through the covariance. A market without data keeps what the others imply.
    pub(crate) fn valuation_step(&mut self, fp: &FilterParams, p: &Params, s: &ShowInputs, day: &str, ts: &str) {
        let Some(today) = day_no(day) else { return };
        let sched = self.air_days_of(s.id);
        let k = F::last_air_before(&sched, today).map(|a| today - a);
        // after the finale's week the level is observed through the post-finale profile (the decline it
        // describes is not news) and no longer drifts
        // (finished: exactly the announced number of episodes aired; a show that keeps airing past its
        // count is not finished)
        let fin = match Self::planned_episodes(s.episodes, &sched, today) {
            Some(n) if !s.long_runner && n > 0 && sched.len() as i64 == n => Some(sched[(n - 1) as usize]),
            _ => None,
        };
        // (the profile is relative to the finale's week, bump included)
        let after_finale = fin.is_some_and(|f| today >= f + 7);
        // the long tail's prior: from the inputs until the finale's week ends, then the state's
        let mult = match self.st.jstate.get(&s.id) {
            Some(j) if after_finale => j.tail_mult,
            _ => Self::tail_mult_of(fp, s),
        };
        let prev_state = self.st.jstate.get(&s.id).cloned();
        let (seq_z, seq_zs) = match (&prev_state, &fp.sequel) {
            (Some(j), _) if after_finale => (j.seq_z, j.seq_zs),
            (_, Some(sp)) => {
                let z = F::sequel_z(sp, s.score, s.source.as_deref(), s.prequel, s.top_pop);
                (z, if s.prequel { z } else { z + sp.beta.sequel })
            }
            _ => (0.0, 0.0),
        };
        // an announced sequel (a new SEQUEL relation not yet aired): a company notice the first time; the prequel's
        // us attention in the announcement week and the two after is read through the measured transient
        let sequel = s.sequel.as_ref().and_then(|q| {
            let a = day_no(&q.announced)?;
            Some((q.id, a, q.premieres.iter().filter_map(|d| day_no(d)).collect::<Vec<i64>>()))
        });
        if let Some((sid, _, pr)) = &sequel {
            let known = prev_state.as_ref().and_then(|j| j.sequel.as_ref()).is_some_and(|x| x.0 == *sid);
            if !known {
                if let Some(c) = self.st.companies.get(&s.id).filter(|c| c.status == "listed").cloned() {
                    let when = match (s.sequel.as_ref().is_some_and(|q| q.exact), pr.first()) {
                        (true, Some(d)) => NaiveDate::from_num_days_from_ce_opt(*d as i32)
                            .map_or(String::new(), |d| format!("续作定于 {} 年 {} 月 {} 日开播。", d.year(), d.month(), d.day())),
                        _ => "开播时间未公布。".into(),
                    };
                    self.push_news(ts, day, "sequel", Some(c.id), format!("《{}》续作公布", c.title), when);
                }
            }
        }
        let post: M3 = std::array::from_fn(|m| {
            fin.map_or(1.0, |f| if today >= f + 7 { F::post_finale_factor(fp, p, m, f, today, mult) * fp.finale_bump[m] } else { 1.0 })
        });
        let in_finale_week = fin.is_some_and(|f| f <= today && today < f + 7);
        let mut att = s.att.m3();
        for (m, (a, f)) in att.iter_mut().zip(post).enumerate() {
            *a /= f;
            if in_finale_week {
                *a /= fp.finale_bump[m];
            }
        }
        if let (Some((_, a, _)), Some(tr)) = (&sequel, fp.sequel.as_ref().and_then(|sp| sp.transient.as_ref())) {
            if *a <= today && today < a + 7 {
                att[0] /= tr.week1;
            } else if a + 7 <= today && today < a + 21 {
                att[0] /= tr.weeks2_3;
            }
        }
        let observed = Self::observed(s);
        let sources = Self::sources(s);
        let premiered = s.aired >= 1;
        let prior = Self::prior_mean(fp, s, today);
        let pcov = Self::prior_cov_at(fp, s, today);
        let since_premiere = day_no(&s.first_air).map_or(0, |f| today - f);
        let fresh = |mean: M3, cov: Cov| {
            let mut j = JState::empty(day);
            j.mean = mean;
            j.cov = cov;
            j.active = sources;
            j.tail_mult = mult;
            j.seq_z = seq_z;
            j.seq_zs = seq_zs;
            j.sequel = sequel.clone();
            j
        };
        let cur = self.st.jstate.get(&s.id).cloned();
        let mut st = match (cur, prior) {
            // before the premiere: the prior, every day (the estimate moves until the premiere)
            (_, Some(pm)) if !premiered => {
                let mut j = fresh(pm, pcov);
                // the us attention it already has before airing (it earns revenue): carried until the premiere
                j.last_obs[0] = (att[0] > 0.0).then_some(att[0]);
                self.save_jstate(s.id, j, ts);
                return;
            }
            (None, Some(pm)) if !(s.has_real_data && since_premiere >= 28) => fresh(pm, pcov),
            (None, _) => {
                // no usable prior (a long-runner, a show that premiered before the data): centred on the first
                // observed market, the others at their usual ratio to it, with the prior's uncertainty
                let Some(m0) = (0..3).find(|&m| sources[m] && observed[m] && att[m] > 0.0) else { return };
                let y = (att[m0] / F::curve(fp, m0, k)).ln();
                // (the markets' usual relation: log level_m = bias_m + beta_m × log us estimate)
                let lp = (y - fp.prior_bias[m0]) / fp.prior_beta[m0].max(1e-6);
                let mean = std::array::from_fn(|m| fp.prior_bias[m] + fp.prior_beta[m] * lp);
                fresh(mean, F::prior_cov(&fp.p0, fp.rho))
            }
            (Some(st), _) => {
                let days = day_no(&st.day).map_or(1, |d| (today - d).max(0));
                // each market's drift over the days since the state, by weeks since the premiere (per day)
                let prem = day_no(&s.first_air).unwrap_or(today);
                let from = day_no(&st.day).unwrap_or(today);
                let drift: M3 = std::array::from_fn(|m| {
                    if after_finale {
                        0.0
                    } else if s.long_runner || days == 0 {
                        fp.drift_for(m, s.long_runner)
                    } else {
                        fp.cum_drift(m, from, today, prem) / days as f64
                    }
                });
                // after the finale's week the level is a random walk whose variance grows by the spread of
                // show-specific tails D(t) (docs/v2-series.md §4); before it, the in-season process noise
                let q: M3 = std::array::from_fn(|m| match (fin, &fp.tail[m]) {
                    (Some(f), Some(tl)) if today >= f + 7 && days > 0 => {
                        let (t0, t1) = (from - f - 7, today - f - 7);
                        let pre = (-t0).clamp(0, days) as f64 * fp.q[m];
                        (pre + F::tail_var(tl, t1) - F::tail_var(tl, t0.max(0))) / days as f64
                    }
                    _ => fp.q[m],
                });
                let (mean, cov) = F::kf_predict_joint(&st.mean, &st.cov, days, &drift, &q);
                JState { mean, cov, day: day.into(), ..st }
            }
        };
        st.day = day.into();
        st.post = post;
        st.tail_mult = mult;
        st.seq_z = seq_z;
        st.seq_zs = seq_zs;
        st.sequel = sequel;
        // the observations of the day, us first
        let mut ys: Vec<(usize, f64)> = Vec::new();
        let drift_us = if after_finale {
            0.0
        } else if s.long_runner {
            fp.drift_for(0, true)
        } else {
            fp.drift_at(0, day_no(&s.first_air).map_or(0, |p| (today - p).div_euclid(7)))
        };
        if st.acc_n > 0 && (k == Some(0) || st.acc_n >= 7) {
            ys.push((0, F::week_observation(st.acc / st.acc_n as f64, st.acc_n, drift_us)));
            st.acc = 0.0;
            st.acc_n = 0;
        }
        if att[0] > 0.0 {
            st.acc += (att[0] / F::curve(fp, 0, k)).ln();
            st.acc_n += 1;
            st.last_obs[0] = Some(att[0]);
            st.seen[0] = true;
        }
        for m in 1..3 {
            if !sources[m] || !observed[m] {
                continue;
            }
            if att[m] > 0.0 {
                if !st.active[m] {
                    // a market that had been ruled out reports after all: back in, at the level the other
                    // markets imply (its mean and covariance kept moving with them)
                    st.active[m] = true;
                }
                if st.last_obs[m] != Some(att[m]) {
                    st.last_obs[m] = Some(att[m]);
                    st.seen[m] = true;
                    ys.push((m, att[m].ln()));
                }
            } else {
                // the source reports zero (not missing): a show four weeks past its premiere that has never had
                // anything there earns nothing there (a week-one zero is just the data's lag)
                st.zero_days[m] += 1;
                if !st.seen[m] && since_premiere >= 28 {
                    st.active[m] = false;
                }
            }
        }
        for (m, y) in ys {
            if st.active[m] {
                let (mean, cov, _, _) = F::kf_update_joint(&st.mean, &st.cov, m, y, fp.r[m], fp.clip);
                st.mean = mean;
                st.cov = cov;
            }
        }
        st.active[0] = true;
        for m in 1..3 {
            if !sources[m] {
                st.active[m] = false;
            } else if !st.active[m] && st.zero_days[m] == 0 {
                // newly mapped to its data source: in, at the level the other markets imply
                st.active[m] = true;
            }
        }
        self.save_jstate(s.id, st, ts);
    }

    /// Expected attention paths of every listed company with a filter state, from `today`.
    fn paths(&self, fp: &FilterParams, p: &Params, today: i64) -> HashMap<i64, [Vec<f64>; 3]> {
        let h = fp.horizon_days;
        let mut out = HashMap::new();
        for c in self.st.companies.values().filter(|c| c.status == "listed" || c.status == "subscribing") {
            let Some(js) = self.st.jstate.get(&c.id) else { continue };
            let (sched, first_air, long) = self.shape(c, p);
            let mut path: [Vec<f64>; 3] = std::array::from_fn(|m| {
                if js.active[m] {
                    F::att_path(fp, p, m, js.mean[m], js.var(m), today, &sched, first_air, long, h, js.tail_mult)
                } else {
                    vec![0.0; h]
                }
            });
            Self::fill_preair(&mut path[0], today, first_air, js.last_obs[0]);
            out.insert(c.id, path);
        }
        out
    }

    /// Today's total attention per market of this season's shows on air (long-runners and earlier seasons'
    /// shows aside): the expected levels of the listed shows that have premiered.
    fn current_total(&self, today: i64) -> [f64; 3] {
        // this season's shows: premiered from 14 days before the start of the season `today` belongs to (the
        // same membership as the "not listed yet" estimate below)
        let season = prev_season_start(today + 14);
        let mut cs: Vec<&Company> = self
            .st
            .companies
            .values()
            .filter(|c| {
                c.status == "listed"
                    && c.long_runner == 0
                    && Self::in_totals(c, today)
                    && day_no(&c.first_air).is_some_and(|f| f >= season - 14)
            })
            .collect();
        cs.sort_unstable_by_key(|c| c.id);
        let mut t = [0.0f64; 3];
        for c in cs {
            if let Some(js) = self.st.jstate.get(&c.id) {
                for (m, tm) in t.iter_mut().enumerate() {
                    if js.active[m] {
                        *tm += (js.mean[m] + 0.5 * js.var(m)).exp() * js.post[m];
                    }
                }
            }
        }
        t
    }

    /// Record today's total (state `valuation_totals`: "day:us,cn,jp" for the last 200 days). Once per close
    /// (and per history day of the bootstrap).
    pub(crate) fn record_total(&mut self, today: i64) {
        let cur = self.current_total(today);
        let mut hist: Vec<(i64, [f64; 3])> = self.totals_history().into_iter().filter(|(d, _)| *d < today && *d > today - 200).collect();
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

    /// The market's total attention per market and future day (docs/v2-valuation.md §3.3): the sum of every
    /// listed (and subscribing) show's own expected path, the same paths the values come from, so the total
    /// fades exactly as the model expects each show to (its finale and post-finale decay included) and a show
    /// not yet airing joins on its first air date; plus the shows not listed yet (what the shows on air drew
    /// at the same point of the previous season, times the share of the season's attention still to come,
    /// fitted from how a season's shows arrive). Each piece
    /// is an expectation that moves only with news, so neither the fading of the shows on air, nor a finale,
    /// a listing, a premiere or the season change is a predictable change of anyone's share.
    fn totals_from(&self, paths: &HashMap<i64, [Vec<f64>; 3]>, fp: &FilterParams, today: i64) -> [Vec<f64>; 3] {
        let h = fp.horizon_days;
        let mut ids: Vec<&i64> = paths.keys().collect();
        ids.sort_unstable();
        let mut sum: [Vec<f64>; 3] = std::array::from_fn(|_| vec![0.0; h]);
        for id in ids {
            for m in 0..3 {
                for (x, v) in sum[m].iter_mut().zip(&paths[id][m]) {
                    *x += v;
                }
            }
        }
        // the shows not listed yet: what the shows of a season drew at the same point of the previous season
        // (the seasonal total recorded then; the peak of the recorded ones when there is no such day), times
        // the share of that season's attention still to come, by how far today is from its start. A future day
        // belongs to the season whose shows are airing then (those premiering from 14 days before its start)
        let hist: HashMap<i64, [f64; 3]> = self.totals_history().into_iter().collect();
        let mut peak = self.current_total(today);
        for t in hist.values() {
            for m in 0..3 {
                peak[m] = peak[m].max(t[m]);
            }
        }
        for t in 0..h {
            let d = today + t as i64 + 1;
            let season = prev_season_start(d + 14);
            let r = fp.unlisted(today - season);
            if r <= 0.0 {
                continue;
            }
            // the same day of the season in the latest earlier season recorded that far (looking back up to four
            // seasons, so the source only moves one recorded day at a time, never a whole season at once)
            let phase = d - season;
            let mut same = None;
            let mut s0 = season;
            for _ in 0..4 {
                s0 = prev_season_start(s0 - 1);
                if let Some(v) = hist.get(&(s0 + phase)) {
                    same = Some(*v);
                    break;
                }
            }
            let level = same.unwrap_or(peak);
            for m in 0..3 {
                sum[m][t] += level[m] * r;
            }
        }
        sum
    }

    /// Before the premiere a show already draws some us attention (list adds, trending), and earns from it:
    /// today's level until the day before the premiere (the path index t is day today + t + 1).
    fn fill_preair(path: &mut [f64], today: i64, first_air: i64, level: Option<f64>) {
        let Some(level) = level else { return };
        for (t, x) in path.iter_mut().enumerate() {
            if today + t as i64 + 1 >= first_air {
                break;
            }
            *x = level;
        }
    }

    fn in_totals(c: &Company, today: i64) -> bool {
        c.long_runner != 0 || day_no(&c.first_air).is_some_and(|d| d <= today)
    }

    fn with_own(totals: &[Vec<f64>; 3], own: &[Vec<f64>; 3]) -> [Vec<f64>; 3] {
        std::array::from_fn(|m| totals[m].iter().zip(&own[m]).map(|(t, o)| t + o).collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn value_of(&self, fp: &FilterParams, c: &Company, own: &[Vec<f64>; 3], totals: &[Vec<f64>; 3], p: &Params, rate: f64, a_ref: f64, today: i64) -> i64 {
        let book = self.account_of("company", c.id).map_or(0, |a| self.st.accounts[&a].cash);
        let (sched, _, long) = self.shape(c, p);
        let mult = self.st.jstate.get(&c.id).map_or(1.0, |j| j.tail_mult);
        let tails = Self::tail_ends(fp, &sched, long, today, mult);
        let extra = self.st.jstate.get(&c.id).and_then(|js| self.sequel_extra(fp, p, js, &sched, long, day_no(&c.first_air).unwrap_or(0), today, rate));
        let v = F::value_from_paths(own, totals, &p.market_weights, a_ref, p, c.shares, book, rate, c.long_runner != 0, &tails, self.platform_rule(), extra.as_ref());
        ((v * 100.0).round_ties_even() as i64).max(1)
    }

    /// The hidden value of one company (cents) with given market totals: after an allotment.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn valuation_value_of(&self, fp: &FilterParams, p: &Params, id: i64, today: i64, totals: &[Vec<f64>; 3], rate: f64, a_ref: f64) -> Option<i64> {
        let c = self.st.companies.get(&id)?;
        let js = self.st.jstate.get(&id)?;
        let (sched, first_air, long) = self.shape(c, p);
        let mut own: [Vec<f64>; 3] = std::array::from_fn(|m| {
            if js.active[m] {
                F::att_path(fp, p, m, js.mean[m], js.var(m), today, &sched, first_air, long, fp.horizon_days, js.tail_mult)
            } else {
                vec![0.0; fp.horizon_days]
            }
        });
        Self::fill_preair(&mut own[0], today, first_air, js.last_obs[0]);
        Some(self.value_of(fp, c, &own, totals, p, rate, a_ref, today))
    }

    /// Hidden values of every listed company with a filter state (cents), and the market totals used.
    pub(crate) fn valuation_values(&self, fp: &FilterParams, p: &Params, today: i64, rate: f64, a_ref: f64) -> (HashMap<i64, i64>, [Vec<f64>; 3]) {
        let paths = self.paths(fp, p, today);
        let totals = self.totals_from(&paths, fp, today);
        if std::env::var("SIM_TRACE_TOTALS").is_ok() {
            // the us total over days 7..90 ahead, by group: shows premiered 30+ days ago, recent/upcoming, long-runners
            let mut g = [0.0f64; 3];
            for (id, pth) in &paths {
                let c = &self.st.companies[id];
                let fa = day_no(&c.first_air).unwrap_or(0);
                let k = if c.long_runner != 0 { 2 } else if today - fa >= 30 { 0 } else { 1 };
                g[k] += pth[0][6..90.min(pth[0].len())].iter().sum::<f64>();
            }
            let tot: f64 = totals[0][6..90.min(totals[0].len())].iter().sum();
            eprintln!("TOTALS {today} old {:.0} new {:.0} long {:.0} unlisted {:.0}", g[0], g[1], g[2], tot - g[0] - g[1] - g[2]);
        }
        // SIM_FORECAST=<file>: append each company's forecast (own attention, market totals, demand) at a few
        // horizons, to compare with what is realised (sim/forecast_check.py)
        let fc = std::env::var("SIM_FORECAST").ok();
        let mut fc_lines = String::new();
        let mut vals = HashMap::new();
        let trace_all = std::env::var("SIM_TRACE").is_ok_and(|v| v == "all");
        let trace: Option<i64> = std::env::var("SIM_TRACE").ok().and_then(|v| v.parse().ok());
        for (id, own) in &paths {
            let c = &self.st.companies[id];
            let v = self.value_of(fp, c, own, &totals, p, rate, a_ref, today);
            if trace_all || trace == Some(*id) {
                let st: Vec<String> = (0..3)
                    .map(|m| match self.st.jstate.get(id) {
                        Some(s) if s.active[m] => format!("{:.3}±{:.3}", s.mean[m], s.var(m).sqrt()),
                        _ => "-".into(),
                    })
                    .collect();
                let own_sum: Vec<f64> = (0..3).map(|m| own[m].iter().take(28).sum::<f64>()).collect();
                let tot_sum: Vec<f64> = (0..3).map(|m| totals[m].iter().take(28).sum::<f64>()).collect();
                let book = self.account_of("company", *id).map_or(0, |a| self.st.accounts[&a].cash);
                let own_all: Vec<f64> = (0..3).map(|m| own[m].iter().sum::<f64>()).collect();
                eprintln!("TRACE {today} {id} value {v} book/share {} states {st:?} own28 {own_sum:.0?} own_all {own_all:.0?} tot28 {tot_sum:.0?}", book / c.shares.max(1));
            }
            if fc.is_some() {
                let hs = [1usize, 7, 14, 28, 56, 84];
                let own_h: Vec<[f64; 3]> = hs.iter().map(|&h| [own[0][h - 1], own[1][h - 1], own[2][h - 1]]).collect();
                let tot_h: Vec<[f64; 3]> = hs.iter().map(|&h| [totals[0][h - 1], totals[1][h - 1], totals[2][h - 1]]).collect();
                let dd_h: Vec<f64> = hs.iter().map(|&h| crate::valuation::demand_day(totals[0][h - 1], a_ref, p)).collect();
                let book = self.account_of("company", *id).map_or(0, |a| self.st.accounts[&a].cash);
                fc_lines.push_str(&serde_json::json!({"d": today, "id": id, "v": v, "book": book, "shares": c.shares,
                    "own": own_h, "tot": tot_h, "dd": dd_h}).to_string());
                fc_lines.push('\n');
            }
            vals.insert(*id, v);
        }
        if let Some(path) = fc {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(fc_lines.as_bytes());
            }
        }
        (vals, totals)
    }

    /// At go-live (no filter state yet, the v2 economy): bootstrap from the daily inputs files on disk, up
    /// to 120 days back to the last close (`$ANISTOCK_INPUTS_DIR` or data/derived/inputs). Returns the number
    /// of companies valued, None when there was nothing to do. Without history files the filter simply
    /// starts at the next close (from each show's pre-air estimate, or its first observation).
    pub fn valuation_go_live(&mut self, cfg: &Config, ts: &str) -> Result<Option<usize>, String> {
        // ANISTOCK_VALUATION_REBUILD=1: replay the history even when there is a state (a model change)
        let rebuild = std::env::var("ANISTOCK_VALUATION_REBUILD").is_ok_and(|v| v == "1");
        if cfg.economy_v1 || (!self.st.jstate.is_empty() && !rebuild) {
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
        // a replay starts from nothing
        let ids: Vec<i64> = self.st.jstate.keys().copied().collect();
        for id in ids {
            self.st.jstate.remove(&id);
            self.out.push(Change::ValuationJointDelete { company_id: id });
        }
        let mut last = None;
        for inp in history {
            let listed: Vec<i64> = self.st.companies.values().filter(|c| c.status == "listed").map(|c| c.id).collect();
            for id in listed {
                if let Some(s) = inp.show(id) {
                    self.valuation_step(&fp, &p, s, &inp.day, ts);
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
        let mut part: Vec<(i64, JState)> = self.st.jstate.iter().filter(|(_, v)| v.acc_n >= 3).map(|(k, v)| (*k, v.clone())).collect();
        part.sort_by_key(|(k, _)| *k);
        for (id, mut st) in part {
            let long = self.st.companies.get(&id).is_some_and(|c| c.long_runner != 0);
            let y = F::week_observation(st.acc / st.acc_n as f64, st.acc_n, fp.drift_for(0, long));
            let (mean, cov, _, _) = F::kf_update_joint(&st.mean, &st.cov, 0, y, fp.r[0], fp.clip);
            st.mean = mean;
            st.cov = cov;
            st.acc = 0.0;
            st.acc_n = 0;
            self.save_jstate(id, st, ts);
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
        let Some(pm) = Self::prior_mean(fp, s, today) else {
            return (p.ipo_price_min * 100.0).round_ties_even() as i64;
        };
        let src = Self::sources(s);
        let long = s.long_runner;
        let episodes = s.episodes.or(if long { None } else { Some(p.default_episodes) });
        let first = day_no(&s.first_air).unwrap_or(today);
        let sched = F::schedule(&[first], episodes, long);
        let pc = Self::prior_cov_at(fp, s, today);
        let mult = Self::tail_mult_of(fp, s);
        let mut own: [Vec<f64>; 3] = std::array::from_fn(|m| {
            if src[m] {
                F::att_path(fp, p, m, pm[m], pc[m][m], today, &sched, first, long, fp.horizon_days, mult)
            } else {
                vec![0.0; fp.horizon_days]
            }
        });
        let us_now = s.att.us;
        Self::fill_preair(&mut own[0], today, first, (us_now > 0.0).then_some(us_now));
        // (the new show is not in today's totals yet: it joins them)
        let tails = Self::tail_ends(fp, &sched, long, today, mult);
        // the sequel option from the prior (the show is not valued yet)
        let extra: Option<M3> = match (&fp.sequel, sched.last()) {
            (Some(sp), Some(&fin)) if !long => {
                let z = F::sequel_z(sp, s.score, s.source.as_deref(), s.prequel, s.top_pop);
                let zs = if s.prequel { z } else { z + sp.beta.sequel };
                let disc = F::discount_day(rate, p);
                Some(std::array::from_fn(|m| if src[m] { F::sequel_extra(fp, m, pm[m], pc[m][m], today, fin, first, z, zs, disc, &[]) } else { 0.0 }))
            }
            _ => None,
        };
        let v = F::value_from_paths(&own, &Self::with_own(totals, &own), &p.market_weights, a_ref, p, shares, 0, rate, long, &tails, self.platform_rule(), extra.as_ref());
        let used: Vec<f64> = (0..3).filter(|m| src[*m]).map(|m| fp.p0[m]).collect();
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
        let Some(st) = self.st.jstate.get(&company).cloned().filter(|s| s.active[1]) else { return Ok(None) };
        let rate: f64 = self.state("rate").and_then(crate::pyfmt::parse_float).unwrap_or(0.03);
        let a_ref: f64 = self.state("a_ref").and_then(crate::pyfmt::parse_float).unwrap_or(1.0);
        let today = day_no(&st.day).unwrap_or(0);
        // observation of this episode's cn level: from the state before the episode's first flash, moved by
        // the surprise; every market moves through the covariance
        #[derive(Serialize, Deserialize)]
        struct Base {
            episode: f64,
            mean: M3,
            cov: Cov,
            last: M3,
        }
        let key = format!("flash:{company}");
        let base: Option<Base> =
            self.state(&key).and_then(|v| serde_json::from_str::<Base>(v).ok()).filter(|b| b.episode == episode);
        let first = base.is_none();
        let b = base.unwrap_or(Base { episode, mean: st.mean, cov: st.cov, last: st.mean });
        let (post, post_cov, _, _) = F::kf_update_joint(&b.mean, &b.cov, 1, b.mean[1] + y_rel, r, fp.clip);
        let mean: M3 = std::array::from_fn(|m| st.mean[m] + (post[m] - b.last[m]));
        // the uncertainty shrinks once per episode (a later hour refines the same observation)
        let cov = if first { post_cov } else { st.cov };
        let nb = Base { episode, mean: b.mean, cov: b.cov, last: post };
        self.set_state(&key, &serde_json::to_string(&nb).unwrap_or_default());
        let old = c.fair_value;
        self.save_jstate(company, JState { mean, cov, ..st.clone() }, ts);
        let paths = self.paths(&fp, &p, today);
        let totals = self.totals_from(&paths, &fp, today);
        let new = match paths.get(&company) {
            Some(own) => self.value_of(&fp, &c, own, &totals, &p, rate, a_ref, today),
            None => old,
        };
        self.st.companies.get_mut(&company).unwrap().fair_value = new;
        self.mark_company(company);
        self.commit();
        Ok(Some(FlashOutcome { surprise: y_rel.exp() - 1.0, old_value: old, new_value: new }))
    }
}
