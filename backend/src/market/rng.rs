//! Deterministic randomness. Beliefs are pure functions of (seed, agent, company, day) so nothing about
//! them is stored; the agents' action times and choices use one seeded generator.

/// SplitMix64 finalizer.
pub fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Hash of a tuple of integers.
pub fn hash(keys: &[u64]) -> u64 {
    let mut h = 0x243f_6a88_85a3_08d3u64;
    for k in keys {
        h = mix(h ^ k);
    }
    h
}

/// Uniform in (0, 1) from a hash.
pub fn unit(h: u64) -> f64 {
    ((h >> 11) as f64 + 0.5) / (1u64 << 53) as f64
}

/// Standard normal from a tuple (Box–Muller on two derived uniforms).
pub fn normal(keys: &[u64]) -> f64 {
    let h = hash(keys);
    let u1 = unit(h);
    let u2 = unit(mix(h ^ 0x5851_f42d_4c95_7f2d));
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// A small, fast, seedable generator (xoshiro256**), fully deterministic across platforms.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut x = seed;
        let mut s = [0u64; 4];
        for v in &mut s {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            *v = mix(x);
        }
        Rng { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let r = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        r
    }

    /// Uniform in (0, 1).
    pub fn f64(&mut self) -> f64 {
        unit(self.next_u64())
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }

    pub fn below(&mut self, n: usize) -> usize {
        ((self.f64() * n as f64) as usize).min(n.saturating_sub(1))
    }

    /// Exponential waiting time with the given rate.
    pub fn exp(&mut self, rate: f64) -> f64 {
        -self.f64().ln() / rate
    }

    pub fn normal(&mut self) -> f64 {
        (-2.0 * self.f64().ln()).sqrt() * (std::f64::consts::TAU * self.f64()).cos()
    }

    /// Index drawn with probability ∝ weights (all >= 0, some > 0).
    pub fn weighted(&mut self, w: &[f64]) -> Option<usize> {
        let total: f64 = w.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut x = self.f64() * total;
        for (i, v) in w.iter().enumerate() {
            if x < *v {
                return Some(i);
            }
            x -= v;
        }
        Some(w.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_moments() {
        let n = 200_000;
        let xs: Vec<f64> = (0..n).map(|i| normal(&[7, i])).collect();
        let m = xs.iter().sum::<f64>() / n as f64;
        let v = xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n as f64;
        assert!(m.abs() < 0.01 && (v - 1.0).abs() < 0.02, "{m} {v}");
        let mut r = Rng::new(1);
        let mean_exp = (0..n).map(|_| r.exp(2.0)).sum::<f64>() / n as f64;
        assert!((mean_exp - 0.5).abs() < 0.005);
    }
}
