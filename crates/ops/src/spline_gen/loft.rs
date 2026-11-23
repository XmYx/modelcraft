//! Lofting: a surface through two or more sections.

use glam::{Vec2, Vec3};
use modelcraft_spline::Polyline;
use serde::{Deserialize, Serialize};

use super::{Curve, MAX_STEPS, add_cap, check_faces, newell, prepare, push_quad};
use crate::{OpError, OpResult, Polygon, rebuild};

/// Most sections of one loft, and most points per section after resampling.
pub const MAX_SECTIONS: usize = 256;
pub const MAX_SECTION_POINTS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoftOptions {
    /// Rings between one section and the next.
    pub steps: u32,
    /// A smooth curve through the sections (three or more) instead of straight blends.
    pub smooth: bool,
    /// Cap the first and last section (closed sections).
    pub caps: bool,
    /// Points round each section; 0 uses the most any section has.
    pub points: u32,
}

impl Default for LoftOptions {
    fn default() -> Self {
        LoftOptions { steps: 4, smooth: true, caps: true, points: 0 }
    }
}

impl LoftOptions {
    pub fn sanitized(self) -> LoftOptions {
        LoftOptions { steps: self.steps.clamp(1, MAX_STEPS), points: self.points.min(MAX_SECTION_POINTS as u32), ..self }
    }
}

/// `m` points spread evenly by length along (or round) a curve.
fn resample(c: &Curve, m: usize) -> Vec<Vec3> {
    let n = c.pts.len();
    let along = c.along();
    let mut out = Vec::with_capacity(m);
    let mut seg = 0usize;
    for i in 0..m {
        let f = if c.closed { i as f32 / m as f32 } else { i as f32 / (m - 1).max(1) as f32 };
        while seg + 1 < c.edges() && along[seg + 1] < f {
            seg += 1;
        }
        let (a, b) = (along[seg], along[seg + 1]);
        let t = if b - a > 1e-12 { ((f - a) / (b - a)).clamp(0.0, 1.0) } else { 0.0 };
        out.push(c.pts[seg].lerp(c.pts[(seg + 1) % n], t));
    }
    out
}

fn centroid(p: &[Vec3]) -> Vec3 {
    p.iter().copied().sum::<Vec3>() / p.len().max(1) as f32
}

/// The shift of closed ring `b` that puts its points nearest to ring `a` (sampled, so the cost
/// stays small for fine rings).
fn best_shift(a: &[Vec3], b: &[Vec3]) -> usize {
    let m = a.len();
    let stride = (m / 64).max(1);
    let mut best = (f32::MAX, 0usize);
    for s in 0..m {
        let cost: f32 = (0..m).step_by(stride).map(|j| a[j].distance_squared(b[(j + s) % m])).sum();
        if cost < best.0 {
            best = (cost, s);
        }
    }
    best.1
}

/// A surface through `sections` (all closed, or all open), in order. The sections are resampled to
/// the same number of points, turned the same way round and lined up with each other.
pub fn loft(sections: &[Polyline], options: &LoftOptions) -> OpResult {
    let o = options.sanitized();
    if sections.len() < 2 {
        return Err(OpError::Invalid("a loft needs at least two curves".into()));
    }
    if sections.len() > MAX_SECTIONS {
        return Err(OpError::Invalid(format!("a loft of {} curves is too many; at most {MAX_SECTIONS}", sections.len())));
    }
    let mut cs = Vec::with_capacity(sections.len());
    for s in sections {
        let mut p = prepare(std::slice::from_ref(s))?;
        cs.push(p.remove(0));
    }
    let closed = cs[0].closed;
    if cs.iter().any(|c| c.closed != closed) {
        return Err(OpError::Invalid("the curves of a loft must be all closed or all open".into()));
    }
    let m =
        if o.points > 0 { o.points as usize } else { cs.iter().map(|c| c.pts.len()).max().unwrap_or(0) }.clamp(if closed { 3 } else { 2 }, MAX_SECTION_POINTS);
    let mut rings: Vec<Vec<Vec3>> = cs.iter().map(|c| resample(c, m)).collect();
    let s_count = rings.len();
    let steps = o.steps as usize;
    let total = (s_count - 1) * steps + 1;
    check_faces((total - 1).saturating_mul(m).saturating_add(2 * m))?;
    // The way the loft travels, and the turning of its sections about it.
    let travel = (centroid(&rings[s_count - 1]) - centroid(&rings[0])).try_normalize().or_else(|| newell(&rings[0]).try_normalize()).unwrap_or(Vec3::Z);
    if closed {
        for r in &mut rings {
            if newell(r).dot(travel) < 0.0 {
                r.reverse();
            }
        }
    }
    for k in 1..s_count {
        let (a, b) = (rings[k - 1].clone(), rings[k].clone());
        if closed {
            let s = best_shift(&a, &b);
            rings[k].rotate_left(s);
        } else {
            let (straight, crossed) = (a[0].distance(b[0]) + a[m - 1].distance(b[m - 1]), a[0].distance(b[m - 1]) + a[m - 1].distance(b[0]));
            if crossed < straight {
                rings[k].reverse();
            }
        }
    }
    // Rings between the sections.
    let pick = |s: usize| &rings[s.min(s_count - 1)];
    let mut positions: Vec<Vec3> = Vec::with_capacity(total * m);
    for r in 0..total {
        let (s, t) = if r == total - 1 { (s_count - 1, 0.0) } else { (r / steps, (r % steps) as f32 / steps as f32) };
        for j in 0..m {
            let p1 = pick(s)[j];
            let p2 = pick(s + 1)[j];
            let p = if o.smooth && s_count >= 3 {
                let p0 = pick(s.saturating_sub(1))[j];
                let p3 = pick(s + 2)[j];
                let (t2, t3) = (t * t, t * t * t);
                ((p1 * 2.0) + (p2 - p0) * t + (p0 * 2.0 - p1 * 5.0 + p2 * 4.0 - p3) * t2 + (p1 * 3.0 - p0 - p2 * 3.0 + p3) * t3) * 0.5
            } else {
                p1.lerp(p2, t)
            };
            positions.push(p);
        }
    }
    let mut polys: Vec<Polygon> = Vec::new();
    let edges = if closed { m } else { m - 1 };
    for r in 0..total - 1 {
        for j in 0..edges {
            let v = |rr: usize, jj: usize| (rr * m + jj % m) as u32;
            let (u0, u1) = (j as f32 / edges as f32, (j + 1) as f32 / edges as f32);
            let (v0, v1) = (r as f32 / (total - 1) as f32, (r + 1) as f32 / (total - 1) as f32);
            push_quad(
                &mut polys,
                [v(r, j), v(r, j + 1), v(r + 1, j + 1), v(r + 1, j)],
                [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
            );
        }
    }
    if closed && o.caps {
        let first: Vec<u32> = (0..m as u32).collect();
        let last: Vec<u32> = (((total - 1) * m) as u32..(total * m) as u32).collect();
        add_cap(&mut polys, &positions, &[first], -travel)?;
        add_cap(&mut polys, &positions, &[last], travel)?;
    }
    rebuild(positions, polys)
}
