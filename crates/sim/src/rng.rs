//! A small seeded generator, so a saved scene simulates the same way on every machine.

use glam::Vec3;

/// SplitMix64: the same seed gives the same sequence on every platform, and nearby seeds give
/// unrelated sequences. Not for anything secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0xD1B5_4A32_D192_ED03)
    }

    /// An independent generator for `stream` (one per emitter, per body…), derived from this
    /// one's seed without advancing it.
    pub fn fork(&self, stream: u64) -> Rng {
        let mut r = Rng(self.0 ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform in `0..n` (zero for `n == 0`).
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { (((self.next_u64() >> 32) * u64::from(n)) >> 32) as u32 }
    }

    /// A direction, uniform over the sphere.
    pub fn unit_vec3(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let a = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * a.cos(), r * a.sin(), z)
    }

    /// A point uniform inside the unit ball.
    pub fn in_unit_ball(&mut self) -> Vec3 {
        self.unit_vec3() * self.f32().cbrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence_and_streams_differ() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        assert_eq!(xs, (0..8).map(|_| b.next_u64()).collect::<Vec<_>>());
        assert_ne!(Rng::new(8).next_u64(), xs[0]);
        let base = Rng::new(7);
        assert_ne!(base.fork(1).next_u64(), base.fork(2).next_u64());
        assert_eq!(base.fork(1).next_u64(), base.fork(1).next_u64());
    }

    #[test]
    fn values_stay_in_range() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let f = r.f32();
            assert!((0.0..1.0).contains(&f));
            assert!(r.below(5) < 5);
            assert!((r.unit_vec3().length() - 1.0).abs() < 1e-4);
            assert!(r.in_unit_ball().length() <= 1.0001);
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn the_sphere_is_roughly_even() {
        let mut r = Rng::new(3);
        let mean: Vec3 = (0..20_000).map(|_| r.unit_vec3()).sum::<Vec3>() / 20_000.0;
        assert!(mean.length() < 0.03, "{mean:?}");
    }
}
