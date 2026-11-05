//! Game configuration (port of `server/config.py`) and the game clock.
//!
//! `server/game.toml` names a base parameter file (`sim/params_recommended.toml`); its `[overrides]`
//! table is applied on top by dotted path, exactly like v1, so the game and the simulation share one
//! source of truth. Environment overrides: `ANISTOCK_DB`, `ANISTOCK_GAME_DB`, `ANISTOCK_SECRET`,
//! `ANISTOCK_INVITE`, `ANISTOCK_ALWAYS_OPEN=1`, `ANISTOCK_NO_LOOPS=1`, plus `ANISTOCK_ROOT` (repository root;
//! found automatically otherwise).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;

use crate::exchange::{Fees, Rules};

#[derive(Debug, Clone, Deserialize)]
pub struct GameRules {
    pub timezone: String,
    pub initial_cash: f64,
    pub auction_start: String,
    pub trading_open: String,
    pub trading_close: String,
    pub daily_step_at: String,
    pub vcm_threshold: f64,
    pub vcm_window_min: i64,
    pub vcm_cool_min: i64,
    pub commission: f64,
    pub stamp_duty: f64,
    pub dividend_tax: Vec<(f64, f64)>,
    pub transfer_ratio: f64,
    pub delist_after_weeks: i64,
    pub delist_revenue_share: f64,
    pub min_listing_popularity: i64,
    pub ipo_subscription_days: i64,
    pub ipo_public_base: f64,
    pub ipo_clawback: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NpcConfig {
    pub market_maker_cash: f64,
    pub noise_traders: i64,
    pub noise_trader_cash: f64,
    pub tick_seconds: f64,
    pub mm_spread: f64,
    pub mm_quote_frac: f64,
    pub mm_value_noise: f64,
    pub mm_anchor_to_price: f64,
    pub mm_inventory_skew: f64,
    pub committee_sell_above: f64,
    pub committee_sell_frac: f64,
    pub noise_trade_prob: f64,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    /// Economic parameters (same shape as sim/params.toml) with game.toml's overrides applied.
    pub params: toml::Table,
    pub game: GameRules,
    /// The raw `[game]` table, for keys not modelled in `GameRules`.
    pub game_raw: toml::Table,
    pub npc: NpcConfig,
    /// `[market]`: the v2 NPC market (defaults when the section is absent; disabled by default)
    pub market: crate::market::MarketConfig,
    pub tz: Tz,
    pub collector_db: PathBuf,
    pub game_db: PathBuf,
    pub secret: Vec<u8>,
    pub invite_code: String,
    pub always_open: bool,
    pub no_loops: bool,
}

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

fn err<E: std::fmt::Display>(ctx: &str) -> impl FnOnce(E) -> ConfigError + '_ {
    move |e| ConfigError(format!("{ctx}: {e}"))
}

