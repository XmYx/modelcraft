//! Surfaces from curves: extrude, revolve, loft and sweep.
//!
//! Every generator takes tessellated curves ([`modelcraft_spline::Polyline`]: points and
//! whether the curve is closed) and builds a [`PolyMesh`] with UVs, so they work for any spline
//! source: a drawn profile, a primitive, or an outline from a font.
//!
//! - [`extrude`] pushes the curves along an axis, with caps for closed curves (holes included).
//! - [`revolve`] turns the curves about an axis through the origin, by any angle; a partial turn is
//!   capped, points on the axis are shared.
//! - [`loft`] blends through two or more sections, aligned to each other, optionally smoothly.
//! - [`sweep`] moves a profile along a path without twisting it (a rotation-minimizing frame), with
//!   optional twist and end scale.
//!
//! Faces point outwards whichever way the curves were drawn: outlines are told apart from holes by
//! nesting, and turned to agree. Closed curves close up, with the caps filled by [`cap`]. Every
//! input is hostile: curves are cleaned of repeated points, counts are capped
//! ([`MAX_STEPS`], [`MAX_FACES`], [`cap::MAX_CAP_POINTS`]) and a request that would pass a cap is an
//! [`OpError`], decided before the mesh is built.

use glam::{Vec2, Vec3};
use modelcraft_spline::Polyline;
use serde::{Deserialize, Serialize};

use crate::{OpError, OpResult, Polygon, rebuild};

pub mod cap;
mod loft;
mod sweep;

pub use loft::{LoftOptions, loft};
pub use sweep::{SweepOptions, sweep};

/// Most steps along an extrusion, around a revolution, or between lofted sections.
pub const MAX_STEPS: u32 = 1_024;
/// Most faces one generator may build.
pub const MAX_FACES: usize = 2_000_000;
/// Most points in all the input curves of one generator.
pub const MAX_INPUT_POINTS: usize = 200_000;
/// Most spline objects one generator takes.
pub const MAX_INPUTS: usize = 256;

/// An axis through the origin; `Auto` means the normal of the curve's plane (extrude) or Y
/// (revolve).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    #[default]
    Auto,
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 4] = [Axis::Auto, Axis::X, Axis::Y, Axis::Z];

    fn vector(self) -> Option<Vec3> {
        match self {
            Axis::Auto => None,
            Axis::X => Some(Vec3::X),
            Axis::Y => Some(Vec3::Y),
            Axis::Z => Some(Vec3::Z),
        }
    }
}

/// A curve ready to build from: its points without repeats, and whether it closes up.
#[derive(Clone, Debug)]
pub(crate) struct Curve {
    pub pts: Vec<Vec3>,
    pub closed: bool,
}

impl Curve {
    /// Edges around or along the curve.
    pub fn edges(&self) -> usize {
        if self.closed { self.pts.len() } else { self.pts.len().saturating_sub(1) }
    }
    /// Cumulative length fraction (0..=1) at each point, and one more entry for the closing edge.
    pub fn along(&self) -> Vec<f32> {
        let n = self.pts.len();
        let mut cum = vec![0.0f32; n + 1];
        for i in 0..self.edges() {
            cum[i + 1] = cum[i] + self.pts[i].distance(self.pts[(i + 1) % n]);
        }
        if self.edges() < n {
            cum[n] = cum[n - 1];
        }
        let total = cum[n].max(1e-12);
        cum.iter().map(|c| c / total).collect()
    }
}

