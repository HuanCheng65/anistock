//! The filtered valuation (docs/v2-valuation.md): each show's level per market tracked with a Kalman filter
//! (attention = level × within-episode curve), the expected attention path → expected dividends → present
//! value + the company's cash. Same formulas, same operation order as engine/valuation.py (the "filtered
//! valuation" section), so the game server, the simulation and the fitting script agree.
//!
//! Parameters are fitted by sim/fit_valuation.py (maximum likelihood on the last season) and read from
//! sim/calibration/valuation_filter.json.

use serde::Deserialize;

use crate::config::Config;
use crate::valuation::{blended_share, demand_day, Params, M3};

pub const MARKETS: [&str; 3] = ["us", "cn", "jp"];

#[derive(Debug, Clone, Deserialize)]
struct PerMarket {
    us: f64,
    cn: f64,
    jp: f64,
}

impl PerMarket {
    fn m3(&self) -> M3 {
        [self.us, self.cn, self.jp]
    }
}

#[derive(Debug, Clone, Deserialize)]
struct PerMarketVec {
    us: Vec<f64>,
    cn: Vec<f64>,
    jp: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonListing {
    pub from_day: i64,
    pub u: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawParams {
    curve_us: Vec<f64>,
    drift: PerMarket,
    q: PerMarket,
    r: PerMarket,
    p0: PerMarket,
    prior_bias: PerMarket,
    outlier_p: PerMarket,
    outlier_r: PerMarket,
    #[serde(default)]
    flash_curve: Vec<f64>,
    #[serde(default)]
    flash_r: Vec<f64>,
    #[serde(default)]
    clip: Option<f64>,
    #[serde(default)]
    horizon_days: Option<usize>,
    #[serde(default)]
    flash_min_expected: Option<f64>,
    #[serde(default)]
    ipo_risk: Option<f64>,
    /// v0.3.2: the common correlation of the three markets' prior log levels (0: independent markets)
    #[serde(default)]
    rho: Option<f64>,
    /// v0.3.2: "us" = cn/jp priors derive from the us pre-air estimate (prior_bias per market converts it)
    #[serde(default)]
    prior: Option<String>,
    /// v0.3.2: mean and variance of the pre-air estimate's log growth from k days before its freeze (index k)
    #[serde(default)]
    preair_growth: Vec<[f64; 2]>,
    /// v0.3.2: the share of a season's attention not yet listed, by days from the season start
    #[serde(default)]
    season_listing: Option<SeasonListing>,
    /// v0.3.2: attention by week after the finale week, and its weekly tail, per market
    /// v0.3.2: elasticity of each market's prior to the us pre-air estimate (us 1)
    #[serde(default)]
    prior_beta: Option<PerMarket>,
    /// v0.3.2: daily drift per market for weeks 0-3, 4-7, 8+ since the premiere
    #[serde(default)]
    drift_by_week: Option<PerMarketVec>,
    /// v0.3.2: attention in the finale's week relative to the level, per market
    #[serde(default)]
    finale_bump: Option<PerMarket>,
    /// phase 1: the spread across shows (log variance) of the finale week's bump: extra noise of a finale-week
    /// observation
    #[serde(default)]
    finale_bump_var: Option<PerMarket>,
    #[serde(default)]
    post_finale: Option<PerMarketVec>,
    #[serde(default)]
    post_finale_tail: Option<PerMarket>,
    /// phase 1: the long tail after the finale, per market, and the tail-level prior (docs/v2-series.md §4)
    #[serde(default)]
    tail: Option<RawTail>,
    /// the us episode week day by day: mean and variance of (first n days' average − the week's), n = 1..7
    #[serde(default)]
    partial_us: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawTail {
    us: Option<Tail>,
    cn: Option<Tail>,
    jp: Option<Tail>,
    prior: Option<TailPrior>,
}

/// One market's long tail (Python `tail_of`): the expected post-finale factor t days after the finale's week is
/// `trans[t / 7]` for the transition weeks, then floor × mult × yearly^(t/365) + excess × exp(-t/tau); the spread
/// across shows D(t) = var[0] (1 - e^(-t/var[1])) + var[2] (1 - e^(-t/var[3])).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Tail {
    pub trans: Vec<f64>,
    pub floor: f64,
    pub yearly: f64,
    pub excess: f64,
    pub tau: f64,
    pub excess2: f64,
    pub tau2: f64,
    pub var: [f64; 3],
}

/// The tail level's prior by score and format (Python `tail_mult`).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TailPrior {
    pub score: f64,
    pub ona: f64,
    pub center: f64,
    pub score_ref: f64,
    #[serde(default)]
    pub pop_bottom: f64,
    #[serde(default)]
    pub pop_top: f64,
    /// the reference year's popularity at the finale, sorted
    #[serde(default)]
    pub pop_ref: Vec<f64>,
    /// the effect of each popularity fifth (empty: the bottom/top flags above)
    #[serde(default)]
    pub pop_fifth: Vec<f64>,
}

#[derive(Debug, Clone)]
pub struct FilterParams {
    pub curve_us: [f64; 7],
    pub drift: M3,
    pub q: M3,
    pub r: M3,
    pub p0: M3,
    pub prior_bias: M3,
    pub outlier_p: M3,
    pub outlier_r: M3,
    pub flash_curve: Vec<f64>,
    pub flash_r: Vec<f64>,
    pub clip: f64,
    pub horizon_days: usize,
    pub flash_min_expected: f64,
    pub ipo_risk: f64,
    /// common correlation of the markets' prior log levels
    pub rho: f64,
    /// cn/jp priors from the us pre-air estimate (v0.3.2 fit), else from each market's own estimate
    pub prior_from_us: bool,
    /// the pre-air estimate's expected log growth (and its variance) until it freezes, by days left
    pub preair_growth: Vec<[f64; 2]>,
    /// the share of a season's attention not yet listed, by days from the season start
    pub season_listing: Option<SeasonListing>,
    /// the elasticity of each market's prior log level to the log us pre-air estimate
    pub prior_beta: M3,
    /// per market: daily drift in weeks 0-3, 4-7 and 8+ since the premiere (empty: `drift` throughout)
    pub drift_by_week: [Vec<f64>; 3],
    /// per market: attention in the finale's week relative to the level (1: none)
    pub finale_bump: M3,
    /// per market: the finale bump's log variance across shows (0: exact)
    pub finale_bump_var: M3,
    /// per market: attention by week after the finale week and its weekly tail (empty: the game's profile)
    pub post_finale: [Vec<f64>; 3],
    pub post_finale_tail: M3,
    /// per market: the long tail (None: the weekly profile above)
    pub tail: [Option<Tail>; 3],
    pub tail_prior: Option<TailPrior>,
    /// the sequel option (sim/calibration/sequel.json next to the filter's file; None without it)
    pub sequel: Option<SequelParams>,
    /// long-runners' weekly level factor (`[valuation] long_runner_decay`; their filter's drift)
    pub long_runner_decay: f64,
    /// the us episode week day by day (`partial_observation`): per n = 1..7 days, the mean and variance of the
    /// first n days' average log attention less the full week's (empty: the week is observed only when complete)
    pub partial_us: Vec<[f64; 2]>,
}

impl FilterParams {
    pub fn load(cfg: &Config) -> Result<FilterParams, String> {
        // ANISTOCK_FILTER_FILE overrides the configured file (calibration experiments)
        let rel = std::env::var("ANISTOCK_FILTER_FILE").ok().map(toml::Value::String);
        let rel = rel
            .as_ref()
            .or(cfg.param("valuation.filter_file"))
            .and_then(|v| v.as_str())
            .unwrap_or("sim/calibration/valuation_filter.json")
            .to_string();
        let path = cfg.root.join(&rel);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut fp = FilterParams::parse(&text)?;
        // the sequel option (docs/v2-series.md §5): off unless `valuation.sequel_option` = 1 (it needs the AniList
        // relations the collector has only just started to keep)
        let sq = path.with_file_name("sequel.json");
        let sequel_on = cfg.param("valuation.sequel_option").and_then(|v| v.as_integer()).unwrap_or(0) == 1;
        if sequel_on && sq.exists() {
            let t = std::fs::read_to_string(&sq).map_err(|e| format!("{}: {e}", sq.display()))?;
            fp.sequel = Some(serde_json::from_str(&t).map_err(|e| format!("{}: {e}", sq.display()))?);
        }
        if let Some(d) = cfg.param("valuation.long_runner_decay").and_then(|v| v.as_float()) {
            fp.long_runner_decay = d;
        }
        Ok(fp)
    }

