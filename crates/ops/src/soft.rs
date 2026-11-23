//! Soft selection and symmetric editing for moving, rotating and scaling vertices.
//!
//! *Soft selection*: vertices near the selection follow it by a weight that falls from 1 at the
//! selection to 0 at `radius`, shaped by a remap curve (`modelcraft_field::Curve`, the same
//! curves the falloff fields use). *Symmetry*: the edit is mirrored across a plane through the
//! origin of the mesh's space (X, Y or Z); vertices are matched with their mirror image by
//! position, within a tolerance.

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec3};
use modelcraft_field::Curve;
use modelcraft_mesh::PolyMesh;
use serde::{Deserialize, Serialize};

use crate::{OpError, OpResult, clean_selection};

/// Most vertices a soft selection or a symmetric match looks at (the rest are left alone).
pub const MAX_VERTICES: usize = 20_000_000;

/// A falloff around the selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Soft {
    /// Distance at which the weight reaches 0, in the mesh's units.
    pub radius: f32,
    /// Weight against closeness: the curve's input is `1 - distance / radius` (1 at the
    /// selection), its output the weight.
    #[serde(default)]
    pub curve: Curve,
}

impl Soft {
    pub fn new(radius: f32) -> Soft {
        Soft { radius, curve: Curve::preset("smooth").unwrap_or_default() }
    }
}

/// Mirror the edit across the plane through the origin that is perpendicular to an axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symmetry {
    /// 0 = X, 1 = Y, 2 = Z.
    pub axis: u8,
    /// How far a vertex may be from the mirror image of another to count as its partner.
    #[serde(default = "default_tolerance")]
    pub tolerance: f32,
}

fn default_tolerance() -> f32 {
    1e-4
}

impl Symmetry {
    pub fn new(axis: u8) -> Symmetry {
        Symmetry { axis, tolerance: default_tolerance() }
    }
    fn axis(&self) -> usize {
        usize::from(self.axis.min(2))
    }
    fn tolerance(&self) -> f32 {
        if self.tolerance.is_finite() { self.tolerance.clamp(1e-7, 1.0) } else { default_tolerance() }
    }
}

/// `p` reflected across the plane perpendicular to `axis` through the origin.
pub fn mirror_point(p: Vec3, axis: usize) -> Vec3 {
    let mut q = p;
    q[axis] = -q[axis];
    q
}

/// A grid of positions for neighbour searches: cell size `cell`.
struct Grid {
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl Grid {
    fn key(cell: f32, p: Vec3) -> (i32, i32, i32) {
        let c = |x: f32| (x / cell).floor().clamp(-1e9, 1e9) as i32;
        (c(p.x), c(p.y), c(p.z))
    }
    fn new(cell: f32, points: impl Iterator<Item = (u32, Vec3)>) -> Grid {
        let mut cells: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, p) in points {
            cells.entry(Grid::key(cell, p)).or_default().push(i);
        }
        Grid { cell, cells }
    }
    /// The points in the 27 cells around `p`.
    fn near(&self, p: Vec3) -> impl Iterator<Item = u32> + '_ {
        let (x, y, z) = Grid::key(self.cell, p);
        (-1..=1)
            .flat_map(move |dx| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| (x + dx, y + dy, z + dz))))
            .flat_map(|k| self.cells.get(&k).into_iter().flatten().copied())
    }
}

/// The weight of every vertex: 1 for the selected ones, the falloff for the others, 0 beyond the
/// radius. Empty selection or a radius that is not a positive number gives only the selection.
pub fn weights(m: &PolyMesh, selected: &[u32], soft: &Soft) -> Vec<f32> {
    let n = m.vertex_count();
    let mut w = vec![0.0f32; n];
    let sel = clean_selection(selected, n);
    for v in &sel {
        if let Some(x) = w.get_mut(*v as usize) {
            *x = 1.0;
        }
    }
    if sel.is_empty() || !(soft.radius.is_finite() && soft.radius > 1e-9) || n > MAX_VERTICES {
        return w;
    }
    let r = soft.radius;
    let curve = soft.curve.compile();
    let grid = Grid::new(r, sel.iter().map(|v| (*v, m.position(*v))));
    for v in 0..n as u32 {
        if w.get(v as usize).copied().unwrap_or(0.0) >= 1.0 {
            continue;
        }
        let p = m.position(v);
        let d = grid.near(p).map(|s| m.position(s).distance(p)).fold(f32::MAX, f32::min);
        if d < r
            && let Some(x) = w.get_mut(v as usize)
        {
            *x = curve.eval(1.0 - d / r).clamp(0.0, 1.0);
        }
    }
    w
}

