//! Skinning: binding the points of a mesh to joints and moving them with the joints.
//!
//! - [`SkinWeights`]: for every point, up to [`MAX_INFLUENCES`] joints and their weights, with
//!   normalising, pruning, limiting and mirroring.
//! - [`linear_blend`]: each point is the weighted sum of where each joint would carry it
//!   (Magnenat-Thalmann et al., "Joint-dependent local deformations for hand animation", 1988).
//! - [`dual_quaternion`]: the joints' rigid motions are blended as unit dual quaternions, which
//!   keeps volume where linear blending collapses under twist (Kavan, Collins, Žára, O'Sullivan,
//!   "Skinning with dual quaternions", I3D 2007).
//! - [`heat_weights`]: automatic weights as the equilibrium of heat flowing from each bone over
//!   the surface (Baran and Popović, "Automatic rigging and animation of 3D characters",
//!   SIGGRAPH 2007, section 4), solved with conjugate gradients. [`distance_weights`] is the
//!   cheap fallback for meshes without usable connectivity.
//!
//! All functions take plain slices and never panic on mismatched or non-finite input: bad
//! indices are skipped, bad weights dropped.

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

/// Most joints one point can follow.
pub const MAX_INFLUENCES: usize = 8;
/// Most joints one skin can use (joint indices are `u16`).
pub const MAX_JOINTS: usize = 4096;

/// Per-point joint weights in a fixed layout: point `v` owns `joints[v·k .. (v+1)·k]` and the
/// matching `weights`, where `k` is `influences`. Unused slots have weight 0.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinWeights {
    pub influences: usize,
    pub joints: Vec<u16>,
    pub weights: Vec<f32>,
}

impl SkinWeights {
    pub fn new(points: usize, influences: usize) -> SkinWeights {
        let k = influences.clamp(1, MAX_INFLUENCES);
        SkinWeights { influences: k, joints: vec![0; points * k], weights: vec![0.0; points * k] }
    }

    pub fn points(&self) -> usize {
        self.weights.len().checked_div(self.influences).unwrap_or(0)
    }
    pub fn heap_bytes(&self) -> usize {
        self.joints.len() * 2 + self.weights.len() * 4
    }