    /// Expected log growth and its variance of the pre-air estimate over the `k` days until it freezes (the
    /// day before the premiere); the last entry beyond the table.
    pub fn growth(&self, k: i64) -> (f64, f64) {
        if k <= 0 || self.preair_growth.is_empty() {
            return (0.0, 0.0);
        }
        let g = self.preair_growth[(k as usize).min(self.preair_growth.len() - 1)];
        (g[0], g[1])
    }

    /// The share of a season's attention not listed yet, `j` days after its start (negative: before). Without
    /// the table: everything is listed by the season start.
    pub fn unlisted(&self, j: i64) -> f64 {
        match &self.season_listing {
            Some(sl) if !sl.u.is_empty() => {
                let i = j - sl.from_day;
                if i < 0 {
                    1.0
                } else {
                    sl.u[(i as usize).min(sl.u.len() - 1)]
                }
            }
            _ => {
                if j < 0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    /// The filter's daily drift for market `m`: the fitted one, or for a long-runner the one that matches
    /// its valuation path (level × long_runner_decay per week), so its value doesn't fall all week and jump
    /// back at each observation.
    /// Market `m`'s daily drift in week `w` since the premiere (Python `drift_at`).
    pub fn drift_at(&self, m: usize, w: i64) -> f64 {
        let seg = &self.drift_by_week[m];
        if seg.len() < 3 {
            return self.drift[m];
        }
        if w < DRIFT_WEEKS[0] {
            seg[0]
        } else if w < DRIFT_WEEKS[1] {
            seg[1]
        } else {
            seg[2]
        }
    }

    /// The total log drift from day `t0` to `t1` for a show premiering on `premiere` (Python `cum_drift`).
    pub fn cum_drift(&self, m: usize, t0: i64, t1: i64, premiere: i64) -> f64 {
        (t0..t1).map(|t| self.drift_at(m, (t - premiere).div_euclid(7))).sum()
    }

    pub fn drift_for(&self, m: usize, long_runner: bool) -> f64 {
        if long_runner { self.long_runner_decay.ln() / 7.0 - 0.5 * self.q[m] } else { self.drift[m] }
    }

    pub fn parse(text: &str) -> Result<FilterParams, String> {
        let r: RawParams = serde_json::from_str(text).map_err(|e| format!("valuation filter: {e}"))?;
        if r.curve_us.len() != 7 {
            return Err("valuation filter: curve_us needs 7 values".into());
        }
        let mut c = [0.0; 7];
        c.copy_from_slice(&r.curve_us);
        Ok(FilterParams {
            curve_us: c,
            drift: r.drift.m3(),
            q: r.q.m3(),
            r: r.r.m3(),
            p0: r.p0.m3(),
            prior_bias: r.prior_bias.m3(),
            outlier_p: r.outlier_p.m3(),
            outlier_r: r.outlier_r.m3(),
            flash_curve: r.flash_curve,
            flash_r: r.flash_r,
            clip: r.clip.unwrap_or(4.0),
            horizon_days: r.horizon_days.unwrap_or(400),
            flash_min_expected: r.flash_min_expected.unwrap_or(5.0),
            ipo_risk: r.ipo_risk.unwrap_or(0.25),
            long_runner_decay: 1.0,
            rho: r.rho.unwrap_or(0.0),
            prior_from_us: r.prior.as_deref() == Some("us"),
            preair_growth: r.preair_growth,
            season_listing: r.season_listing,
            prior_beta: r.prior_beta.map_or([1.0; 3], |b| b.m3()),
            finale_bump: r.finale_bump.map_or([1.0; 3], |b| b.m3()),
            finale_bump_var: r.finale_bump_var.map_or([0.0; 3], |b| b.m3()),
            drift_by_week: r.drift_by_week.map_or([Vec::new(), Vec::new(), Vec::new()], |v| [v.us, v.cn, v.jp]),
            post_finale: r.post_finale.map_or([Vec::new(), Vec::new(), Vec::new()], |v| [v.us, v.cn, v.jp]),
            post_finale_tail: r.post_finale_tail.map_or([0.0; 3], |t| t.m3()),
            tail_prior: r.tail.as_ref().and_then(|t| t.prior.clone()),
            tail: r.tail.map_or([None, None, None], |t| [t.us, t.cn, t.jp]),
            sequel: None,
            partial_us: r.partial_us,
        })
    }
}

/// Within-episode curve (Python `curve`): expected attention on day k after an episode, relative to the week.
pub fn curve(fp: &FilterParams, m: usize, k: Option<i64>) -> f64 {
    match k {
        Some(k) if m == 0 && (0..7).contains(&k) => fp.curve_us[k as usize],
        _ => 1.0,
    }
}

pub fn kf_predict(mean: f64, var: f64, days: i64, drift: f64, q: f64) -> (f64, f64) {
    (mean + drift * days as f64, var + q * days as f64)
}

/// Kalman update with outlier-prone observations (Python `robust_update`): (mean, var, log-likelihood).
#[allow(clippy::too_many_arguments)]
pub fn robust_update(mean: f64, var: f64, y: f64, r: f64, pi: f64, r_out: f64, clip: f64) -> (f64, f64, f64) {
    let v = y - mean;
    let s1 = var + r;
    let s2 = s1 + r_out;
    let tau = std::f64::consts::PI * 2.0;
    let l1 = (1.0 - pi) * (-0.5 * v * v / s1).exp() / (tau * s1).sqrt();
    let l2 = pi * (-0.5 * v * v / s2).exp() / (tau * s2).sqrt();
    let tot = l1 + l2;
    let w_out = if tot > 0.0 { l2 / tot } else { 1.0 };
    let r_eff = r + w_out * r_out;
    let (m2, v2, _) = kf_update(mean, var, y, r_eff, clip);
    (m2, v2, if tot > 0.0 { tot.ln() } else { -1e9 })
}

/// (mean, var, clipped innovation)
pub fn kf_update(mean: f64, var: f64, y: f64, r: f64, clip: f64) -> (f64, f64, f64) {
    let s = var + r;
    let sd = s.sqrt();
    let mut v = y - mean;
    if v > clip * sd {
        v = clip * sd;
    } else if v < -clip * sd {
        v = -clip * sd;
    }
    let k = var / s;
    (mean + k * v, (1.0 - k) * var, v)
}

/// The in-season fade changes speed after these weeks since the premiere (Python `DRIFT_WEEKS`).
pub const DRIFT_WEEKS: [i64; 2] = [4, 8];

/// The joint filter's 3×3 covariance (us, cn, jp).
pub type Cov = [[f64; 3]; 3];

/// Prior covariance of the three markets' log levels: variances p0, one common correlation rho (Python
/// `prior_cov`).
pub fn prior_cov(p0: &M3, rho: f64) -> Cov {
    let sd = [p0[0].sqrt(), p0[1].sqrt(), p0[2].sqrt()];
    std::array::from_fn(|i| std::array::from_fn(|j| if i == j { p0[i] } else { rho * sd[i] * sd[j] }))
}

/// Each market's level drifts on its own (Python `kf_predict_joint`).
pub fn kf_predict_joint(mean: &M3, cov: &Cov, days: i64, drift: &M3, q: &M3) -> (M3, Cov) {
    let m2 = std::array::from_fn(|i| mean[i] + drift[i] * days as f64);
    let c2 = std::array::from_fn(|i| std::array::from_fn(|j| cov[i][j] + if i == j { q[i] * days as f64 } else { 0.0 }));
    (m2, c2)
}

/// One observation y of market m's log level, noise variance r; moves every market through the covariance
/// (Python `kf_update_joint`). Returns (mean, cov, log-likelihood, innovation variance).
pub fn kf_update_joint(mean: &M3, cov: &Cov, m: usize, y: f64, r: f64, clip: f64) -> (M3, Cov, f64, f64) {
    let s = cov[m][m] + r;
    let sd = s.sqrt();
    let mut v = y - mean[m];
    let ll = -0.5 * ((2.0 * std::f64::consts::PI * s).ln() + v * v / s);
    if v > clip * sd {
        v = clip * sd;
    } else if v < -clip * sd {
        v = -clip * sd;
    }
    let k: M3 = std::array::from_fn(|i| cov[i][m] / s);
    let m2 = std::array::from_fn(|i| mean[i] + k[i] * v);
    let c2 = std::array::from_fn(|i| std::array::from_fn(|j| cov[i][j] - k[i] * cov[m][j]));
    (m2, c2, ll, s)
}

/// An observation that averages `n` daily log values describes the level about (n-1)/2 days ago: moved to
/// today with the drift (Python `week_observation`).
pub fn week_observation(y_mean: f64, n: i64, drift: f64) -> f64 {
    y_mean + drift * (n - 1) as f64 / 2.0
}

/// The first `n` days of a us episode week as an observation of the level today (Python `partial_observation`):
/// their average log attention, less the fitted mean of (n days' average − the week's), moved to today with the
/// drift; and its extra noise variance beyond the full week's `r`. None without the fit (or n < 1).
pub fn partial_observation(fp: &FilterParams, y_mean: f64, n: i64, drift: f64) -> Option<(f64, f64)> {
    if n < 1 || fp.partial_us.is_empty() {
        return None;
    }
    let [mu, var] = fp.partial_us[(n as usize).min(fp.partial_us.len()) - 1];
    Some((week_observation(y_mean - mu, n, drift), var))
}

/// Every episode's air day (day numbers): the known ones, then weekly until `episodes`.
pub fn schedule(air_days: &[i64], episodes: Option<i64>, long_runner: bool) -> Vec<i64> {
    let mut out = air_days.to_vec();
    out.sort_unstable();
    if long_runner || out.is_empty() {
        return out;
    }
    if let Some(n) = episodes {
        while (out.len() as i64) < n {
            let next = *out.last().unwrap() + 7;
            out.push(next);
        }
    }
    out
}

pub fn last_air_before(sched: &[i64], t: i64) -> Option<i64> {
    let mut best = None;
    for &a in sched {
        if a <= t {
            best = Some(a);
        } else {
            break;
        }
    }
    best
}

/// Expected attention in market `m` for days today+1 ..= today+horizon (Python `att_path`).
#[allow(clippy::too_many_arguments)]
pub fn att_path(
    fp: &FilterParams,
    p: &Params,
    m: usize,
    mean: f64,
    var: f64,
    today: i64,
    sched: &[i64],
    first_air: i64,
    long_runner: bool,
    horizon: usize,
    mult: f64,
) -> Vec<f64> {
    let q = fp.q[m];
    let (prof, tail) = post_finale_curve(fp, p, m);
    let fin = if !sched.is_empty() && !long_runner { Some(*sched.last().unwrap()) } else { None };
    // the level drifts from today, or from the premiere for a show not yet airing (Python `att_path`)
    let base = today.max(first_air);
    let mut cd = 0.0; // cumulative drift from base to t
    let mut out = Vec::with_capacity(horizon);
    for j in 1..=horizon as i64 {
        let t = today + j;
        if t < first_air {
            out.push(0.0);
            continue;
        }
        if long_runner {
            out.push((mean + 0.5 * var).exp() * p.long_runner_decay.powf(j as f64 / 7.0));
            continue;
        }
        if let Some(fin) = fin {
            if t >= fin + 7 {
                let jf = (fin + 7 - base).max(0);
                let lvl = (mean + fp.cum_drift(m, base, base + jf, first_air) + 0.5 * (var + q * jf as f64)).exp() * fp.finale_bump[m];
                if let Some(tl) = &fp.tail[m] {
                    out.push(lvl * tail_mean(tl, t - fin - 7, mult) * (-0.5 * tail_var(tl, today - fin - 7)).exp());
                    continue;
                }
                let w = (t - fin - 7).div_euclid(7);
                let f = if (w as usize) < prof.len() {
                    prof[w as usize]
                } else {
                    prof[prof.len() - 1] * tail.powf((w - prof.len() as i64 + 1) as f64)
                };
                out.push(lvl * f);
                continue;
            }
        }
        let k = last_air_before(sched, t).map(|a| t - a);
        let je = (t - base) as f64;
        if t - 1 >= base {
            cd += fp.drift_at(m, (t - 1 - first_air).div_euclid(7));
        }
        let bump = match fin {
            Some(f) if f <= t && t < f + 7 => fp.finale_bump[m],
            _ => 1.0,
        };
        out.push((mean + cd + 0.5 * (var + q * je)).exp() * curve(fp, m, k) * bump);
    }
    out
}

pub fn discount_day(rate: f64, p: &Params) -> f64 {
    let rw = (rate + p.equity_premium) / 52.0;
    (1.0 / (1.0 + rw)).powf(1.0 / 7.0)
}

/// Market `m`'s attention by week after the finale week and its weekly tail (Python `post_finale_curve`):
/// fitted per market, else the game's profile.
pub fn post_finale_curve<'a>(fp: &'a FilterParams, p: &'a Params, m: usize) -> (&'a [f64], f64) {
    if !fp.post_finale[m].is_empty() {
        (&fp.post_finale[m], fp.post_finale_tail[m])
    } else {
        (&p.post_finale_profile, p.post_finale_tail_decay)
    }
}

/// The post-finale factor of market `m`'s level on day `t` for a show whose last episode aired on `fin`
/// (Python `post_finale_factor`): 1 until a week after it, then the profile and its tail.
pub fn post_finale_factor(fp: &FilterParams, p: &Params, m: usize, fin: i64, t: i64, mult: f64) -> f64 {
    if t < fin + 7 {
        return 1.0;
    }
    if let Some(tl) = &fp.tail[m] {
        return tail_mean(tl, t - fin - 7, mult) * (-0.5 * tail_var(tl, t - fin - 7)).exp();
    }
    let (prof, tail) = post_finale_curve(fp, p, m);
    let w = (t - fin - 7).div_euclid(7);
    if (w as usize) < prof.len() {
        prof[w as usize]
    } else {
        prof[prof.len() - 1] * tail.powf((w - prof.len() as i64 + 1) as f64)
    }
}

/// The three parts of the expected post-finale factor beyond the transition weeks (Python `tail_parts`): floor ×
/// yearly^(t/365) and slow excess × exp(-t/tau2), both × the show's prior; fast excess × exp(-t/tau).
pub fn tail_parts(tl: &Tail, t: f64, mult: f64) -> (f64, f64, f64) {
    (mult * tl.floor * tl.yearly.powf(t / 365.0), mult * tl.excess2 * (-t / tl.tau2).exp(), mult * tl.excess * (-t / tl.tau).exp())
}

/// The expected post-finale factor beyond the transition weeks (Python `tail_formula`).
pub fn tail_formula(tl: &Tail, t: f64, mult: f64) -> f64 {
    let (a, b, c) = tail_parts(tl, t, mult);
    a + b + c
}

/// The expected post-finale factor `t` days after the finale's week (Python `tail_mean`).
pub fn tail_mean(tl: &Tail, t: i64, mult: f64) -> f64 {
    let w = t.div_euclid(7);
    if w >= 0 && (w as usize) < tl.trans.len() {
        tl.trans[w as usize]
    } else {
        tail_formula(tl, t as f64, mult)
    }
}

/// D(t), the spread across shows of the log post-finale factor (Python `tail_var`).
pub fn tail_var(tl: &Tail, t: i64) -> f64 {
    if t <= 0 {
        return 0.0;
    }
    let v = &tl.var;
    let t = t as f64;
    v[0] * (1.0 - (-t / v[1]).exp()) + v[2] * t
}

/// The show's prior tail level relative to the average (Python `tail_mult`).
pub fn tail_mult(fp: &FilterParams, score: Option<f64>, ona: bool, popularity: Option<f64>) -> f64 {
    let Some(pr) = &fp.tail_prior else { return 1.0 };
    let s = match score {
        Some(x) if x != 0.0 => (x - pr.score_ref) / 10.0,
        _ => 0.0,
    };
    let mut x = pr.score * s + if ona { pr.ona } else { 0.0 };
    if let (Some(pop), false) = (popularity, pr.pop_ref.is_empty()) {
        // the percentile among the reference year's finales (Python `pop_percentile`)
        let q = pr.pop_ref.iter().filter(|v| **v < pop).count() as f64 / pr.pop_ref.len() as f64;
        if pr.pop_fifth.is_empty() {
            x = x + if q < 0.2 { pr.pop_bottom } else { 0.0 } + if q >= 0.9 { pr.pop_top } else { 0.0 };
        } else {
            x = x + pr.pop_fifth[((q * 5.0) as usize).min(4)];
        }
    }
    (x - pr.center).exp()
}

/// Σ_{k>=1} disc^k × attention(t_last + k) of a path in the long tail (Python `tail_beyond`).
pub fn tail_beyond(tl: &Tail, mult: f64, t_last: i64, last: f64, disc: f64) -> f64 {
    if t_last < 7 * tl.trans.len() as i64 {
        return 0.0;
    }
    let (a, b, c) = tail_parts(tl, t_last as f64, mult);
    if a + b + c <= 0.0 {
        return 0.0;
    }
    let k = last / (a + b + c);
    let g1 = tl.yearly.powf(1.0 / 365.0) * disc;
    let g2 = (-1.0 / tl.tau2).exp() * disc;
    let g3 = (-1.0 / tl.tau).exp() * disc;
    k * (a * g1 / (1.0 - g1) + b * g2 / (1.0 - g2) + c * g3 / (1.0 - g3))
}

// ---- the sequel option (docs/v2-series.md §5; Python `sequel_*`; fitted by sim/fit_sequel.py)

/// A sequel season: 12 weekly episodes (Python `SEASON_DAYS`).
pub const SEASON_DAYS: i64 = 84;
const SEQ_HORIZON: i64 = 3650;
const SEQ_STEP: i64 = 30;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SequelBeta {
    pub score: f64,
    pub light_novel: f64,
    pub original: f64,
    pub sequel: f64,
    pub top_pop: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GapRatio {
    pub until_days: f64,
    pub ratio: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SequelParams {
    pub alpha: Vec<f64>,
    pub beta: SequelBeta,
    pub period_days: Vec<i64>,
    pub lag_days: Vec<i64>,
    pub gap_ratio: Vec<GapRatio>,
    pub season_to_finale: PerMarketPub,
    /// the announcement's transient on the prequel's us attention: the announcement week, the two weeks after
    #[serde(default)]
    pub transient: Option<Transient>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Transient {
    pub week1: f64,
    pub weeks2_3: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct PerMarketPub {
    pub us: f64,
    pub cn: f64,
    pub jp: f64,
}

impl PerMarketPub {
    pub fn m3(&self) -> M3 {
        [self.us, self.cn, self.jp]
    }
}

fn sig(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

/// The show's log-odds shift of the announcement hazard (Python `sequel_z`).
pub fn sequel_z(sp: &SequelParams, score: Option<f64>, source: Option<&str>, prequel: bool, top_pop: bool) -> f64 {
    let b = &sp.beta;
    let s = match score {
        Some(x) if x != 0.0 => (x - 70.0) / 10.0,
        _ => 0.0,
    };
    b.score * s
        + if source == Some("LIGHT_NOVEL") { b.light_novel } else { 0.0 }
        + if source == Some("ORIGINAL") { b.original } else { 0.0 }
        + if prequel { b.sequel } else { 0.0 }
        + if top_pop { b.top_pop } else { 0.0 }
}

/// Python `gap_ratio`.
pub fn gap_ratio(sp: &SequelParams, gap: f64) -> f64 {
    for b in &sp.gap_ratio {
        if gap < b.until_days {
            return b.ratio;
        }
    }
    sp.gap_ratio.last().map_or(0.0, |b| b.ratio)
}

/// Python `announce_schedule`.
pub fn announce_schedule(sp: &SequelParams, z: f64, t_now: i64) -> Vec<(i64, f64)> {
    let (a, per) = (&sp.alpha, &sp.period_days);
    let mut out = Vec::new();
    let mut surv = 1.0;
    let mut start = t_now;
    if t_now < per[0] {
        let p0 = sig(a[0] + z);
        out.push((t_now.max(0), p0));
        surv = 1.0 - p0;
        start = per[0];
    }
    let mut s = start;
    while s < SEQ_HORIZON {
        let k = if s < per[1] {
            1
        } else if s < per[2] {
            2
        } else if s < per[3] {
            3
        } else {
            4
        };
        let lam = -(1.0 - sig(a[k] + z)).ln() / 365.0;
        let q = 1.0 - (-lam * SEQ_STEP as f64).exp();
        out.push((s + SEQ_STEP / 2, surv * q));
        surv *= 1.0 - q;
        s += SEQ_STEP;
    }
    out
}

/// P(a sequel is announced within `years` of today), `t_now` days after the finale, none announced so far.
pub fn sequel_within(sp: &SequelParams, z: f64, t_now: i64, years: f64) -> f64 {
    let end = t_now as f64 + 365.0 * years;
    announce_schedule(sp, z, t_now).iter().filter(|(t, _)| (*t as f64) <= end).map(|(_, p)| p).sum()
}

/// Python `season_value`.
pub fn season_value(fp: &FilterParams, m: usize, disc: f64) -> f64 {
    let Some(sp) = &fp.sequel else { return 0.0 };
    let mut w = 0.0;
    let mut f = 1.0;
    for _ in 0..SEASON_DAYS {
        w += f;
        f *= disc;
    }
    let Some(tl) = &fp.tail[m] else { return w };
    let c = sp.season_to_finale.m3()[m];
    let mut last = 0.0;
    for t in 0..400 {
        last = c * tail_mean(tl, t, 1.0);
        w += f * last;
        f *= disc;
    }
    w + f / disc * tail_beyond(tl, 1.0, 399, last, disc)
}

/// Market `m`'s expected attention in the finale's week (Python `finale_level`, as in `att_path`).
pub fn finale_level(fp: &FilterParams, m: usize, mean: f64, var: f64, today: i64, fin: i64, first_air: i64) -> f64 {
    let base = today.max(first_air);
    let jf = (fin + 7 - base).max(0);
    (mean + fp.cum_drift(m, base, base + jf, first_air) + 0.5 * (var + fp.q[m] * jf as f64)).exp() * fp.finale_bump[m]
}

/// The sequel option of market `m` in attention-days discounted to today (Python `sequel_extra`).
#[allow(clippy::too_many_arguments)]
pub fn sequel_extra(fp: &FilterParams, m: usize, mean: f64, var: f64, today: i64, fin: i64, first_air: i64, z: f64, zs: f64, disc: f64, known: &[i64], watched: bool) -> f64 {
    let Some(sp) = &fp.sequel else { return 0.0 };
    let lvl = finale_level(fp, m, mean, var, today, fin, first_air) / sp.season_to_finale.m3()[m];
    let k: Vec<i64> = known.iter().map(|d| d - fin).collect();
    lvl * sequel_multiple(fp, m, z, zs, today - fin, disc, &k, watched)
}

fn option_factor(sp: &SequelParams, z: f64, t_now: i64, disc: f64, t_from: i64) -> f64 {
    let lags = &sp.lag_days;
    let mut acc = 0.0;
    for (ta, p) in announce_schedule(sp, z, t_from) {
        let mut s = 0.0;
        for lag in lags {
            let d = ta + lag;
            s += disc.powf((d - t_now).max(0) as f64) * gap_ratio(sp, d as f64);
        }
        acc += p * s / lags.len() as f64;
    }
    acc
}

/// Python `sequel_multiple`: the sequel option in attention-days per unit of in-season mean attention, today.
pub fn sequel_multiple(fp: &FilterParams, m: usize, z: f64, zs: f64, t_now: i64, disc: f64, known: &[i64], watched: bool) -> f64 {
    let Some(sp) = &fp.sequel else { return 0.0 };
    let w = season_value(fp, m, disc);
    let a_s = option_factor(sp, zs, 0, disc, 0) * disc.powf(SEASON_DAYS as f64);
    let own = if a_s < 1.0 { a_s * w / (1.0 - a_s) } else { 0.0 };
    if !known.is_empty() {
        let mut acc = 0.0;
        for d in known {
            acc += disc.powf((d - t_now).max(0) as f64) * gap_ratio(sp, *d as f64);
        }
        return acc / known.len() as f64 * (w + own);
    }
    option_factor(sp, z, t_now, disc, if watched { t_now } else { t_now.min(0) }) * (w + own)
}

/// A path that ends in the long tail: its market's tail, the show's prior, days after the finale's week on the
/// path's last day (Python `value_from_paths` `tails`).
pub type TailEnd<'a> = Option<(&'a Tail, f64, i64)>;

/// Absolute revenue (docs/v2-series.md §6; Python `absolute_parts`): each market's revenue per day of a show with
/// attention `own`, base/7 × scale × w_m × own_m / platform_m (0 for a market without a platform size).
pub fn absolute_parts(own: &M3, platform: &M3, scale: f64, weights: &M3, p: &Params) -> M3 {
    std::array::from_fn(|m| if platform[m] > 0.0 { p.demand_base_week / 7.0 * scale * weights[m] * own[m] / platform[m] } else { 0.0 })
}

/// The absolute revenue rule's inputs: each market's platform size and the scale set at the switch.
pub type PlatformRule = Option<(M3, f64)>;

/// The revenue rule the value prices: the absolute rule's platform sizes and scale, and the first path index it
/// applies to (0: in force; later: a scheduled switch, the days before it under the relative rule).
pub type ValueRule = Option<(M3, f64, usize)>;

/// The relative rule's revenue per unit of attention per market for given market totals: demand × the market's
/// renormalised weight ÷ its total (0 for a market without any).
fn per_unit(tt: &M3, weights: &M3, a_ref: f64, p: &Params) -> M3 {
    let mut wsum = 0.0;
    let mut any = false;
    for m in 0..3 {
        if tt[m] > 0.0 {
            wsum += weights[m];
            any = true;
        }
    }
    if !any || wsum == 0.0 {
        wsum = 1.0;
    }
    let dd = demand_day(tt[0], a_ref, p);
    std::array::from_fn(|m| if tt[m] > 0.0 { dd * (weights[m] / wsum) / tt[m] } else { 0.0 })
}

/// Per-share value (yuan) from the attention paths (Python `value_from_paths`).
#[allow(clippy::too_many_arguments)]
pub fn value_from_paths(
    own: &[Vec<f64>; 3],
    totals: &[Vec<f64>; 3],
    weights: &M3,
    a_ref: f64,
    p: &Params,
    shares: i64,
    book_cents: i64,
    rate: f64,
    long_runner: bool,
    tails: &[TailEnd; 3],
    platform: ValueRule,
    extra: Option<&M3>,
) -> f64 {
    let disc = discount_day(rate, p);
    // all after-tax profit reaches the shareholders (dividends, or retained cash in the book value)
    let keep = (1.0 - p.operating_cost_ratio) * (1.0 - p.corp_tax);
    let h = own[0].len();
    let mut pv = 0.0;
    let mut f = 1.0;
    let mut last = 0.0;
    let sw = platform.as_ref().map_or(0, |x| x.2);
    // a scheduled switch: the relative rule's revenue per unit of attention on the switch day, held from then on
    let mut frozen: Option<M3> = None;
    for j in 0..h {
        f *= disc;
        let o = [own[0][j], own[1][j], own[2][j]];
        if let Some((pl, sc, _)) = platform.as_ref().filter(|_| sw == 0) {
            let parts = absolute_parts(&o, pl, *sc, weights, p);
            last = (parts[0] + parts[1] + parts[2]) * keep / shares as f64;
        } else if platform.is_some() && j >= sw {
            let fz = *frozen.get_or_insert_with(|| per_unit(&[totals[0][j], totals[1][j], totals[2][j]], weights, a_ref, p));
            last = (fz[0] * o[0] + fz[1] * o[1] + fz[2] * o[2]) * keep / shares as f64;
        } else {
            let tt = [totals[0][j], totals[1][j], totals[2][j]];
            let (share, _) = blended_share(&o, &tt, weights);
            let dd = demand_day(tt[0], a_ref, p);
            last = dd * share * keep / shares as f64;
        }
        pv += last * f;
    }
    if long_runner && h > 0 {
        let g = p.long_runner_decay.powf(1.0 / 7.0) * disc;
        pv += last * f * g / (1.0 - g);
    } else if h > 0 && (tails.iter().any(|t| t.is_some()) || extra.is_some()) {
        // beyond the horizon: revenue per unit of attention at the last day's, attention along the long tail
        let rel = per_unit(&[totals[0][h - 1], totals[1][h - 1], totals[2][h - 1]], weights, a_ref, p);
        for m in 0..3 {
            let per = match (&platform, &frozen) {
                (Some((pl, sc, _)), _) if sw == 0 => {
                    if pl[m] <= 0.0 {
                        continue;
                    }
                    p.demand_base_week / 7.0 * sc * weights[m] / pl[m] * keep / shares as f64
                }
                (_, Some(fz)) => {
                    if fz[m] <= 0.0 {
                        continue;
                    }
                    fz[m] * keep / shares as f64
                }
                _ => {
                    if totals[m][h - 1] <= 0.0 {
                        continue;
                    }
                    rel[m] * keep / shares as f64
                }
            };
            if let Some((tl, mult, t_last)) = tails[m] {
                pv += f * per * tail_beyond(tl, mult, t_last, own[m][h - 1], disc);
            }
            if let Some(x) = extra {
                pv += per * x[m];
            }
        }
    }
    pv + book_cents as f64 / 100.0 / shares as f64
}

/// A 热度快讯 as an observation of the cn level's log change (Python `flash_observation`):
/// (log surprise, noise variance).
pub fn flash_observation(fp: &FilterParams, hours: f64, comments: i64, prev_final: &[i64]) -> Option<(f64, f64)> {
    if prev_final.is_empty() || hours <= 0.0 || fp.flash_curve.is_empty() {
        return None;
    }
    let mean_prev = prev_final.iter().sum::<i64>() as f64 / prev_final.len() as f64;
    let i = ((hours as i64 - 1).max(0) as usize).min(fp.flash_curve.len() - 1);
    let expected = mean_prev * fp.flash_curve[i];
    if expected < fp.flash_min_expected || comments <= 0 {
        return None;
    }
    let var = fp.flash_r.get(i).copied().unwrap_or(1.0);
    Some(((comments as f64 / expected).ln(), var))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same case through engine/valuation.py (values printed by Python, bit for bit).
    #[test]
    fn matches_python() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let cfg = Config::load_from(&root, |_| None).unwrap();
        let p = Params::from_config(&cfg).unwrap();
        let mut fp = FilterParams::load(&cfg).unwrap();
        // the sequel option is off by config; the reference covers it
        fp.sequel = Some(serde_json::from_str(&std::fs::read_to_string(root.join("sim/calibration/sequel.json")).unwrap()).unwrap());
        let today = 740000;
        let s1 = schedule(&[today - 20, today - 13, today - 6], Some(12), false);
        let s2 = schedule(&[], None, true);
        let m1 = [7.5, 3.0, 2.0];
        let v1 = [0.05, 0.2, 0.03];
        let own1: [Vec<f64>; 3] =
            std::array::from_fn(|m| att_path(&fp, &p, m, m1[m], v1[m], today, &s1, today - 20, false, 120, 1.0));
        let m2 = [8.0, 3.5, 2.5];
        let own2: [Vec<f64>; 3] = std::array::from_fn(|m| att_path(&fp, &p, m, m2[m], 0.01, today, &s2, today - 900, true, 120, 1.0));
        let tot: [Vec<f64>; 3] = std::array::from_fn(|m| {
            own1[m].iter().zip(&own2[m]).map(|(a, b)| a + b + 1000.0 * (m as f64 + 1.0)).collect()
        });
        let w = p.market_weights;
        let a = value_from_paths(&own1, &tot, &w, 110925.85, &p, 10000, 1234567, 0.0325, false, &[None, None, None], None, None);
        let b = value_from_paths(&own2, &tot, &w, 110925.85, &p, 10000, 555, 0.03, true, &[None, None, None], None, None);
        // within 2 ulp: the platform's exp/pow and Python's may differ in the last bit (the values are
        // rounded to cents before use)
        let ulps = |x: f64, y: f64| (x.to_bits() as i64 - y.to_bits() as i64).abs();
        assert!(ulps(a, 15.174402325076468) <= 2, "{a}");
        assert!(ulps(b, 231.95802641536656) <= 2, "{b}");
        assert!(ulps(own1[0][5], 835.2637132741504) <= 2, "{}", own1[0][5]);
        assert!(ulps(own1[1][100], 10.094127256246056) <= 2, "{}", own1[1][100]);
        assert_eq!(kf_update(1.0, 0.3, 2.5, 0.1, 4.0), (2.125, 0.07500000000000002, 1.5));
        assert_eq!(flash_observation(&fp, 6.5, 120, &[100, 90, 110]), Some((1.6638061049175943, 0.644)));
        assert_eq!(robust_update(1.0, 0.3, 2.5, 0.01, 0.1, 0.5, 4.0), (1.8889928389684356, 0.12220143220631287, -3.5695315770294784));
        // a show not airing yet: no fading before its premiere; the post-finale profile
        let s3 = schedule(&[today + 10], Some(12), false);
        let o3 = att_path(&fp, &p, 0, 7.0, 0.1, today, &s3, today + 10, false, 120, 1.0);
        assert_eq!(o3[5], 0.0);
        // (after the finale the tail's power and the summed drift may differ in the last few bits)
        assert!(ulps(o3[15], 456.9515821577898) <= 2 && (o3[110] / 220.84182663908234 - 1.0).abs() < 1e-13, "{} {}", o3[15], o3[110]);
        assert!(ulps(post_finale_factor(&fp, &p, 0, today, today + 30, 1.0), 0.16330143897601446) <= 2);
        // the long tail (phase 1): a show that finished 200 days ago with a score prior; the value includes the
        // closed-form sum beyond the horizon (backend/difftest/valuation_ref.py prints these)
        let mult = tail_mult(&fp, Some(78.0), true, Some(250000.0));
        assert!(ulps(mult, 1.694491676526353) <= 2, "{mult}");
        let s4 = schedule(&(0..12).map(|k| today - 290 + 7 * k).collect::<Vec<_>>(), Some(12), false);
        let fin4 = *s4.last().unwrap();
        let own4: [Vec<f64>; 3] = std::array::from_fn(|m| att_path(&fp, &p, m, m1[m], v1[m], today, &s4, s4[0], false, 400, mult));
        let tot4: [Vec<f64>; 3] = std::array::from_fn(|m| own4[m].iter().map(|x| x + 500.0 * (m as f64 + 1.0)).collect());
        let tails: [TailEnd; 3] = std::array::from_fn(|m| fp.tail[m].as_ref().map(|tl| (tl, mult, today + 400 - fin4 - 7)));
        let c4 = value_from_paths(&own4, &tot4, &w, 110925.85, &p, 10000, 4321, 0.03, false, &tails, None, None);
        let c0 = value_from_paths(&own4, &tot4, &w, 110925.85, &p, 10000, 4321, 0.03, false, &[None, None, None], None, None);
        assert!((own4[0][0] / 390.37406564780855 - 1.0).abs() < 1e-13, "{}", own4[0][0]);
        assert!((own4[1][399] / 2.455339142749654 - 1.0).abs() < 1e-13, "{}", own4[1][399]);
        assert!((c4 / 66.28505106341618 - 1.0).abs() < 1e-13 && (c0 / 30.13526095470089 - 1.0).abs() < 1e-13, "{c4} {c0}");
        assert!(ulps(post_finale_factor(&fp, &p, 1, today - 100, today, mult), 0.318943692361672) <= 2);
        assert!(ulps(tail_var(fp.tail[0].as_ref().unwrap(), 250), 0.4865285274988463) <= 2);
        // absolute revenue (phase 1 step 2)
        let d4 = value_from_paths(&own4, &tot4, &w, 110925.85, &p, 10000, 4321, 0.03, false, &tails, Some(([105000.0, 450.0, 1460.0], 0.83, 0)), None);
        assert!((d4 / 25.041291478777065 - 1.0).abs() < 1e-13, "{d4}");
        // the sequel option (phase 1 step 3; off by config, loaded here directly)
        let sp = fp.sequel.as_ref().unwrap();
        let z = sequel_z(sp, Some(78.0), Some("LIGHT_NOVEL"), false, true);
        let zs = z + sp.beta.sequel;
        let disc = discount_day(0.03, &p);
        let fin1 = *s1.last().unwrap();
        let e1 = sequel_extra(&fp, 0, 7.5, 0.05, today, fin1, today - 20, z, zs, disc, &[], true);
        let e4 = sequel_extra(&fp, 1, 3.0, 0.2, today, fin4, s4[0], z, zs, disc, &[], true);
        let e5 = sequel_extra(&fp, 2, 2.0, 0.03, today, fin4, s4[0], z, zs, disc, &[today + 200, today + 300], true);
        let close = |x: f64, y: f64| (x / y - 1.0).abs() < 1e-12;
        assert!(ulps(z, 1.4591888) <= 2, "{z}");
        assert!(close(e1, 185259.0513666545) && close(e4, 1742.2352104961365) && close(e5, 1147.126310729848), "{e1} {e4} {e5}");
        let g = value_from_paths(&own4, &tot4, &w, 110925.85, &p, 10000, 4321, 0.03, false, &tails, Some(([105000.0, 450.0, 1460.0], 0.83, 0)), Some(&[e1, e4, e5]));
        assert!(close(g, 43.93005747882378), "{g}");
        // a scheduled switch 30 days ahead
        let swv = value_from_paths(&own4, &tot4, &w, 110925.85, &p, 10000, 4321, 0.03, false, &tails, Some(([105000.0, 450.0, 1460.0], 0.83, 30)), None);
        assert!(close(swv, 63.208759021039384), "{swv}");
        // relations not collected: the option is not updated as time passes
        let e6 = sequel_extra(&fp, 1, 3.0, 0.2, today, fin4, s4[0], z, zs, disc, &[], false);
        assert!(close(e6, 3944.0482601881704), "{e6}");
        // the us episode week observed day by day
        assert_eq!(partial_observation(&fp, 7.3, 3, -0.004), Some((7.270578, 0.04953)));
        assert_eq!(partial_observation(&fp, 7.3, 7, -0.004), Some((7.288, 0.0)));
        // the joint filter (v0.3.2)
        let c = prior_cov(&[0.35, 1.08, 0.86], 0.48);
        let want_c = [0.35, 0.2951121820596364, 0.26334464110742783, 0.2951121820596364, 1.08, 0.4625964980412195, 0.2633446411074279, 0.46259649804121955, 0.86];
        for (i, w) in want_c.iter().enumerate() {
            assert!(ulps(c[i / 3][i % 3], *w) <= 2, "prior_cov {i}");
        }
        let (m, c2) = kf_predict_joint(&[1.0, -4.0, -2.5], &c, 3, &[-0.02, -0.022, -0.027], &[0.01, 0.015, 0.019]);
        let (m3, c3, ll, sv) = kf_update_joint(&m, &c2, 1, -3.2, 0.0146, 4.0);
        let want_m = [1.1642603980902466, -3.2110947700947703, -2.2294656306566374];
        let want_c3 = [0.3035773955773956, 0.0037808335012904926, 0.14355009745167968, 0.0037808335012904926, 0.014412951912951888, 0.005926560961216021, 0.1435500974516797, 0.005926560961216021, 0.7292187434187434];
        for i in 0..3 {
            assert!(ulps(m3[i], want_m[i]) <= 2, "kf_update_joint mean {i}: {}", m3[i]);
        }
        for (i, w) in want_c3.iter().enumerate() {
            assert!(ulps(c3[i / 3][i % 3], *w) <= 2, "kf_update_joint cov {i}: {}", c3[i / 3][i % 3]);
        }
        assert!(ulps(ll, -1.3133207190690046) <= 2 && ulps(sv, 1.1396) <= 2, "{ll} {sv}");
    }
}
