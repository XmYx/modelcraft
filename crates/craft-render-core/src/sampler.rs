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

/// Interleave the low 16 bits of `x` and `y` (the position of a pixel along the z-curve).
#[inline]
pub fn morton(x: u32, y: u32) -> u32 {
    fn spread(v: u32) -> u32 {
        let mut v = v & 0xffff;
        v = (v | (v << 8)) & 0x00ff_00ff;
        v = (v | (v << 4)) & 0x0f0f_0f0f;
        v = (v | (v << 2)) & 0x3333_3333;
        (v | (v << 1)) & 0x5555_5555
    }
    spread(x) | (spread(y) << 1)
}

/// Index bits needed to give every pixel of a `width × height` image its place on the z-curve.
pub fn z_order_bits(width: u32, height: u32) -> u32 {
    let side = width.max(height).max(1).next_power_of_two();
    2 * side.trailing_zeros()
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

    /// The sampler for sample `sample` of pixel `(x, y)` when pixels share one sequence in
    /// Morton (z-curve) order: each pass hands consecutive blocks of the sequence to neighbouring
    /// pixels, so neighbours get complementary samples and the image's error is pushed towards
    /// high frequencies, where it is less visible and easier to filter (Ahmed and Wonka,
    /// "Screen-Space Blue-Noise Diffusion of Monte Carlo Sampling Error via Hierarchical Ordering
    /// of Pixels", 2020). `bits` is the number of index bits the image's pixels need
    /// ([`z_order_bits`]). `None` when the sequence's 32-bit index would run out; the caller
    /// then uses independent pixels.
    #[inline]
    pub fn z_ordered(x: u32, y: u32, sample: u32, bits: u32, render_seed: u32) -> Option<Sampler> {
        if bits >= 32 || (bits > 0 && sample >= (1u32 << (32 - bits))) {
            return None;
        }
        Some(Sampler { index: (sample << bits) | morton(x, y), seed: hash32(render_seed ^ 0x5bd1_e995), dim: 0 })
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
    fn z_order_gives_neighbours_complementary_samples() {
        assert_eq!(morton(0b11, 0b00), 0b0101);
        assert_eq!(morton(0b00, 0b11), 0b1010);
        assert_eq!((z_order_bits(640, 360), z_order_bits(1, 1), z_order_bits(1024, 1024)), (20, 0, 20));
        // The 16 pixels of any aligned 4×4 block, in one pass, together hold a stratified set.
        let bits = z_order_bits(64, 64);
        for (bx, by, pass) in [(0, 0, 0), (8, 20, 3), (60, 60, 17)] {
            let mut cells = [0u32; 16];
            for y in 0..4 {
                for x in 0..4 {
                    let u = Sampler::z_ordered(bx + x, by + y, pass, bits, 9).unwrap().next2();
                    cells[(u.y * 4.0) as usize * 4 + (u.x * 4.0) as usize] += 1;
                }
            }
            assert!(cells.iter().all(|c| *c == 1), "block ({bx}, {by}) pass {pass}: {cells:?}");
        }
        // The index runs out after 2^(32 − bits) passes; the caller falls back.
        assert!(Sampler::z_ordered(0, 0, 4095, bits, 9).is_some());
        assert!(Sampler::z_ordered(0, 0, 1 << 20, bits, 9).is_none());
        assert!(Sampler::z_ordered(0, 0, 0, 32, 9).is_none());
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