/// For each vertex, the vertex at its mirror image (itself on the plane), if there is one.
pub fn partners(m: &PolyMesh, sym: &Symmetry) -> Vec<Option<u32>> {
    let n = m.vertex_count();
    let axis = sym.axis();
    let tol = sym.tolerance();
    if n > MAX_VERTICES {
        return vec![None; n];
    }
    let grid = Grid::new((tol * 4.0).max(1e-6), (0..n as u32).map(|v| (v, m.position(v))));
    (0..n as u32)
        .map(|v| {
            let target = mirror_point(m.position(v), axis);
            grid.near(target)
                .filter(|c| m.position(*c).distance(target) <= tol)
                .min_by(|a, b| m.position(*a).distance(target).total_cmp(&m.position(*b).distance(target)))
        })
        .collect()
}

/// A transform applied about a pivot: scale, then rotate, then translate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub translate: Vec3,
    pub rotate: Quat,
    pub scale: Vec3,
}

impl Xform {
    fn rotation(&self) -> Quat {
        if self.rotate.is_finite() && self.rotate.length() > 1e-6 { self.rotate.normalize() } else { Quat::IDENTITY }
    }
    fn apply(&self, p: Vec3, pivot: Vec3) -> Vec3 {
        pivot + self.rotation() * ((p - pivot) * self.scale) + self.translate
    }
    /// The same edit seen in a mirror: translation reflected, rotation reflected (its axis is a
    /// pseudo-vector), scale unchanged.
    fn mirrored(&self, axis: usize) -> Xform {
        let q = self.rotation();
        let mut v = Vec3::new(q.x, q.y, q.z);
        // Reflection across the plane x = 0 negates the components of the axis in the plane.
        for k in 0..3 {
            if k != axis {
                v[k] = -v[k];
            }
        }
        Xform { translate: mirror_point(self.translate, axis), rotate: Quat::from_xyzw(v.x, v.y, v.z, q.w), scale: mirror_scale(self.scale) }
    }
}

fn mirror_scale(s: Vec3) -> Vec3 {
    s
}

