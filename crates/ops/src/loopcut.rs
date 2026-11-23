//! Loop cut with a slide, symmetric placement and a spacing profile.

use std::collections::{HashMap, HashSet};

use glam::Vec2;
use modelcraft_mesh::NONE;

use crate::edit::push;
use crate::{OpResult, PolyMesh, Polygon, rebuild};

/// Most loops one cut may add.
pub const MAX_CUTS: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopCutOptions {
    /// How many loops to add, 1 to [`MAX_CUTS`].
    pub cuts: u32,
    /// Slide, -1 to 1. Without `symmetric` it moves all loops towards one end of the edges
    /// (±1 reaches the end); with it, it spreads them out (positive) or pulls them together
    /// (negative) about the middle.
    pub offset: f32,
    /// Lay the loops out as mirror images about the middle of the edges.
    pub symmetric: bool,
    /// Spacing curve, -1 to 1. 0 is even. Without `symmetric` positive values crowd the loops
    /// towards the start of the ring's first edge and negative ones towards its end; with it,
    /// positive values crowd them towards the two ends and negative ones towards the middle.
    pub profile: f32,
}

impl Default for LoopCutOptions {
    fn default() -> Self {
        LoopCutOptions { cuts: 1, offset: 0.0, symmetric: false, profile: 0.0 }
    }
}

const EDGE_MARGIN: f32 = 0.01;

/// Where along an edge each loop crosses, from 0 at the ring's start to 1 at its end, in
/// increasing order. Always strictly inside the edge.
pub fn cut_positions(o: &LoopCutOptions) -> Vec<f32> {
    let n = o.cuts.clamp(1, MAX_CUTS) as usize;
    let fin = |x: f32| if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
    let (offset, profile) = (fin(o.offset), fin(o.profile));
    let gamma = 4.0f32.powf(profile); // 1/4 .. 4
    let even = |k: usize| k as f32 / (n + 1) as f32;
    let mut s: Vec<f32> = (1..=n)
        .map(|k| {
            let u = even(k);
            if o.symmetric { if u < 0.5 { 0.5 * (2.0 * u).powf(gamma) } else { 1.0 - 0.5 * (2.0 * (1.0 - u)).powf(gamma) } } else { u.powf(gamma) }
        })
        .collect();
    if o.symmetric {
        let widest = s.iter().map(|x| (x - 0.5).abs()).fold(0.0f32, f32::max);
        let max_scale = if widest > 1e-6 { (0.5 - EDGE_MARGIN) / widest } else { 1.0 };
        let scale = (1.0 + offset).clamp(0.0, max_scale);
        for x in &mut s {
            *x = 0.5 + (*x - 0.5) * scale;
        }
    } else {
        let (first, last) = (s.first().copied().unwrap_or(0.5), s.last().copied().unwrap_or(0.5));
        let shift = if offset >= 0.0 { offset * (1.0 - EDGE_MARGIN - last) } else { offset * (first - EDGE_MARGIN) };
        for x in &mut s {
            *x += shift;
        }
    }
    for x in &mut s {
        *x = x.clamp(EDGE_MARGIN, 1.0 - EDGE_MARGIN);
    }
    s
}

/// The ring through `edge` (as [`crate::edit::edge_ring`]) with the direction of each edge: `true`
/// when the edge's canonical half-edge runs the same way as the ring's first edge.
pub fn oriented_ring(m: &PolyMesh, edge: u32) -> Vec<(u32, bool)> {
    if edge as usize >= m.edge_count() {
        return Vec::new();
    }
    let start = m.edge_half_edge(edge);
    let mut ring = vec![(edge, true)];
    let mut seen: HashSet<u32> = HashSet::from([edge]);
    for (first, reversed) in [(start, false), (m.twin(start), true)] {
        let mut h = first;
        for _ in 0..m.edge_count() {
            let f = m.face_of(h);
            if f == NONE || m.face_len(f) != 4 {
                break;
            }
            let opp = m.next(m.next(h));
            let e = m.edge_of(opp);
            if !seen.insert(e) {
                break;
            }
            let parallel = m.twin(opp); // runs the way `h` does
            let along_ring = if reversed { opp } else { parallel };
            let entry = (e, m.edge_half_edge(e) == along_ring);
            if reversed {
                ring.insert(0, entry);
            } else {
                ring.push(entry);
            }
            h = parallel;
        }
    }
    ring
}

