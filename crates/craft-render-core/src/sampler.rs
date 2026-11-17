//! Sample sequences.
//!
//! Each pixel draws from an Owen-scrambled Sobol' (0,2)-sequence, with every pair of dimensions
//! scrambled and shuffled independently ("padding"), following Burley, "Practical Hash-based Owen
//! Scrambling" (JCGT 2020), which builds on Laine and Karras's hash-based scramble. The first two
//! Sobol' dimensions need no table: the first is the bit-reversed index (van der Corput) and the
//! second follows from the recurrence `v[k] = v[k-1] ^ (v[k-1] >> 1)`.

use glam::Vec2;

/// An integer mixer with good avalanche (a multiply–xorshift finalizer).
#[inline]
pub fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

#[inline]
pub fn hash_combine(seed: u32, v: u32) -> u32 {
    hash32(seed ^ v.wrapping_mul(0x9e37_79b9).wrapping_add(0x85eb_ca6b).wrapping_add(seed << 6).wrapping_add(seed >> 2))
}

/// A permutation in which every output bit depends only on lower input bits; applied to a
/// bit-reversed value it is an Owen scramble.
#[inline]
fn laine_karras(mut x: u32, seed: u32) -> u32 {
    x = x.wrapping_add(seed);
    x ^= x.wrapping_mul(0x6c50_b47c);
    x ^= x.wrapping_mul(0xb82f_1e52);
    x ^= x.wrapping_mul(0xc7af_e638);
    x ^= x.wrapping_mul(0x8d22_f6e6);
    x
}

#[inline]
fn owen_scramble(x: u32, seed: u32) -> u32 {
    laine_karras(x.reverse_bits(), seed).reverse_bits()
}

const fn sobol_directions() -> [u32; 32] {
    let mut v = [0u32; 32];
    v[0] = 1 << 31;
    let mut k = 1;
    while k < 32 {
        v[k] = v[k - 1] ^ (v[k - 1] >> 1);
        k += 1;
    }
    v
}
const SOBOL1: [u32; 32] = sobol_directions();

#[inline]
fn sobol1(mut i: u32) -> u32 {
    let mut x = 0;
    let mut k = 0;
    while i != 0 {
        if i & 1 != 0 {
            x ^= SOBOL1[k & 31];
        }
        i >>= 1;
        k += 1;
    }
    x
}

#[inline]
fn to_unit(x: u32) -> f32 {
    (x >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// The stream of numbers for one sample of one pixel.
#[derive(Clone, Copy, Debug)]
pub struct Sampler {
    index: u32,
    seed: u32,
    dim: u32,
}

impl Sampler {
    /// `pixel_seed` identifies the pixel (and the render); `index` is the sample number.
    #[inline]
    pub fn new(pixel_seed: u32, index: u32) -> Sampler {
        Sampler { index, seed: pixel_seed, dim: 0 }
    }

    /// The seed for a pixel of a render.
    #[inline]
    pub fn pixel_seed(x: u32, y: u32, render_seed: u32) -> u32 {
        hash_combine(hash_combine(render_seed, x), y)
    }

    /// Two numbers in `[0, 1)` that are well distributed together across sample indices.
    #[inline]
    pub fn next2(&mut self) -> Vec2 {
        let s = hash_combine(self.seed, self.dim);
        self.dim += 1;
        let i = owen_scramble(self.index, s);
        let x = owen_scramble(i.reverse_bits(), hash32(s ^ 0x68bc_21eb));
        let y = owen_scramble(sobol1(i), hash32(s ^ 0x02e5_be93));
        Vec2::new(to_unit(x), to_unit(y))
    }

    /// One number in `[0, 1)`.
    #[inline]
    pub fn next1(&mut self) -> f32 {
        self.next2().x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_is_stratified() {
        // Any 2^k prefix of a scrambled (0,2)-sequence puts one point in each elementary interval.
        for seed in [1u32, 77, 0xdead_beef] {
            let n = 256u32;
            for (a, b) in [(n, 1), (16, 16), (1, n), (64, 4)] {
                let mut cells = vec![0u32; (a * b) as usize];
                for i in 0..n {
                    let u = Sampler::new(seed, i).next2();
                    assert!((0.0..1.0).contains(&u.x) && (0.0..1.0).contains(&u.y));
                    let (cx, cy) = ((u.x * a as f32) as u32, (u.y * b as f32) as u32);
                    cells[(cy * a + cx) as usize] += 1;
                }
                assert!(cells.iter().all(|c| *c == 1), "seed {seed} grid {a}x{b}");
            }
        }
    }

    #[test]
    fn dimensions_and_pixels_decorrelate() {
        // The mean of x·y over independent dimensions tends to 1/4.
        let n = 4096;
        let mut acc = 0.0f64;
        for i in 0..n {
            let mut s = Sampler::new(Sampler::pixel_seed(3, 9, 1), i);
            let a = s.next2();
            let b = s.next2();
            acc += f64::from(a.x * b.y);
        }
        assert!((acc / f64::from(n) - 0.25).abs() < 0.01);
        assert_ne!(Sampler::pixel_seed(1, 2, 0), Sampler::pixel_seed(2, 1, 0));
        // Integrates a smooth function far better than 1/sqrt(n).
        let mut acc = 0.0f64;
        for i in 0..n {
            let u = Sampler::new(5, i).next2();
            acc += f64::from((u.x * std::f32::consts::PI).sin() * u.y);
        }
        assert!((acc / f64::from(n) - 1.0 / f64::from(std::f32::consts::PI)).abs() < 1e-3);
    }
}
