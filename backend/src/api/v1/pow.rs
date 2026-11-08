//! Proof of work for register and login (docs/design/account.md §3.2, docs/api-v1.md §3.4).
//!
//! ALTCHA-style: the server picks `n` in `[0, max)` and sends `target = PBKDF2-SHA-256(n, salt, cost)`;
//! the browser tries `0, 1, 2, …` until it finds `n` again. The work is bounded by `max` (no unlucky
//! tail), and one PBKDF2 call runs `cost` rounds inside the browser's native code, so a native solver
//! gains little over `crypto.subtle`. Challenges are signed (HMAC), not stored; a used salt is kept in
//! memory until the challenge expires, and each challenge gets one try.
//!
//! Register always needs a solution (unless `register_max = 0`). Login needs one only while the
//! site-wide rate of register + login attempts is over `surge_per_min` (and for `cooldown_secs` after),
//! when register's `max` is also multiplied by `surge_factor`.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

pub const ALGORITHM: &str = "PBKDF2-SHA-256";

#[derive(Clone, Debug)]
pub struct PowConfig {
    /// PBKDF2 iterations per try
    pub cost: u32,
    /// register: `n` is drawn from `[0, register_max)`; 0 turns the proof of work off
    pub register_max: u64,
    /// login while surging
    pub login_max: u64,
    /// register + login attempts per minute, site-wide, above which the site is surging
    pub surge_per_min: usize,
    /// register's `max` multiplier while surging
    pub surge_factor: u64,
    pub cooldown_secs: u64,
    pub expires_secs: i64,
}

impl PowConfig {
    pub fn from_config(cfg: &crate::config::Config) -> PowConfig {
        let f = |k: &str, d: f64| cfg.api_f64(&format!("pow.{k}"), d);
        PowConfig {
            cost: f("cost", 5000.0) as u32,
            register_max: f("register_max", 1000.0) as u64,
            login_max: f("login_max", 1000.0) as u64,
            surge_per_min: f("surge_per_min", 60.0) as usize,
            surge_factor: f("surge_factor", 4.0) as u64,
            cooldown_secs: f("cooldown_secs", 600.0) as u64,
            expires_secs: f("expires_secs", 300.0) as i64,
        }
    }
}

/// 验证题目：原样随 `pow.challenge` 交回
#[derive(Serialize, Deserialize, ToSchema, Clone, Debug, PartialEq)]
pub struct PowChallenge {
    /// 固定为 `PBKDF2-SHA-256`
    pub algorithm: String,
    /// `register` 或 `login`
    pub purpose: String,
    pub salt: String,
    /// PBKDF2 迭代次数
    pub cost: u32,
    /// 答案在 `[0, max)` 之内
    pub max: u64,
    /// PBKDF2-SHA-256(password = 答案的十进制字符串, salt = `salt` 的 UTF-8 字节, iterations = `cost`, 32 字节) 的十六进制
    pub target: String,
    /// 过期时间（Unix 秒）
    pub expires: i64,
    pub signature: String,
}

/// 验证答案
#[derive(Serialize, Deserialize, ToSchema, Clone, Debug)]
pub struct PowSolution {
    pub challenge: PowChallenge,
    pub number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowError {
    /// wrong algorithm or purpose, bad signature, `number >= max`, wrong answer
    Invalid,
    Expired,
    /// this challenge has been tried already
    Used,
}

/// PBKDF2-HMAC-SHA-256 with a 32-byte output (one block).
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mac = <Hmac<Sha256>>::new_from_slice(password).expect("hmac accepts any key length");
    let mut m = mac.clone();
    m.update(salt);
    m.update(&1u32.to_be_bytes());
    let mut u: [u8; 32] = m.finalize().into_bytes().into();
    let mut t = u;
    for _ in 1..iterations.max(1) {
        let mut m = mac.clone();
        m.update(&u);
        u = m.finalize().into_bytes().into();
        for (a, b) in t.iter_mut().zip(u) {
            *a ^= b;
        }
    }
    t
}

fn hash_of(number: u64, salt: &str, cost: u32) -> String {
    hex::encode(pbkdf2_sha256(number.to_string().as_bytes(), salt.as_bytes(), cost))
}