    /// The joints and weights of point `v` (empty when out of range).
    pub fn of(&self, v: usize) -> impl Iterator<Item = (u16, f32)> + '_ {
        let a = v.saturating_mul(self.influences);
        let j = self.joints.get(a..a + self.influences).unwrap_or_default();
        let w = self.weights.get(a..a + self.influences).unwrap_or_default();
        j.iter().copied().zip(w.iter().copied()).filter(|(_, w)| *w > 0.0)
    }

    /// The weight of `joint` on point `v`.
    pub fn weight(&self, v: usize, joint: u16) -> f32 {
        self.of(v).filter(|(j, _)| *j == joint).map(|(_, w)| w).sum()
    }

    /// Set the influences of point `v`: the strongest `influences` entries are kept and
    /// normalised to sum to one. Non-finite and non-positive weights are dropped.
    pub fn set(&mut self, v: usize, entries: &[(u16, f32)]) {
        let k = self.influences;
        let a = v.saturating_mul(k);
        if k == 0 || a + k > self.weights.len() {
            return;
        }
        // Merge repeated joints, then keep the strongest.
        let mut list: Vec<(u16, f32)> = Vec::with_capacity(entries.len());
        for (j, w) in entries.iter().copied().filter(|(_, w)| w.is_finite() && *w > 0.0) {
            match list.iter_mut().find(|(x, _)| *x == j) {
                Some((_, sum)) => *sum += w,
                None => list.push((j, w)),
            }
        }
        list.sort_by(|a, b| b.1.total_cmp(&a.1));
        list.truncate(k);
        let total: f32 = list.iter().map(|(_, w)| w).sum();
        for i in 0..k {
            let (j, w) = list.get(i).copied().unwrap_or((0, 0.0));
            if let (Some(js), Some(ws)) = (self.joints.get_mut(a + i), self.weights.get_mut(a + i)) {
                *js = j;
                *ws = if total > 0.0 { w / total } else { 0.0 };
            }
        }
    }

    /// Give `joint` the weight `w` (0..=1) on point `v` and scale the point's other joints so
    /// the total stays one. When the point has no other joint, `joint` gets everything.
    pub fn set_weight(&mut self, v: usize, joint: u16, w: f32) {
        let w = if w.is_finite() { w.clamp(0.0, 1.0) } else { return };
        let others: Vec<(u16, f32)> = self.of(v).filter(|(j, _)| *j != joint).collect();
        let rest: f32 = others.iter().map(|(_, x)| x).sum();
        let mut entries: Vec<(u16, f32)> = Vec::with_capacity(others.len() + 1);
        if rest > 0.0 {
            entries.extend(others.iter().map(|(j, x)| (*j, x / rest * (1.0 - w))));
            entries.push((joint, w));
        } else {
            entries.push((joint, 1.0));
        }
        self.set(v, &entries);
    }

    /// Repair weights read from a file: the layout must fit, joints must exist, weights must be
    /// finite and non-negative; every point is re-normalised. Returns `false` when the layout
    /// cannot be used at all (the caller should drop the skin).
    pub fn sanitize(&mut self, joint_count: usize) -> bool {
        let k = self.influences;
        if k == 0 || k > MAX_INFLUENCES || self.joints.len() != self.weights.len() || !self.weights.len().is_multiple_of(k) {
            return false;
        }
        for (j, w) in self.joints.iter_mut().zip(self.weights.iter_mut()) {
            if (*j as usize) >= joint_count || !w.is_finite() || *w < 0.0 {
                *j = 0;
                *w = 0.0;
            }
        }
        self.normalize();
        true
    }

    /// Make every point's weights sum to one (points without any weight are left at zero).
    pub fn normalize(&mut self) {
        let k = self.influences.max(1);
        for chunk in self.weights.chunks_mut(k) {
            let total: f32 = chunk.iter().sum();
            // Already normalised weights are left bit for bit as they are (files round-trip).
            if total > 0.0 && (total - 1.0).abs() > 1e-6 {
                for w in chunk {
                    *w /= total;
                }
            }
        }
    }

    /// Drop weights below `threshold` and re-normalise. A point keeps at least its strongest
    /// joint. Returns how many weights were dropped.
    pub fn prune(&mut self, threshold: f32) -> usize {
        let k = self.influences.max(1);
        let threshold = if threshold.is_finite() { threshold.max(0.0) } else { 0.0 };
        let mut dropped = 0;
        for chunk in self.weights.chunks_mut(k) {
            let strongest = chunk.iter().copied().fold(0.0f32, f32::max);
            for w in chunk.iter_mut() {
                if *w > 0.0 && *w < threshold && *w < strongest {
                    *w = 0.0;
                    dropped += 1;
                }
            }
        }
        self.normalize();
        dropped
    }

    /// Keep at most `max` joints per point (the strongest) and re-normalise.
    pub fn limit(&mut self, max: usize) {
        let max = max.clamp(1, MAX_INFLUENCES);
        for v in 0..self.points() {
            let mut list: Vec<(u16, f32)> = self.of(v).collect();
            if list.len() > max {
                list.sort_by(|a, b| b.1.total_cmp(&a.1));
                list.truncate(max);
                self.set(v, &list);
            }
        }
    }

    /// Mirror weights across the plane through the origin with normal `axis` (0 X, 1 Y, 2 Z):
    /// every point on the negative side takes the weights of the nearest point to its mirror
    /// image on the positive side (within `tolerance`), with joints renamed by `joint_map`
    /// (`joint_map[j]` is the joint mirroring `j`). Returns how many points were updated.
    pub fn mirror(&mut self, positions: &[Vec3], axis: usize, joint_map: &[u16], tolerance: f32) -> usize {
        let axis = axis.min(2);
        let tol = if tolerance.is_finite() { tolerance.max(1e-6) } else { 1e-3 };
        let n = self.points().min(positions.len());
        // Bucket the positive side on a grid of the tolerance.
        let cell = |p: Vec3| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64, (p.z / tol).floor() as i64);
        let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<u32>> = std::collections::HashMap::new();
        for (i, p) in positions.iter().enumerate().take(n) {
            if p.is_finite() && p.to_array().get(axis).is_some_and(|c| *c >= 0.0) {
                grid.entry(cell(*p)).or_default().push(i as u32);
            }
        }
        let mut updated = 0;
        for v in 0..n {
            let Some(p) = positions.get(v).copied().filter(|p| p.is_finite()) else { continue };
            if p.to_array().get(axis).is_none_or(|c| *c >= 0.0) {
                continue;
            }
            let mut m = p.to_array();
            if let Some(c) = m.get_mut(axis) {
                *c = -*c;
            }
            let m = Vec3::from_array(m);
            let (cx, cy, cz) = cell(m);
            let mut best: Option<(f32, u32)> = None;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        for i in grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                            let d = positions.get(*i as usize).map_or(f32::MAX, |q| q.distance(m));
                            if d <= tol && best.is_none_or(|(b, _)| d < b) {
                                best = Some((d, *i));
                            }
                        }
                    }
                }
            }
            if let Some((_, src)) = best {
                let entries: Vec<(u16, f32)> = self.of(src as usize).map(|(j, w)| (joint_map.get(j as usize).copied().unwrap_or(j), w)).collect();
                self.set(v, &entries);
                updated += 1;
            }
        }
        updated
    }
}

