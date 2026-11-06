//! Token buckets per subject (docs/api-v1.md §2.12), configured in `[api.rate_limit]` of game.toml.

use std::collections::HashMap;
use std::time::Instant;

use axum::http::{HeaderMap, HeaderValue};

#[derive(Clone, Debug)]
pub struct Limits {
    pub player: (f64, f64),
    pub bot: (f64, f64),
    pub anon: (f64, f64),
    /// login / register per IP: (per second, burst)
    pub auth: (f64, f64),
    /// registrations per IP (on top of `auth`): (per second, burst = per day)
    pub register: (f64, f64),
    /// bind-code redemptions per QQ number
    pub bind: (f64, f64),
    pub ws_per_ip: usize,
    /// concurrent WebSocket connections opened with API keys, per account
    pub ws_per_account: usize,
    /// concurrent WebSocket connections, site-wide
    pub ws_total: usize,
    /// password hashes (scrypt) computed at the same time
    pub hash_concurrency: usize,
}

impl Limits {
    pub fn from_config(cfg: &crate::config::Config) -> Limits {
        // development and tests only: ANISTOCK_NO_RATE_LIMIT=1 lifts every limit
        let off = std::env::var("ANISTOCK_NO_RATE_LIMIT").as_deref() == Ok("1");
        let f = |k: &str, d: f64| if off { 1e9 } else { cfg.api_f64(&format!("rate_limit.{k}"), d) };
        let auth = f("auth_per_min", 10.0);
        let bind = f("bind_per_hour", 5.0);
        let register = f("register_per_ip_day", 20.0);
        let auth = if off { 1e9 * 60.0 } else { auth };
        let bind = if off { 1e9 * 3600.0 } else { bind };
        let register = if off { 1e9 * 86400.0 } else { register };
        Limits {
            player: (f("player_rps", 10.0), f("player_burst", 20.0)),
            bot: (f("bot_rps", 30.0), f("bot_burst", 60.0)),
            anon: (f("anon_rps", 10.0), f("anon_burst", 20.0)),
            auth: (auth / 60.0, auth),
            bind: (bind / 3600.0, bind),
            register: (register / 86400.0, register),
            ws_per_ip: f("ws_per_ip", 32.0) as usize,
            ws_per_account: f("ws_per_account", 3.0) as usize,
            ws_total: f("ws_total", 2000.0) as usize,
            hash_concurrency: (cfg.api_f64("rate_limit.hash_concurrency", 2.0) as usize).max(1),
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    Player(i64),
    Bot(String),
    Anon(String),
    Auth(String),
    Register(String),
    Bind(String),
}

/// The per-IP limit key: an IPv4 address as is, an IPv6 address by its /64 (one subscriber usually holds
/// a whole /64 and can rotate addresses inside it; an IPv4-mapped address counts as IPv4).
pub fn ip_key(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
        Ok(a) => a.to_string(),
        Err(_) => ip.to_string(),
    }
}

#[derive(Default)]
pub struct Limiter {
    buckets: HashMap<Key, (f64, Instant)>,
    sweeps: u64,
}

pub struct Verdict {
    pub allowed: bool,
    pub limit: u64,
    pub remaining: u64,
    /// seconds until a token is available again (for `Retry-After` / `RateLimit-Reset`)
    pub reset: u64,
}

impl Verdict {
    pub fn headers(&self, h: &mut HeaderMap) {
        for (k, v) in [("ratelimit-limit", self.limit), ("ratelimit-remaining", self.remaining), ("ratelimit-reset", self.reset)] {
            if let Ok(v) = HeaderValue::from_str(&v.to_string()) {
                h.insert(k, v);
            }
        }
    }
}

impl Limiter {
    pub fn take(&mut self, limits: &Limits, key: Key) -> Verdict {
        self.step(limits, key, true)
    }

    /// Like `take`, without spending a token (a check now, `take` later if the request succeeds).
    pub fn peek(&mut self, limits: &Limits, key: Key) -> Verdict {
        self.step(limits, key, false)
    }

    fn step(&mut self, limits: &Limits, key: Key, spend: bool) -> Verdict {
        let (rate, burst) = match &key {
            Key::Player(_) => limits.player,
            Key::Bot(_) => limits.bot,
            Key::Anon(_) => limits.anon,
            Key::Auth(_) => limits.auth,
            Key::Register(_) => limits.register,
            Key::Bind(_) => limits.bind,
        };
        let now = Instant::now();
        self.sweeps += 1;
        if self.sweeps % 4096 == 0 {
            // forget buckets that have refilled
            self.buckets.retain(|_, (_, t)| now.duration_since(*t).as_secs() < 3600);
        }
        let b = self.buckets.entry(key).or_insert((burst, now));
        let elapsed = now.duration_since(b.1).as_secs_f64();
        b.0 = (b.0 + elapsed * rate).min(burst);
        b.1 = now;
        let allowed = b.0 >= 1.0;
        if allowed && spend {
            b.0 -= 1.0;
        }
        let missing = (1.0 - b.0).max(0.0);
        let reset = if rate > 0.0 { (missing / rate).ceil() as u64 } else { 0 };
        Verdict { allowed, limit: burst as u64, remaining: b.0.floor().max(0.0) as u64, reset }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bucket_allows_a_burst_then_limits() {
        let l = Limits {
            player: (10.0, 3.0),
            bot: (1.0, 1.0),
            anon: (1.0, 1.0),
            auth: (1.0, 1.0),
            register: (1.0, 1.0),
            bind: (1.0, 1.0),
            ws_per_ip: 8,
            ws_per_account: 3,
            ws_total: 100,
            hash_concurrency: 2,
        };
        let mut lim = Limiter::default();
        for _ in 0..3 {
            assert!(lim.take(&l, Key::Player(1)).allowed);
        }
        let v = lim.take(&l, Key::Player(1));
        assert!(!v.allowed);
        assert!(v.reset >= 1);
        assert!(lim.take(&l, Key::Player(2)).allowed);
    }

    #[test]
    fn peek_spends_nothing() {
        let l = Limits {
            player: (0.0, 2.0),
            bot: (1.0, 1.0),
            anon: (1.0, 1.0),
            auth: (1.0, 1.0),
            register: (1.0, 1.0),
            bind: (1.0, 1.0),
            ws_per_ip: 8,
            ws_per_account: 3,
            ws_total: 100,
            hash_concurrency: 2,
        };
        let mut lim = Limiter::default();
        for _ in 0..5 {
            assert!(lim.peek(&l, Key::Player(1)).allowed);
        }
        assert!(lim.take(&l, Key::Player(1)).allowed);
        assert!(lim.take(&l, Key::Player(1)).allowed);
        assert!(!lim.peek(&l, Key::Player(1)).allowed);
    }

    #[test]
    fn ipv6_counts_by_slash_64() {
        assert_eq!(ip_key("203.0.113.7"), "203.0.113.7");
        assert_eq!(ip_key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert_eq!(ip_key("2001:db8:1:2:bbbb:cccc:dddd:eeee"), "2001:db8:1:2::/64");
        assert_ne!(ip_key("2001:db8:1:3::1"), ip_key("2001:db8:1:2::1"));
        assert_eq!(ip_key("::ffff:203.0.113.7"), "203.0.113.7");
        assert_eq!(ip_key("local"), "local");
    }
}
