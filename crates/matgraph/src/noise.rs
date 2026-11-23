//! Gradient noise over the plane, tiling across the UV square.
//!
//! Gradient noise on an integer lattice with a smooth blend between lattice points (Perlin, "An
//! image synthesizer", 1985; the quintic fade is from Perlin, "Improving noise", 2002), summed
//! over octaves. The lattice wraps every `period` cells, so a map made from it tiles.

use glam::Vec2;

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// A unit gradient at a lattice point.
fn gradient(ix: i32, iy: i32, seed: u32) -> Vec2 {
    let h = hash((ix as u32).wrapping_mul(0x9e37_79b1) ^ (iy as u32).wrapping_mul(0x85eb_ca77) ^ seed.wrapping_mul(0xc2b2_ae3d));
    let a = (h >> 8) as f32 / 16_777_216.0 * std::f32::consts::TAU;
    Vec2::new(a.cos(), a.sin())
}

fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// One octave at `p`, wrapping every `period` cells; about -0.7 to 0.7.
fn gradient_noise(p: Vec2, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (p.x.floor(), p.y.floor());
    let (fx, fy) = (p.x - x0, p.y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let corner = |dx: i32, dy: i32| {
        let g = gradient((ix + dx).rem_euclid(period), (iy + dy).rem_euclid(period), seed);
        g.dot(Vec2::new(fx - dx as f32, fy - dy as f32))
    };
    let (u, v) = (fade(fx), fade(fy));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * u;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * u;
    top + (bottom - top) * v
}

/// Fractal noise in 0 to 1 at `uv` (0 to 1 across the square): `scale` cells across, `detail`
/// octaves each twice as fine and `roughness` times as strong. The result tiles in `uv`.
pub fn fractal(uv: Vec2, scale: f32, detail: u32, roughness: f32, seed: u32) -> f32 {
    let cells = (scale.round() as i32).clamp(1, 4096);
    let (mut sum, mut amp, mut total) = (0.0f32, 1.0f32, 0.0f32);
    let mut period = cells;
    for o in 0..detail.clamp(1, 12) {
        sum += amp * gradient_noise(uv * period as f32, period, seed.wrapping_add(o.wrapping_mul(0x51ed)));
        total += amp;
        amp *= roughness;
        period = period.saturating_mul(2).min(1 << 20);
    }
    (sum / total.max(1e-6) * 0.7 + 0.5).clamp(0.0, 1.0)
}