/// Repository root: `ANISTOCK_ROOT`, else the first ancestor of the working directory (then of this
/// crate's source directory) that contains `server/game.toml`.
pub fn find_root() -> PathBuf {
    if let Ok(r) = std::env::var("ANISTOCK_ROOT") {
        if !r.trim().is_empty() {
            return PathBuf::from(r);
        }
    }
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for s in starts {
        for dir in s.ancestors() {
            if dir.join("server").join("game.toml").is_file() {
                return dir.to_path_buf();
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn set_dotted(table: &mut toml::Table, path: &str, value: toml::Value) -> Result<(), ConfigError> {
    let parts: Vec<&str> = path.split('.').collect();
    let (key, secs) = parts.split_last().unwrap();
    let mut t = table;
    for s in secs {
        t = t
            .get_mut(*s)
            .and_then(|v| v.as_table_mut())
            .ok_or_else(|| ConfigError(format!("override {path}: no section {s}")))?;
    }
    t.insert((*key).to_string(), value);
    Ok(())
}

impl Config {
    /// Load from the repository root found by `find_root`, with environment overrides.
    pub fn load() -> Result<Config, ConfigError> {
        Self::load_from(&find_root(), |k| std::env::var(k).ok())
    }

    /// Load with an explicit root and environment lookup (tests pass a closure).
    pub fn load_from(root: &Path, env: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let game_toml = root.join("server").join("game.toml");
        let text = std::fs::read_to_string(&game_toml).map_err(err(&game_toml.display().to_string()))?;
        let raw: toml::Table = toml::from_str(&text).map_err(err("server/game.toml"))?;
        let base = raw
            .get("base_params")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ConfigError("game.toml: base_params missing".into()))?;
        let base_path = root.join(base);
        let ptext = std::fs::read_to_string(&base_path).map_err(err(&base_path.display().to_string()))?;
        let mut params: toml::Table = toml::from_str(&ptext).map_err(err(base))?;
        if let Some(ov) = raw.get("overrides").and_then(|v| v.as_table()) {
            for (k, v) in ov {
                set_dotted(&mut params, k, v.clone())?;
            }
        }
        let game_raw = raw
            .get("game")
            .and_then(|v| v.as_table())
            .cloned()
            .ok_or_else(|| ConfigError("game.toml: [game] missing".into()))?;
        let game: GameRules = toml::Value::Table(game_raw.clone()).try_into().map_err(err("[game]"))?;
        let npc: NpcConfig = raw
            .get("npc")
            .cloned()
            .ok_or_else(|| ConfigError("game.toml: [npc] missing".into()))?
            .try_into()
            .map_err(err("[npc]"))?;
        let market = crate::market::MarketConfig::from_table(raw.get("market")).map_err(ConfigError)?;
        let tz: Tz = game.timezone.parse().map_err(|e| ConfigError(format!("timezone: {e}")))?;

        let secret = env("ANISTOCK_SECRET").unwrap_or_default().trim().to_string();
        let invite = env("ANISTOCK_INVITE").unwrap_or_default().trim().to_string();
        for (name, v) in [("ANISTOCK_SECRET", &secret), ("ANISTOCK_INVITE", &invite)] {
            if v.starts_with('#') {
                let head: String = v.chars().take(20).collect();
                return Err(ConfigError(format!(
                    "{name} looks like a comment ({head}...): put comments on their own line in .env"
                )));
            }
        }
        let path_env = |k: &str, default: PathBuf| env(k).map(PathBuf::from).unwrap_or(default);
        Ok(Config {
            root: root.to_path_buf(),
            params,
            npc,
            market,
            tz,
            collector_db: path_env("ANISTOCK_DB", root.join("data").join("anistock.db")),
            game_db: path_env("ANISTOCK_GAME_DB", root.join("data").join("game.db")),
            secret: if secret.is_empty() { b"dev-only-secret-change-me".to_vec() } else { secret.into_bytes() },
            invite_code: if invite.is_empty() { "anistock".into() } else { invite },
            always_open: env("ANISTOCK_ALWAYS_OPEN").as_deref() == Some("1"),
            no_loops: env("ANISTOCK_NO_LOOPS").as_deref() == Some("1"),
            game,
            game_raw,
        })
    }

    pub fn fees(&self) -> Fees {
        Fees { commission: self.game.commission, stamp_duty: self.game.stamp_duty }
    }

    pub fn rules(&self) -> Rules {
        Rules {
            vcm_threshold: self.game.vcm_threshold,
            vcm_window_min: self.game.vcm_window_min,
            vcm_cool_min: self.game.vcm_cool_min,
            ..Rules::default()
        }
    }

    /// A parameter by dotted path, e.g. `param("company.shares")`.
    pub fn param(&self, path: &str) -> Option<&toml::Value> {
        let mut parts = path.split('.');
        let mut v = self.params.get(parts.next()?)?;
        for p in parts {
            v = v.as_table()?.get(p)?;
        }
        Some(v)
    }
}

/// Trading phase, like v1 `app.phase()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// 集合竞价: limit orders rest without matching.
    Auction,
    Continuous,
    Closed,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Auction => "auction",
            Phase::Continuous => "continuous",
            Phase::Closed => "closed",
        }
    }
}

/// Wall clock in game time. Tests and replays pin it with `freeze`; clones share the frozen time.
#[derive(Clone)]
pub struct Clock {
    pub tz: Tz,
    frozen: Arc<Mutex<Option<DateTime<Tz>>>>,
}

impl Clock {
    pub fn new(tz: Tz) -> Clock {
        Clock { tz, frozen: Arc::new(Mutex::new(None)) }
    }

    /// A clock frozen at an ISO timestamp (any offset; converted to `tz`).
    pub fn fixed(tz: Tz, iso: &str) -> Clock {
        let c = Clock::new(tz);
        let dt = DateTime::parse_from_rfc3339(iso).expect("fixed clock: bad timestamp");
        c.freeze(Some(dt.with_timezone(&tz)));
        c
    }

    pub fn now(&self) -> DateTime<Tz> {
        if let Some(t) = *self.frozen.lock().unwrap() {
            return t;
        }
        Utc::now().with_timezone(&self.tz)
    }

    pub fn freeze(&self, when: Option<DateTime<Tz>>) {
        *self.frozen.lock().unwrap() = when;
    }

    /// Advance a frozen clock (no-op when running).
    pub fn advance(&self, d: chrono::Duration) {
        let mut f = self.frozen.lock().unwrap();
        if let Some(t) = *f {
            *f = Some(t + d);
        }
    }