/// Clean the input curves: drop repeated points (and a closing point equal to the first), curves
/// with too few points, and refuse non-finite numbers or too many points.
pub(crate) fn prepare(curves: &[Polyline]) -> Result<Vec<Curve>, OpError> {
    let total: usize = curves.iter().map(|c| c.points.len()).sum();
    if total > MAX_INPUT_POINTS {
        return Err(OpError::Invalid(format!("the curves have {total} points; at most {MAX_INPUT_POINTS} are allowed")));
    }
    let mut out = Vec::new();
    for c in curves {
        if c.points.iter().any(|p| !p.is_finite()) {
            return Err(OpError::Invalid("a curve has a point that is not a finite number".into()));
        }
        let mut pts: Vec<Vec3> = Vec::with_capacity(c.points.len());
        for p in &c.points {
            if pts.last().is_none_or(|l| l.distance_squared(*p) > 1e-14) {
                pts.push(*p);
            }
        }
        if c.closed && pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.distance_squared(*b) <= 1e-14) {
            pts.pop();
        }
        let need = if c.closed { 3 } else { 2 };
        if pts.len() >= need {
            out.push(Curve { pts, closed: c.closed });
        }
    }
    if out.is_empty() {
        return Err(OpError::Invalid("there is no curve with enough points to build from".into()));
    }
    Ok(out)
}

/// The area vector of a (closed or chord-closed) polygon; its direction is the plane's normal and
/// its length twice the area.
pub(crate) fn newell(pts: &[Vec3]) -> Vec3 {
    let n = pts.len();
    (0..n).map(|i| pts[i].cross(pts[(i + 1) % n])).sum()
}

/// Reverse curves so that outlines run counter-clockwise about `normal` and holes (nested an odd
/// number of levels) clockwise; open curves are turned so that the curve closed by its chord is
/// counter-clockwise. Returns whether each curve is a hole.
pub(crate) fn orient(curves: &mut [Curve], normal: Vec3) -> Result<Vec<bool>, OpError> {
    let closed: Vec<usize> = (0..curves.len()).filter(|i| curves[*i].closed).collect();
    let views: Vec<&[Vec3]> = closed.iter().map(|i| curves[*i].pts.as_slice()).collect();
    let nest = cap::nesting(&views, normal)?;
    let mut hole = vec![false; curves.len()];
    for (k, i) in closed.iter().enumerate() {
        hole[*i] = nest.get(k).is_some_and(|n| n.0);
    }
    for (i, c) in curves.iter_mut().enumerate() {
        let a = newell(&c.pts).dot(normal);
        if (a < 0.0) != hole[i] {
            c.pts.reverse();
        }
    }
    Ok(hole)
}

/// Check the face count a build would make before it is allocated.
pub(crate) fn check_faces(n: usize) -> Result<(), OpError> {
    if n > MAX_FACES { Err(OpError::TooLarge(n)) } else { Ok(()) }
}

/// A quad (or a triangle when two neighbouring corners are the same vertex) with UVs.
pub(crate) fn push_quad(out: &mut Vec<Polygon>, v: [u32; 4], uv: [Vec2; 4]) {
    let mut verts = Vec::with_capacity(4);
    let mut uvs = Vec::with_capacity(4);
    for k in 0..4 {
        if verts.last() != Some(&v[k]) && !(k == 3 && verts.first() == Some(&v[k])) {
            verts.push(v[k]);
            uvs.push(uv[k]);
        }
    }
    if verts.len() >= 3 {
        out.push(Polygon::with_uvs(verts, uvs));
    }
}

/// Planar UVs for cap points: the position in the cap's plane, fitted to the unit square.
pub(crate) fn cap_uvs(points: &[Vec3], normal: Vec3) -> Vec<Vec2> {
    let (u, v) = cap::plane_basis(normal);
    let p2: Vec<Vec2> = points.iter().map(|p| Vec2::new(p.dot(u), p.dot(v))).collect();
    let (lo, hi) = p2.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
    let size = (hi - lo).max(Vec2::splat(1e-9));
    p2.iter().map(|p| (*p - lo) / size).collect()
}

