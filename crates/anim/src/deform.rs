//! Point deformations that rigs are built from, on plain point lists.
//!
//! - [`morph`]: blend shapes (morph targets) with in-between shapes.
//! - [`ffd`]: free-form deformation by a lattice of control points (Sederberg and Parry,
//!   "Free-form deformation of solid geometric models", 1986), as a trivariate Bézier volume or
//!   cell by cell.
//! - [`cluster`]: a weighted set of points following a transform.
//!
//! Every function leaves points it cannot handle (indices out of range, non-finite numbers)
//! where they are.

use glam::{Mat4, Vec3};

/// Most cells along one axis of a lattice.
pub const MAX_LATTICE_DIVISIONS: u32 = 8;
/// Most in-between shapes of one blend-shape target.
pub const MAX_SHAPE_FRAMES: usize = 16;

/// One shape of a target: the offsets of some points, reached when the weight is `at`.
#[derive(Clone, Copy, Debug)]
pub struct ShapeFrame<'a> {
    pub at: f32,
    pub indices: &'a [u32],
    pub deltas: &'a [Vec3],
}

/// The share of each frame at `weight`: frames are passed through in order of `at`, linearly
/// from the rest shape (weight 0) to the first, between neighbours, and on past the last. A
/// negative weight scales the first frame backwards. Returns `(frame index, factor)` pairs.
pub fn frame_shares(ats: &[f32], weight: f32) -> Vec<(usize, f32)> {
    if !weight.is_finite() || weight == 0.0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..ats.len()).filter(|i| ats.get(*i).is_some_and(|a| a.is_finite() && *a > 1e-6)).collect();
    order.sort_by(|a, b| ats.get(*a).zip(ats.get(*b)).map_or(std::cmp::Ordering::Equal, |(x, y)| x.total_cmp(y)));
    let at = |i: usize| order.get(i).and_then(|f| ats.get(*f)).copied().unwrap_or(1.0);
    let (Some(first), Some(last)) = (order.first().copied(), order.last().copied()) else { return Vec::new() };
    if weight <= at(0) {
        return vec![(first, weight / at(0))];
    }
    for i in 1..order.len() {
        if weight <= at(i) {
            let (Some(a), Some(b)) = (order.get(i - 1).copied(), order.get(i).copied()) else { break };
            let span = at(i) - at(i - 1);
            let t = if span > 1e-6 { (weight - at(i - 1)) / span } else { 1.0 };
            return vec![(a, 1.0 - t), (b, t)];
        }
    }
    // Past the last shape: keep going the way the last one goes.
    vec![(last, weight / at(order.len() - 1))]
}

/// Add a blend-shape target to `points` at `weight`.
pub fn morph(points: &mut [Vec3], frames: &[ShapeFrame<'_>], weight: f32) {
    let ats: Vec<f32> = frames.iter().map(|f| f.at).collect();
    for (frame, share) in frame_shares(&ats, weight) {
        let Some(f) = frames.get(frame) else { continue };
        if !share.is_finite() {
            continue;
        }
        for (i, d) in f.indices.iter().zip(f.deltas) {
            if let Some(p) = points.get_mut(*i as usize)
                && d.is_finite()
            {
                *p += *d * share;
            }
        }
    }
}

/// The offsets that turn `base` into `shape`, leaving out points that move less than
/// `tolerance`. `None` when the two lists differ in length.
pub fn shape_delta(base: &[Vec3], shape: &[Vec3], tolerance: f32) -> Option<(Vec<u32>, Vec<Vec3>)> {
    if base.len() != shape.len() || base.len() > u32::MAX as usize {
        return None;
    }
    let mut indices = Vec::new();
    let mut deltas = Vec::new();
    for (i, (a, b)) in base.iter().zip(shape).enumerate() {
        let d = *b - *a;
        if d.is_finite() && d.length() > tolerance {
            indices.push(i as u32);
            deltas.push(d);
        }
    }
    Some((indices, deltas))
}

/// Control points per axis for `divisions` cells per axis (each clamped to 1..=8).
pub fn lattice_counts(divisions: [u32; 3]) -> [usize; 3] {
    divisions.map(|d| d.clamp(1, MAX_LATTICE_DIVISIONS) as usize + 1)
}

/// The control points of an undeformed lattice: a regular grid over the cube from −0.5 to 0.5,
/// X fastest, then Y, then Z.
pub fn lattice_rest(divisions: [u32; 3]) -> Vec<Vec3> {
    let [nx, ny, nz] = lattice_counts(divisions);
    let at = |i: usize, n: usize| i as f32 / (n - 1) as f32 - 0.5;
    let mut out = Vec::with_capacity(nx * ny * nz);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                out.push(Vec3::new(at(i, nx), at(j, ny), at(k, nz)));
            }
        }
    }
    out
}

