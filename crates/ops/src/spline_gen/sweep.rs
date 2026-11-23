//! Sweeping: a profile moved along a path.
//!
//! The profile keeps its size and shape: each section is the profile turned by a
//! rotation-minimizing frame (the smallest turn that takes the path's direction at one point to the
//! direction at the next), so it does not roll about the path as a frame built from a fixed "up"
//! direction does. A closed path's leftover roll is spread along it so the ends meet.

use glam::{Quat, Vec2, Vec3};
use modelcraft_spline::Polyline;
use serde::{Deserialize, Serialize};

use super::{Curve, add_cap, check_faces, newell, orient, prepare, push_quad};
use crate::{OpError, OpResult, rebuild};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SweepOptions {
    /// The profile's size at the end of the path, as a multiple of its size at the start.
    pub end_scale: f32,
    /// Extra turn about the path from start to end, in degrees.
    pub twist: f32,
    /// A turn of the profile about the path at the start, in degrees.
    pub roll: f32,
    /// Cap the ends of an open path (closed profiles).
    pub caps: bool,
}

impl Default for SweepOptions {
    fn default() -> Self {
        SweepOptions { end_scale: 1.0, twist: 0.0, roll: 0.0, caps: true }
    }
}

impl SweepOptions {
    pub fn sanitized(self) -> SweepOptions {
        let f = |v: f32, d: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        SweepOptions {
            end_scale: f(self.end_scale, 1.0, 0.001, 1000.0),
            twist: f(self.twist, 0.0, -36_000.0, 36_000.0),
            roll: f(self.roll, 0.0, -36_000.0, 36_000.0),
            caps: self.caps,
        }
    }
}

/// The smallest rotation taking unit vector `a` to unit vector `b`.
fn arc(a: Vec3, b: Vec3) -> Quat {
    let d = a.dot(b);
    if d < -0.999_999 {
        Quat::from_axis_angle(a.any_orthonormal_vector(), std::f32::consts::PI)
    } else {
        let c = a.cross(b);
        Quat::from_xyzw(c.x, c.y, c.z, 1.0 + d).normalize()
    }
}

/// Sweep `profiles` along `path`. The profile's origin rides on the path: each section is the
/// profile as it is placed (offsets from its origin kept), turned to face along the path.
pub fn sweep(profiles: &[Polyline], path: &Polyline, options: &SweepOptions) -> OpResult {
    let o = options.sanitized();
    let mut cs = prepare(profiles)?;
    let path_curve: Curve = prepare(std::slice::from_ref(path))?.remove(0);
    let pp = &path_curve.pts;
    let n = pp.len();
    if n < 2 {
        return Err(OpError::Invalid("the path of a sweep needs at least two points".into()));
    }
    // The profile's plane: the normal of its biggest outline.
    let mut np =
        cs.iter().map(|c| newell(&c.pts)).max_by(|a, b| a.length_squared().total_cmp(&b.length_squared())).and_then(Vec3::try_normalize).unwrap_or(Vec3::Z);
    if !np.is_finite() {
        np = Vec3::Z;
    }
    orient(&mut cs, np)?;
    let profile_points: usize = cs.iter().map(|c| c.pts.len()).sum();
    let sections = n;
    check_faces(cs.iter().map(Curve::edges).sum::<usize>().saturating_mul(path_curve.edges()).saturating_add(profile_points * 2))?;
    // Directions along the path.
    let dir = |i: usize, j: usize| (pp[j % n] - pp[i % n]).try_normalize().unwrap_or(Vec3::ZERO);
    let mut tangents = Vec::with_capacity(n);
    for i in 0..n {
        let before = if i > 0 || path_curve.closed { dir(i + n - 1, i) } else { Vec3::ZERO };
        let after = if i + 1 < n || path_curve.closed { dir(i, i + 1) } else { Vec3::ZERO };
        let t = (before + after).try_normalize().or_else(|| after.try_normalize()).or_else(|| before.try_normalize()).unwrap_or(Vec3::Z);
        tangents.push(t);
    }
    // Rotation-minimizing frames.
    let q0 = Quat::from_axis_angle(tangents[0], o.roll.to_radians()) * arc(np, tangents[0]);
    let mut frames = Vec::with_capacity(n);
    frames.push(q0);
    for i in 1..n {
        let prev = frames[i - 1];
        frames.push((arc(tangents[i - 1], tangents[i]) * prev).normalize());
    }
    // The roll a closed path comes back with, spread along it.
    let mut closing_roll = 0.0f32;
    if path_curve.closed {
        let back = (arc(tangents[n - 1], tangents[0]) * frames[n - 1]).normalize();
        let delta = back * q0.inverse();
        let (axis, angle) = delta.to_axis_angle();
        let angle = if angle > std::f32::consts::PI { angle - std::f32::consts::TAU } else { angle };
        closing_roll = angle * axis.dot(tangents[0]).signum();
    }
    let mut positions: Vec<Vec3> = Vec::new();
    let mut base = Vec::with_capacity(cs.len());
    let span = if path_curve.closed { n as f32 } else { (n - 1) as f32 };
    for c in &cs {
        base.push(positions.len() as u32);
        for i in 0..sections {
            let f = i as f32 / span;
            let twist = (o.twist * f).to_radians() - closing_roll * f;
            let q = Quat::from_axis_angle(tangents[i], twist) * frames[i];
            let scale = if path_curve.closed { 1.0 } else { 1.0 + (o.end_scale - 1.0) * f };
            positions.extend(c.pts.iter().map(|p| pp[i] + q * (*p * scale)));
        }
    }
    // Distance along the path, for the V coordinate.
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..path_curve.edges() {
        cum[i + 1] = cum[i] + pp[i].distance(pp[(i + 1) % n]);
    }
    if !path_curve.closed {
        cum[n] = cum[n - 1];
    }
    let total = cum[n].max(1e-12);
    let mut polys = Vec::new();
    for (ci, c) in cs.iter().enumerate() {
        let m = c.pts.len();
        let along = c.along();
        let v = |sec: usize, j: usize| base[ci] + ((sec % sections) * m + j % m) as u32;
        for i in 0..path_curve.edges() {
            let (v0, v1) = (cum[i] / total, cum[i + 1] / total);
            for j in 0..c.edges() {
                let (u0, u1) = (along[j], along[j + 1]);
                push_quad(
                    &mut polys,
                    [v(i, j), v(i, j + 1), v(i + 1, j + 1), v(i + 1, j)],
                    [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
                );
            }
        }
    }
    let closed: Vec<usize> = (0..cs.len()).filter(|i| cs[*i].closed).collect();
    if o.caps && !path_curve.closed && !closed.is_empty() {
        for (sec, normal) in [(0usize, -tangents[0]), (sections - 1, tangents[n - 1])] {
            let rings: Vec<Vec<u32>> = closed.iter().map(|i| (0..cs[*i].pts.len()).map(|j| base[*i] + (sec * cs[*i].pts.len() + j) as u32).collect()).collect();
            add_cap(&mut polys, &positions, &rings, normal)?;
        }
    }
    rebuild(positions, polys)
}
