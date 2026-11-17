//! Small numeric helpers: frames, warps and heuristics.

use glam::{Vec2, Vec3A};
use std::f32::consts::PI;

/// Linear RGB radiance, reflectance or throughput.
pub type Rgb = Vec3A;

pub const INV_PI: f32 = 1.0 / PI;

/// Relative luminance of linear Rec. 709 / sRGB primaries.
#[inline]
pub fn luminance(c: Rgb) -> f32 {
    0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z
}

/// An orthonormal basis around `n` (Duff et al., "Building an Orthonormal Basis, Revisited", 2017).
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub t: Vec3A,
    pub b: Vec3A,
    pub n: Vec3A,
}

impl Frame {
    #[inline]
    pub fn new(n: Vec3A) -> Frame {
        let sign = 1.0f32.copysign(n.z);
        let a = -1.0 / (sign + n.z);
        let b = n.x * n.y * a;
        Frame { t: Vec3A::new(1.0 + sign * n.x * n.x * a, sign * b, -sign * n.x), b: Vec3A::new(b, sign + n.y * n.y * a, -n.y), n }
    }
    #[inline]
    pub fn to_local(&self, v: Vec3A) -> Vec3A {
        Vec3A::new(v.dot(self.t), v.dot(self.b), v.dot(self.n))
    }
    #[inline]
    pub fn to_world(&self, v: Vec3A) -> Vec3A {
        self.t * v.x + self.b * v.y + self.n * v.z
    }
}

/// Unit square to unit disk, area preserving (Shirley and Chiu 1997).
#[inline]
pub fn concentric_disk(u: Vec2) -> Vec2 {
    let o = u * 2.0 - Vec2::ONE;
    if o.x == 0.0 && o.y == 0.0 {
        return Vec2::ZERO;
    }
    let (r, theta) = if o.x.abs() > o.y.abs() { (o.x, PI / 4.0 * (o.y / o.x)) } else { (o.y, PI / 2.0 - PI / 4.0 * (o.x / o.y)) };
    Vec2::new(theta.cos(), theta.sin()) * r
}

/// Cosine-weighted direction on the +z hemisphere; its density is `z / π`.
#[inline]
pub fn cosine_hemisphere(u: Vec2) -> Vec3A {
    let d = concentric_disk(u);
    Vec3A::new(d.x, d.y, (1.0 - d.length_squared()).max(0.0).sqrt())
}

/// Uniform direction on the unit sphere; its density is `1 / 4π`.
#[inline]
pub fn uniform_sphere(u: Vec2) -> Vec3A {
    let z = 1.0 - 2.0 * u.x;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = 2.0 * PI * u.y;
    Vec3A::new(r * phi.cos(), r * phi.sin(), z)
}

/// Uniform barycentric weights `(b1, b2)` of a triangle's second and third corners.
#[inline]
pub fn uniform_triangle(u: Vec2) -> (f32, f32) {
    let s = u.x.max(0.0).sqrt();
    (1.0 - s, u.y * s)
}

/// The power heuristic with exponent 2 (Veach 1997) for a sample drawn with density `a`.
#[inline]
pub fn power_heuristic(a: f32, b: f32) -> f32 {
    if a.is_infinite() {
        return 1.0;
    }
    let (a2, b2) = (a * a, b * b);
    if a2 + b2 > 0.0 { a2 / (a2 + b2) } else { 0.0 }
}

/// Replace non-finite or negative components with zero (keeps one bad sample from spoiling a pixel).
#[inline]
pub fn sanitize(c: Rgb) -> Rgb {
    if c.is_finite() { c.max(Vec3A::ZERO) } else { Vec3A::ZERO }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_orthonormal() {
        for n in [Vec3A::Z, -Vec3A::Z, Vec3A::X, Vec3A::new(0.3, -0.8, 0.52).normalize(), Vec3A::new(-0.6, 0.0, -0.8)] {
            let f = Frame::new(n);
            assert!(f.t.dot(f.b).abs() < 1e-5 && f.t.dot(f.n).abs() < 1e-5 && f.b.dot(f.n).abs() < 1e-5);
            assert!((f.t.length() - 1.0).abs() < 1e-5 && (f.b.length() - 1.0).abs() < 1e-5);
            assert!(f.t.cross(f.b).dot(f.n) > 0.99);
            let v = Vec3A::new(0.1, 0.7, -0.4);
            assert!((f.to_world(f.to_local(v)) - v).length() < 1e-5);
        }
    }

    #[test]
    fn warps_stay_in_range() {
        for i in 0..64 {
            for j in 0..64 {
                let u = Vec2::new((i as f32 + 0.5) / 64.0, (j as f32 + 0.5) / 64.0);
                assert!(concentric_disk(u).length() <= 1.0 + 1e-5);
                let c = cosine_hemisphere(u);
                assert!(c.z >= 0.0 && (c.length() - 1.0).abs() < 1e-4);
                assert!((uniform_sphere(u).length() - 1.0).abs() < 1e-4);
                let (b1, b2) = uniform_triangle(u);
                assert!(b1 >= 0.0 && b2 >= 0.0 && b1 + b2 <= 1.0 + 1e-6);
            }
        }
        assert_eq!(power_heuristic(f32::INFINITY, 1.0), 1.0);
        assert_eq!(power_heuristic(0.0, 0.0), 0.0);
        assert_eq!(sanitize(Vec3A::new(f32::NAN, 1.0, 1.0)), Vec3A::ZERO);
    }
}