/// Insert loops across the ring of quads through `edge`.
pub fn loop_cut_with(m: &PolyMesh, edge: u32, opts: &LoopCutOptions) -> OpResult {
    let ring = oriented_ring(m, edge);
    if ring.is_empty() {
        return Ok(m.clone());
    }
    let s = cut_positions(opts);
    let n = s.len();
    let ring_set: HashSet<u32> = ring.iter().map(|(e, _)| *e).collect();
    let mut pos = m.positions().to_vec();
    // New vertices per ring edge and their parameters, both along the edge's canonical direction.
    let mut nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut params: HashMap<u32, Vec<f32>> = HashMap::new();
    for &(e, aligned) in &ring {
        let (a, b) = m.edge_vertices(e);
        let (pa, pb) = (m.position(a), m.position(b));
        let ts: Vec<f32> = if aligned { s.clone() } else { s.iter().rev().map(|x| 1.0 - x).collect() };
        let ids = ts.iter().map(|t| push(&mut pos, pa.lerp(pb, *t))).collect();
        nodes.insert(e, ids);
        params.insert(e, ts);
    }
    // Points on half-edge h from its origin to its dest, ends included.
    let along = |h: u32| -> Vec<u32> {
        let e = m.edge_of(h);
        let mut mid = nodes.get(&e).cloned().unwrap_or_default();
        if m.edge_half_edge(e) != h {
            mid.reverse();
        }
        let mut v = vec![m.origin(h)];
        v.extend(mid);
        v.push(m.dest(h));
        v
    };
    let uv_along = |h: u32| -> Vec<Vec2> {
        let e = m.edge_of(h);
        let mut ts = params.get(&e).cloned().unwrap_or_default();
        if m.edge_half_edge(e) != h {
            ts = ts.iter().rev().map(|t| 1.0 - t).collect();
        }
        let (u0, u1) = (m.uv(h), m.uv(m.next(h)));
        let mut out = vec![u0];
        out.extend(ts.iter().map(|t| u0.lerp(u1, *t)));
        out.push(u1);
        out
    };
    let uv_mode = m.has_uvs();
    let mut polys = Vec::new();
    for f in 0..m.face_count() as u32 {
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let ring_hs: Vec<usize> = hs.iter().enumerate().filter(|(_, h)| ring_set.contains(&m.edge_of(**h))).map(|(i, _)| i).collect();
        let patch = m.patch(f);
        let quad_cut = hs.len() == 4 && ring_hs.len() == 2 && ring_hs.get(1).zip(ring_hs.first()).is_some_and(|(b, a)| b - a == 2);
        if quad_cut {
            let i0 = ring_hs.first().copied().unwrap_or(0);
            let (h, h2) = (hs.get(i0).copied().unwrap_or(0), hs.get(i0 + 2).copied().unwrap_or(0));
            let p = along(h);
            let mut q = along(h2);
            q.reverse(); // q[k] sits across from p[k]
            let up = uv_along(h);
            let mut uq = uv_along(h2);
            uq.reverse();
            for k in 0..=n {
                let (Some(a), Some(b), Some(c), Some(d)) = (p.get(k), p.get(k + 1), q.get(k + 1), q.get(k)) else { continue };
                let mut poly = Polygon::new(vec![*a, *b, *c, *d]);
                if uv_mode {
                    let g = |v: &Vec<Vec2>, i: usize| v.get(i).copied().unwrap_or_default();
                    poly.uvs = Some(vec![g(&up, k), g(&up, k + 1), g(&uq, k + 1), g(&uq, k)]);
                }
                poly.patch = patch;
                polys.push(poly);
            }
        } else {
            // Any other face that touches the ring keeps conforming: insert the new points.
            let mut verts = Vec::new();
            let mut uvs = Vec::new();
            for &h in &hs {
                if ring_set.contains(&m.edge_of(h)) {
                    let pts = along(h);
                    let us = uv_along(h);
                    verts.extend(pts.iter().take(pts.len().saturating_sub(1)));
                    uvs.extend(us.iter().take(us.len().saturating_sub(1)));
                } else {
                    verts.push(m.origin(h));
                    uvs.push(m.uv(h));
                }
            }
            let mut poly = Polygon::new(verts);
            poly.uvs = uv_mode.then_some(uvs);
            poly.patch = patch;
            polys.push(poly);
        }
    }
    rebuild(pos, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;
    use glam::Vec3;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
    }

    #[test]
    fn positions_are_ordered_inside_the_edge_and_hostile_numbers_are_safe() {
        for cuts in [0, 1, 2, 7, 64, 1000] {
            for symmetric in [false, true] {
                for offset in [-5.0, -1.0, -0.3, 0.0, 0.7, 1.0, 9.0, f32::NAN, f32::INFINITY] {
                    for profile in [-1.0, 0.0, 0.5, 1.0, f32::NAN] {
                        let s = cut_positions(&LoopCutOptions { cuts, offset, symmetric, profile });
                        assert_eq!(s.len(), cuts.clamp(1, MAX_CUTS) as usize);
                        assert!(s.iter().all(|x| x.is_finite() && *x >= EDGE_MARGIN - 1e-6 && *x <= 1.0 - EDGE_MARGIN + 1e-6), "{s:?}");
                        assert!(s.windows(2).all(|w| w[0] <= w[1] + 1e-6), "not ordered: {s:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_single_cut_sits_in_the_middle_then_slides_to_the_ends() {
        let mid = cut_positions(&LoopCutOptions::default());
        assert!((mid[0] - 0.5).abs() < 1e-6);
        let hi = cut_positions(&LoopCutOptions { offset: 1.0, ..Default::default() });
        let lo = cut_positions(&LoopCutOptions { offset: -1.0, ..Default::default() });
        assert!((hi[0] - (1.0 - EDGE_MARGIN)).abs() < 1e-6 && (lo[0] - EDGE_MARGIN).abs() < 1e-6);
        let quarter = cut_positions(&LoopCutOptions { offset: 0.5, ..Default::default() });
        assert!(quarter[0] > 0.5 && quarter[0] < hi[0]);
    }

    #[test]
    fn symmetric_positions_mirror_about_the_middle() {
        for cuts in 1..=9 {
            for profile in [-0.8, 0.0, 0.6] {
                for offset in [-0.5, 0.0, 0.5] {
                    let s = cut_positions(&LoopCutOptions { cuts, offset, symmetric: true, profile });
                    for (a, b) in s.iter().zip(s.iter().rev()) {
                        assert!((a + b - 1.0).abs() < 1e-5, "{s:?}");
                    }
                }
            }
        }
        let crowded = cut_positions(&LoopCutOptions { cuts: 3, symmetric: true, profile: 1.0, ..Default::default() });
        let even = cut_positions(&LoopCutOptions { cuts: 3, symmetric: true, ..Default::default() });
        assert!(crowded[0] < even[0], "a positive profile pushes the outer loops towards the ends");
    }

    #[test]
    fn the_plain_cut_is_unchanged_and_closed() {
        let c = cube();
        let m = loop_cut_with(&c, 0, &LoopCutOptions { cuts: 2, ..Default::default() }).unwrap();
        assert_eq!(m, crate::edit::loop_cut(&c, 0, 2).unwrap());
        assert_eq!(m.face_count(), 6 + 4 * 2);
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
    }

    #[test]
    fn loops_line_up_across_the_ring_even_when_uneven() {
        // Every cut vertex of a slid loop lies on one plane: the loop is straight across the cube.
        let c = cube();
        let ring = oriented_ring(&c, 0);
        assert_eq!(ring.len(), 4);
        let axis_edge = c.edge_vertices(0);
        let dir = (c.position(axis_edge.1) - c.position(axis_edge.0)).normalize();
        for opts in [
            LoopCutOptions { cuts: 1, offset: 0.6, ..Default::default() },
            LoopCutOptions { cuts: 3, profile: 0.7, ..Default::default() },
            LoopCutOptions { cuts: 4, symmetric: true, profile: -0.4, offset: 0.3 },
        ] {
            let m = loop_cut_with(&c, 0, &opts).unwrap();
            assert!(closed(&m));
            assert_eq!(euler(&m), 2);
            let new: Vec<Vec3> = m.positions().iter().copied().filter(|p| !c.positions().contains(p)).collect();
            assert_eq!(new.len(), 4 * opts.cuts as usize);
            // Group by distance along the edge direction: each loop has four points.
            let mut heights: Vec<f32> = new.iter().map(|p| p.dot(dir)).collect();
            heights.sort_by(|a, b| a.total_cmp(b));
            for chunk in heights.chunks(4) {
                assert!(chunk.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-4), "a loop is not on one plane: {chunk:?}");
            }
            // And the wanted spacing.
            let s = cut_positions(&opts);
            let lo = c.positions().iter().map(|p| p.dot(dir)).fold(f32::MAX, f32::min);
            for (k, chunk) in heights.chunks(4).enumerate() {
                let want = lo + 2.0 * s[k];
                assert!(chunk.iter().all(|h| (h - want).abs() < 1e-3), "{chunk:?} vs {want}");
            }
        }
    }

    #[test]
    fn uvs_follow_the_slide() {
        let mut c = cube();
        let uvs: Vec<Vec2> = (0..c.corner_count()).map(|i| Vec2::new(i as f32, 0.0)).collect();
        c.set_uvs(&uvs).unwrap();
        let m = loop_cut_with(&c, 0, &LoopCutOptions { cuts: 2, offset: 0.4, profile: 0.5, ..Default::default() }).unwrap();
        assert!(m.has_uvs() && closed(&m));
    }

    #[test]
    fn a_ring_through_triangles_stays_conforming() {
        let s = Primitive::Sphere { radius: 1.0, segments: 8, rings: 6 }.build().unwrap();
        let e = (0..s.edge_count() as u32)
            .find(|e| {
                let (a, b) = s.edge_vertices(*e);
                (s.position(a).y - s.position(b).y).abs() > 0.1 && s.position(a).y.abs() < 0.6 && s.position(b).y.abs() < 0.6
            })
            .unwrap();
        for opts in
            [LoopCutOptions { cuts: 2, offset: 0.5, profile: 0.5, symmetric: false }, LoopCutOptions { cuts: 3, offset: -0.3, profile: -0.5, symmetric: true }]
        {
            let m = loop_cut_with(&s, e, &opts).unwrap();
            assert!(closed(&m));
            assert_eq!(euler(&m), 2);
        }
    }

    #[test]
    fn a_missing_edge_changes_nothing() {
        let c = cube();
        assert_eq!(loop_cut_with(&c, 9999, &LoopCutOptions::default()).unwrap(), c);
        assert!(oriented_ring(&c, 9999).is_empty());
    }
}
