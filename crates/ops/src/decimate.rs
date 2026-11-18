//! Decimation: fewer triangles on the same shape, keeping UVs and patches.
//!
//! The method is the quadric error metric of Garland and Heckbert ("Surface Simplification Using
//! Quadric Error Metrics", 1997): every vertex carries the sum of the squared distances to the
//! planes of the triangles around it, and the cheapest edge is collapsed first. Borders are held
//! in place the way the paper describes, with an extra plane through each border edge at right
//! angles to its triangle; here a border is an open edge, a UV seam or the line between two
//! patches.
//!
//! An edge always collapses onto one of its two ends, so every vertex of the result is a vertex
//! of the input and every corner keeps a UV the input had at that vertex. A vertex in the middle
//! of a UV island may go to any neighbour of the same island; a vertex on a seam, on a patch
//! line or on an open edge only moves along that line; a vertex where three or more of them meet
//! stays. The result is triangles.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use glam::{DVec3, Vec2, Vec3};

use crate::{OpError, OpResult, PolyMesh, Polygon, rebuild};

/// How strongly a border plane counts next to a face plane.
const BORDER_WEIGHT: f64 = 1000.0;
/// A collapse may not turn a triangle further than this from where it faced (cosine).
const MIN_TURN_COS: f64 = 0.2;

/// A symmetric 4×4 quadric, upper triangle.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn plane(n: DVec3, d: f64, w: f64) -> Quadric {
        let (a, b, c) = (n.x, n.y, n.z);
        Quadric([a * a, a * b, a * c, a * d, b * b, b * c, b * d, c * c, c * d, d * d].map(|x| x * w))
    }
    fn add(&mut self, o: &Quadric) {
        for (x, y) in self.0.iter_mut().zip(&o.0) {
            *x += *y;
        }
    }
    fn sum(a: &Quadric, b: &Quadric) -> Quadric {
        let mut q = *a;
        q.add(b);
        q
    }
    fn error(&self, p: DVec3) -> f64 {
        let q = &self.0;
        let (x, y, z) = (p.x, p.y, p.z);
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
}

/// A candidate: move vertex `from` onto `to`.
struct Candidate {
    cost: f64,
    from: u32,
    to: u32,
    /// The versions of both vertices when the cost was computed; a stale candidate is skipped.
    stamp: (u32, u32),
}

impl PartialEq for Candidate {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Candidate {}
impl PartialOrd for Candidate {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Candidate {
    // The heap gives the largest first: the cheapest collapse is the largest, ties by index so
    // the order never depends on anything but the mesh.
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost).then_with(|| (o.from, o.to).cmp(&(self.from, self.to)))
    }
}

struct Work {
    pos: Vec<DVec3>,
    tris: Vec<[u32; 3]>,
    uvs: Vec<[Vec2; 3]>,
    patch: Vec<u32>,
    alive: Vec<bool>,
    vtris: Vec<Vec<u32>>,
    quadric: Vec<Quadric>,
    version: Vec<u32>,
    has_uvs: bool,
}

fn same_uv(a: Vec2, b: Vec2) -> bool {
    a.distance_squared(b) <= 1e-12
}

impl Work {
    fn corner(&self, t: u32, v: u32) -> Option<usize> {
        self.tris.get(t as usize)?.iter().position(|x| *x == v)
    }

