//! A bit-for-bit port of CPython's `random.Random` (MT19937) for the parts v1 uses: integer seeding,
//! `random()`, `getrandbits(k <= 32)`, `choice`, `uniform`, `gauss`, and weighted `choices(k=1)`.
//!
//! v1 draws random numbers in the IPO ballot (`random.Random(company_id).choices`), in the market maker's
//! daily noise (`random.Random(int(sha256(...)[:12], 16)).gauss`) and in the noise traders (a shared
//! `random.Random`). Reproducing the generator makes the Rust results identical to v1's for the same seed.

const N: usize = 624;
const M: usize = 397;

#[derive(Clone)]
pub struct PyRandom {
    mt: [u32; N],
    mti: usize,
    gauss_next: Option<f64>,
}

impl PyRandom {
    /// `random.Random(seed)` for an integer seed (the absolute value is used, like CPython).
    pub fn new(seed: i128) -> PyRandom {
        let mut a = seed.unsigned_abs();
        let mut key = Vec::new();
        while a > 0 {
            key.push((a & 0xffff_ffff) as u32);
            a >>= 32;
        }
        if key.is_empty() {
            key.push(0);
        }
        let mut r = PyRandom { mt: [0; N], mti: N + 1, gauss_next: None };
        r.init_by_array(&key);
        r
    }

    /// Seeded from the OS clock and address entropy (production NPC loop; v1 uses `random.Random()`).
    pub fn from_entropy() -> PyRandom {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        PyRandom::new(h.finish() as i128)
    }

