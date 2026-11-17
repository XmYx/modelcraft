//! Even remesh: rebuild a closed mesh as evenly sized quads that follow the same surface.
//!
//! The mesh is sampled on a regular grid of cubes with edge `cell`. Every grid point is marked
//! inside or outside by counting how many times a ray along X crosses the surface; grid points
//! next to the surface also get their exact distance to it. One new vertex is placed in every
//! cube the surface passes through (at the mean of where the surface crosses the cube's edges,
//! the "surface nets" construction of Gibson, 1998), and every grid edge that crosses the
//! surface gives one quad joining the four cubes around it. The result keeps the volume and
//! loses UVs, patches and anything thinner than a cell.

use std::collections::HashMap;

use glam::{DVec3, Vec3};

use crate::{MAX_RESULT_FACES, OpError, OpResult, PolyMesh, Polygon, rebuild};

/// Most grid points one remesh may use (the inside/outside bits and the scan dominate).
pub const MAX_REMESH_POINTS: usize = 40_000_000;

/// Grid cells of padding around the mesh bounds.
const PAD: f32 = 2.0;

struct Grid {
    origin: Vec3,
    cell: f32,
    n: [usize; 3],
}

impl Grid {
    fn index(&self, i: usize, j: usize, k: usize) -> usize {
        i + self.n[0] * (j + self.n[1] * k)
    }
    fn point(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + Vec3::new(i as f32, j as f32, k as f32) * self.cell
    }
    /// Grid coordinate range `[lo, hi]` covering `[a, b]` on `axis`, clamped to the grid.
    fn range(&self, axis: usize, a: f32, b: f32) -> (usize, usize) {
        let n = self.n[axis] as f32 - 1.0;
        let lo = ((a - self.origin[axis]) / self.cell).ceil().clamp(0.0, n);
        let hi = ((b - self.origin[axis]) / self.cell).floor().clamp(0.0, n);
        (lo as usize, hi as usize)
    }
}

/// The smallest cell size [`even_remesh`] accepts for `m`.
pub fn min_cell(m: &PolyMesh) -> f32 {
    let (lo, hi) = m.bounds();
    let e = (hi - lo).max(Vec3::splat(1e-6));
    // Solve (ex/c + 2·PAD + 2)(ey/c + …)(ez/c + …) ≤ MAX by bisection on c.
    let count = |c: f32| (0..3).map(|a| (e[a] / c).ceil() as f64 + 2.0 * PAD as f64 + 2.0).product::<f64>();
    let (mut a, mut b) = (e.max_element() / 4096.0, e.max_element().max(1e-6));
    for _ in 0..48 {
        let mid = 0.5 * (a + b);
        if count(mid) > MAX_REMESH_POINTS as f64 { a = mid } else { b = mid }
    }
    b
}

fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    // Ericson, Real-Time Collision Detection, 5.1.5.
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = va + vb + vc;
    if denom.abs() < 1e-30 {
        return a;
    }
    a + ab * (vb / denom) + ac * (vc / denom)
}

/// Edge function of `p` against the edge `a → b` in the YZ plane, computed from the lower vertex
/// index to the higher so both triangles sharing the edge get exactly opposite values. Returns
/// the value and whether the edge runs from the lower index to the higher.
fn edge(pa: DVec3, pb: DVec3, ia: u32, ib: u32, y: f64, z: f64) -> (f64, bool) {
    let f = |a: DVec3, b: DVec3| (b.y - a.y) * (z - a.z) - (b.z - a.z) * (y - a.y);
    if ia < ib { (f(pa, pb), true) } else { (-f(pb, pa), false) }
}

