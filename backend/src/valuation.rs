//! Port of `engine/valuation.py`: the pure formulas that turn the model's daily inputs plus game state
//! into revenue shares and per-share values. Every floating-point operation is in the same order as the
//! Python source (and callers sum in the same order), so the results are bit-identical.

use crate::config::Config;

pub const MARKETS: [&str; 3] = ["us", "cn", "jp"];

/// Per-market values in `MARKETS` order (us, cn, jp).
pub type M3 = [f64; 3];

/// The economic parameters the close and the valuation read, from `Config::params`.
#[derive(Debug, Clone)]
pub struct Params {
    pub market_weights: M3,
    // company
    pub shares: i64,
    pub operating_cost_ratio: f64,
    pub corp_tax: f64,
    pub payout_ratio: f64,
    pub ipo_float: f64,
    pub ipo_discount: f64,
    pub ipo_price_min: f64,
    pub production_fee_ratio: f64,
    // revenue
    pub demand_base_week: f64,
    pub demand_elasticity: f64,
    /// the absolute revenue rule starts once the platform size averages this many days (docs/design/series.md §6)
    pub platform_min_days: i64,
    /// the absolute revenue rule (docs/design/series.md §6) at all: off (0) until the platform history allows it
    pub absolute_revenue: bool,
    /// the cn market's revenue under the absolute rule relative to the comment-based series' coverage: Σ cn
    /// attention of all listed shows ÷ Σ of the shows the old series measured, on the switch (daily.rs
    /// `platform_switch`); 1: every market's total kept
    pub absolute_cn_coverage: f64,
    /// the score premium on attention (`[revenue] score_*`, engine/economy.py `score_mult`): the inputs' att carries it
    pub score_ref: f64,
    pub score_power: f64,
    pub score_mult_min: f64,
    pub score_mult_max: f64,
    /// cn/jp post-finale days without data earn the filter's expectation (the §4.2 stopgap)
    pub impute_tail: bool,
    /// delisting (docs/design/series.md §9): a finished series' weekly revenue under this share of the median airing
    /// company's, while its chance of a sequel announcement within three years is under `delist_sequel_p`
    pub delist_tail_share: f64,
    pub delist_sequel_p: f64,
    /// a listed series' earlier works join it by an asset injection at their public appraisal, and a delisting buys the
    /// works out at it (docs/design/series.md §13)
    pub asset_injection: bool,
    // valuation
    pub equity_premium: f64,
    pub post_finale_profile: Vec<f64>,
    pub post_finale_tail_decay: f64,
    pub long_runner_decay: f64,
    pub min_remaining_weeks: i64,
    pub premiere_drop: f64,
    pub in_season_retention: f64,
    pub min_days_after_premiere: i64,
    /// episode count assumed when it is unknown
    pub default_episodes: i64,
    // central bank
    pub r_neutral: f64,
    pub r_min: f64,
    pub r_max: f64,
    pub k_money: f64,
    pub m2_growth_target: f64,
    pub max_step: f64,
    pub growth_lookback_days: i64,
    pub deposit_spread: f64,
    // universe
    pub ipo_lead_days: i64,
}

fn num(cfg: &Config, path: &str) -> Result<f64, String> {
    match cfg.param(path) {
        Some(toml::Value::Float(f)) => Ok(*f),
        Some(toml::Value::Integer(i)) => Ok(*i as f64),
        other => Err(format!("parameter {path}: expected a number, got {other:?}")),
    }
}

fn int(cfg: &Config, path: &str) -> Result<i64, String> {
    match cfg.param(path) {
        Some(toml::Value::Integer(i)) => Ok(*i),
        other => Err(format!("parameter {path}: expected an integer, got {other:?}")),
    }
}

impl Params {
    /// The score premium on attention (Python `engine/economy.py score_mult`): (score / ref)^power within
    /// [min, max]; 1 without a score.
    pub fn score_mult(&self, score: Option<f64>) -> f64 {
        match score {
            Some(sc) if sc > 0.0 => (sc / self.score_ref).powf(self.score_power).clamp(self.score_mult_min, self.score_mult_max),
            _ => 1.0,
        }
    }
}