    fn init_genrand(&mut self, s: u32) {
        self.mt[0] = s;
        for i in 1..N {
            self.mt[i] = 1812433253u32.wrapping_mul(self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_add(i as u32);
        }
        self.mti = N;
    }

    fn init_by_array(&mut self, key: &[u32]) {
        self.init_genrand(19650218);
        let (mut i, mut j) = (1usize, 0usize);
        let mut k = N.max(key.len());
        while k > 0 {
            self.mt[i] = (self.mt[i] ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1664525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
            k -= 1;
        }
        k = N - 1;
        while k > 0 {
            self.mt[i] = (self.mt[i] ^ (self.mt[i - 1] ^ (self.mt[i - 1] >> 30)).wrapping_mul(1566083941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                self.mt[0] = self.mt[N - 1];
                i = 1;
            }
            k -= 1;
        }
        self.mt[0] = 0x8000_0000;
    }

    pub fn genrand_u32(&mut self) -> u32 {
        const MAG01: [u32; 2] = [0, 0x9908_b0df];
        if self.mti >= N {
            for kk in 0..N - M {
                let y = (self.mt[kk] & 0x8000_0000) | (self.mt[kk + 1] & 0x7fff_ffff);
                self.mt[kk] = self.mt[kk + M] ^ (y >> 1) ^ MAG01[(y & 1) as usize];
            }
            for kk in N - M..N - 1 {
                let y = (self.mt[kk] & 0x8000_0000) | (self.mt[kk + 1] & 0x7fff_ffff);
                self.mt[kk] = self.mt[kk + M - N] ^ (y >> 1) ^ MAG01[(y & 1) as usize];
            }
            let y = (self.mt[N - 1] & 0x8000_0000) | (self.mt[0] & 0x7fff_ffff);
            self.mt[N - 1] = self.mt[M - 1] ^ (y >> 1) ^ MAG01[(y & 1) as usize];
            self.mti = 0;
        }
        let mut y = self.mt[self.mti];
        self.mti += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// `random()`: 53-bit float in [0, 1).
    pub fn random(&mut self) -> f64 {
        let a = (self.genrand_u32() >> 5) as f64;
        let b = (self.genrand_u32() >> 6) as f64;
        (a * 67108864.0 + b) * (1.0 / 9007199254740992.0)
    }

    /// `getrandbits(k)` for 0 <= k <= 32.
    pub fn getrandbits(&mut self, k: u32) -> u32 {
        assert!(k <= 32, "getrandbits: k > 32 not ported");
        if k == 0 {
            return 0;
        }
        self.genrand_u32() >> (32 - k)
    }

    /// `_randbelow(n)` (n > 0, n < 2^32).
    pub fn randbelow(&mut self, n: usize) -> usize {
        assert!(n > 0 && (n as u64) < (1u64 << 32));
        let k = usize::BITS - n.leading_zeros();
        loop {
            let r = self.getrandbits(k) as usize;
            if r < n {
                return r;
            }
        }
    }

    /// Index chosen by `choice(seq)` for a sequence of length `n` (> 0).
    pub fn choice_index(&mut self, n: usize) -> usize {
        self.randbelow(n)
    }

    /// `uniform(a, b)` = `a + (b - a) * random()`.
    pub fn uniform(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.random()
    }

    /// `gauss(mu, sigma)`, including the cached second value.
    pub fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        let z = match self.gauss_next.take() {
            Some(z) => z,
            None => {
                let x2pi = self.random() * std::f64::consts::TAU;
                let g2rad = (-2.0 * (1.0 - self.random()).ln()).sqrt();
                self.gauss_next = Some(x2pi.sin() * g2rad);
                x2pi.cos() * g2rad
            }
        };
        mu + z * sigma
    }

    /// Index chosen by `choices(population, weights=weights)[0]` (integer weights).
    pub fn choices_index(&mut self, weights: &[i64]) -> usize {
        let mut cum = Vec::with_capacity(weights.len());
        let mut acc: i64 = 0;
        for w in weights {
            acc += w;
            cum.push(acc);
        }
        let total = *cum.last().expect("choices: empty population") as f64;
        assert!(total > 0.0, "Total of weights must be greater than zero");
        let x = self.random() * total;
        // bisect_right(cum, x, 0, n - 1)
        let (mut lo, mut hi) = (0usize, cum.len() - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if x < cum[mid] as f64 {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        lo
    }
}

/// `int(hashlib.sha256(text.encode()).hexdigest()[:12], 16)`
pub fn sha256_prefix48(text: &str) -> i128 {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(text.as_bytes());
    let mut v: i128 = 0;
    for b in &d[..6] {
        v = (v << 8) | *b as i128;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    // reference values printed by CPython 3.14 (generated into pyrandom_ref.rs)
    #[test]
    fn matches_cpython() {
        let mut r = PyRandom::new(42);
        assert_eq!(r.random(), 0.6394267984578837);
        assert_eq!(r.random(), 0.025010755222666936);
        let mut r = PyRandom::new(0);
        assert_eq!(r.random(), 0.8444218515250481);
        let mut r = PyRandom::new(2026);
        let picks: Vec<usize> = (0..6).map(|_| r.choice_index(141)).collect();
        assert_eq!(picks, PY_CHOICE_141);
        let mut r = PyRandom::new(7);
        assert_eq!(r.gauss(0.0, 1.0).to_bits(), PY_GAUSS_7.0.to_bits());
        assert_eq!(r.gauss(0.0, 1.0).to_bits(), PY_GAUSS_7.1.to_bits());
        let mut r = PyRandom::new(1 << 40);
        assert_eq!(r.uniform(0.01, 0.05).to_bits(), PY_UNIFORM_2_40.to_bits());
        let mut r = PyRandom::new(5);
        let w = [10, 0, 7, 3];
        let picks: Vec<usize> = (0..8).map(|_| r.choices_index(&w)).collect();
        assert_eq!(picks, PY_CHOICES_5);
        assert_eq!(sha256_prefix48("171018:2026-11-05"), PY_SHA_PREFIX);
        let mut r = PyRandom::new(PY_SHA_PREFIX);
        assert_eq!(r.gauss(0.0, 0.05).to_bits(), PY_DAILY_NOISE.to_bits());
    }

    include!("pyrandom_ref.rs");
}