/// Add the triangles filling `contours` (as vertex index lists into `positions`, each counter-
/// clockwise about `normal` after the fill) to the polygons.
pub(crate) fn add_cap(out: &mut Vec<Polygon>, positions: &[Vec3], rings: &[Vec<u32>], normal: Vec3) -> Result<(), OpError> {
    let pts: Vec<Vec<Vec3>> = rings.iter().map(|r| r.iter().map(|i| positions.get(*i as usize).copied().unwrap_or(Vec3::ZERO)).collect()).collect();
    let views: Vec<&[Vec3]> = pts.iter().map(Vec::as_slice).collect();
    let flat: Vec<u32> = rings.iter().flatten().copied().collect();
    let flat_pts: Vec<Vec3> = pts.iter().flatten().copied().collect();
    let uv = cap_uvs(&flat_pts, normal);
    for t in cap::fill(&views, normal)? {
        let corner = |k: u32| (flat.get(k as usize).copied().unwrap_or(0), uv.get(k as usize).copied().unwrap_or(Vec2::ZERO));
        let [(a, ua), (b, ub), (c, uc)] = [corner(t[0]), corner(t[1]), corner(t[2])];
        out.push(Polygon::with_uvs(vec![a, b, c], vec![ua, ub, uc]));
    }
    Ok(())
}

// ---------------------------------------------------------------- fill

/// A flat mesh filling the closed `curves` (a closed curve inside another is a hole), facing the
/// positive side of the curves' plane. Open curves are left out; with no closed curve it is an error.
pub fn fill(curves: &[Polyline]) -> OpResult {
    let mut cs: Vec<Curve> = prepare(curves)?.into_iter().filter(|c| c.closed).collect();
    if cs.is_empty() {
        return Err(OpError::Invalid("filling needs a closed curve".into()));
    }
    let normal = extrude_dir(&cs, Axis::Auto);
    orient(&mut cs, normal);
    let mut positions: Vec<Vec3> = Vec::new();
    let mut rings: Vec<Vec<u32>> = Vec::new();
    for c in &cs {
        let base = positions.len() as u32;
        positions.extend(c.pts.iter().copied());
        rings.push((base..base + c.pts.len() as u32).collect());
    }
    let mut polys = Vec::new();
    add_cap(&mut polys, &positions, &rings, normal)?;
    rebuild(positions, polys)
}

// ---------------------------------------------------------------- extrude

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtrudeOptions {
    /// How far to push; negative pushes the other way.
    pub distance: f32,
    pub axis: Axis,
    /// Steps along the way (each a ring of quads).
    pub steps: u32,
    pub cap_start: bool,
    pub cap_end: bool,
}

impl Default for ExtrudeOptions {
    fn default() -> Self {
        ExtrudeOptions { distance: 1.0, axis: Axis::Auto, steps: 1, cap_start: true, cap_end: true }
    }
}

impl ExtrudeOptions {
    pub fn sanitized(self) -> ExtrudeOptions {
        ExtrudeOptions {
            distance: if self.distance.is_finite() { self.distance.clamp(-1.0e6, 1.0e6) } else { 1.0 },
            steps: self.steps.clamp(1, MAX_STEPS),
            ..self
        }
    }
}

/// The direction of an extrusion: the axis, or the normal of the biggest closed curve's plane,
/// turned to the positive side of its strongest axis so that it does not depend on which way the
/// curve was drawn (a circle in the XY plane always extrudes towards +Z).
fn extrude_dir(curves: &[Curve], axis: Axis) -> Vec3 {
    if let Some(v) = axis.vector() {
        return v;
    }
    let best = curves.iter().map(|c| newell(&c.pts)).max_by(|a, b| a.length_squared().total_cmp(&b.length_squared())).unwrap_or(Vec3::Z);
    let n = best.try_normalize().unwrap_or(Vec3::Z);
    let a = n.abs();
    let strongest = if a.x >= a.y && a.x >= a.z {
        n.x
    } else if a.y >= a.z {
        n.y
    } else {
        n.z
    };
    if strongest < 0.0 { -n } else { n }
}