/// Move, rotate and scale the selected vertices about their centre, with a soft falloff and a
/// mirrored edit as asked. Without either this is [`crate::edit::transform_vertices`].
pub fn transform(m: &PolyMesh, verts: &[u32], t: Xform, soft: Option<&Soft>, sym: Option<&Symmetry>) -> OpResult {
    let n = m.vertex_count();
    let sel = clean_selection(verts, n);
    if sel.is_empty() {
        return Ok(m.clone());
    }
    if n > MAX_VERTICES {
        return Err(OpError::TooLarge(n));
    }
    let pos = m.positions();
    let centre = |ids: &[u32]| ids.iter().map(|v| pos.get(*v as usize).copied().unwrap_or_default()).sum::<Vec3>() / ids.len().max(1) as f32;
    let mut out = pos.to_vec();
    let set = |out: &mut Vec<Vec3>, i: u32, p: Vec3| {
        if let Some(slot) = out.get_mut(i as usize) {
            *slot = p;
        }
    };
    let Some(sym) = sym else {
        // One group.
        let w = soft.map(|s| weights(m, &sel, s));
        let pivot = centre(&sel);
        for v in 0..n as u32 {
            let weight = w.as_ref().map_or_else(|| f32::from(sel.binary_search(&v).is_ok()), |w| w.get(v as usize).copied().unwrap_or(0.0));
            if weight > 0.0 {
                let p = pos.get(v as usize).copied().unwrap_or_default();
                set(&mut out, v, p.lerp(t.apply(p, pivot), weight));
            }
        }
        return Ok(rebuilt(m, out));
    };
    let axis = sym.axis();
    let partner = partners(m, sym);
    let part = |v: u32| partner.get(v as usize).copied().flatten();
    // A pair with both ends selected is edited once, through the end on the positive side of the
    // plane (the lower index when both are on it).
    let chosen: HashSet<u32> = sel.iter().copied().collect();
    let positive = |v: u32, p: u32| {
        let (a, b) = (pos.get(v as usize).map_or(0.0, |x| x[axis]), pos.get(p as usize).map_or(0.0, |x| x[axis]));
        a > b || (a == b && v < p)
    };
    let group_a: Vec<u32> = sel.iter().copied().filter(|v| part(*v).is_none_or(|p| p == *v || !chosen.contains(&p) || positive(*v, p))).collect();
    // The mirror images of group A that are not in A (group B), as vertices of the mesh.
    let a_set: HashSet<u32> = group_a.iter().copied().collect();
    let group_b: Vec<u32> = group_a.iter().filter_map(|v| part(*v)).filter(|p| !a_set.contains(p)).collect();
    let b_set: HashSet<u32> = group_b.iter().copied().collect();
    let (wa, wb) = match soft {
        Some(s) => (weights(m, &group_a, s), weights(m, &group_b, s)),
        None => {
            let mut a = vec![0.0f32; n];
            let mut b = vec![0.0f32; n];
            for v in &group_a {
                if let Some(x) = a.get_mut(*v as usize) {
                    *x = 1.0;
                }
            }
            for v in &group_b {
                if let Some(x) = b.get_mut(*v as usize) {
                    *x = 1.0;
                }
            }
            (a, b)
        }
    };
    let (pivot_a, pivot_b) = (centre(&group_a), mirror_point(centre(&group_a), axis));
    let tb = t.mirrored(axis);
    // The pull of each side, vertex by vertex. A vertex of a group moves with that group alone,
    // exactly as asked; the others take both pulls by their weights.
    let mut disp = vec![Vec3::ZERO; n];
    for v in 0..n as u32 {
        let p = pos.get(v as usize).copied().unwrap_or_default();
        let (mut a, mut b) = (wa.get(v as usize).copied().unwrap_or(0.0), wb.get(v as usize).copied().unwrap_or(0.0));
        if a_set.contains(&v) {
            b = 0.0;
        } else if b_set.contains(&v) {
            a = 0.0;
        }
        if a > 0.0 || b > 0.0 {
            set(&mut disp, v, (t.apply(p, pivot_a) - p) * a + (tb.apply(p, pivot_b) - p) * b);
        }
    }
    // Make the pulls mirror images of each other: a vertex takes the mean of its own and its
    // partner's (mirrored). A vertex on the plane keeps the part of the pull that lies in it.
    for v in 0..n as u32 {
        let d = disp.get(v as usize).copied().unwrap_or_default();
        let q = match part(v) {
            Some(p) => (d + mirror_point(disp.get(p as usize).copied().unwrap_or_default(), axis)) * 0.5,
            None => d,
        };
        if q != Vec3::ZERO {
            let base = pos.get(v as usize).copied().unwrap_or_default();
            set(&mut out, v, base + q);
        }
    }
    Ok(rebuilt(m, out))
}

