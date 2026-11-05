//! `[market]` in server/game.toml: the v2 NPC population (docs/v2-npc-market.md). Every key has a default
//! here, so the section may list only what it changes. Money amounts are in 币 (yuan), rates per hour.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MarketConfig {
    /// Run the v2 NPC market instead of the v1 NPCs (`npc.rs`).
    pub enabled: bool,
    /// Seed of everything random about the population (deterministic per seed).
    pub seed: u64,

    // ---- population (§2)
    pub n_value: usize,
    pub n_trend: usize,
    pub n_news: usize,
    pub n_retail: usize,
    /// the counts above are for this many listed stocks and scale with the actual number (0: fixed)
    pub population_reference: f64,
    /// starting cash per agent, 币
    pub cash_value: f64,
    pub cash_trend: f64,
    pub cash_news: f64,
    pub cash_retail: f64,
    /// watch lists: this many stocks, drawn with probability ∝ heat^heat_alpha
    pub watch_min: usize,
    pub watch_max: usize,
    pub heat_alpha: f64,
    /// most of an agent's wealth one stock may take
    pub max_position_frac: f64,

    // ---- activity (§3): actions per hour at intraday factor 1
    pub rate_value: f64,
    pub rate_trend: f64,
    pub rate_news: f64,
    pub rate_retail: f64,
    /// intraday curve (factor by local time): auction, morning, lunch (12:00-13:30), afternoon,
    /// evening (18:00-20:00), night (20:00-23:00)
    pub curve: [f64; 6],
    /// activity multiplier per unit event magnitude (magnitude 10 => 1 + 10 * x), decaying with
    /// `event_activity_half_life_h`
    pub event_activity: f64,
    pub event_activity_half_life_h: f64,
    /// activity multiplier from 1 h before to 2 h after an episode airs
    pub air_activity: f64,

    // ---- beliefs (§4)
    /// stationary standard deviation of an agent's log valuation bias
    pub bias_sigma: f64,
    pub bias_half_life_days: f64,
    /// log valuation shock of a magnitude-10 official event
    pub event_shock: f64,
    /// credibility weights: official / media / rumor
    pub credibility: [f64; 3],

    // ---- value agents
    /// trade only when |log(belief/price)| exceeds a per-agent threshold drawn from this range
    pub value_threshold: [f64; 2],
    /// limit price = price * exp(aggression * mispricing), aggression drawn from this range
    pub value_aggression: [f64; 2],
    /// share of cash (buy) or holding (sell) per action at full conviction
    pub value_size: f64,
    /// mispricing beyond the threshold for full conviction
    pub value_full: f64,

    // ---- trend agents
    /// look-back, minutes (log-uniform)
    pub trend_lookback_min: [f64; 2],
    /// trade when |return over the look-back| exceeds this (per agent, uniform)
    pub trend_threshold: [f64; 2],
    pub trend_size: f64,

    // ---- news agents
    pub news_size: f64,
    /// share of a news agent's holding sold per action once no event is active
    pub news_unwind: f64,

    // ---- retail
    /// share of cash per buy (uniform)
    pub retail_size: [f64; 2],
    pub retail_market_prob: f64,
    /// sd of a retail limit price around the last price
    pub retail_price_sd: f64,

    // ---- market maker (improved; one per stock)
    /// share of the public float it aims to hold
    pub mm_target_frac: f64,
    /// requotes per hour
    pub mm_rate: f64,
    pub mm_base_spread: f64,
    /// extra half-spread per unit of recent volatility (per sqrt(5 min))
    pub mm_vol_mult: f64,
    /// EWMA half-life of the volatility estimate, minutes
    pub mm_vol_half_life_min: f64,
    /// quote size, share of the company's shares (level k quotes k times this)
    pub mm_quote_frac: f64,
    /// price levels per side; level k sits (1 + (k-1) * mm_level_step) half-spreads from the centre
    pub mm_levels: usize,
    pub mm_level_step: f64,
    pub mm_inventory_skew: f64,
    /// weight of the last price (vs the hidden value) in its reference
    pub mm_anchor_to_price: f64,
    /// market maker's cash after the float migration, 币
    pub mm_cash: f64,

    // ---- committee (v1 rule)
    pub committee_rate: f64,

    // ---- IPO subscriptions by value agents
    pub ipo_subscribe_frac: f64,

    // ---- fund flows (§6), applied after each close
    /// below this share of starting cash an agent receives money from the world account
    pub fund_floor: f64,
    /// cash above this multiple of starting cash flows back to the world account
    pub fund_ceiling: f64,
    /// most it may receive or pay per week, share of its starting cash
    pub fund_weekly_cap: f64,

    // ---- 热度快讯 (episode comment surprise)
    /// a flash fires when the episode's comments differ from the recent average by this ratio
    pub flash_ratio: f64,
    /// hours after air time the flash appears
    pub flash_delay_h: f64,
    pub flash_half_life_h: f64,
    /// a 热度快讯 is published when the episode's comments differ from expectations by this much
    pub flash_news_min: f64,
    /// a change of the public value by this much (log) is a revision the news agents react to
    pub revision_min: f64,
}

