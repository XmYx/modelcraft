//! Image stamps: grayscale pictures that shape each dab. The built-in library is drawn by the
//! functions below at start-up (original work, no image files); every picture is a square grid
//! of values 0..1 sampled with bilinear filtering, the same path an imported picture would take.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// Side of a built-in stamp picture, in samples.
pub const SIDE: usize = 128;

/// A built-in stamp picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Alpha {
    /// A grid of round bumps.
    Dots,
    /// Overlapping rounded scales.
    Scales,
    /// Branching cracks.
    Cracks,
    /// Over-and-under woven bands.
    Weave,
    /// Small scattered pits.
    Pores,
    /// A five-pointed star.
    Star,
    /// Parallel ridges.
    Stripes,
    /// A honeycomb of hexagons.
    Hex,
}

pub const ALPHAS: &[Alpha] = &[Alpha::Dots, Alpha::Scales, Alpha::Cracks, Alpha::Weave, Alpha::Pores, Alpha::Star, Alpha::Stripes, Alpha::Hex];

impl Alpha {
    pub fn label(self) -> &'static str {
        match self {
            Alpha::Dots => "Dots",
            Alpha::Scales => "Scales",
            Alpha::Cracks => "Cracks",
            Alpha::Weave => "Weave",
            Alpha::Pores => "Pores",
            Alpha::Star => "Star",
            Alpha::Stripes => "Stripes",
            Alpha::Hex => "Honeycomb",
        }
    }

    /// The picture: `SIDE * SIDE` values in 0..1, row by row.
    pub fn pixels(self) -> &'static [f32] {
        static ALL: LazyLock<Vec<Vec<f32>>> = LazyLock::new(|| ALPHAS.iter().map(|a| a.draw()).collect());
        ALPHAS.iter().position(|a| *a == self).and_then(|i| ALL.get(i)).map(Vec::as_slice).unwrap_or_default()
    }

    /// Value at (`u`, `v`) in −1..1 across the brush; 0 outside the picture.
    pub fn sample(self, u: f32, v: f32) -> f32 {
        sample(self.pixels(), SIDE, u, v)
    }

    fn draw(self) -> Vec<f32> {
        let mut out = Vec::with_capacity(SIDE * SIDE);
        for j in 0..SIDE {
            for i in 0..SIDE {
                let (x, y) = ((i as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0, (j as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0);
                out.push(self.value(x, y).clamp(0.0, 1.0));
            }
        }
        out
    }

    fn value(self, x: f32, y: f32) -> f32 {
        let smooth = |e0: f32, e1: f32, t: f32| {
            let k = ((t - e0) / (e1 - e0)).clamp(0.0, 1.0);
            k * k * (3.0 - 2.0 * k)
        };
        let cell = |t: f32, n: f32| (t * n).rem_euclid(1.0) - 0.5;
        match self {
            Alpha::Dots => {
                let (cx, cy) = (cell(x, 3.0), cell(y, 3.0));
                1.0 - smooth(0.18, 0.42, (cx * cx + cy * cy).sqrt())
            }
            Alpha::Scales => {
                let row = (y * 3.0).floor();
                let (cx, cy) = (cell(x + row * 0.1667, 3.0), (y * 3.0).rem_euclid(1.0));
                let d = (cx * cx + (cy - 0.15) * (cy - 0.15)).sqrt();
                (1.0 - smooth(0.35, 0.62, d)) * smooth(0.0, 0.5, cy + 0.2)
            }
            Alpha::Cracks => {
                // Distance to the borders of an irregular cell pattern: thin dark lines.
                let (gx, gy) = (x * 2.5, y * 2.5);
                let (ix, iy) = (gx.floor(), gy.floor());
                let (mut d1, mut d2) = (f32::MAX, f32::MAX);
                for oy in -1..=1 {
                    for ox in -1..=1 {
                        let (px, py) = (ix + ox as f32, iy + oy as f32);
                        let (jx, jy) = (hash(px, py, 1), hash(px, py, 2));
                        let d = ((gx - px - jx) * (gx - px - jx) + (gy - py - jy) * (gy - py - jy)).sqrt();
                        if d < d1 {
                            d2 = d1;
                            d1 = d;
                        } else if d < d2 {
                            d2 = d;
                        }
                    }
                }
                1.0 - smooth(0.0, 0.12, d2 - d1)
            }
            Alpha::Weave => {
                let (cx, cy) = (cell(x, 4.0), cell(y, 4.0));
                let over = ((x * 4.0).floor() + (y * 4.0).floor()).rem_euclid(2.0) < 1.0;
                let (a, b) = if over { (cx, cy) } else { (cy, cx) };
                (1.0 - smooth(0.25, 0.45, a.abs())) * (0.55 + 0.45 * (b * std::f32::consts::PI).cos())
            }
            Alpha::Pores => {
                let (gx, gy) = (x * 7.0, y * 7.0);
                let (ix, iy) = (gx.floor(), gy.floor());
                let mut v = 0.0f32;
                for oy in -1..=1 {
                    for ox in -1..=1 {
                        let (px, py) = (ix + ox as f32, iy + oy as f32);
                        let r = 0.12 + 0.2 * hash(px, py, 3);
                        let d = ((gx - px - hash(px, py, 4)).powi(2) + (gy - py - hash(px, py, 5)).powi(2)).sqrt();
                        v = v.max(1.0 - smooth(r * 0.5, r, d));
                    }
                }
                v
            }
            Alpha::Star => {
                let (r, a) = ((x * x + y * y).sqrt(), y.atan2(x) + std::f32::consts::FRAC_PI_2);
                let k = (a / (std::f32::consts::TAU / 5.0)).rem_euclid(1.0);
                let edge = 0.38 + 0.52 * (1.0 - (k - 0.5).abs() * 2.0).powf(1.6);
                1.0 - smooth(edge - 0.06, edge, r)
            }
            Alpha::Stripes => 0.5 + 0.5 * (y * 5.0 * std::f32::consts::TAU).cos(),
            Alpha::Hex => {
                // Distance to the nearest hexagon centre on a triangular lattice.
                let s = 3.2;
                let (qx, qy) = (x * s, y * s);
                let (ax, ay) = (qx.rem_euclid(1.732_050_8) - 0.866_025_4, qy.rem_euclid(1.0) - 0.5);
                let (bx, by) = ((qx + 0.866_025_4).rem_euclid(1.732_050_8) - 0.866_025_4, (qy + 0.5).rem_euclid(1.0) - 0.5);
                let d = (ax * ax + ay * ay).min(bx * bx + by * by).sqrt();
                smooth(0.36, 0.48, d).mul_add(-1.0, 1.0)
            }
        }
    }
}

/// A repeatable value in 0..1 for an integer lattice point.
fn hash(x: f32, y: f32, salt: u32) -> f32 {
    let mut h = (x as i32 as u32).wrapping_mul(0x9E37_79B1) ^ (y as i32 as u32).wrapping_mul(0x85EB_CA77) ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

/// Bilinear sample of a square picture at (`u`, `v`) in −1..1; 0 outside or for a bad picture.
pub fn sample(pixels: &[f32], side: usize, u: f32, v: f32) -> f32 {
    if side == 0 || pixels.len() != side * side || !(u.is_finite() && v.is_finite()) || u.abs() > 1.0 || v.abs() > 1.0 {
        return 0.0;
    }
    let f = |t: f32| ((t * 0.5 + 0.5) * side as f32 - 0.5).clamp(0.0, side as f32 - 1.0);
    let (x, y) = (f(u), f(v));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(side - 1), (y0 + 1).min(side - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let g = |i: usize, j: usize| pixels.get(i + j * side).copied().unwrap_or(0.0);
    let a = g(x0, y0) + (g(x1, y0) - g(x0, y0)) * fx;
    let b = g(x0, y1) + (g(x1, y1) - g(x0, y1)) * fx;
    a + (b - a) * fy
}