fn rebuilt(m: &PolyMesh, positions: Vec<Vec3>) -> PolyMesh {
    let mut out = m.clone();
    // A position list of the right length cannot fail; keep the mesh as it was otherwise.
    if out.set_positions(&positions).is_err() {
        return m.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use proptest::prelude::*;

    fn grid() -> PolyMesh {
        Primitive::Plane { width: 8.0, depth: 8.0, segments: 8 }.build().unwrap()
    }
    fn id() -> Xform {
        Xform { translate: Vec3::ZERO, rotate: Quat::IDENTITY, scale: Vec3::ONE }
    }

    #[test]
    fn weights_follow_the_curve_and_the_radius() {
        let g = grid();
        // The vertex at the origin, falloff radius 2.
        let origin = (0..g.vertex_count() as u32).find(|v| g.position(*v).length() < 1e-5).unwrap();
        for preset in ["linear", "smooth", "ease_in", "ease_out", "steep"] {
            let soft = Soft { radius: 2.0, curve: Curve::preset(preset).unwrap() };
            let w = weights(&g, &[origin], &soft);
            assert_eq!(w[origin as usize], 1.0);
            let compiled = soft.curve.compile();
            for v in 0..g.vertex_count() as u32 {
                let d = g.position(v).length();
                let want = if v == origin {
                    1.0
                } else if d < 2.0 {
                    compiled.eval(1.0 - d / 2.0)
                } else {
                    0.0
                };
                assert!((w[v as usize] - want).abs() < 1e-6, "{preset}: vertex at {d}: {} vs {want}", w[v as usize]);
            }
        }
        // Linear: halfway out is one half.
        let soft = Soft { radius: 2.0, curve: Curve::linear() };
        let w = weights(&g, &[origin], &soft);
        let one_out = (0..g.vertex_count() as u32).find(|v| (g.position(*v) - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5).unwrap();
        assert!((w[one_out as usize] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn hostile_radius_and_selection_give_only_the_selection() {
        let g = grid();
        for r in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let w = weights(&g, &[3], &Soft { radius: r, curve: Curve::linear() });
            assert_eq!(w.iter().filter(|x| **x > 0.0).count(), 1);
        }
        assert!(weights(&g, &[], &Soft::new(5.0)).iter().all(|x| *x == 0.0));
        assert_eq!(weights(&g, &[9999], &Soft::new(5.0)).iter().filter(|x| **x > 0.0).count(), 0);
    }

    #[test]
    fn a_soft_move_pulls_neighbours_by_their_weight() {
        let g = grid();
        let origin = (0..g.vertex_count() as u32).find(|v| g.position(*v).length() < 1e-5).unwrap();
        let soft = Soft { radius: 2.0, curve: Curve::linear() };
        let up = Xform { translate: Vec3::Y, ..id() };
        let m = transform(&g, &[origin], up, Some(&soft), None).unwrap();
        m.validate().unwrap();
        let w = weights(&g, &[origin], &soft);
        for v in 0..g.vertex_count() as u32 {
            assert!((m.position(v).y - w[v as usize]).abs() < 1e-5, "vertex {v}");
            assert_eq!(m.position(v).x, g.position(v).x);
        }
        // Without it only the selection moves.
        let hard = transform(&g, &[origin], up, None, None).unwrap();
        assert_eq!((0..g.vertex_count() as u32).filter(|v| hard.position(*v).y != 0.0).count(), 1);
    }

    #[test]
    fn symmetric_edit_moves_the_mirror_side_the_other_way() {
        let g = grid();
        let v = (0..g.vertex_count() as u32).find(|v| (g.position(*v) - Vec3::new(2.0, 0.0, 1.0)).length() < 1e-5).unwrap();
        let m = transform(&g, &[v], Xform { translate: Vec3::new(0.5, 1.0, 0.0), ..id() }, None, Some(&Symmetry::new(0))).unwrap();
        let twin = (0..g.vertex_count() as u32).find(|u| (g.position(*u) - Vec3::new(-2.0, 0.0, 1.0)).length() < 1e-5).unwrap();
        assert!((m.position(v) - Vec3::new(2.5, 1.0, 1.0)).length() < 1e-5);
        assert!((m.position(twin) - Vec3::new(-2.5, 1.0, 1.0)).length() < 1e-5, "{:?}", m.position(twin));
        // Two selected partners are edited once.
        let both = transform(&g, &[v, twin], Xform { translate: Vec3::new(0.5, 1.0, 0.0), ..id() }, None, Some(&Symmetry::new(0))).unwrap();
        assert!((both.position(twin) - Vec3::new(-2.5, 1.0, 1.0)).length() < 1e-5);
        assert!((both.position(v) - Vec3::new(2.5, 1.0, 1.0)).length() < 1e-5);
    }

    #[test]
    fn on_plane_vertices_stay_on_the_plane() {
        let g = grid();
        let centre = (0..g.vertex_count() as u32).find(|v| g.position(*v).length() < 1e-5).unwrap();
        let m = transform(&g, &[centre], Xform { translate: Vec3::new(1.0, 1.0, 0.0), ..id() }, None, Some(&Symmetry::new(0))).unwrap();
        assert_eq!(m.position(centre).x, 0.0);
        assert_eq!(m.position(centre).y, 1.0);
    }

    #[test]
    fn partners_match_by_position() {
        let g = grid();
        let p = partners(&g, &Symmetry::new(0));
        assert!(p.iter().all(|x| x.is_some()), "a symmetric grid has a partner for every vertex");
        for (v, q) in p.iter().enumerate() {
            let q = q.unwrap();
            assert!((g.position(q) - mirror_point(g.position(v as u32), 0)).length() < 1e-4);
        }
        // Nudge one vertex: it has no partner any more.
        let mut moved = g.clone();
        moved.map_positions(|i, p| if i == 5 { p + Vec3::new(0.3, 0.0, 0.0) } else { p });
        let p = partners(&moved, &Symmetry::new(0));
        assert!(p.iter().any(|x| x.is_none()));
    }

    #[test]
    fn mirrored_rotation_matches_the_mirrored_points() {
        // Rotating a vertex about the pivot and mirroring the result equals rotating the mirror
        // vertex with the mirrored rotation about the mirrored pivot.
        let t = Xform { translate: Vec3::new(0.2, 0.1, -0.3), rotate: Quat::from_euler(glam::EulerRot::XYZ, 0.4, -0.7, 0.2), scale: Vec3::new(1.2, 0.9, 1.1) };
        for axis in 0..3 {
            let (p, pivot) = (Vec3::new(1.0, 2.0, -0.5), Vec3::new(0.3, -0.2, 0.8));
            let a = mirror_point(t.apply(p, pivot), axis);
            let b = t.mirrored(axis).apply(mirror_point(p, axis), mirror_point(pivot, axis));
            assert!(a.distance(b) < 1e-4, "axis {axis}: {a:?} {b:?}");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// A symmetric edit of a symmetric mesh keeps it symmetric, for any selection, move,
        /// turn, scale, falloff and axis.
        #[test]
        fn symmetric_edits_stay_symmetric(
            kind in 0usize..3,
            axis in 0u8..3,
            picks in prop::collection::vec(0u32..400, 1..8),
            t in prop::array::uniform3(-1.0f32..1.0),
            r in prop::array::uniform3(-1.0f32..1.0),
            s in prop::array::uniform3(0.5f32..1.5),
            radius in prop::option::of(0.1f32..3.0),
        ) {
            // Primitives built symmetric about all three coordinate planes.
            let m = match kind {
                0 => Primitive::Cube { size: Vec3::splat(2.0), segments: 3 }.build().unwrap(),
                1 => Primitive::Plane { width: 4.0, depth: 4.0, segments: 4 }.build().unwrap(),
                _ => Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 8, rings: 2, caps: true }.build().unwrap(),
            };
            let n = m.vertex_count() as u32;
            let verts: Vec<u32> = picks.iter().map(|p| p % n).collect();
            let sym = Symmetry::new(axis);
            let soft = radius.map(Soft::new);
            let xf = Xform { translate: Vec3::from(t), rotate: Quat::from_euler(glam::EulerRot::XYZ, r[0], r[1], r[2]), scale: Vec3::from(s) };
            let partner = partners(&m, &sym);
            // Only meshes that really are symmetric about this plane (a cylinder is not about
            // every axis, a plane is flat in Y).
            prop_assume!(partner.iter().all(|p| p.is_some()));
            let out = transform(&m, &verts, xf, soft.as_ref(), Some(&sym)).unwrap();
            prop_assert!(out.validate().is_ok());
            for (v, p) in partner.iter().enumerate() {
                let p = p.unwrap();
                let want = mirror_point(out.position(v as u32), sym.axis());
                prop_assert!(out.position(p).distance(want) < 2e-3, "vertex {} and partner {}: {:?} vs {:?}", v, p, out.position(p), want);
            }
        }
    }
}
