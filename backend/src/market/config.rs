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
    /// agents added to a running population (`grow_population`) start with this share of it
    pub grow_cash_scale: f64,
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
    /// night session: 23:00-02:00 (late-night anime has just aired) and 02:00-04:00
    pub curve_night: [f64; 2],
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
    /// stock lent to the market maker by the rights holders (committee), share of the company's shares, on
    /// top of `mm_target_frac` of the float (the issuer lending stock to its designated market maker); also
    /// the most it sells short, borrowing from the lending pool, once its inventory is gone (0: never)
    pub mm_loan_frac: f64,
    /// designated-market-maker obligation: a quote unit is at least this many 币 (level k quotes k units)
    pub mm_quote_min: f64,
    /// quote size skew by inventory: at x = (position - target) / target (at most 1; -1 at zero inventory and
    /// below that on the short side) bids are (1 - s x) and offers (1 + s x) times the size
    pub mm_size_skew: f64,
    /// half-spread widening per quote unit of recent adverse markout (what its fills lost by the next
    /// requote, EWMA over `mm_vol_half_life_min`)
    pub mm_toxicity_mult: f64,
    /// in an information window (air times, fresh events, the first half hour) the half-spread is
    /// (1 + this) times wider and the quote size (1 + this) times smaller
    pub mm_event_widen: f64,

    // ---- adaptive liquidity (the designated market maker's obligations bind only when humans are thin)
    /// the market maker quotes its target depth less the players' resting depth, and does not step ahead
    /// of a player's best price near its own
    pub adapt_mm: bool,
    /// players' orders count as depth within this many of its half-spreads from the centre (at the inside,
    /// where they stand in for its own quotes; deep orders do not), after resting this long
    pub adapt_depth_halves: f64,
    pub adapt_min_age_min: f64,
    /// the market maker keeps at least this share of its quote per side
    pub adapt_min_quote: f64,
    /// half-life of the human activity measure, minutes
    pub adapt_half_life_min: f64,
    /// retail and trend agents' rate multiplier: from `adapt_rate_max` (no humans) down to `adapt_rate_min`
    pub adapt_rate_min: f64,
    pub adapt_rate_max: f64,
    /// human activity (traded 币 per hour plus resting depth 币) at which the multiplier has moved 63% of
    /// the way down
    pub adapt_activity_ref: f64,
    /// the market maker stops bidding once its inventory reaches this multiple of its target
    pub mm_inventory_cap: f64,

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

    // ---- sentiment layer, batch 1 (docs/v2-sentiment.md). Off in these defaults; on in server/game.toml.
    // Every random strength, duration and occurrence below is a hash of (seed, server secret, company, time):
    // the same after a restart, not computable from the public repository.
    /// market-wide sentiment (Baker–Wurgler): a daily AR(1) in standard units; its effect on retail and trend
    /// agents is `sentiment_strength` per standard deviation at sensitivity 1 (0: off)
    pub sentiment_strength: f64,
    pub sentiment_half_life_days: f64,
    /// sensitivity of a stock ∝ (1 / heat)^`sentiment_heat_exp`, clamped to this range, then scaled so the
    /// listed stocks average 1 (cold, hard-to-value stocks are more sentiment-prone)
    pub sentiment_heat_exp: f64,
    pub sentiment_sens: [f64; 2],
    /// band shown to players: below -x 冷淡, above +x 火热 (standard deviations)
    pub sentiment_band: f64,
    /// attention-driven buying (Barber–Odean): retail buy tilt per unit of attention (0: off)
    pub attention: f64,
    /// chance that a trigger (premiere night, a published 热度快讯, entering the movers' list) draws attention
    pub attention_prob: f64,
    /// log-normal spread of an attention episode's strength (mean 1)
    pub attention_sigma: f64,
    /// half-life range of an attention episode, hours (log-uniform)
    pub attention_half_life_h: [f64; 2],
    /// 上热榜: the top N gainers and losers of the day (the market page's lists) draw attention (0: off), with
    /// this weight relative to a premiere or a flash
    pub attention_rank_n: usize,
    pub attention_rank_weight: f64,
    /// under- and over-reaction (Barberis–Shleifer–Vishny): the news agents' reaction to a revision is scaled
    /// by a random factor in `reaction_first` for the first surprise in a direction, and by
    /// 1 + (k - 1) * step (step drawn from `reaction_step`, at most `reaction_max`) for the k-th in a row;
    /// the reaction decays with a random half-life from `reaction_half_life_h` (log-uniform). Off: false.
    pub reaction: bool,
    pub reaction_first: [f64; 2],
    pub reaction_step: [f64; 2],
    pub reaction_max: f64,
    pub reaction_half_life_h: [f64; 2],
    /// revisions further apart than this do not form a streak (at most 8: the events kept)
    pub reaction_window_days: f64,
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
            grow_cash_scale: 1.0,
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
            curve_night: [2.4, 1.0],
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
            mm_loan_frac: 0.0,
            mm_quote_min: 0.0,
            mm_inventory_cap: 2.0,
            mm_toxicity_mult: 0.0,
            mm_event_widen: 0.0,
            adapt_mm: false,
            adapt_depth_halves: 1.5,
            adapt_min_age_min: 5.0,
            adapt_min_quote: 0.25,
            adapt_half_life_min: 30.0,
            adapt_rate_min: 1.0,
            adapt_rate_max: 1.0,
            adapt_activity_ref: 20000.0,
            mm_size_skew: 0.0,
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
            sentiment_strength: 0.0,
            sentiment_half_life_days: 14.0,
            sentiment_heat_exp: 0.5,
            sentiment_sens: [0.5, 2.0],
            sentiment_band: 0.67,
            attention: 0.0,
            attention_prob: 0.75,
            attention_sigma: 0.75,
            attention_half_life_h: [1.0, 6.0],
            attention_rank_n: 0,
            attention_rank_weight: 0.5,
            reaction: false,
            reaction_first: [0.3, 0.8],
            reaction_step: [0.2, 0.6],
            reaction_max: 2.5,
            reaction_half_life_h: [1.0, 8.0],
            reaction_window_days: 8.0,
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