    fn normal(&self, tri: &[u32; 3]) -> DVec3 {
        let p = |i: u32| self.pos.get(i as usize).copied().unwrap_or_default();
        (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]))
    }

    /// The live triangles on the edge between `a` and `b`.
    fn edge_tris(&self, a: u32, b: u32) -> Vec<u32> {
        let ts = self.vtris.get(a as usize).map(Vec::as_slice).unwrap_or_default();
        ts.iter().copied().filter(|t| self.tris.get(*t as usize).is_some_and(|tri| tri.contains(&b))).collect()
    }

    /// The vertices joined to `v` by an edge, sorted.
    fn neighbours(&self, v: u32) -> Vec<u32> {
        let mut out: Vec<u32> =
            self.vtris.get(v as usize).into_iter().flatten().filter_map(|t| self.tris.get(*t as usize)).flatten().copied().filter(|w| *w != v).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// May `from` move onto `to`, and what would it cost? `None`: not allowed.
    fn cost(&self, from: u32, to: u32) -> Option<f64> {
        let on_edge = self.edge_tris(from, to);
        if on_edge.is_empty() || on_edge.len() > 2 {
            return None;
        }
        let around = self.vtris.get(from as usize)?;
        // The link condition: the two ends share exactly the vertices opposite the edge, or the
        // collapse would pinch the surface.
        let (nf, nt) = (self.neighbours(from), self.neighbours(to));
        let shared = nf.iter().filter(|v| nt.binary_search(v).is_ok()).count();
        if shared != on_edge.len() {
            return None;
        }
        // An open edge keeps its outline: a vertex on one only moves along it.
        let open = |a: u32, b: u32| self.edge_tris(a, b).len() == 1;
        if on_edge.len() == 2 && nf.iter().any(|w| open(from, *w)) {
            return None;
        }
        // Patches: every patch around `from` must also lie on the edge, so the line between two
        // patches only slides along itself.
        let edge_patch = |p: u32| on_edge.iter().any(|t| self.patch.get(*t as usize).copied() == Some(p));
        if around.iter().any(|t| !edge_patch(self.patch.get(*t as usize).copied().unwrap_or(0))) {
            return None;
        }
        // UVs: every UV `from` has must be carried over by a triangle on the edge.
        if self.has_uvs {
            for t in around {
                let Some(c) = self.corner(*t, from) else { continue };
                let uv = self.uvs.get(*t as usize).map(|u| u[c])?;
                let mut target: Option<Vec2> = None;
                for e in &on_edge {
                    let (Some(cf), Some(ct)) = (self.corner(*e, from), self.corner(*e, to)) else { continue };
                    let Some(eu) = self.uvs.get(*e as usize) else { continue };
                    if same_uv(eu[cf], uv) {
                        match target {
                            Some(x) if !same_uv(x, eu[ct]) => return None,
                            _ => target = Some(eu[ct]),
                        }
                    }
                }
                target?;
            }
        }
        // No remaining triangle may turn over.
        let q = self.pos.get(to as usize).copied()?;
        for t in around {
            if on_edge.contains(t) {
                continue;
            }
            let tri = self.tris.get(*t as usize)?;
            let before = self.normal(tri);
            let moved = tri.map(|v| if v == from { to } else { v });
            let p = |i: u32| if i == to { q } else { self.pos.get(i as usize).copied().unwrap_or_default() };
            let after = (p(moved[1]) - p(moved[0])).cross(p(moved[2]) - p(moved[0]));
            if after.length_squared() <= 1e-30 || before.dot(after) < MIN_TURN_COS * before.length() * after.length() {
                return None;
            }
        }
        let sum = Quadric::sum(self.quadric.get(from as usize)?, self.quadric.get(to as usize)?);
        let e = sum.error(q);
        e.is_finite().then_some(e.max(0.0))
    }

    fn push(&self, heap: &mut BinaryHeap<Candidate>, from: u32, to: u32) {
        if let Some(cost) = self.cost(from, to) {
            let stamp = (self.version.get(from as usize).copied().unwrap_or(0), self.version.get(to as usize).copied().unwrap_or(0));
            heap.push(Candidate { cost, from, to, stamp });
        }
    }

    /// Move `from` onto `to`. Returns how many triangles went.
    fn collapse(&mut self, from: u32, to: u32) -> usize {
        let on_edge = self.edge_tris(from, to);
        let around = self.vtris.get(from as usize).cloned().unwrap_or_default();
        // The UV each of `from`'s UVs turns into, read off the edge's triangles.
        let mut carry: Vec<(Vec2, Vec2)> = Vec::new();
        if self.has_uvs {
            for e in &on_edge {
                if let (Some(cf), Some(ct), Some(eu)) = (self.corner(*e, from), self.corner(*e, to), self.uvs.get(*e as usize)) {
                    carry.push((eu[cf], eu[ct]));
                }
            }
        }
        for t in &around {
            if on_edge.contains(t) {
                if let Some(a) = self.alive.get_mut(*t as usize) {
                    *a = false;
                }
                let tri = self.tris.get(*t as usize).copied().unwrap_or_default();
                for v in tri {
                    if let Some(l) = self.vtris.get_mut(v as usize) {
                        l.retain(|x| x != t);
                    }
                }
                continue;
            }
            let Some(c) = self.corner(*t, from) else { continue };
            if let Some(tri) = self.tris.get_mut(*t as usize) {
                tri[c] = to;
            }
            if let Some(u) = self.uvs.get_mut(*t as usize)
                && let Some((_, new)) = carry.iter().find(|(old, _)| same_uv(*old, u[c]))
            {
                u[c] = *new;
            }
            if let Some(l) = self.vtris.get_mut(to as usize) {
                l.push(*t);
            }
        }
        if let Some(l) = self.vtris.get_mut(from as usize) {
            l.clear();
        }
        let q = self.quadric.get(from as usize).copied().unwrap_or_default();
        if let Some(x) = self.quadric.get_mut(to as usize) {
            x.add(&q);
        }
        on_edge.len()
    }
}

