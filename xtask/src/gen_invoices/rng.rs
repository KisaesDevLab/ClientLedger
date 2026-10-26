//! A small seeded generator (`SplitMix64`) so every synthetic document is
//! the same on every machine and run. Not for anything but test data.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    /// A generator for one named stream below this seed (layouts, documents
    /// and scan variants each get their own, so adding one never shifts
    /// another's values).
    pub fn fork(&self, salt: &str) -> Rng {
        let mut h = self.0 ^ 0x9e37_79b9_7f4a_7c15;
        for b in salt.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let mut r = Rng(h);
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        let span = u64::try_from(hi - lo).unwrap_or(0) + 1;
        lo + i64::try_from(self.next_u64() % span).unwrap_or(0)
    }

    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % u64::try_from(n).unwrap_or(1)).unwrap_or(0)
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// True with probability `percent` / 100.
    pub fn chance(&mut self, percent: u32) -> bool {
        self.next_u64() % 100 < u64::from(percent)
    }

    /// Uniform in `[lo, hi)`, for image geometry and noise only (never money).
    pub fn unit_range(&mut self, lo: f64, hi: f64) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + (hi - lo) * u
    }

    /// Roughly normal noise with mean 0 and standard deviation 1.
    pub fn noise(&mut self) -> f64 {
        self.unit_range(-1.0, 1.0) + self.unit_range(-1.0, 1.0) + self.unit_range(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_values_and_forks_differ() {
        let a: Vec<u64> = (0..4).map(|_| Rng::new(7).fork("x").next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(
            Rng::new(7).fork("x").next_u64(),
            Rng::new(7).fork("y").next_u64()
        );
        let mut r = Rng::new(1);
        for _ in 0..1000 {
            let v = r.range(-3, 5);
            assert!((-3..=5).contains(&v));
        }
    }
}