/// One matrix per joint that carries a bind-pose point to its posed place:
/// `joint_world[i] · inverse_bind[i]`.
pub fn skin_matrices(joint_world: &[Mat4], inverse_bind: &[Mat4]) -> Vec<Mat4> {
    joint_world.iter().zip(inverse_bind).map(|(w, ib)| if w.is_finite() && ib.is_finite() { *w * *ib } else { Mat4::IDENTITY }).collect()
}

/// Points from which skinning runs on every core: below this the threads cost more than they
/// save.
pub const PARALLEL_FROM: usize = 8_192;
/// Most pieces a mesh is cut into for skinning, and so the most cores it uses.
pub const MAX_PIECES: usize = 8;

/// Fill `out` with `one(index, point)` for every point, on every core for large meshes. The
/// result is the same either way: each point is computed alone, in the same order of operations.
fn for_each_point(positions: &[Vec3], out: &mut Vec<Vec3>, parallel: bool, one: impl Fn(usize, &Vec3) -> Vec3 + Sync) {
    use rayon::prelude::*;
    out.clear();
    if parallel && positions.len() >= PARALLEL_FROM {
        // A handful of large pieces, not one per core: skinning is memory-bound, so more threads
        // add little, and on a busy machine many small pieces cost more than they save (measured:
        // docs/performance.md).
        let piece = positions.len().div_ceil(MAX_PIECES).max(2_048);
        positions.par_iter().enumerate().with_min_len(piece).map(|(v, p)| one(v, p)).collect_into_vec(out);
    } else {
        out.reserve(positions.len());
        out.extend(positions.iter().enumerate().map(|(v, p)| one(v, p)));
    }
}

/// Linear blend skinning. `out` gets one position per input point; a point without weights
/// stays where it is. Large meshes are skinned on every core.
pub fn linear_blend(positions: &[Vec3], weights: &SkinWeights, matrices: &[Mat4], out: &mut Vec<Vec3>) {
    linear_blend_with(positions, weights, matrices, out, true);
}

/// [`linear_blend`] with the choice of running on one core (`parallel` false).
pub fn linear_blend_with(positions: &[Vec3], weights: &SkinWeights, matrices: &[Mat4], out: &mut Vec<Vec3>, parallel: bool) {
    let k = weights.influences.max(1);
    for_each_point(positions, out, parallel, |v, p| {
        let a = v * k;
        let (Some(js), Some(ws)) = (weights.joints.get(a..a + k), weights.weights.get(a..a + k)) else { return *p };
        let mut sum = Vec3::ZERO;
        let mut total = 0.0;
        for (j, w) in js.iter().zip(ws) {
            if *w > 0.0
                && let Some(m) = matrices.get(*j as usize)
            {
                sum += m.transform_point3(*p) * *w;
                total += *w;
            }
        }
        if total > 0.0 { sum / total } else { *p }
    });
}

/// A rigid motion as a unit dual quaternion: `real` is the rotation, `dual` = ½·t·real.
#[derive(Clone, Copy, Debug)]
struct DualQuat {
    real: Quat,
    dual: Quat,
}

impl DualQuat {
    fn from_matrix(m: &Mat4) -> DualQuat {
        let (_, r, t) = m.to_scale_rotation_translation();
        let r = if r.is_finite() { r.normalize() } else { Quat::IDENTITY };
        let t = if t.is_finite() { t } else { Vec3::ZERO };
        let tq = Quat::from_xyzw(t.x, t.y, t.z, 0.0);
        DualQuat { real: r, dual: (tq * r) * 0.5 }
    }
    fn transform(&self, p: Vec3) -> Vec3 {
        // t = 2 · dual · conj(real)
        let t = (self.dual * self.real.conjugate()) * 2.0;
        self.real * p + Vec3::new(t.x, t.y, t.z)
    }
}

/// Dual-quaternion skinning. The joints' rotations and translations are blended; any scale in
/// the matrices is ignored. `out` gets one position per input point. Large meshes are skinned on
/// every core.
pub fn dual_quaternion(positions: &[Vec3], weights: &SkinWeights, matrices: &[Mat4], out: &mut Vec<Vec3>) {
    dual_quaternion_with(positions, weights, matrices, out, true);
}

