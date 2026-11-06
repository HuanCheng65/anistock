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
    #[serde(default)]
    post_finale: Option<PerMarketVec>,
    #[serde(default)]
    post_finale_tail: Option<PerMarket>,

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
    /// per market: attention by week after the finale week and its weekly tail (empty: the game's profile)
    pub post_finale: [Vec<f64>; 3],
    pub post_finale_tail: M3,
    /// long-runners' weekly level factor (`[valuation] long_runner_decay`; their filter's drift)
    pub long_runner_decay: f64,
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
            drift_by_week: r.drift_by_week.map_or([Vec::new(), Vec::new(), Vec::new()], |v| [v.us, v.cn, v.jp]),
            post_finale: r.post_finale.map_or([Vec::new(), Vec::new(), Vec::new()], |v| [v.us, v.cn, v.jp]),
            post_finale_tail: r.post_finale_tail.map_or([0.0; 3], |t| t.m3()),
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
pub fn post_finale_factor(fp: &FilterParams, p: &Params, m: usize, fin: i64, t: i64) -> f64 {
    if t < fin + 7 {
        return 1.0;
    }
    let (prof, tail) = post_finale_curve(fp, p, m);
    let w = (t - fin - 7).div_euclid(7);
    if (w as usize) < prof.len() {
        prof[w as usize]
    } else {
        prof[prof.len() - 1] * tail.powf((w - prof.len() as i64 + 1) as f64)
    }
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
) -> f64 {
    let disc = discount_day(rate, p);
    // all after-tax profit reaches the shareholders (dividends, or retained cash in the book value)
    let keep = (1.0 - p.operating_cost_ratio) * (1.0 - p.corp_tax);
    let h = own[0].len();
    let mut pv = 0.0;
    let mut f = 1.0;
    let mut last = 0.0;
    for j in 0..h {
        f *= disc;
        let o = [own[0][j], own[1][j], own[2][j]];
        let tt = [totals[0][j], totals[1][j], totals[2][j]];
        let (share, _) = blended_share(&o, &tt, weights);
        let dd = demand_day(tt[0], a_ref, p);
        last = dd * share * keep / shares as f64;
        pv += last * f;
    }
    if long_runner && h > 0 {
        let g = p.long_runner_decay.powf(1.0 / 7.0) * disc;
        pv += last * f * g / (1.0 - g);
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
        let fp = FilterParams::load(&cfg).unwrap();
        let today = 740000;
        let s1 = schedule(&[today - 20, today - 13, today - 6], Some(12), false);
        let s2 = schedule(&[], None, true);
        let m1 = [7.5, 3.0, 2.0];
        let v1 = [0.05, 0.2, 0.03];
        let own1: [Vec<f64>; 3] =
            std::array::from_fn(|m| att_path(&fp, &p, m, m1[m], v1[m], today, &s1, today - 20, false, 120));
        let m2 = [8.0, 3.5, 2.5];
        let own2: [Vec<f64>; 3] = std::array::from_fn(|m| att_path(&fp, &p, m, m2[m], 0.01, today, &s2, today - 900, true, 120));
        let tot: [Vec<f64>; 3] = std::array::from_fn(|m| {
            own1[m].iter().zip(&own2[m]).map(|(a, b)| a + b + 1000.0 * (m as f64 + 1.0)).collect()
        });
        let w = p.market_weights;
        let a = value_from_paths(&own1, &tot, &w, 110925.85, &p, 10000, 1234567, 0.0325, false);
        let b = value_from_paths(&own2, &tot, &w, 110925.85, &p, 10000, 555, 0.03, true);
        // within 2 ulp: the platform's exp/pow and Python's may differ in the last bit (the values are
        // rounded to cents before use)
        let ulps = |x: f64, y: f64| (x.to_bits() as i64 - y.to_bits() as i64).abs();
        assert!(ulps(a, 22.23599386634486) <= 2, "{a}");
        assert!(ulps(b, 359.9638471331933) <= 2, "{b}");
        assert!(ulps(own1[0][5], 835.2637132741504) <= 2, "{}", own1[0][5]);
        assert!(ulps(own1[1][100], 2.8463816081531292) <= 2, "{}", own1[1][100]);
        assert_eq!(kf_update(1.0, 0.3, 2.5, 0.1, 4.0), (2.125, 0.07500000000000002, 1.5));
        assert_eq!(flash_observation(&fp, 6.5, 120, &[100, 90, 110]), Some((1.6638061049175943, 0.644)));
        assert_eq!(robust_update(1.0, 0.3, 2.5, 0.01, 0.1, 0.5, 4.0), (1.8889928389684356, 0.12220143220631287, -3.5695315770294784));
        // a show not airing yet: no fading before its premiere; the post-finale profile
        let s3 = schedule(&[today + 10], Some(12), false);
        let o3 = att_path(&fp, &p, 0, 7.0, 0.1, today, &s3, today + 10, false, 120);
        assert_eq!(o3[5], 0.0);
        // (after the finale the tail's power and the summed drift may differ in the last few bits)
        assert!(ulps(o3[15], 456.9515821577898) <= 2 && (o3[110] / 224.61921276713286 - 1.0).abs() < 1e-13, "{} {}", o3[15], o3[110]);
        assert!(ulps(post_finale_factor(&fp, &p, 0, today, today + 30), 0.179913) <= 2);
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