/// Rebuild `m` as even quads of about `cell` size. The mesh must be closed.
pub fn even_remesh(m: &PolyMesh, cell: f32) -> OpResult {
    if !(cell.is_finite() && cell > 0.0) {
        return Err(OpError::Invalid("the cell size must be a positive number".into()));
    }
    if m.face_count() == 0 {
        return Err(OpError::Invalid("there is nothing to remesh".into()));
    }
    if (0..m.edge_count() as u32).any(|e| m.is_boundary_edge(e)) {
        return Err(OpError::Invalid("even remesh needs a closed mesh: fill its holes first".into()));
    }
    let least = min_cell(m);
    if cell < least {
        return Err(OpError::Invalid(format!("the cell size is too small for this object: use {least:.4} or more")));
    }
    let (lo, hi) = m.bounds();
    let origin = lo - Vec3::splat(PAD * cell) - Vec3::splat(cell * 0.013);
    let dims = |a: usize| ((hi[a] - origin[a]) / cell).ceil() as usize + PAD as usize + 1;
    let g = Grid { origin, cell, n: [dims(0), dims(1), dims(2)] };
    let total = g.n[0].saturating_mul(g.n[1]).saturating_mul(g.n[2]);
    if total > MAX_REMESH_POINTS * 2 {
        return Err(OpError::TooLarge(total));
    }
    let (tris, _) = m.triangulate();
    let pos = m.positions();
    let corner = |t: &[u32; 3]| [0, 1, 2].map(|c| pos.get(t[c] as usize).copied().unwrap_or_default());

    // 1. Where rays along +X cross the surface, per (j, k) column.
    let mut hits: Vec<(u32, f32)> = Vec::new();
    for t in &tris {
        let p = corner(t);
        let d = p.map(|v| v.as_dvec3());
        let area = (d[1].y - d[0].y) * (d[2].z - d[0].z) - (d[1].z - d[0].z) * (d[2].y - d[0].y);
        if area == 0.0 {
            continue;
        }
        let front = area > 0.0;
        let (j0, j1) = g.range(1, p[0].y.min(p[1].y).min(p[2].y), p[0].y.max(p[1].y).max(p[2].y));
        let (k0, k1) = g.range(2, p[0].z.min(p[1].z).min(p[2].z), p[0].z.max(p[1].z).max(p[2].z));
        for k in k0..=k1 {
            for j in j0..=j1 {
                let q = g.point(0, j, k);
                let (y, z) = (f64::from(q.y), f64::from(q.z));
                let mut w = [0.0f64; 3];
                let mut inside = true;
                for e in 0..3 {
                    let (a, b) = (e, (e + 1) % 3);
                    let (v, forward) = edge(d[a], d[b], t[a], t[b], y, z);
                    let v = if front { v } else { -v };
                    if v < 0.0 || (v == 0.0 && forward != front) {
                        inside = false;
                        break;
                    }
                    w[(e + 2) % 3] = v;
                }
                if !inside {
                    continue;
                }
                let sum = w[0] + w[1] + w[2];
                if sum <= 0.0 {
                    continue;
                }
                let x = (w[0] * d[0].x + w[1] * d[1].x + w[2] * d[2].x) / sum;
                hits.push(((j + g.n[1] * k) as u32, x as f32));
            }
        }
    }
    hits.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    // 2. Inside bits: a grid point is inside when an odd number of crossings lie before it.
    let mut inside = vec![false; total];
    let mut at = 0usize;
    while at < hits.len() {
        let col = hits.get(at).map(|h| h.0).unwrap_or(0);
        let end = hits.get(at..).map(|s| s.iter().take_while(|h| h.0 == col).count()).unwrap_or(0) + at;
        let (j, k) = (col as usize % g.n[1], col as usize / g.n[1]);
        let mut c = at;
        let mut odd = false;
        for i in 0..g.n[0] {
            let x = g.origin.x + i as f32 * g.cell;
            while c < end && hits.get(c).is_some_and(|h| h.1 < x) {
                odd = !odd;
                c += 1;
            }
            if odd && let Some(b) = inside.get_mut(g.index(i, j, k)) {
                *b = true;
            }
        }
        at = end;
    }
    let is_in = |i: usize, j: usize, k: usize| inside.get(g.index(i, j, k)).copied().unwrap_or(false);

    // 3. Cubes the surface passes through get a vertex slot; their corners need a distance.
    let (cx, cy, cz) = (g.n[0].saturating_sub(1), g.n[1].saturating_sub(1), g.n[2].saturating_sub(1));
    let mut cube_vertex: HashMap<u32, u32> = HashMap::new();
    let mut cubes: Vec<[u32; 3]> = Vec::new();
    let mut dist: HashMap<u32, f32> = HashMap::new();
    for k in 0..cz {
        for j in 0..cy {
            for i in 0..cx {
                let first = is_in(i, j, k);
                let mixed = (1..8).any(|c| is_in(i + (c & 1), j + ((c >> 1) & 1), k + (c >> 2)) != first);
                if !mixed {
                    continue;
                }
                cube_vertex.insert(g.index(i, j, k) as u32, cubes.len() as u32);
                cubes.push([i as u32, j as u32, k as u32]);
                for c in 0..8 {
                    dist.insert(g.index(i + (c & 1), j + ((c >> 1) & 1), k + (c >> 2)) as u32, f32::MAX);
                }
            }
        }
    }
    if cubes.len().saturating_mul(3) > MAX_RESULT_FACES * 2 {
        return Err(OpError::TooLarge(cubes.len()));
    }

    // 4. Exact distance from those grid points to the surface (a point next to the surface is
    //    within one cube diagonal of it, so only triangles that close can matter).
    let reach = cell * 1.75;
    for t in &tris {
        let p = corner(t);
        let (mn, mx) = (p[0].min(p[1]).min(p[2]) - Vec3::splat(reach), p[0].max(p[1]).max(p[2]) + Vec3::splat(reach));
        let (i0, i1) = g.range(0, mn.x, mx.x);
        let (j0, j1) = g.range(1, mn.y, mx.y);
        let (k0, k1) = g.range(2, mn.z, mx.z);
        for k in k0..=k1 {
            for j in j0..=j1 {
                for i in i0..=i1 {
                    if let Some(d) = dist.get_mut(&(g.index(i, j, k) as u32)) {
                        let q = g.point(i, j, k);
                        let v = q.distance(closest_on_triangle(q, p[0], p[1], p[2]));
                        if v < *d {
                            *d = v;
                        }
                    }
                }
            }
        }
    }
    let signed = |i: usize, j: usize, k: usize| {
        let d = dist.get(&(g.index(i, j, k) as u32)).copied().filter(|d| *d < f32::MAX).unwrap_or(cell * 0.5);
        if is_in(i, j, k) { -d.max(1e-6 * cell) } else { d.max(1e-6 * cell) }
    };

    // 5. One vertex per cube: the mean of the surface crossings on its twelve edges.
    const EDGES: [(usize, usize); 12] = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    let mut verts = Vec::with_capacity(cubes.len());
    for [i, j, k] in &cubes {
        let (i, j, k) = (*i as usize, *j as usize, *k as usize);
        let at = |c: usize| (i + (c & 1), j + ((c >> 1) & 1), k + (c >> 2));
        let (mut sum, mut n) = (Vec3::ZERO, 0.0f32);
        for (a, b) in EDGES {
            let (pa, pb) = (at(a), at(b));
            let (da, db) = (signed(pa.0, pa.1, pa.2), signed(pb.0, pb.1, pb.2));
            if (da < 0.0) == (db < 0.0) {
                continue;
            }
            let t = (da / (da - db)).clamp(0.0, 1.0);
            sum += g.point(pa.0, pa.1, pa.2).lerp(g.point(pb.0, pb.1, pb.2), t);
            n += 1.0;
        }
        verts.push(if n > 0.0 { sum / n } else { g.point(i, j, k) + Vec3::splat(0.5 * cell) });
    }

    // 6. One quad per grid edge that crosses the surface, facing out of the inside end.
    let mut quads: Vec<Polygon> = Vec::new();
    let vertex = |i: usize, j: usize, k: usize| cube_vertex.get(&(g.index(i, j, k) as u32)).copied();
    for [i, j, k] in &cubes {
        let (i, j, k) = (*i as usize, *j as usize, *k as usize);
        // The three grid edges leaving this cube's far corner (i+1, j+1, k+1) backwards are
        // shared with cubes at lower indices; handle each grid edge from its lowest cube: the
        // edges starting at this cube's own corner (i, j, k).
        let here = is_in(i, j, k);
        for axis in 0..3 {
            let (di, dj, dk) = (usize::from(axis == 0), usize::from(axis == 1), usize::from(axis == 2));
            if is_in(i + di, j + dj, k + dk) == here {
                continue;
            }
            // The four cubes around the edge: step back along the two other axes.
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let back = |su: usize, sv: usize| {
                let mut c = [i, j, k];
                if c[u] < su || c[v] < sv {
                    return None;
                }
                c[u] -= su;
                c[v] -= sv;
                vertex(c[0], c[1], c[2])
            };
            let (Some(a), Some(b), Some(c), Some(d)) = (back(0, 0), back(1, 0), back(1, 1), back(0, 1)) else { continue };
            // Going a → b → c → d turns clockwise seen from +axis, so that order faces −axis:
            // the quad faces away from whichever end of the edge is inside.
            quads.push(Polygon::new(if here { vec![a, b, c, d] } else { vec![a, d, c, b] }));
        }
    }
    if quads.is_empty() {
        return Err(OpError::Invalid("the cell size is larger than the object: nothing is left".into()));
    }
    rebuild(verts, quads)
}