impl Default for MarketConfig {
    fn default() -> Self {
        MarketConfig {
            enabled: false,
            seed: 20261105,
            n_value: 240,
            n_trend: 120,
            n_news: 60,
            n_retail: 180,
            population_reference: 140.0,
            cash_value: 15000.0,
            cash_trend: 8000.0,
            cash_news: 8000.0,
            cash_retail: 3000.0,
            watch_min: 5,
            watch_max: 15,
            heat_alpha: 0.3,
            max_position_frac: 0.35,
            rate_value: 4.0,
            rate_trend: 1.5,
            rate_news: 0.3,
            rate_retail: 3.0,
            curve: [0.5, 1.0, 1.3, 0.9, 1.2, 2.0],
            event_activity: 0.4,
            event_activity_half_life_h: 2.0,
            air_activity: 2.0,
            bias_sigma: 0.05,
            bias_half_life_days: 4.0,
            event_shock: 0.12,
            credibility: [1.0, 0.6, 0.3],
            value_threshold: [0.01, 0.05],
            value_aggression: [0.2, 0.6],
            value_size: 0.15,
            value_full: 0.10,
            trend_lookback_min: [5.0, 240.0],
            trend_threshold: [0.01, 0.04],
            trend_size: 0.1,
            news_size: 0.15,
            news_unwind: 0.3,
            retail_size: [0.01, 0.05],
            retail_market_prob: 0.35,
            retail_price_sd: 0.01,
            mm_target_frac: 0.2,
            mm_rate: 20.0,
            mm_base_spread: 0.008,
            mm_vol_mult: 2.0,
            mm_vol_half_life_min: 30.0,
            mm_quote_frac: 0.005,
            mm_levels: 3,
            mm_level_step: 1.0,
            mm_inventory_skew: 0.3,
            mm_anchor_to_price: 0.3,
            mm_cash: 10000.0,
            committee_rate: 3.0,
            ipo_subscribe_frac: 0.1,
            fund_floor: 0.3,
            fund_ceiling: 1.2,
            fund_weekly_cap: 0.1,
            flash_ratio: 1.5,
            flash_delay_h: 3.0,
            flash_half_life_h: 6.0,
            flash_news_min: 0.2,
            revision_min: 0.05,
        }
    }
}

impl MarketConfig {
    pub fn from_table(t: Option<&toml::Value>) -> Result<MarketConfig, String> {
        match t {
            None => Ok(MarketConfig::default()),
            Some(v) => v.clone().try_into().map_err(|e| format!("[market]: {e}")),
        }
    }

    /// Set one key from a string (`key=value`, for calibration runs); arrays as `a,b`.
    pub fn set(&mut self, kv: &str) -> Result<(), String> {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("expected key=value: {kv}"))?;
        let mut cur = toml::Table::try_from(&*self).map_err(|e| e.to_string())?;
        let old = cur.get(k).ok_or_else(|| format!("unknown [market] key {k}"))?;
        let parsed = match old {
            toml::Value::Array(_) => toml::Value::Array(
                v.split(',')
                    .map(|x| x.trim().parse::<f64>().map(toml::Value::Float))
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("{k}: {e}"))?,
            ),
            toml::Value::Integer(_) => toml::Value::Integer(v.parse().map_err(|e| format!("{k}: {e}"))?),
            toml::Value::Boolean(_) => toml::Value::Boolean(v.parse().map_err(|e| format!("{k}: {e}"))?),
            _ => toml::Value::Float(v.parse().map_err(|e| format!("{k}: {e}"))?),
        };
        cur.insert(k.to_string(), parsed);
        *self = toml::Value::Table(cur).try_into().map_err(|e: toml::de::Error| e.to_string())?;
        Ok(())
    }

    /// The configuration as a TOML `[market]` body (for reports).
    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }
}