fn sign(secret: &[u8], c: &PowChallenge) -> String {
    let mut m = <Hmac<Sha256>>::new_from_slice(secret).expect("hmac accepts any key length");
    let payload =
        format!("pow|{}|{}|{}|{}|{}|{}|{}", c.algorithm, c.purpose, c.salt, c.cost, c.max, c.target, c.expires);
    m.update(payload.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

/// Find the answer (tests, bots and the reference for the browser worker).
pub fn solve(c: &PowChallenge) -> Option<u64> {
    (0..c.max).find(|n| hash_of(*n, &c.salt, c.cost) == c.target)
}

#[derive(Default)]
struct Inner {
    /// salt -> expires
    used: HashMap<String, i64>,
    /// register / login attempts in the last minute
    window: VecDeque<Instant>,
    surge_until: Option<Instant>,
}

pub struct Pow {
    pub cfg: PowConfig,
    inner: Mutex<Inner>,
}

impl Pow {
    pub fn new(cfg: PowConfig) -> Pow {
        Pow { cfg, inner: Mutex::new(Inner::default()) }
    }

    /// One register or login attempt (counted before rate limiting: floods are what this measures).
    pub fn note_attempt(&self) {
        let now = Instant::now();
        let mut g = self.inner.lock().unwrap();
        g.window.push_back(now);
        while g.window.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            g.window.pop_front();
        }
        if g.window.len() > self.cfg.surge_per_min {
            g.surge_until = Some(now + Duration::from_secs(self.cfg.cooldown_secs));
        }
    }

    pub fn surging(&self) -> bool {
        self.inner.lock().unwrap().surge_until.is_some_and(|t| Instant::now() < t)
    }

    /// `max` for a new challenge; 0 = no proof of work needed.
    pub fn max_for(&self, purpose: &str) -> u64 {
        let surge = self.surging();
        match purpose {
            "register" if surge => self.cfg.register_max.saturating_mul(self.cfg.surge_factor),
            "register" => self.cfg.register_max,
            "login" if surge => self.cfg.login_max,
            _ => 0,
        }
    }

    /// A new challenge, or `None` when this purpose needs no proof of work right now.
    pub fn issue(&self, secret: &[u8], purpose: &str, now: i64) -> Option<PowChallenge> {
        let max = self.max_for(purpose);
        if max == 0 {
            return None;
        }
        let salt = hex::encode(rand::random::<[u8; 16]>());
        let n = rand::random::<u64>() % max;
        let mut c = PowChallenge {
            algorithm: ALGORITHM.into(),
            purpose: purpose.into(),
            target: hash_of(n, &salt, self.cfg.cost),
            salt,
            cost: self.cfg.cost,
            max,
            expires: now + self.cfg.expires_secs,
            signature: String::new(),
        };
        c.signature = sign(secret, &c);
        Some(c)
    }

    /// Check a solution for `purpose` at `now` (Unix seconds). A challenge is spent by its first try.
    pub fn verify(&self, secret: &[u8], purpose: &str, s: &PowSolution, now: i64) -> Result<(), PowError> {
        let c = &s.challenge;
        if c.algorithm != ALGORITHM || c.purpose != purpose || c.cost == 0 {
            return Err(PowError::Invalid);
        }
        if !bool::from(sign(secret, c).as_bytes().ct_eq(c.signature.as_bytes())) {
            return Err(PowError::Invalid);
        }
        if c.expires < now {
            return Err(PowError::Expired);
        }
        if s.number >= c.max {
            return Err(PowError::Invalid);
        }
        {
            let mut g = self.inner.lock().unwrap();
            g.used.retain(|_, exp| *exp >= now);
            if g.used.insert(c.salt.clone(), c.expires).is_some() {
                return Err(PowError::Used);
            }
        }
        if hash_of(s.number, &c.salt, c.cost) != c.target {
            return Err(PowError::Invalid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max: u64) -> PowConfig {
        PowConfig {
            cost: 3,
            register_max: max,
            login_max: max,
            surge_per_min: 5,
            surge_factor: 4,
            cooldown_secs: 600,
            expires_secs: 300,
        }
    }

    #[test]
    fn pbkdf2_matches_known_vectors() {
        // RFC 7914 §11 style vectors for PBKDF2-HMAC-SHA-256 (dkLen 32)
        assert_eq!(
            hex::encode(pbkdf2_sha256(b"password", b"salt", 1)),
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"
        );
        assert_eq!(
            hex::encode(pbkdf2_sha256(b"password", b"salt", 4096)),
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        );
    }

    #[test]
    fn solve_verify_and_replay() {
        let p = Pow::new(cfg(50));
        let c = p.issue(b"k", "register", 1000).unwrap();
        let n = solve(&c).unwrap();
        let s = PowSolution { challenge: c.clone(), number: n };
        assert_eq!(p.verify(b"k", "login", &s, 1000), Err(PowError::Invalid));
        assert_eq!(p.verify(b"other", "register", &s, 1000), Err(PowError::Invalid));
        assert_eq!(p.verify(b"k", "register", &s, 1000 + 301), Err(PowError::Expired));
        assert_eq!(p.verify(b"k", "register", &s, 1000), Ok(()));
        assert_eq!(p.verify(b"k", "register", &s, 1000), Err(PowError::Used));
        // a tampered max breaks the signature
        let mut t = c.clone();
        t.max = 1_000_000;
        let s = PowSolution { challenge: t, number: n };
        assert_eq!(p.verify(b"k", "register", &s, 1000), Err(PowError::Invalid));
        // a wrong answer spends the challenge
        let c = p.issue(b"k", "register", 1000).unwrap();
        let wrong = (solve(&c).unwrap() + 1) % c.max;
        let s = PowSolution { challenge: c.clone(), number: wrong };
        assert_eq!(p.verify(b"k", "register", &s, 1000), Err(PowError::Invalid));
        let s = PowSolution { challenge: c.clone(), number: solve(&c).unwrap() };
        assert_eq!(p.verify(b"k", "register", &s, 1000), Err(PowError::Used));
    }

    #[test]
    fn login_needs_work_only_while_surging() {
        let p = Pow::new(cfg(10));
        assert!(p.issue(b"k", "login", 0).is_none());
        assert_eq!(p.max_for("register"), 10);
        for _ in 0..6 {
            p.note_attempt();
        }
        assert!(p.surging());
        assert_eq!(p.max_for("login"), 10);
        assert_eq!(p.max_for("register"), 40);
        assert!(p.issue(b"k", "login", 0).is_some());
        assert!(Pow::new(cfg(0)).issue(b"k", "register", 0).is_none());
    }
}