/// Volume enclosed by a closed mesh (signed: positive when the faces point outwards).
pub fn volume(m: &PolyMesh) -> f64 {
    let (tris, _) = m.triangulate();
    let p = |i: u32| m.position(i).as_dvec3();
    tris.iter().map(|t| p(t[0]).dot(p(t[1]).cross(p(t[2])))).sum::<f64>() / 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;
    use proptest::prelude::*;

    fn quads_only(m: &PolyMesh) -> bool {
        (0..m.face_count() as u32).all(|f| m.face_len(f) == 4)
    }

    #[test]
    fn a_sphere_keeps_its_volume_and_becomes_even_quads() {
        let s = Primitive::RoundCube { radius: 1.0, segments: 12 }.build().unwrap();
        let before = volume(&s);
        let r = even_remesh(&s, 0.08).unwrap();
        assert!(closed(&r) && euler(&r) == 2 && quads_only(&r));
        let after = volume(&r);
        assert!((after / before - 1.0).abs() < 0.01, "volume {before} -> {after}");
        // Every vertex lies near the original surface, and edges are about one cell long.
        assert!(r.positions().iter().all(|p| (p.length() - 1.0).abs() < 0.03));
        let lens: Vec<f32> = (0..r.edge_count() as u32)
            .map(|e| {
                let (a, b) = r.edge_vertices(e);
                r.position(a).distance(r.position(b))
            })
            .collect();
        let mean = lens.iter().sum::<f32>() / lens.len() as f32;
        assert!((0.04..0.12).contains(&mean), "mean edge {mean}");
        assert!(lens.iter().all(|l| *l < 0.08 * 2.0));
    }

    #[test]
    fn a_torus_keeps_its_hole_and_a_box_its_volume() {
        let t = Primitive::Torus { radius: 1.0, tube: 0.35, segments: 32, sides: 16 }.build().unwrap();
        let r = even_remesh(&t, 0.06).unwrap();
        assert!(closed(&r) && quads_only(&r));
        assert_eq!(euler(&r), 0, "still one handle");
        assert!((volume(&r) / volume(&t) - 1.0).abs() < 0.02);
        let b = Primitive::Cube { size: Vec3::new(2.0, 1.0, 0.5), segments: 1 }.build().unwrap();
        let r = even_remesh(&b, 0.05).unwrap();
        assert!(closed(&r) && euler(&r) == 2);
        assert!((volume(&r) - 1.0).abs() < 0.03, "{}", volume(&r));
    }

    #[test]
    fn the_result_is_the_same_every_time() {
        let s = Primitive::RoundCube { radius: 1.0, segments: 6 }.build().unwrap();
        assert_eq!(even_remesh(&s, 0.15).unwrap(), even_remesh(&s, 0.15).unwrap());
    }

    #[test]
    fn open_meshes_and_bad_sizes_are_errors() {
        let p = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
        assert!(matches!(even_remesh(&p, 0.1), Err(OpError::Invalid(_))));
        let c = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY, 1e-9] {
            assert!(even_remesh(&c, bad).is_err(), "{bad}");
        }
        assert!(even_remesh(&c, 50.0).is_err(), "a cell far larger than the object leaves nothing");
        assert!(even_remesh(&PolyMesh::default(), 0.1).is_err());
        assert!(min_cell(&c) < 0.01);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]
        #[test]
        fn any_box_and_cell_size_gives_a_valid_closed_mesh(x in 0.2f32..3.0, y in 0.2f32..3.0, z in 0.2f32..3.0, cell in 0.05f32..0.4, seg in 1u32..4) {
            let b = Primitive::Cube { size: Vec3::new(x, y, z), segments: seg }.build().unwrap();
            if let Ok(r) = even_remesh(&b, cell) {
                prop_assert!(r.validate().is_ok());
                prop_assert!(closed(&r));
                let v = f64::from(x * y * z);
                prop_assert!(volume(&r) > 0.0 && volume(&r) < v * 1.05);
            }
        }
    }
}