/// The Bernstein polynomials of degree `n − 1` at `t` (`n` values).
fn bernstein(n: usize, t: f32, out: &mut [f32; 9]) {
    // De Casteljau on the basis: start with degree 0 and raise the degree.
    *out = [0.0; 9];
    out[0] = 1.0;
    for degree in 1..n.min(9) {
        for i in (0..=degree).rev() {
            let left = if i > 0 { out.get(i - 1).copied().unwrap_or(0.0) } else { 0.0 };
            let here = out.get(i).copied().unwrap_or(0.0);
            if let Some(slot) = out.get_mut(i) {
                *slot = here * (1.0 - t) + left * t;
            }
        }
    }
}

/// Deform a point given in the lattice's own space (the cube from −0.5 to 0.5). Points outside
/// the cube, and any point when `points` does not match `divisions`, come back unchanged.
/// `linear` interpolates inside each cell instead of over the whole lattice: control points then
/// act locally and the surface passes through them.
pub fn ffd(p: Vec3, divisions: [u32; 3], points: &[Vec3], linear: bool) -> Vec3 {
    p + ffd_offset(p, divisions, points, linear)
}

/// How far [`ffd`] moves a point: the blend of the control points' offsets from their rest
/// places, so an undeformed lattice gives exactly zero.
pub fn ffd_offset(p: Vec3, divisions: [u32; 3], points: &[Vec3], linear: bool) -> Vec3 {
    let [nx, ny, nz] = lattice_counts(divisions);
    const EDGE: f32 = 0.5 + 1e-5;
    if points.len() != nx * ny * nz || !p.is_finite() || p.x.abs() > EDGE || p.y.abs() > EDGE || p.z.abs() > EDGE {
        return Vec3::ZERO;
    }
    let s = (p + Vec3::splat(0.5)).clamp(Vec3::ZERO, Vec3::ONE);
    let rest = |i: usize, n: usize| i as f32 / (n - 1) as f32 - 0.5;
    let offset = |i: usize, j: usize, k: usize| points.get(i + nx * (j + ny * k)).map_or(Vec3::ZERO, |c| *c - Vec3::new(rest(i, nx), rest(j, ny), rest(k, nz)));
    let out = if linear {
        let cell = |t: f32, n: usize| {
            let x = t * (n - 1) as f32;
            let i = (x.floor() as usize).min(n.saturating_sub(2));
            (i, x - i as f32)
        };
        let ((i, u), (j, v), (k, w)) = (cell(s.x, nx), cell(s.y, ny), cell(s.z, nz));
        let mut sum = Vec3::ZERO;
        for (dk, fk) in [(0, 1.0 - w), (1, w)] {
            for (dj, fj) in [(0, 1.0 - v), (1, v)] {
                for (di, fi) in [(0, 1.0 - u), (1, u)] {
                    sum += offset(i + di, j + dj, k + dk) * (fi * fj * fk);
                }
            }
        }
        sum
    } else {
        let (mut bx, mut by, mut bz) = ([0.0; 9], [0.0; 9], [0.0; 9]);
        bernstein(nx, s.x, &mut bx);
        bernstein(ny, s.y, &mut by);
        bernstein(nz, s.z, &mut bz);
        let mut sum = Vec3::ZERO;
        for (k, wk) in bz.iter().enumerate().take(nz) {
            for (j, wj) in by.iter().enumerate().take(ny) {
                for (i, wi) in bx.iter().enumerate().take(nx) {
                    sum += offset(i, j, k) * (wi * wj * wk);
                }
            }
        }
        sum
    };
    if out.is_finite() { out } else { Vec3::ZERO }
}

/// Move the listed points by `motion`, each by its weight (0 stays, 1 follows fully).
pub fn cluster(points: &mut [Vec3], indices: &[u32], weights: &[f32], motion: Mat4) {
    if !motion.is_finite() {
        return;
    }
    for (n, i) in indices.iter().enumerate() {
        let w = weights.get(n).copied().unwrap_or(1.0);
        if let Some(p) = points.get_mut(*i as usize)
            && w.is_finite()
        {
            let moved = motion.transform_point3(*p);
            if moved.is_finite() {
                *p = p.lerp(moved, w.clamp(0.0, 1.0));
            }
        }
    }
}