/// Reduce `m` to about `ratio` of its triangles (0..1). The result is triangles with the UVs
/// and patches of the input; a ratio of 1 or more only triangulates. Collapses that would fold
/// the surface, break a UV seam, move a patch line or change an open edge are left out, so a
/// mesh can stop above the count asked for.
pub fn decimate(m: &PolyMesh, ratio: f32) -> OpResult {
    if !ratio.is_finite() {
        return Err(OpError::Invalid("the ratio is not a number".into()));
    }
    let ratio = f64::from(ratio.clamp(0.0, 1.0));
    let has_uvs = m.has_uvs();
    let n = m.vertex_count();
    let mut w = Work {
        pos: m.positions().iter().map(|p| p.as_dvec3()).collect(),
        tris: Vec::with_capacity(m.triangle_count()),
        uvs: Vec::new(),
        patch: Vec::with_capacity(m.triangle_count()),
        alive: Vec::new(),
        vtris: vec![Vec::new(); n],
        quadric: vec![Quadric::default(); n],
        version: vec![0; n],
        has_uvs,
    };
    for f in 0..m.face_count() as u32 {
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        for k in 1..hs.len().saturating_sub(1) {
            let (Some(a), Some(b), Some(c)) = (hs.first(), hs.get(k), hs.get(k + 1)) else { continue };
            w.tris.push([m.origin(*a), m.origin(*b), m.origin(*c)]);
            if has_uvs {
                w.uvs.push([m.uv(*a), m.uv(*b), m.uv(*c)]);
            }
            w.patch.push(m.patch(f));
        }
    }
    w.alive = vec![true; w.tris.len()];
    for (t, tri) in w.tris.iter().enumerate() {
        for v in tri {
            if let Some(l) = w.vtris.get_mut(*v as usize) {
                l.push(t as u32);
            }
        }
    }
    let target = ((w.tris.len() as f64 * ratio).round() as usize).max(4);
    // Face planes, weighted by area.
    for t in 0..w.tris.len() {
        let Some(tri) = w.tris.get(t).copied() else { continue };
        let nrm = w.normal(&tri);
        let area = nrm.length() * 0.5;
        let Some(unit) = nrm.try_normalize() else { continue };
        let p0 = w.pos.get(tri[0] as usize).copied().unwrap_or_default();
        let q = Quadric::plane(unit, -unit.dot(p0), area);
        for v in tri {
            if let Some(x) = w.quadric.get_mut(v as usize) {
                x.add(&q);
            }
        }
        // Border planes: through the edge, at right angles to the triangle.
        for c in 0..3 {
            let (a, b) = (tri[c], tri[(c + 1) % 3]);
            let others: Vec<u32> = w.edge_tris(a, b).into_iter().filter(|x| *x as usize != t).collect();
            let border = match others.first() {
                None => true,
                Some(o) => {
                    let seam = has_uvs && {
                        let uv = |tt: u32, v: u32| w.corner(tt, v).and_then(|k| w.uvs.get(tt as usize).map(|u| u[k])).unwrap_or_default();
                        !(same_uv(uv(t as u32, a), uv(*o, a)) && same_uv(uv(t as u32, b), uv(*o, b)))
                    };
                    seam || w.patch.get(t) != w.patch.get(*o as usize)
                }
            };
            if !border {
                continue;
            }
            let (pa, pb) = (w.pos.get(a as usize).copied().unwrap_or_default(), w.pos.get(b as usize).copied().unwrap_or_default());
            let Some(side) = (pb - pa).cross(unit).try_normalize() else { continue };
            let q = Quadric::plane(side, -side.dot(pa), (pb - pa).length_squared() * BORDER_WEIGHT);
            for v in [a, b] {
                if let Some(x) = w.quadric.get_mut(v as usize) {
                    x.add(&q);
                }
            }
        }
    }
    let mut heap = BinaryHeap::new();
    for v in 0..n as u32 {
        for to in w.neighbours(v) {
            w.push(&mut heap, v, to);
        }
    }
    let mut live = w.tris.len();
    while live > target {
        let Some(c) = heap.pop() else { break };
        let now = (w.version.get(c.from as usize).copied().unwrap_or(0), w.version.get(c.to as usize).copied().unwrap_or(0));
        if now != c.stamp || w.vtris.get(c.from as usize).is_none_or(Vec::is_empty) {
            continue;
        }
        // The neighbourhood may have changed without either end moving: check again.
        if w.cost(c.from, c.to).is_none() {
            continue;
        }
        live -= w.collapse(c.from, c.to);
        // Only the merged vertex has a new quadric: its edges get new costs, both ways. Other
        // candidates keep theirs and are checked again when their turn comes.
        for v in [c.from, c.to] {
            if let Some(x) = w.version.get_mut(v as usize) {
                *x = x.wrapping_add(1);
            }
        }
        for other in w.neighbours(c.to) {
            w.push(&mut heap, c.to, other);
            w.push(&mut heap, other, c.to);
        }
    }
    // Pack what is left.
    let mut remap = vec![u32::MAX; n];
    let mut positions: Vec<Vec3> = Vec::new();
    let mut polys = Vec::with_capacity(live);
    for (t, tri) in w.tris.iter().enumerate() {
        if !w.alive.get(t).copied().unwrap_or(false) {
            continue;
        }
        let mut verts = Vec::with_capacity(3);
        for v in tri {
            let Some(slot) = remap.get_mut(*v as usize) else { continue };
            if *slot == u32::MAX {
                *slot = positions.len() as u32;
                positions.push(m.position(*v));
            }
            verts.push(*slot);
        }
        let mut p = Polygon::new(verts);
        p.uvs = has_uvs.then(|| w.uvs.get(t).map(|u| u.to_vec()).unwrap_or_default());
        p.patch = w.patch.get(t).copied().unwrap_or(0);
        polys.push(p);
    }
    rebuild(positions, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::{closed, euler};
    use proptest::prelude::*;

    fn ball(seg: u32) -> PolyMesh {
        Primitive::RoundCube { radius: 1.0, segments: seg }.build().unwrap()
    }

    fn patches(m: &PolyMesh) -> Vec<u32> {
        let mut p: Vec<u32> = (0..m.face_count() as u32).map(|f| m.patch(f)).collect();
        p.sort_unstable();
        p.dedup();
        p
    }

    #[test]
    fn a_ball_keeps_its_shape_with_a_tenth_of_the_triangles() {
        let m = ball(24);
        let out = decimate(&m, 0.1).unwrap();
        out.validate().unwrap();
        assert!(closed(&out));
        assert_eq!(euler(&out), 2);
        let want = m.triangle_count() / 10;
        assert!(out.face_count() <= want + want / 5, "{} of {}", out.face_count(), m.triangle_count());
        // Every vertex is one the input had, so it lies on the sphere; the chords stay close.
        assert!(out.positions().iter().all(|p| (p.length() - 1.0).abs() < 1e-4));
        assert!((0..out.face_count() as u32).all(|f| out.face_center(f).length() > 0.93));
        assert_eq!(patches(&out), patches(&m), "no patch lost");
        assert_eq!(out.has_uvs(), m.has_uvs());
        // The volume of the inscribed shape is a little smaller, never larger.
        let vol = |m: &PolyMesh| m.triangulate().0.iter().map(|t| m.position(t[0]).dot(m.position(t[1]).cross(m.position(t[2]))) / 6.0).sum::<f32>();
        let (a, b) = (vol(&m), vol(&out));
        assert!(b <= a + 1e-4 && b > a * 0.93, "{b} of {a}");
    }

    #[test]
    fn a_flat_sheet_keeps_its_outline_and_its_uvs() {
        // A plane's UVs are its positions scaled: any corner whose UV no longer matches its
        // position would show a broken UV.
        let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 16 }.build().unwrap();
        assert!(m.has_uvs());
        let uv_of = |m: &PolyMesh, f: u32| -> Vec<(Vec3, Vec2)> { m.face_half_edges(f).map(|h| (m.position(m.origin(h)), m.uv(h))).collect() };
        let known: Vec<(Vec3, Vec2)> = (0..m.face_count() as u32).flat_map(|f| uv_of(&m, f)).collect();
        let out = decimate(&m, 0.05).unwrap();
        out.validate().unwrap();
        assert!(out.face_count() <= 2 * 16 * 16 / 10, "{}", out.face_count());
        let (lo, hi) = out.bounds();
        assert!((hi - lo - Vec3::new(2.0, 0.0, 2.0)).length() < 1e-5, "the outline stayed");
        let border = |m: &PolyMesh| {
            (0..m.edge_count() as u32)
                .filter(|e| m.is_boundary_edge(*e))
                .map(|e| m.edge_vertices(e))
                .map(|(a, b)| m.position(a).distance(m.position(b)))
                .sum::<f32>()
        };
        assert!((border(&out) - 8.0).abs() < 1e-4);
        for f in 0..out.face_count() as u32 {
            for (p, uv) in uv_of(&out, f) {
                assert!(known.iter().any(|(q, u)| p.distance(*q) < 1e-6 && uv.distance(*u) < 1e-6), "a corner got a UV the input did not have there");
            }
        }
        let area: f32 =
            out.triangulate().0.iter().map(|t| (out.position(t[1]) - out.position(t[0])).cross(out.position(t[2]) - out.position(t[0])).length() * 0.5).sum();
        assert!((area - 4.0).abs() < 1e-4, "nothing folded: {area}");
    }

    #[test]
    fn patch_lines_and_seams_stay_where_they_are() {
        // A cube with many segments: six patches, hard edges between them.
        let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 8 }.build().unwrap();
        let out = decimate(&m, 0.02).unwrap();
        out.validate().unwrap();
        assert!(closed(&out));
        assert_eq!(patches(&out), patches(&m));
        // Flat sides with straight borders reduce to almost nothing, and the cube is still a cube.
        assert!(out.face_count() <= 40, "{}", out.face_count());
        let (lo, hi) = out.bounds();
        assert_eq!((lo, hi), (Vec3::splat(-1.0), Vec3::splat(1.0)));
        for f in 0..out.face_count() as u32 {
            let n = out.face_normal(f);
            assert!(n.abs().max_element() > 0.999, "a face left its side: {n}");
        }
    }

    #[test]
    fn ratios_at_the_ends_and_bad_numbers() {
        let m = ball(6);
        let all = decimate(&m, 1.0).unwrap();
        assert_eq!(all.face_count(), m.triangle_count(), "only triangulated");
        assert_eq!(decimate(&m, 7.0).unwrap().face_count(), m.triangle_count());
        let least = decimate(&m, 0.0).unwrap();
        least.validate().unwrap();
        assert!(least.face_count() >= 4 && closed(&least));
        assert!(decimate(&m, f32::NAN).is_err());
        let empty = PolyMesh::build(vec![], vec![]).unwrap().0;
        assert_eq!(decimate(&empty, 0.5).unwrap().face_count(), 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn any_ratio_gives_a_valid_closed_mesh(ratio in 0.0f32..1.0, seg in 2u32..9, kind in 0usize..3) {
            let m = match kind {
                0 => ball(seg),
                1 => Primitive::Torus { radius: 1.0, tube: 0.3, segments: seg * 3, sides: seg + 2 }.build().unwrap(),
                _ => Primitive::Cube { size: Vec3::ONE, segments: seg }.build().unwrap(),
            };
            let out = decimate(&m, ratio).unwrap();
            prop_assert!(out.validate().is_ok());
            prop_assert!(closed(&out));
            prop_assert_eq!(euler(&out), euler(&m));
            prop_assert!(out.face_count() <= m.triangle_count());
            prop_assert_eq!(patches(&out), patches(&m));
        }
    }
}