    /// `YYYY-MM-DD`, like `Clock.today()`.
    pub fn today(&self) -> String {
        self.now().date_naive().format("%Y-%m-%d").to_string()
    }

    /// `2026-11-05T10:00:00+08:00`, byte-identical to Python's `isoformat(timespec="seconds")`.
    pub fn ts(&self) -> String {
        format_ts(&self.now())
    }

    pub fn phase(&self, game: &GameRules, always_open: bool) -> Phase {
        if always_open {
            return Phase::Continuous;
        }
        let t = self.now().format("%H:%M").to_string();
        if game.auction_start.as_str() <= t.as_str() && t.as_str() < game.trading_open.as_str() {
            Phase::Auction
        } else if game.trading_open.as_str() <= t.as_str() && t.as_str() < game.trading_close.as_str() {
            Phase::Continuous
        } else {
            Phase::Closed
        }
    }

    /// Like `Clock.trading`: between trading_open and trading_close.
    pub fn trading(&self, game: &GameRules) -> bool {
        let t = self.now().time();
        hm(&game.trading_open) <= t && t < hm(&game.trading_close)
    }
}

pub fn hm(s: &str) -> NaiveTime {
    let (h, m) = s.split_once(':').expect("HH:MM");
    NaiveTime::from_hms_opt(h.trim().parse().unwrap(), m.trim().parse().unwrap(), 0).unwrap()
}

/// Next time of day `at` strictly after `now` (v1 `next_daily_step`).
pub fn next_daily_step(now: &DateTime<Tz>, at: &str) -> DateTime<Tz> {
    let t = hm(at);
    let tz = now.timezone();
    let mk = |d: chrono::NaiveDate| tz.from_local_datetime(&d.and_time(t)).earliest().unwrap();
    let candidate = mk(now.date_naive());
    if candidate > *now {
        candidate
    } else {
        mk(now.date_naive() + chrono::Duration::days(1))
    }
}

/// The day before `day` (`YYYY-MM-DD`).
pub fn prev_day(day: &str) -> String {
    let d = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").expect("bad day");
    (d - chrono::Duration::days(1)).format("%Y-%m-%d").to_string()
}

pub fn format_ts<T: TimeZone>(dt: &DateTime<T>) -> String
where
    T::Offset: std::fmt::Display,
{
    dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

/// `datetime.fromisoformat(ts) + timedelta(minutes=m)` then `.isoformat(timespec="seconds")`:
/// keeps the offset of the input (or none, for a naive timestamp).
pub fn ts_add_minutes(ts: &str, minutes: i64) -> String {
    let d = chrono::Duration::minutes(minutes);
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return format_ts(&(dt + d));
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(n) = chrono::NaiveDateTime::parse_from_str(ts, f) {
            return (n + d).format("%Y-%m-%dT%H:%M:%S").to_string();
        }
    }
    for f in ["%Y-%m-%dT%H:%M:%S%.f%:z", "%Y-%m-%d %H:%M:%S%.f%:z", "%Y-%m-%dT%H:%M%:z"] {
        if let Ok(dt) = DateTime::parse_from_str(ts, f) {
            return format_ts(&(dt + d));
        }
    }
    panic!("unparseable timestamp {ts:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_format_matches_python() {
        assert_eq!(ts_add_minutes("2026-11-05T10:00:00+08:00", -5), "2026-11-05T09:55:00+08:00");
        assert_eq!(ts_add_minutes("2026-11-05T23:58:30+08:00", 5), "2026-11-06T00:03:30+08:00");
        assert_eq!(ts_add_minutes("2026-11-05T10:00:00", 5), "2026-11-05T10:05:00");
        assert_eq!(ts_add_minutes("2026-11-05T10:00:00Z", 5), "2026-11-05T10:05:00+00:00");
        let c = Clock::fixed(chrono_tz::Asia::Shanghai, "2026-11-05T02:00:00.700Z");
        assert_eq!(c.ts(), "2026-11-05T10:00:00+08:00");
        assert_eq!(c.today(), "2026-11-05");
        assert_eq!(prev_day("2026-03-01"), "2026-02-28");
    }

    #[test]
    fn loads_repo_config() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let cfg = Config::load_from(&root, |k| (k == "ANISTOCK_GAME_DB").then(|| "/x/g.db".to_string())).unwrap();
        assert_eq!(cfg.game_db, PathBuf::from("/x/g.db"));
        assert_eq!(cfg.param("company.shares").and_then(|v| v.as_integer()), Some(10000));
        assert_eq!(cfg.game.commission, 0.00025);
        assert_eq!(cfg.tz, chrono_tz::Asia::Shanghai);
        assert!(Config::load_from(&root, |k| (k == "ANISTOCK_SECRET").then(|| "# x".to_string())).is_err());
    }
}