/// Soft cluster weights: 1 at `center`, falling smoothly to 0 at `radius`. Returns the points
/// inside the radius and their weights.
pub fn radial_weights(points: &[Vec3], center: Vec3, radius: f32) -> (Vec<u32>, Vec<f32>) {
    let mut indices = Vec::new();
    let mut weights = Vec::new();
    if !radius.is_finite() || radius <= 0.0 || !center.is_finite() || points.len() > u32::MAX as usize {
        return (indices, weights);
    }
    for (i, p) in points.iter().enumerate() {
        let d = p.distance(center) / radius;
        if d < 1.0 {
            indices.push(i as u32);
            weights.push(1.0 - d * d * (3.0 - 2.0 * d));
        }
    }
    (indices, weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn a_target_blends_in_and_passes_through_its_in_betweens() {
        let rest = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        // Point 0 goes up by 2 at full weight, but through a sideways bulge at half weight.
        let full = ShapeFrame { at: 1.0, indices: &[0], deltas: &[Vec3::Y * 2.0] };
        let half = ShapeFrame { at: 0.5, indices: &[0, 2], deltas: &[Vec3::new(1.0, 1.0, 0.0), Vec3::Z] };
        let at = |w: f32| {
            let mut p = rest.clone();
            // Frames given out of order on purpose.
            morph(&mut p, &[full, half], w);
            p
        };
        assert_eq!(at(0.0), rest);
        assert_eq!(at(0.5)[0], Vec3::new(1.0, 1.0, 0.0));
        assert_eq!(at(0.5)[2], Vec3::Y + Vec3::Z, "a point only the in-between moves");
        assert_eq!(at(1.0)[0], Vec3::Y * 2.0);
        assert_eq!(at(1.0)[2], Vec3::Y);
        assert_eq!(at(0.25)[0], Vec3::new(0.5, 0.5, 0.0));
        assert_eq!(at(0.75)[0], Vec3::new(0.5, 1.5, 0.0));
        assert_eq!(at(1.5)[0], Vec3::Y * 3.0, "past the last shape it keeps going");
        assert_eq!(at(-0.5)[0], Vec3::new(-1.0, -1.0, 0.0), "a negative weight goes the other way");
        assert_eq!(at(1.0)[1], Vec3::X, "untouched points stay");
        // Hostile: indices out of range, NaN deltas and weights change nothing.
        let mut p = rest.clone();
        morph(&mut p, &[ShapeFrame { at: 1.0, indices: &[9, 0], deltas: &[Vec3::ONE, Vec3::NAN] }], 1.0);
        morph(&mut p, &[full], f32::NAN);
        morph(&mut p, &[ShapeFrame { at: 0.0, indices: &[0], deltas: &[Vec3::ONE] }], 1.0);
        assert_eq!(p, rest);
    }

    #[test]
    fn shape_deltas_are_sparse_and_round_trip() {
        let base = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let shape = vec![Vec3::ZERO, Vec3::X + Vec3::Z, Vec3::Y];
        let (i, d) = shape_delta(&base, &shape, 1e-6).unwrap();
        assert_eq!((i.clone(), d.clone()), (vec![1], vec![Vec3::Z]));
        let mut p = base.clone();
        morph(&mut p, &[ShapeFrame { at: 1.0, indices: &i, deltas: &d }], 1.0);
        assert_eq!(p, shape);
        assert!(shape_delta(&base, &shape[..2], 1e-6).is_none());
    }

    #[test]
    fn a_lattice_at_rest_changes_nothing_and_a_moved_point_pulls_smoothly() {
        for div in [[1, 1, 1], [2, 3, 4], [8, 8, 8], [0, 99, 2]] {
            let rest = lattice_rest(div);
            let [nx, ny, nz] = lattice_counts(div);
            assert_eq!(rest.len(), nx * ny * nz);
            for linear in [false, true] {
                for p in [Vec3::ZERO, Vec3::new(0.3, -0.2, 0.45), Vec3::splat(0.5), Vec3::splat(-0.5)] {
                    assert_eq!(ffd(p, div, &rest, linear), p, "{div:?} {linear}: exactly unchanged");
                }
            }
        }
        // 2×2×2 cells: move the centre control point up.
        let div = [2, 2, 2];
        let mut pts = lattice_rest(div);
        pts[13] += Vec3::Y;
        let centre = ffd(Vec3::ZERO, div, &pts, false);
        assert!((centre.y - 0.125).abs() < 1e-5, "the Bézier volume is pulled by B₁(½)³ = ⅛: {centre:?}");
        assert!((ffd(Vec3::ZERO, div, &pts, true).y - 1.0).abs() < 1e-5, "cell by cell the surface goes through the point");
        assert_eq!(ffd(Vec3::splat(0.5), div, &pts, false), Vec3::splat(0.5), "corners stay with their control points");
        // Outside the lattice, or with the wrong point count: unchanged.
        assert_eq!(ffd(Vec3::new(0.6, 0.0, 0.0), div, &pts, false), Vec3::new(0.6, 0.0, 0.0));
        assert_eq!(ffd(Vec3::ZERO, div, &pts[..5], false), Vec3::ZERO);
        assert!(ffd(Vec3::NAN, div, &pts, false).is_nan());
    }

    #[test]
    fn clusters_follow_by_weight() {
        let mut p = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        cluster(&mut p, &[0, 1, 7], &[1.0, 0.5], Mat4::from_translation(Vec3::Z * 2.0));
        assert_eq!(p, vec![Vec3::Z * 2.0, Vec3::X + Vec3::Z, Vec3::Y]);
        cluster(&mut p, &[2], &[], Mat4::from_translation(Vec3::Z));
        assert_eq!(p[2], Vec3::Y + Vec3::Z, "a missing weight counts as 1");
        cluster(&mut p, &[0], &[1.0], Mat4::NAN);
        assert_eq!(p[0], Vec3::Z * 2.0);
        let (i, w) = radial_weights(&[Vec3::ZERO, Vec3::X * 0.5, Vec3::X * 2.0], Vec3::ZERO, 1.0);
        assert_eq!(i, vec![0, 1]);
        assert!((w[0] - 1.0).abs() < 1e-6 && (w[1] - 0.5).abs() < 1e-6);
        assert!(radial_weights(&[Vec3::ZERO], Vec3::ZERO, -1.0).0.is_empty());
    }

    proptest! {
        /// Moving every control point by the same offset moves every inside point by it
        /// (partition of unity), and the result stays within the control points' bounds.
        #[test]
        fn ffd_is_a_convex_combination(
            dx in 1u32..5, dy in 1u32..5, dz in 1u32..5,
            p in (-0.5f32..0.5, -0.5f32..0.5, -0.5f32..0.5),
            shift in (-2.0f32..2.0, -2.0f32..2.0, -2.0f32..2.0),
            noise in proptest::collection::vec(-0.3f32..0.3, 3 * 125),
            linear in any::<bool>(),
        ) {
            let div = [dx, dy, dz];
            let p = Vec3::new(p.0, p.1, p.2);
            let shift = Vec3::new(shift.0, shift.1, shift.2);
            let moved: Vec<Vec3> = lattice_rest(div).into_iter().map(|c| c + shift).collect();
            prop_assert!((ffd(p, div, &moved, linear) - (p + shift)).length() < 1e-4);
            let bumpy: Vec<Vec3> = lattice_rest(div).into_iter().enumerate().map(|(i, c)| c + Vec3::new(noise[3 * i], noise[3 * i + 1], noise[3 * i + 2])).collect();
            let out = ffd(p, div, &bumpy, linear);
            let (lo, hi) = bumpy.iter().fold((Vec3::MAX, Vec3::MIN), |(a, b), c| (a.min(*c), b.max(*c)));
            prop_assert!(out.cmpge(lo - 1e-4).all() && out.cmple(hi + 1e-4).all());
        }

        #[test]
        fn frame_shares_are_continuous_and_exact_at_the_frames(ats in proptest::collection::vec(0.05f32..1.0, 1..5), w in -1.0f32..2.0) {
            // The blended delta of a 1-D target whose frame i has delta `at²` (any curve through the frames).
            let value = |w: f32| frame_shares(&ats, w).into_iter().map(|(i, s)| ats[i] * ats[i] * s).sum::<f32>();
            for a in &ats {
                prop_assert!((value(*a) - a * a).abs() < 1e-4, "at its own weight a frame shows exactly");
            }
            prop_assert!((value(w + 1e-4) - value(w)).abs() < 1e-2, "no jumps");
            prop_assert!(value(0.0) == 0.0);
        }
    }
}