/// [`dual_quaternion`] with the choice of running on one core (`parallel` false).
pub fn dual_quaternion_with(positions: &[Vec3], weights: &SkinWeights, matrices: &[Mat4], out: &mut Vec<Vec3>, parallel: bool) {
    let dqs: Vec<DualQuat> = matrices.iter().map(DualQuat::from_matrix).collect();
    let k = weights.influences.max(1);
    for_each_point(positions, out, parallel, |v, p| {
        let a = v * k;
        let (Some(js), Some(ws)) = (weights.joints.get(a..a + k), weights.weights.get(a..a + k)) else { return *p };
        let mut real = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
        let mut dual = real;
        let mut pivot: Option<Quat> = None;
        for (j, w) in js.iter().zip(ws) {
            if *w > 0.0
                && let Some(dq) = dqs.get(*j as usize)
            {
                // q and −q are the same rotation: blend on the pivot's side of the sphere.
                let sign = match pivot {
                    Some(q) if q.dot(dq.real) < 0.0 => -*w,
                    _ => *w,
                };
                pivot.get_or_insert(dq.real);
                real += dq.real * sign;
                dual += dq.dual * sign;
            }
        }
        let len = real.length();
        if len < 1e-8 || !len.is_finite() {
            return *p;
        }
        DualQuat { real: real * (1.0 / len), dual: dual * (1.0 / len) }.transform(*p)
    });
}

/// Distance from `p` to the segment `a`–`b`.
pub fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-12 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// Weights from distance alone: each point follows the bones nearest to it, with weight falling
/// off as 1/d⁴. `bones[i]` is the segment of joint `i` (head, tail) in the mesh's space.
pub fn distance_weights(positions: &[Vec3], bones: &[(Vec3, Vec3)], influences: usize) -> SkinWeights {
    let mut out = SkinWeights::new(positions.len(), influences);
    let bones = bones.get(..bones.len().min(MAX_JOINTS)).unwrap_or_default();
    for (v, p) in positions.iter().enumerate() {
        let mut entries: Vec<(u16, f32)> = bones
            .iter()
            .enumerate()
            .map(|(j, (a, b))| {
                let d = segment_distance(*p, *a, *b).max(1e-4);
                (j as u16, 1.0 / (d * d * d * d))
            })
            .filter(|(_, w)| w.is_finite())
            .collect();
        entries.sort_by(|a, b| b.1.total_cmp(&a.1));
        entries.truncate(out.influences);
        out.set(v, &entries);
    }
    out
}