impl Params {
    pub fn from_config(cfg: &Config) -> Result<Params, String> {
        let profile = cfg
            .param("valuation.post_finale_profile")
            .and_then(|v| v.as_array())
            .ok_or("valuation.post_finale_profile missing")?
            .iter()
            .map(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).ok_or("bad profile entry"))
            .collect::<Result<Vec<f64>, _>>()?;
        Ok(Params {
            market_weights: [num(cfg, "markets.us")?, num(cfg, "markets.cn")?, num(cfg, "markets.jp")?],
            shares: int(cfg, "company.shares")?,
            // p["company"].get("operating_cost_ratio", 0)
            operating_cost_ratio: if cfg.param("company.operating_cost_ratio").is_some() {
                num(cfg, "company.operating_cost_ratio")?
            } else {
                0.0
            },
            corp_tax: num(cfg, "company.corp_tax")?,
            payout_ratio: num(cfg, "company.payout_ratio")?,
            ipo_float: num(cfg, "company.ipo_float")?,
            ipo_discount: num(cfg, "company.ipo_discount")?,
            ipo_price_min: num(cfg, "company.ipo_price_min")?,
            production_fee_ratio: num(cfg, "company.production_fee_ratio")?,
            demand_base_week: num(cfg, "revenue.demand_base_week")?,
            demand_elasticity: num(cfg, "revenue.demand_elasticity")?,
            platform_min_days: int(cfg, "revenue.platform_min_days").unwrap_or(182),
            absolute_revenue: int(cfg, "revenue.absolute_revenue").unwrap_or(0) == 1,
            absolute_cn_coverage: num(cfg, "revenue.absolute_cn_coverage").unwrap_or(1.0),
            score_ref: num(cfg, "revenue.score_ref").unwrap_or(70.0),
            score_power: num(cfg, "revenue.score_power").unwrap_or(2.0),
            score_mult_min: num(cfg, "revenue.score_mult_min").unwrap_or(0.7),
            score_mult_max: num(cfg, "revenue.score_mult_max").unwrap_or(1.4),
            impute_tail: int(cfg, "valuation.impute_tail").unwrap_or(1) == 1,
            delist_tail_share: num(cfg, "company.delist_tail_share").unwrap_or(0.01),
            delist_sequel_p: num(cfg, "company.delist_sequel_p").unwrap_or(0.10),
            asset_injection: int(cfg, "company.asset_injection").unwrap_or(0) == 1,
            equity_premium: num(cfg, "valuation.equity_premium")?,
            post_finale_profile: profile,
            post_finale_tail_decay: num(cfg, "valuation.post_finale_tail_decay")?,
            long_runner_decay: num(cfg, "valuation.long_runner_decay")?,
            min_remaining_weeks: int(cfg, "valuation.min_remaining_weeks")?,
            premiere_drop: num(cfg, "valuation.premiere_drop")?,
            in_season_retention: num(cfg, "valuation.in_season_retention")?,
            min_days_after_premiere: int(cfg, "valuation.min_days_after_premiere")?,
            default_episodes: int(cfg, "valuation.default_episodes").unwrap_or(13),
            r_neutral: num(cfg, "central_bank.r_neutral")?,
            r_min: num(cfg, "central_bank.r_min")?,
            r_max: num(cfg, "central_bank.r_max")?,
            k_money: num(cfg, "central_bank.k_money")?,
            m2_growth_target: num(cfg, "central_bank.m2_growth_target")?,
            max_step: num(cfg, "central_bank.max_step")?,
            growth_lookback_days: int(cfg, "central_bank.growth_lookback_days")?,
            deposit_spread: num(cfg, "central_bank.deposit_spread")?,
            ipo_lead_days: int(cfg, "universe.ipo_lead_days")?,
        })
    }
}

/// Σ_m w_m * own_m/T_m, renormalised over markets that have any data. Returns (share, per-market part).
pub fn blended_share(own: &M3, totals: &M3, weights: &M3) -> (f64, M3) {
    let mut wsum = 0.0;
    let mut any = false;
    for m in 0..3 {
        if totals[m] > 0.0 {
            wsum += weights[m];
            any = true;
        }
    }
    // `sum(...) or 1.0`: an empty or zero sum becomes 1.0
    if !any || wsum == 0.0 {
        wsum = 1.0;
    }
    let mut parts = [0.0; 3];
    for m in 0..3 {
        if totals[m] > 0.0 {
            parts[m] = (weights[m] / wsum) * own[m] / totals[m];
        }
    }
    (parts[0] + parts[1] + parts[2], parts)
}

/// A show's expected share of external demand, with every listed show's expectation in the denominator
/// (summed in `listed` order); a show that is not listed yet is added to the denominator last.
pub fn expected_share(own: &M3, own_listed: bool, listed: &[&M3], weights: &M3) -> f64 {
    let mut tot = [0.0; 3];
    for a in listed {
        for m in 0..3 {
            tot[m] += a[m];
        }
    }
    if !own_listed {
        for m in 0..3 {
            tot[m] += own[m];
        }
    }
    blended_share(own, &tot, weights).0
}

