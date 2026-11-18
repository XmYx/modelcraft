//! Force fields the solvers share: wind with turbulence, from a smooth seeded noise.
//!
//! The noise is gradient-free value noise on an integer lattice (a hash of the cell and the seed,
//! smoothly interpolated), so a field is a pure function of position, time and seed: it needs no
//! state and gives the same air on every run. Turbulence is three decorrelated noise samples
//! forming a vector, drifting along the wind over time.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// A uniform wind with a turbulent part.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wind {
    /// Which way the air moves (any length; zero gives no wind).
    pub direction: [f32; 3],
    /// Air speed in metres per second.
    pub strength: f32,
    /// How much the speed varies, 0 (steady) to 1 (gusts as strong as the wind).
    pub turbulence: f32,
    /// The size of a gust in metres.
    pub scale: f32,
    /// How fast the gusts change, in gust-sizes per second.
    pub speed: f32,
}

impl Default for Wind {
    fn default() -> Self {
        Wind { direction: [1.0, 0.0, 0.0], strength: 0.0, turbulence: 0.0, scale: 2.0, speed: 0.5 }
    }
}

fn hash(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    let mut h =
        (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise in `[-1, 1]`.
pub fn noise3(p: Vec3, seed: u32) -> f32 {
    if !p.is_finite() {
        return 0.0;
    }
    let f = p.floor();
    let (x, y, z) = (f.x as i32, f.y as i32, f.z as i32);
    let (u, v, w) = (smooth(p.x - f.x), smooth(p.y - f.y), smooth(p.z - f.z));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx, dy, dz| hash(x.wrapping_add(dx), y.wrapping_add(dy), z.wrapping_add(dz), seed);
    l(l(l(c(0, 0, 0), c(1, 0, 0), u), l(c(0, 1, 0), c(1, 1, 0), u), v), l(l(c(0, 0, 1), c(1, 0, 1), u), l(c(0, 1, 1), c(1, 1, 1), u), v), w)
}

/// A smooth random vector field, each component in `[-1, 1]`.
pub fn vector_noise(p: Vec3, seed: u32) -> Vec3 {
    Vec3::new(noise3(p, seed), noise3(p + Vec3::new(31.4, 17.2, 5.9), seed ^ 0x55aa), noise3(p + Vec3::new(8.3, 41.7, 23.1), seed ^ 0xaa55))
}

impl Wind {
    /// Clamp every number into a usable range.
    pub fn sanitize(&mut self) {
        let fin = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        self.direction = self.direction.map(|x| fin(x, -1e3, 1e3, 0.0));
        self.strength = fin(self.strength, 0.0, 200.0, 0.0);
        self.turbulence = fin(self.turbulence, 0.0, 1.0, 0.0);
        self.scale = fin(self.scale, 0.01, 1e3, 2.0);
        self.speed = fin(self.speed, 0.0, 100.0, 0.5);
    }

    /// The air velocity at `p` at time `t` seconds, for a run seeded with `seed`.
    pub fn velocity_at(&self, p: Vec3, t: f32, seed: u32) -> Vec3 {
        let dir = Vec3::from_array(self.direction).normalize_or_zero();
        if self.strength <= 0.0 || dir == Vec3::ZERO {
            return Vec3::ZERO;
        }
        let base = dir * self.strength;
        if self.turbulence <= 0.0 {
            return base;
        }
        // Gusts drift downwind: sample the noise in a frame that moves with the air.
        let q = (p - dir * (self.speed * self.scale * t)) / self.scale.max(1e-3);
        // Gusts push along the wind more than across it.
        let n = vector_noise(q, seed);
        let along = (1.0 + self.turbulence * n.x).max(0.0);
        base * along + self.strength * self.turbulence * 0.5 * (n - dir * n.dot(dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_smooth_bounded_and_seeded() {
        let mut worst = 0.0f32;
        let mut prev = noise3(Vec3::new(0.0, 0.3, 0.7), 1);
        for i in 1..2000 {
            let p = Vec3::new(i as f32 * 0.01, 0.3, 0.7);
            let n = noise3(p, 1);
            assert!((-1.0..=1.0).contains(&n), "{n}");
            worst = worst.max((n - prev).abs());
            prev = n;
        }
        assert!(worst < 0.1, "neighbouring samples are close: {worst}");
        assert_ne!(noise3(Vec3::new(0.4, 0.4, 0.4), 1), noise3(Vec3::new(0.4, 0.4, 0.4), 2));
        assert_eq!(noise3(Vec3::new(0.4, 0.4, 0.4), 9), noise3(Vec3::new(0.4, 0.4, 0.4), 9));
        assert_eq!(noise3(Vec3::NAN, 0), 0.0);
        let _ = noise3(Vec3::splat(1e30), 0);
    }

    #[test]
    fn steady_wind_is_uniform_and_turbulence_varies_around_it() {
        let w = Wind { strength: 5.0, ..Wind::default() };
        assert_eq!(w.velocity_at(Vec3::splat(3.0), 1.0, 0), Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(Wind::default().velocity_at(Vec3::ONE, 0.0, 0), Vec3::ZERO, "no strength, no wind");
        let g = Wind { strength: 5.0, turbulence: 0.8, ..Wind::default() };
        let samples: Vec<Vec3> = (0..400).map(|i| g.velocity_at(Vec3::new(i as f32 * 0.37, (i % 7) as f32, 0.0), i as f32 * 0.1, 3)).collect();
        let mean: Vec3 = samples.iter().copied().sum::<Vec3>() / samples.len() as f32;
        assert!((mean.x - 5.0).abs() < 1.0, "the mean follows the wind: {mean:?}");
        let spread = samples.iter().map(|s| (s.x - mean.x).abs()).fold(0.0, f32::max);
        assert!(spread > 1.0, "gusts vary: {spread}");
        assert!(samples.iter().all(|s| s.is_finite() && s.length() < 20.0));
        assert_eq!(g.velocity_at(Vec3::ONE, 2.0, 3), g.velocity_at(Vec3::ONE, 2.0, 3));
        assert_ne!(g.velocity_at(Vec3::ONE, 2.0, 3), g.velocity_at(Vec3::ONE, 2.0, 4), "another seed, other air");
    }

    #[test]
    fn hostile_wind_is_clamped() {
        let mut w = Wind { direction: [f32::NAN, 1e9, 0.0], strength: f32::INFINITY, turbulence: -3.0, scale: 0.0, speed: f32::NAN };
        w.sanitize();
        assert!(w.velocity_at(Vec3::ONE, 1.0, 0).is_finite());
        assert_eq!((w.strength, w.turbulence, w.scale), (0.0, 0.0, 0.01));
    }
}