/// Automatic weights by heat equilibrium (Baran and Popović 2007, section 4).
///
/// For each bone `i` the weights `wⁱ` over the points solve `(L + H)·wⁱ = H·pⁱ`, where `L` is the
/// Laplacian of the surface (here the graph Laplacian of `edges`, each edge weighing 1), `H` is
/// diagonal with `H_jj = 1/d(j)²` for the distance `d(j)` from point `j` to its nearest bone, and
/// `pⁱ_j` is 1 when bone `i` is that nearest bone (shared equally when several bones are equally
/// near). Heat thus enters at the points nearest each
/// bone and spreads along the surface, so weights follow the mesh rather than straight-line
/// distance. The system is symmetric positive definite and is solved with Jacobi-preconditioned
/// conjugate gradients. The paper also switches a point's heat off when its nearest bone is not
/// visible from it; that visibility test is not done here.
///
/// `edges` are pairs of point indices (each undirected edge once). Returns the strongest
/// `influences` weights per point, normalised. Points not connected to anything keep their
/// nearest bone.
pub fn heat_weights(positions: &[Vec3], edges: &[(u32, u32)], bones: &[(Vec3, Vec3)], influences: usize) -> SkinWeights {
    let n = positions.len();
    let bones = bones.get(..bones.len().min(MAX_JOINTS)).unwrap_or_default();
    let mut out = SkinWeights::new(n, influences);
    if n == 0 || bones.is_empty() {
        return out;
    }
    // Adjacency in compressed rows.
    let mut degree = vec![0u32; n];
    for (a, b) in edges {
        if let (Some(_), Some(_)) = (positions.get(*a as usize), positions.get(*b as usize))
            && a != b
        {
            degree[*a as usize] += 1;
            degree[*b as usize] += 1;
        }
    }
    let mut start = vec![0usize; n + 1];
    for i in 0..n {
        start[i + 1] = start[i] + degree[i] as usize;
    }
    let mut fill = start.clone();
    let mut neighbours = vec![0u32; start[n]];
    for (a, b) in edges {
        let (ai, bi) = (*a as usize, *b as usize);
        if ai < n && bi < n && ai != bi {
            neighbours[fill[ai]] = *b;
            fill[ai] += 1;
            neighbours[fill[bi]] = *a;
            fill[bi] += 1;
        }
    }
    // Nearest bone and heat coefficient per point. Distances are measured in units of the mean
    // edge length so the result does not depend on the mesh's scale.
    let mean_edge = {
        let (mut sum, mut count) = (0.0f64, 0usize);
        for (a, b) in edges.iter().take(200_000) {
            if let (Some(p), Some(q)) = (positions.get(*a as usize), positions.get(*b as usize)) {
                sum += p.distance(*q) as f64;
                count += 1;
            }
        }
        if count > 0 && sum > 0.0 { (sum / count as f64) as f32 } else { 1.0 }
    };
    // A point equally near several bones (a ring around a joint) shares its heat between them.
    let near = |d: f32, best: f32| d <= best * (1.0 + 1e-4) + 1e-6;
    let mut nearest = vec![0u16; n];
    let mut nearest_d = vec![0.0f32; n];
    let mut ties = vec![1.0f32; n];
    let mut heat = vec![0.0f32; n];
    for (v, p) in positions.iter().enumerate() {
        let (mut best, mut best_d) = (0u16, f32::MAX);
        for (j, (a, b)) in bones.iter().enumerate() {
            let d = segment_distance(*p, *a, *b);
            if d < best_d {
                best_d = d;
                best = j as u16;
            }
        }
        nearest[v] = best;
        nearest_d[v] = best_d;
        ties[v] = bones.iter().filter(|(a, b)| near(segment_distance(*p, *a, *b), best_d)).count().max(1) as f32;
        let d = (best_d / mean_edge).max(1e-3);
        heat[v] = if d.is_finite() { 1.0 / (d * d) } else { 0.0 };
    }
    let diagonal: Vec<f32> = (0..n).map(|i| degree[i] as f32 + heat[i]).collect();
    // y = (L + H)·x
    let apply = |x: &[f32], y: &mut [f32]| {
        for i in 0..n {
            let mut acc = diagonal[i] * x[i];
            for k in start[i]..start[i + 1] {
                acc -= x[neighbours[k] as usize];
            }
            y[i] = acc;
        }
    };
    let mut per_point: Vec<Vec<(u16, f32)>> = vec![Vec::new(); n];
    let (mut x, mut r, mut z, mut p, mut ap) = (vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n], vec![0.0f32; n]);
    let mut source = vec![0.0f32; n];
    for (bone, (head, tail)) in bones.iter().enumerate() {
        let bone = bone as u16;
        // pⁱ: this bone's share of each point's heat (1, or 1/ties where bones tie).
        let mut any = false;
        for i in 0..n {
            let mine = nearest[i] == bone || positions.get(i).is_some_and(|p| near(segment_distance(*p, *head, *tail), nearest_d[i]));
            source[i] = if mine { 1.0 / ties[i] } else { 0.0 };
            any |= mine;
        }
        if !any {
            continue;
        }
        // Right-hand side H·p, starting from x = p (a good first guess: 1 near the bone).
        x.copy_from_slice(&source);
        apply(&x, &mut ap);
        let mut rz = 0.0f64;
        for i in 0..n {
            r[i] = heat[i] * source[i] - ap[i];
            z[i] = if diagonal[i] > 0.0 { r[i] / diagonal[i] } else { 0.0 };
            p[i] = z[i];
            rz += (r[i] * z[i]) as f64;
        }
        let stop = rz * 1e-10;
        for _ in 0..400 {
            if rz <= stop || rz <= 1e-20 {
                break;
            }
            apply(&p, &mut ap);
            let pap: f64 = p.iter().zip(&ap).map(|(a, b)| (*a * *b) as f64).sum();
            if pap <= 0.0 || !pap.is_finite() {
                break;
            }
            let alpha = (rz / pap) as f32;
            let mut next = 0.0f64;
            for i in 0..n {
                x[i] += alpha * p[i];
                r[i] -= alpha * ap[i];
                z[i] = if diagonal[i] > 0.0 { r[i] / diagonal[i] } else { 0.0 };
                next += (r[i] * z[i]) as f64;
            }
            let beta = (next / rz) as f32;
            for i in 0..n {
                p[i] = z[i] + beta * p[i];
            }
            rz = next;
        }
        for (i, list) in per_point.iter_mut().enumerate() {
            let w = x[i];
            if w.is_finite() && w > 1e-4 {
                list.push((bone, w));
            }
        }
    }
    for (v, list) in per_point.iter().enumerate() {
        if list.is_empty() {
            out.set(v, &[(nearest[v], 1.0)]);
        } else {
            out.set(v, list);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skinning_on_every_core_gives_exactly_the_one_core_result() {
        let n = PARALLEL_FROM * 3 + 17;
        let positions: Vec<Vec3> = (0..n).map(|i| Vec3::new((i % 97) as f32 * 0.1, i as f32 / n as f32 * 8.0, (i % 13) as f32 * 0.3)).collect();
        let bones: Vec<(Vec3, Vec3)> = (0..8).map(|i| (Vec3::Y * i as f32, Vec3::Y * (i + 1) as f32)).collect();
        let weights = distance_weights(&positions, &bones, 4);
        let matrices: Vec<Mat4> = (0..8)
            .map(|i| Mat4::from_rotation_translation(Quat::from_rotation_z(0.2 * i as f32) * Quat::from_rotation_y(0.4), Vec3::new(i as f32 * 0.1, 0.0, 0.3)))
            .collect();
        let (mut one, mut many) = (Vec::new(), Vec::new());
        linear_blend_with(&positions, &weights, &matrices, &mut one, false);
        linear_blend_with(&positions, &weights, &matrices, &mut many, true);
        assert_eq!(one.len(), n);
        assert!(one == many, "linear blend: the same bits on one core and on all");
        dual_quaternion_with(&positions, &weights, &matrices, &mut one, false);
        dual_quaternion_with(&positions, &weights, &matrices, &mut many, true);
        assert!(one == many, "dual quaternion: the same bits on one core and on all");
        assert!(one.iter().all(|p| p.is_finite()) && one != positions);
        // Short lists and reused buffers: no stale tail.
        linear_blend(&positions[..5], &weights, &matrices, &mut many);
        assert_eq!(many.len(), 5);
    }
    use proptest::prelude::*;

    /// A tube along Y from 0 to `height`, `rings` + 1 rings of `sides` points, with its edges.
    fn tube(height: f32, radius: f32, rings: usize, sides: usize) -> (Vec<Vec3>, Vec<(u32, u32)>) {
        let mut pts = Vec::new();
        let mut edges = Vec::new();
        for r in 0..=rings {
            for s in 0..sides {
                let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                pts.push(Vec3::new(a.cos() * radius, height * r as f32 / rings as f32, a.sin() * radius));
                let i = (r * sides + s) as u32;
                edges.push((i, (r * sides + (s + 1) % sides) as u32));
                if r < rings {
                    edges.push((i, i + sides as u32));
                }
            }
        }
        (pts, edges)
    }

    #[test]
    fn weights_keep_the_strongest_and_sum_to_one() {
        let mut w = SkinWeights::new(2, 2);
        w.set(0, &[(3, 1.0), (1, 3.0), (2, 0.5), (1, 1.0), (9, f32::NAN), (4, -1.0)]);
        let got: Vec<(u16, f32)> = w.of(0).collect();
        assert_eq!(got, vec![(1, 0.8), (3, 0.2)]);
        assert_eq!(w.weight(0, 1), 0.8);
        assert_eq!(w.of(1).count(), 0);
        assert_eq!(w.of(99).count(), 0);
        w.set(99, &[(0, 1.0)]);
        // Prune drops small weights but never the last joint; limit keeps the strongest.
        let mut w = SkinWeights::new(1, 4);
        w.set(0, &[(0, 0.6), (1, 0.3), (2, 0.08), (3, 0.02)]);
        assert_eq!(w.prune(0.1), 2);
        assert!((w.weight(0, 0) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(w.prune(0.9), 1, "the strongest joint stays");
        assert_eq!(w.of(0).collect::<Vec<_>>(), vec![(0, 1.0)]);
        let mut w = SkinWeights::new(1, 4);
        w.set(0, &[(0, 0.4), (1, 0.3), (2, 0.2), (3, 0.1)]);
        w.limit(2);
        assert_eq!(w.of(0).count(), 2);
        assert!((w.of(0).map(|(_, x)| x).sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn setting_one_weight_rescales_the_others() {
        let mut w = SkinWeights::new(1, 4);
        w.set(0, &[(0, 0.5), (1, 0.25), (2, 0.25)]);
        w.set_weight(0, 3, 0.5);
        let got: Vec<(u16, f32)> = w.of(0).collect();
        assert_eq!(got, vec![(3, 0.5), (0, 0.25), (1, 0.125), (2, 0.125)]);
        w.set_weight(0, 3, 0.0);
        assert_eq!(w.weight(0, 3), 0.0);
        assert!((w.of(0).map(|(_, x)| x).sum::<f32>() - 1.0).abs() < 1e-6);
        let mut lone = SkinWeights::new(1, 2);
        lone.set_weight(0, 4, 0.2);
        assert_eq!(lone.of(0).collect::<Vec<_>>(), vec![(4, 1.0)], "a point always follows something");
        lone.set_weight(0, 4, f32::NAN);
        lone.set_weight(9, 4, 0.5);
        assert_eq!(lone.of(0).collect::<Vec<_>>(), vec![(4, 1.0)]);
    }

    #[test]
    fn hostile_weights_are_repaired_or_refused() {
        let mut w = SkinWeights { influences: 2, joints: vec![0, 7, 1, 1], weights: vec![2.0, 5.0, f32::NAN, -3.0] };
        assert!(w.sanitize(2));
        assert_eq!(w.of(0).collect::<Vec<_>>(), vec![(0, 1.0)], "joint 7 does not exist");
        assert_eq!(w.of(1).count(), 0);
        assert!(!SkinWeights { influences: 0, joints: vec![], weights: vec![] }.sanitize(1));
        assert!(!SkinWeights { influences: 3, joints: vec![0; 4], weights: vec![0.0; 4] }.sanitize(1));
        assert!(!SkinWeights { influences: 2, joints: vec![0; 4], weights: vec![0.0; 2] }.sanitize(1));
        assert!(!SkinWeights { influences: 99, joints: vec![0; 99], weights: vec![0.0; 99] }.sanitize(1));
        // Skinning with bad indices and short arrays leaves points in place.
        let w = SkinWeights { influences: 1, joints: vec![5], weights: vec![1.0] };
        let (mut a, mut b) = (Vec::new(), Vec::new());
        let pts = [Vec3::X, Vec3::Y];
        linear_blend(&pts, &w, &[Mat4::from_translation(Vec3::Z)], &mut a);
        dual_quaternion(&pts, &w, &[Mat4::from_translation(Vec3::Z)], &mut b);
        assert_eq!((a, b), (pts.to_vec(), pts.to_vec()));
    }

    #[test]
    fn both_methods_follow_a_rigid_motion_exactly() {
        let (pts, _) = tube(2.0, 0.5, 4, 8);
        let mut w = SkinWeights::new(pts.len(), 2);
        for v in 0..pts.len() {
            w.set(v, &[(0, 0.3), (1, 0.7)]);
        }
        let m = Mat4::from_rotation_translation(Quat::from_rotation_z(0.9) * Quat::from_rotation_x(-0.4), Vec3::new(1.0, -2.0, 0.5));
        let (mut a, mut b) = (Vec::new(), Vec::new());
        linear_blend(&pts, &w, &[m, m], &mut a);
        dual_quaternion(&pts, &w, &[m, m], &mut b);
        for ((p, x), y) in pts.iter().zip(&a).zip(&b) {
            let want = m.transform_point3(*p);
            assert!((want - *x).length() < 1e-5 && (want - *y).length() < 1e-5);
        }
        // The bind pose is the identity for both.
        let ident = skin_matrices(&[m, m], &[m.inverse(), m.inverse()]);
        linear_blend(&pts, &w, &ident, &mut a);
        assert!(pts.iter().zip(&a).all(|(p, q)| (*p - *q).length() < 1e-5));
    }

    #[test]
    fn dual_quaternions_keep_the_radius_where_linear_blending_collapses() {
        // A tube twisted half a turn between two joints: the ring halfway is weighted half and
        // half. Linear blending pulls it onto the axis; dual quaternions keep it round.
        let (pts, _) = tube(2.0, 0.5, 2, 16);
        let mut w = SkinWeights::new(pts.len(), 2);
        for (v, p) in pts.iter().enumerate() {
            let t = p.y / 2.0;
            w.set(v, &[(0, 1.0 - t), (1, t)]);
        }
        let mats = [Mat4::IDENTITY, Mat4::from_rotation_y(std::f32::consts::PI)];
        let (mut lbs, mut dqs) = (Vec::new(), Vec::new());
        linear_blend(&pts, &w, &mats, &mut lbs);
        dual_quaternion(&pts, &w, &mats, &mut dqs);
        let radius = |p: &Vec3| (p.x * p.x + p.z * p.z).sqrt();
        for v in 16..32 {
            assert!(radius(&lbs[v]) < 1e-4, "linear blend collapses: {}", radius(&lbs[v]));
            assert!((radius(&dqs[v]) - 0.5).abs() < 1e-4, "dual quaternion keeps the radius: {}", radius(&dqs[v]));
            assert!((dqs[v].y - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn heat_weights_follow_the_surface_and_blend_at_the_joint() {
        let (pts, edges) = tube(4.0, 0.5, 40, 12);
        let bones = [(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 2.0, 0.0)), (Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 4.0, 0.0))];
        let w = heat_weights(&pts, &edges, &bones, 4);
        assert_eq!(w.points(), pts.len());
        for (v, p) in pts.iter().enumerate() {
            let total: f32 = w.of(v).map(|(_, x)| x).sum();
            assert!((total - 1.0).abs() < 1e-5, "partition of unity at {v}");
            let lower = w.weight(v, 0);
            if p.y < 0.5 {
                assert!(lower > 0.97, "the far end follows its bone: {lower} at y {}", p.y);
            } else if p.y > 3.5 {
                assert!(lower < 0.03, "{lower} at y {}", p.y);
            } else if (p.y - 2.0).abs() < 1e-3 {
                assert!((lower - 0.5).abs() < 0.05, "half and half at the joint: {lower}");
            }
        }
        // Weights fall monotonically along the tube.
        let along: Vec<f32> = (0..=40).map(|r| w.weight(r * 12, 0)).collect();
        assert!(along.windows(2).all(|p| p[1] <= p[0] + 1e-4), "{along:?}");
        // The cheap fallback agrees at the ends.
        let d = distance_weights(&pts, &bones, 2);
        assert!(d.weight(0, 0) > 0.95 && d.weight(pts.len() - 1, 1) > 0.95);
        // Degenerate input.
        assert_eq!(heat_weights(&[], &[], &bones, 4).points(), 0);
        assert_eq!(heat_weights(&pts, &edges, &[], 4).of(0).count(), 0);
        let lonely = heat_weights(&pts, &[(0, 999_999), (3, 3)], &bones, 4);
        assert_eq!(lonely.of(5).collect::<Vec<_>>(), vec![(0, 1.0)], "unconnected points keep their nearest bone");
    }

    #[test]
    fn mirroring_copies_weights_across_the_plane() {
        let pts = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0), Vec3::new(-2.0, 1.0, 0.0005), Vec3::new(-5.0, 0.0, 0.0)];
        let mut w = SkinWeights::new(pts.len(), 2);
        w.set(0, &[(1, 0.75), (0, 0.25)]);
        w.set(2, &[(1, 1.0)]);
        w.set(4, &[(0, 1.0)]);
        // Joint 1 (left) mirrors to joint 2 (right); joint 0 is on the centre line.
        let updated = w.mirror(&pts, 0, &[0, 2, 1], 1e-2);
        assert_eq!(updated, 2);
        assert_eq!(w.of(1).collect::<Vec<_>>(), vec![(2, 0.75), (0, 0.25)]);
        assert_eq!(w.of(3).collect::<Vec<_>>(), vec![(2, 1.0)]);
        assert_eq!(w.of(4).collect::<Vec<_>>(), vec![(0, 1.0)], "no partner within the tolerance");
    }

    proptest! {
        #[test]
        fn skinning_is_finite_and_bounded_by_the_joint_motions(
            raw in proptest::collection::vec((0u16..6, 0.0f32..1.0, 0u16..6, 0.0f32..1.0), 1..40),
            angles in proptest::collection::vec((-3.0f32..3.0, -3.0f32..3.0, -2.0f32..2.0), 4),
        ) {
            let pts: Vec<Vec3> = (0..raw.len()).map(|i| Vec3::new(i as f32 * 0.1, (i % 3) as f32, 0.5)).collect();
            let mut w = SkinWeights::new(pts.len(), 2);
            for (v, (a, wa, b, wb)) in raw.iter().enumerate() {
                w.set(v, &[(*a, *wa), (*b, *wb)]);
            }
            let mats: Vec<Mat4> = angles.iter().map(|(x, y, t)| Mat4::from_rotation_translation(Quat::from_rotation_x(*x) * Quat::from_rotation_y(*y), Vec3::splat(*t))).collect();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            linear_blend(&pts, &w, &mats, &mut a);
            dual_quaternion(&pts, &w, &mats, &mut b);
            prop_assert_eq!(a.len(), pts.len());
            prop_assert_eq!(b.len(), pts.len());
            for (v, p) in pts.iter().enumerate() {
                prop_assert!(a[v].is_finite() && b[v].is_finite());
                // A linear blend lies in the convex hull of the joints' images of the point.
                let images: Vec<Vec3> = w.of(v).filter_map(|(j, _)| mats.get(j as usize)).map(|m| m.transform_point3(*p)).collect();
                if !images.is_empty() {
                    let (lo, hi) = images.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), q| (lo.min(*q), hi.max(*q)));
                    prop_assert!(a[v].cmpge(lo - 1e-3).all() && a[v].cmple(hi + 1e-3).all());
                }
            }
        }
    }
}