pub fn demand_index(tot_us: f64, a_ref: f64, p: &Params) -> f64 {
    if tot_us > 0.0 {
        (tot_us / a_ref).powf(p.demand_elasticity)
    } else {
        0.0
    }
}

pub fn demand_day(tot_us: f64, a_ref: f64, p: &Params) -> f64 {
    p.demand_base_week / 7.0 * demand_index(tot_us, a_ref, p)
}

pub fn demand_week(tot_us: f64, a_ref: f64, p: &Params) -> f64 {
    p.demand_base_week * demand_index(tot_us, a_ref, p)
}

/// Share of revenue that reaches shareholders as dividends.
pub fn margin(p: &Params) -> f64 {
    (1.0 - p.operating_cost_ratio) * (1.0 - p.corp_tax) * p.payout_ratio
}

pub fn has_real_data(days_since_premiere: i64, p: &Params) -> bool {
    days_since_premiere >= p.min_days_after_premiere
}

/// Python `x ** k` for a float base and an int exponent (C `pow`).
fn powi(x: f64, k: i64) -> f64 {
    x.powf(k as f64)
}

/// Per-share value: discounted expected dividends + book value per share. `episodes` may be None only for
/// long-runners (Python raises otherwise; so does this).
#[allow(clippy::too_many_arguments)]
pub fn fundamental(
    aired: i64,
    episodes: Option<i64>,
    episodes_known: bool,
    long_runner: bool,
    weekly_div: f64,
    book: f64,
    rate: f64,
    p: &Params,
    preair_div: f64,
    use_preair: bool,
) -> Result<f64, String> {
    let rw = (rate + p.equity_premium) / 52.0;
    let disc = 1.0 / (1.0 + rw);
    let div = if use_preair { preair_div } else { weekly_div };
    let pv;
    if long_runner {
        let g = p.long_runner_decay;
        pv = div * g * disc / (1.0 - g * disc);
    } else {
        let episodes = episodes.ok_or("fundamental: episodes is None for a show that is not a long-runner")?;
        let mut remaining = (episodes - aired).max(0);
        if !episodes_known && aired > 0 {
            remaining = remaining.max(p.min_remaining_weeks);
        }
        let rho = p.in_season_retention;
        let level = div * if aired == 1 && !use_preair { p.premiere_drop } else { 1.0 };
        let mut acc = 0.0;
        for k in 1..=remaining {
            acc += level * powi(rho, k - 1) * powi(disc, k);
        }
        let last = level * powi(rho, (remaining - 1).max(0));
        let prof = &p.post_finale_profile;
        let tail = p.post_finale_tail_decay;
        for (j, f) in prof.iter().enumerate() {
            acc += last * f * powi(disc, remaining + j as i64 + 1);
        }
        let n = remaining + prof.len() as i64;
        acc += last * prof[prof.len() - 1] * powi(disc, n) * (tail * disc) / (1.0 - tail * disc);
        pv = acc;
    }
    Ok(pv + book)
}

/// Per-share value in cents from the company's own revenue history (`revs_cents` newest first, at most 7).
#[allow(clippy::too_many_arguments)]
pub fn fair_value_cents(
    revs_cents: &[i64],
    since: i64,
    shares: i64,
    book_cents: i64,
    rate: f64,
    demand_week: f64,
    pre_share: f64,
    aired: i64,
    episodes: Option<i64>,
    episodes_known: bool,
    long_runner: bool,
    real_data: bool,
    p: &Params,
) -> Result<i64, String> {
    let revs: Vec<f64> = revs_cents.iter().map(|r| *r as f64 / 100.0).collect();
    let preair_weekly = demand_week * pre_share;
    let weekly_rev = if since < 7 {
        let n = (since.max(1) as usize).min(revs.len());
        let hist = &revs[..n];
        let mut s = 0.0;
        for h in hist {
            s += h;
        }
        s + preair_weekly / 7.0 * (7 - hist.len() as i64) as f64
    } else {
        let mut s = 0.0;
        for r in &revs {
            s += r;
        }
        s * 7.0 / (revs.len().max(1) as f64)
    };
    let weekly_div = weekly_rev * margin(p) / shares as f64;
    let preair_div = preair_weekly * margin(p) / shares as f64;
    let v = fundamental(
        aired,
        episodes,
        episodes_known,
        long_runner,
        weekly_div,
        book_cents as f64 / 100.0 / shares as f64,
        rate,
        p,
        preair_div,
        !real_data,
    )?;
    Ok(((v * 100.0).round_ties_even() as i64).max(1))
}

/// The last episode has aired.
pub fn finished(aired: i64, episodes: Option<i64>, long_runner: bool) -> bool {
    !long_runner && episodes.is_some_and(|e| e != 0 && aired >= e)
}
