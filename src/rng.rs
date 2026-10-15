//! Small deterministic RNG (SplitMix64) so a race replays identically from
//! its seed, which keeps tests and later career saves reproducible.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// Normal distribution via Box–Muller.
    pub fn normal(&mut self, mean: f32, std: f32) -> f32 {
        let u1 = self.f32().max(1e-7);
        let u2 = self.f32();
        mean + std * (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() % items.len() as u64) as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert!((0..100).all(|_| a.next_u64() == b.next_u64()));
    }

    #[test]
    fn normal_has_expected_mean_and_std() {
        let mut r = Rng::new(1);
        let xs: Vec<f32> = (0..20_000).map(|_| r.normal(4.0, 0.3)).collect();
        let mean = xs.iter().sum::<f32>() / xs.len() as f32;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / xs.len() as f32;
        assert!((mean - 4.0).abs() < 0.01, "{mean}");
        assert!((var.sqrt() - 0.3).abs() < 0.01, "{}", var.sqrt());
    }
}