/// Push `curves` along an axis. Closed curves make closed sides (and caps, if asked); open curves
/// make a ribbon.
pub fn extrude(curves: &[Polyline], options: &ExtrudeOptions) -> OpResult {
    let o = options.sanitized();
    let mut cs = prepare(curves)?;
    let mut dir = extrude_dir(&cs, o.axis);
    let mut dist = o.distance;
    if dist < 0.0 {
        dir = -dir;
        dist = -dist;
    }
    if dist < 1e-9 {
        return Err(OpError::Invalid("the extrusion distance is zero".into()));
    }
    orient(&mut cs, dir)?;
    let steps = o.steps as usize;
    let edges: usize = cs.iter().map(Curve::edges).sum();
    check_faces(edges.saturating_mul(steps).saturating_add(cs.iter().map(|c| c.pts.len()).sum::<usize>() * 2))?;
    let mut positions: Vec<Vec3> = Vec::new();
    let mut base = Vec::with_capacity(cs.len());
    for c in &cs {
        base.push(positions.len() as u32);
        for k in 0..=steps {
            let off = dir * (dist * k as f32 / steps as f32);
            positions.extend(c.pts.iter().map(|p| *p + off));
        }
    }
    let mut polys = Vec::new();
    for (ci, c) in cs.iter().enumerate() {
        let n = c.pts.len();
        let along = c.along();
        let ring = |k: usize, i: usize| base[ci] + (k * n + i % n) as u32;
        for i in 0..c.edges() {
            for k in 0..steps {
                let (v0, v1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
                let (u0, u1) = (along[i], along[i + 1]);
                push_quad(
                    &mut polys,
                    [ring(k, i), ring(k, i + 1), ring(k + 1, i + 1), ring(k + 1, i)],
                    [Vec2::new(u0, v0), Vec2::new(u1, v0), Vec2::new(u1, v1), Vec2::new(u0, v1)],
                );
            }
        }
    }
    let closed: Vec<usize> = (0..cs.len()).filter(|i| cs[*i].closed).collect();
    if !closed.is_empty() && (o.cap_start || o.cap_end) {
        for (cap_on, k, normal) in [(o.cap_start, 0usize, -dir), (o.cap_end, steps, dir)] {
            if cap_on {
                let rings: Vec<Vec<u32>> =
                    closed.iter().map(|i| (0..cs[*i].pts.len()).map(|j| base[*i] + (k * cs[*i].pts.len() + j) as u32).collect()).collect();
                add_cap(&mut polys, &positions, &rings, normal)?;
            }
        }
    }
    rebuild(positions, polys)
}

// ---------------------------------------------------------------- revolve

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RevolveOptions {
    /// Degrees to turn: 360 for a full turn (the last ring joins the first).
    pub angle: f32,
    /// Divisions around the turn.
    pub steps: u32,
    /// The axis (through the origin); `Auto` is Y.
    pub axis: Axis,
    /// Cap the two ends of a partial turn (closed curves).
    pub caps: bool,
}

impl Default for RevolveOptions {
    fn default() -> Self {
        RevolveOptions { angle: 360.0, steps: 32, axis: Axis::Y, caps: true }
    }
}

impl RevolveOptions {
    pub fn sanitized(self) -> RevolveOptions {
        RevolveOptions { angle: if self.angle.is_finite() { self.angle.clamp(-360.0, 360.0) } else { 360.0 }, steps: self.steps.clamp(1, MAX_STEPS), ..self }
    }
}

/// Turn `curves` about an axis through the origin.
pub fn revolve(curves: &[Polyline], options: &RevolveOptions) -> OpResult {
    use glam::Quat;
    let o = options.sanitized();
    let mut cs = prepare(curves)?;
    let axis = o.axis.vector().unwrap_or(Vec3::Y);
    if o.angle.abs() < 1e-4 {
        return Err(OpError::Invalid("the revolve angle is zero".into()));
    }
    let full = o.angle.abs() >= 359.999;
    // A full turn needs at least three steps to enclose anything.
    let steps = (if full { o.steps.max(3) } else { o.steps }) as usize;
    let rings = if full { steps } else { steps + 1 };
    let edges: usize = cs.iter().map(Curve::edges).sum();
    check_faces(edges.saturating_mul(steps).saturating_add(cs.iter().map(|c| c.pts.len()).sum::<usize>() * 2))?;
    // The plane that holds the profile: its radial direction (from the axis to the curves' middle)
    // and the axis. Outlines run counter-clockwise about `radial × axis` there.
    let all: Vec<Vec3> = cs.iter().flat_map(|c| c.pts.iter().copied()).collect();
    let mid = all.iter().copied().sum::<Vec3>() / all.len().max(1) as f32;
    let radial = (mid - axis * mid.dot(axis)).try_normalize().unwrap_or_else(|| axis.any_orthonormal_vector());
    // A turn the other way round turns the outlines round too.
    orient(&mut cs, radial.cross(axis) * o.angle.signum())?;
    let scale = all.iter().map(|p| p.length()).fold(0.0f32, f32::max).max(1e-6);
    let tol = scale * 1e-5;
    let mut positions: Vec<Vec3> = Vec::new();
    // index[c][k][i]: the vertex of point i of curve c on ring k (shared on the axis and, for a full turn, wrapped).
    let mut index: Vec<Vec<Vec<u32>>> = Vec::new();
    for c in &cs {
        let mut per_ring: Vec<Vec<u32>> = Vec::with_capacity(rings);
        let mut axis_vertex: Vec<Option<u32>> = vec![None; c.pts.len()];
        for k in 0..rings {
            let theta = (o.angle * k as f32 / steps as f32).to_radians();
            let q = Quat::from_axis_angle(axis, theta);
            let mut row = Vec::with_capacity(c.pts.len());
            for (i, p) in c.pts.iter().enumerate() {
                let on_axis = (*p - axis * p.dot(axis)).length() <= tol;
                if on_axis && let Some(v) = axis_vertex[i] {
                    row.push(v);
                    continue;
                }
                let v = positions.len() as u32;
                positions.push(q * *p);
                if on_axis {
                    axis_vertex[i] = Some(v);
                }
                row.push(v);
            }
            per_ring.push(row);
        }
        index.push(per_ring);
    }
    let mut polys = Vec::new();
    for (ci, c) in cs.iter().enumerate() {
        let n = c.pts.len();
        let along = c.along();
        for i in 0..c.edges() {
            for k in 0..steps {
                let k1 = if full { (k + 1) % steps } else { k + 1 };
                let (v0, v1) = (k as f32 / steps as f32, (k + 1) as f32 / steps as f32);
                let (u0, u1) = (along[i], along[i + 1]);
                let g = |ring: usize, p: usize| index[ci][ring][p % n];
                // Outlines are counter-clockwise about radial × axis, so this winding faces outwards.
                push_quad(
                    &mut polys,
                    [g(k, i), g(k1, i), g(k1, i + 1), g(k, i + 1)],
                    [Vec2::new(v0, u0), Vec2::new(v1, u0), Vec2::new(v1, u1), Vec2::new(v0, u1)],
                );
            }
        }
    }
    if !full && o.caps && cs.iter().any(|c| c.closed) {
        let closed: Vec<usize> = (0..cs.len()).filter(|i| cs[*i].closed).collect();
        // Moving in the positive direction about the axis at the profile: axis × radial; at the far
        // end the profile has turned with it.
        let motion = axis.cross(radial) * o.angle.signum();
        let far = Quat::from_axis_angle(axis, o.angle.to_radians()) * motion;
        for (k, normal) in [(0usize, -motion), (rings - 1, far)] {
            let ring_idx: Vec<Vec<u32>> = closed.iter().map(|i| index[*i][k].clone()).collect();
            add_cap(&mut polys, &positions, &ring_idx, normal)?;
        }
    }
    rebuild(positions, polys)
}

#[cfg(test)]
mod tests;
