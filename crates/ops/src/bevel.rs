//! Bevel: round off or chamfer selected edges (with several segments and a profile) or cut the
//! corner at selected vertices.
//!
//! The strip along a beveled edge is built from two *rails* (one on each side face) and
//! `segments` rows of quads between them. Where beveled edges meet, the open ring left around the
//! vertex is closed with one polygon, so the result is always a closed surface when the input is.

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use modelcraft_mesh::MAX_POLYGON_CORNERS;
use serde::{Deserialize, Serialize};

use crate::edit::{compact, push};
use crate::{OpResult, PolyMesh, Polygon, clean_selection};

/// Most segments across a bevel.
pub const MAX_SEGMENTS: u32 = 16;

/// The shape of the cross section of a bevel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BevelProfile {
    /// A straight chamfer: the segments lie on the chord.
    #[default]
    Flat,
    /// A circular arc that meets both faces tangentially.
    Round,
    /// Between the chord (tension 0) and a parabola towards the corner (tension 1).
    Custom,
}

impl BevelProfile {
    pub const ALL: [BevelProfile; 3] = [BevelProfile::Flat, BevelProfile::Round, BevelProfile::Custom];
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BevelOptions {
    /// Length of the cut along the faces next to the edge, in model units.
    pub width: f32,
    /// Rows of quads across the bevel, 1 to [`MAX_SEGMENTS`].
    pub segments: u32,
    pub profile: BevelProfile,
    /// Used by [`BevelProfile::Custom`], 0 to 1.
    pub tension: f32,
}

impl Default for BevelOptions {
    fn default() -> Self {
        BevelOptions { width: 0.1, segments: 1, profile: BevelProfile::Flat, tension: 0.5 }
    }
}

/// The point at parameter `t` of the profile between `p1` and `p2`, with `apex` the position of
/// the sharp corner (rational quadratic Bezier for the round profile).
fn profile_point(profile: BevelProfile, tension: f32, p1: Vec3, apex: Vec3, p2: Vec3, t: f32) -> Vec3 {
    let chord = p1.lerp(p2, t);
    let conic = |w: f32| {
        let (a, b, c) = ((1.0 - t) * (1.0 - t), 2.0 * w * (1.0 - t) * t, t * t);
        let d = a + b + c;
        if d > 1e-9 { (p1 * a + apex * b + p2 * c) / d } else { chord }
    };
    match profile {
        BevelProfile::Flat => chord,
        BevelProfile::Round => {
            // A circle through both rail points tangent to the faces: weight = sin(half the
            // angle at the corner).
            let (u, v) = ((p1 - apex).normalize_or_zero(), (p2 - apex).normalize_or_zero());
            let angle = u.dot(v).clamp(-1.0, 1.0).acos();
            conic((angle * 0.5).sin().clamp(0.0, 1.0))
        }
        BevelProfile::Custom => {
            let k = if tension.is_finite() { tension.clamp(0.0, 1.0) } else { 0.5 };
            chord.lerp(conic(1.0), k)
        }
    }
}

/// Chamfer or round the selected edges. Boundary edges are skipped.
pub fn bevel_edges_with(m: &PolyMesh, edges: &[u32], opts: &BevelOptions) -> OpResult {
    let width = opts.width;
    let n_seg = opts.segments.clamp(1, MAX_SEGMENTS) as usize;
    let sel: HashSet<u32> = clean_selection(edges, m.edge_count()).into_iter().filter(|e| !m.is_boundary_edge(*e)).collect();
    if sel.is_empty() || !(width.is_finite() && width > 0.0) {
        return Ok(m.clone());
    }
    let mut pos = m.positions().to_vec();
    let orig_verts = pos.len() as u32;
    let uv_mode = m.has_uvs();
    // Shortest edge length at a vertex limits the width there.
    let limit = |v: u32| m.vertex_neighbors(v).map(|w| m.position(v).distance(m.position(w))).fold(f32::MAX, f32::min) * 0.45;
    // Split point on edge (v → x) near v.
    let mut split: HashMap<(u32, u32), u32> = HashMap::new();
    let mut split_at = |v: u32, h: u32, pos: &mut Vec<Vec3>| -> u32 {
        let e = m.edge_of(h);
        *split.entry((v, e)).or_insert_with(|| {
            let x = if m.origin(h) == v { m.dest(h) } else { m.origin(h) };
            let (pv, px) = (m.position(v), m.position(x));
            let w = width.min(limit(v));
            push(pos, pv + (px - pv).normalize_or_zero() * w)
        })
    };
    let mut polys = Vec::with_capacity(m.face_count() + sel.len() * n_seg);
    // Per face corner, the replacement vertex sequence; remember the corner points of beveled edges.
    let mut corner_pt: HashMap<u32, u32> = HashMap::new(); // half-edge (corner at its origin) -> vertex replacing the corner
    let mut corner_end: HashMap<u32, u32> = HashMap::new(); // half-edge -> vertex replacing the corner at its dest
    let touched: HashSet<u32> = sel.iter().flat_map(|e| [m.edge_vertices(*e).0, m.edge_vertices(*e).1]).collect();
    for f in 0..m.face_count() as u32 {
        let mut verts = Vec::new();
        let mut uvs = Vec::new();
        for h in m.face_half_edges(f) {
            let v = m.origin(h);
            let hin = m.prev(h);
            let (ein, eout) = (sel.contains(&m.edge_of(hin)), sel.contains(&m.edge_of(h)));
            let uv = m.uv(h);
            let seq: Vec<u32> = if !touched.contains(&v) {
                vec![v]
            } else if ein && eout {
                let pv = m.position(v);
                let a = (m.position(m.origin(hin)) - pv).normalize_or_zero();
                let b = (m.position(m.dest(h)) - pv).normalize_or_zero();
                let w = width.min(limit(v));
                vec![push(&mut pos, pv + (a + b).normalize_or_zero() * w * std::f32::consts::SQRT_2)]
            } else if ein {
                vec![split_at(v, h, &mut pos)]
            } else if eout {
                vec![split_at(v, hin, &mut pos)]
            } else {
                // Neither edge of this corner is beveled, but the vertex is: keep the split
                // points of the corner's own edges if a neighbouring corner made them.
                let twin_in_sel = |hh: u32| sel.contains(&m.edge_of(m.prev(m.twin(hh)))) || sel.contains(&m.edge_of(m.next(m.twin(hh))));
                let mut s = Vec::new();
                if twin_in_sel(hin) {
                    s.push(split_at(v, hin, &mut pos));
                }
                let out_split = twin_in_sel(h);
                if s.is_empty() || !out_split {
                    s.push(v);
                }
                if out_split {
                    s.push(split_at(v, h, &mut pos));
                }
                if s.len() == 3 {
                    s.remove(1);
                }
                s
            };
            if eout {
                corner_pt.insert(h, seq.last().copied().unwrap_or(v));
            }
            if ein {
                corner_end.insert(hin, seq.first().copied().unwrap_or(v));
            }
            for s in seq {
                verts.push(s);
                uvs.push(uv);
            }
        }
        let mut p = Polygon::new(verts);
        p.uvs = uv_mode.then_some(uvs);
        p.patch = m.patch(f);
        polys.push(p);
    }
    // Strips: `segments` rows of quads between the two rails.
    let mut sorted: Vec<u32> = sel.iter().copied().collect();
    sorted.sort_unstable();
    // Faces that touch the end of a strip along one edge take the strip's inner points, so the
    // two stay joined edge to edge: directed edge -> points to insert along it.
    let mut expand: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for e in sorted {
        let h1 = m.edge_half_edge(e);
        let h2 = m.twin(h1);
        let get = |map: &HashMap<u32, u32>, h: u32| map.get(&h).copied();
        // (a1, b1) on the side of h1, (b2, a2) on the side of h2 (h2 runs from b to a).
        let (Some(a1), Some(b1), Some(b2), Some(a2)) = (get(&corner_pt, h1), get(&corner_end, h1), get(&corner_pt, h2), get(&corner_end, h2)) else {
            continue;
        };
        let (va, vb) = (m.origin(h1), m.dest(h1));
        // Cross sections at both ends, from the h1 side to the h2 side.
        let chain = |from: u32, apex: Vec3, to: u32, pos: &mut Vec<Vec3>| -> Vec<u32> {
            let (p1, p2) = (pos.get(from as usize).copied().unwrap_or(apex), pos.get(to as usize).copied().unwrap_or(apex));
            let mut c = vec![from];
            for k in 1..n_seg {
                let t = k as f32 / n_seg as f32;
                c.push(push(pos, profile_point(opts.profile, opts.tension, p1, apex, p2, t)));
            }
            c.push(to);
            c
        };
        let ca = chain(a1, m.position(va), a2, &mut pos);
        let cb = chain(b1, m.position(vb), b2, &mut pos);
        if n_seg > 1 {
            let inner = |c: &[u32]| c.get(1..c.len().saturating_sub(1)).map(<[u32]>::to_vec).unwrap_or_default();
            let (ma, mb) = (inner(&ca), inner(&cb));
            expand.insert((a2, a1), ma.iter().rev().copied().collect());
            expand.insert((b1, b2), mb);
        }
        let (uv_a1, uv_a2, uv_b1, uv_b2) = (m.uv(h1), m.uv(m.next(h2)), m.uv(m.next(h1)), m.uv(h2));
        for k in 0..n_seg {
            let (Some(&b_lo), Some(&a_lo), Some(&a_hi), Some(&b_hi)) = (cb.get(k), ca.get(k), ca.get(k + 1), cb.get(k + 1)) else { continue };
            let mut p = Polygon::new(vec![b_lo, a_lo, a_hi, b_hi]);
            if uv_mode {
                let (t0, t1) = (k as f32 / n_seg as f32, (k + 1) as f32 / n_seg as f32);
                p.uvs = Some(vec![uv_b1.lerp(uv_b2, t0), uv_a1.lerp(uv_a2, t0), uv_a1.lerp(uv_a2, t1), uv_b1.lerp(uv_b2, t1)]);
            }
            p.patch = m.patch(m.face_of(h1));
            polys.push(p);
        }
    }
    if !expand.is_empty() {
        for p in polys.iter_mut().take(m.face_count()) {
            expand_edges(p, &expand);
        }
    }
    fill_new_holes(&mut polys, orig_verts, &touched, uv_mode);
    compact(pos, polys)
}

/// Insert the listed points along the directed edges of `p` that have an entry in `expand`.
fn expand_edges(p: &mut Polygon, expand: &HashMap<(u32, u32), Vec<u32>>) {
    let n = p.verts.len();
    if !(0..n).any(|i| p.verts.get(i).zip(p.verts.get((i + 1) % n)).is_some_and(|(a, b)| expand.contains_key(&(*a, *b)))) {
        return;
    }
    let mut verts = Vec::with_capacity(n * 2);
    let mut uvs: Vec<Vec2> = Vec::new();
    for i in 0..n {
        let (Some(&a), Some(&b)) = (p.verts.get(i), p.verts.get((i + 1) % n)) else { continue };
        verts.push(a);
        let (ua, ub) = p.uvs.as_ref().map(|u| (u.get(i).copied().unwrap_or_default(), u.get((i + 1) % n).copied().unwrap_or_default())).unwrap_or_default();
        if p.uvs.is_some() {
            uvs.push(ua);
        }
        if let Some(mid) = expand.get(&(a, b)) {
            for (k, v) in mid.iter().enumerate() {
                verts.push(*v);
                if p.uvs.is_some() {
                    uvs.push(ua.lerp(ub, (k + 1) as f32 / (mid.len() + 1) as f32));
                }
            }
        }
    }
    p.uvs = p.uvs.is_some().then_some(uvs);
    p.verts = verts;
}

/// Cut off the corner at each selected vertex: the vertex becomes a small polygon whose corners
/// lie `width` along each edge that meets there. Vertices on the border of the mesh are skipped.
pub fn bevel_vertices(m: &PolyMesh, verts: &[u32], width: f32) -> OpResult {
    let sel: HashSet<u32> = clean_selection(verts, m.vertex_count()).into_iter().filter(|v| !m.is_boundary_vertex(*v)).collect();
    if sel.is_empty() || !(width.is_finite() && width > 0.0) {
        return Ok(m.clone());
    }
    let mut pos = m.positions().to_vec();
    let orig_verts = pos.len() as u32;
    let uv_mode = m.has_uvs();
    let limit = |v: u32| m.vertex_neighbors(v).map(|w| m.position(v).distance(m.position(w))).fold(f32::MAX, f32::min) * 0.45;
    let mut split: HashMap<(u32, u32), u32> = HashMap::new();
    let mut split_at = |v: u32, h: u32, pos: &mut Vec<Vec3>| -> u32 {
        let e = m.edge_of(h);
        *split.entry((v, e)).or_insert_with(|| {
            let x = if m.origin(h) == v { m.dest(h) } else { m.origin(h) };
            let (pv, px) = (m.position(v), m.position(x));
            push(pos, pv + (px - pv).normalize_or_zero() * width.min(limit(v)))
        })
    };
    let mut polys = Vec::with_capacity(m.face_count());
    for f in 0..m.face_count() as u32 {
        let mut vs = Vec::new();
        let mut uvs: Vec<Vec2> = Vec::new();
        for h in m.face_half_edges(f) {
            let v = m.origin(h);
            let uv = m.uv(h);
            if sel.contains(&v) {
                let (a, b) = (split_at(v, m.prev(h), &mut pos), split_at(v, h, &mut pos));
                vs.extend([a, b]);
                uvs.extend([uv, uv]);
            } else {
                vs.push(v);
                uvs.push(uv);
            }
        }
        let mut p = Polygon::new(vs);
        p.uvs = uv_mode.then_some(uvs);
        p.patch = m.patch(f);
        polys.push(p);
    }
    // The ring of new points around each cut vertex is the only open border: close it.
    fill_new_holes(&mut polys, orig_verts, &HashSet::new(), uv_mode);
    compact(pos, polys)
}

/// Close boundary loops made only of vertices created by the operation (index ≥ `first_new`)
/// or vertices it touched.
fn fill_new_holes(polys: &mut Vec<Polygon>, first_new: u32, touched: &HashSet<u32>, uv_mode: bool) {
    let mut directed: HashSet<(u32, u32)> = HashSet::new();
    for p in polys.iter() {
        let n = p.verts.len();
        for i in 0..n {
            if let (Some(a), Some(b)) = (p.verts.get(i), p.verts.get((i + 1) % n)) {
                directed.insert((*a, *b));
            }
        }
    }
    // Open half-edges: (a, b) whose reverse is missing. The hole runs b → a.
    let mut next: HashMap<u32, u32> = HashMap::new();
    for (a, b) in &directed {
        if !directed.contains(&(*b, *a)) {
            next.insert(*b, *a);
        }
    }
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    let bound = next.len().min(MAX_POLYGON_CORNERS);
    let mut seen = HashSet::new();
    for s in starts {
        if seen.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        let mut c = s;
        let mut ok = false;
        for _ in 0..bound {
            let Some(&n) = next.get(&c) else { break };
            if n == s {
                ok = true;
                break;
            }
            lp.push(n);
            c = n;
        }
        for v in &lp {
            seen.insert(*v);
        }
        if ok && lp.len() >= 3 && lp.iter().any(|v| *v >= first_new) && lp.iter().all(|v| *v >= first_new || touched.contains(v)) {
            let mut p = Polygon::new(lp.clone());
            p.uvs = uv_mode.then(|| vec![Vec2::ZERO; lp.len()]);
            polys.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
    }

    #[test]
    fn bevel_all_cube_edges_stays_closed_for_every_segment_count() {
        let c = cube();
        let all: Vec<u32> = (0..12).collect();
        for profile in BevelProfile::ALL {
            for segments in 1..=MAX_SEGMENTS {
                let m = bevel_edges_with(&c, &all, &BevelOptions { width: 0.3, segments, profile, tension: 0.7 }).unwrap();
                m.validate().unwrap();
                assert!(closed(&m), "{profile:?} x {segments} left a hole");
                assert_eq!(euler(&m), 2, "{profile:?} x {segments}");
                // 6 faces, 12 strips of `segments` quads, 8 corner caps.
                assert_eq!(m.face_count(), 6 + 12 * segments as usize + 8, "{profile:?} x {segments}");
            }
        }
    }

    #[test]
    fn one_beveled_edge_with_segments_is_closed() {
        let c = cube();
        for segments in [1, 2, 5, 16] {
            let m = bevel_edges_with(&c, &[0], &BevelOptions { width: 0.4, segments, profile: BevelProfile::Round, tension: 0.5 }).unwrap();
            assert!(closed(&m));
            assert_eq!(euler(&m), 2);
            assert_eq!(m.face_count(), 6 + segments as usize + 0);
        }
    }

    /// The distance from the original corner `a` of edge 0 to the nearest new vertex that is
    /// not on a rail (the rails sit `width` from the corner).
    fn middle_distance(profile: BevelProfile, tension: f32, width: f32) -> f32 {
        let c = cube();
        let m = bevel_edges_with(&c, &[0], &BevelOptions { width, segments: 2, profile, tension }).unwrap();
        let a = c.position(c.edge_vertices(0).0);
        m.positions().iter().map(|p| p.distance(a)).filter(|d| *d > 1e-4 && (d - width).abs() > 1e-4).fold(f32::MAX, f32::min)
    }

    #[test]
    fn the_round_profile_is_a_circle_on_a_right_angle() {
        // At a right-angled corner with rails `w` out along each leg, the arc's middle lies at
        // w * (sqrt 2 - 1) from the corner (the circle has radius w about the point w along both
        // legs).
        let w = 0.5;
        let want = w * (2f32.sqrt() - 1.0);
        let got = middle_distance(BevelProfile::Round, 0.0, w);
        assert!((got - want).abs() < 1e-4, "{got} vs {want}");
    }

    #[test]
    fn flat_and_custom_profiles_differ_from_round() {
        let w = 0.5;
        let (flat, round, sharp, soft) = (
            middle_distance(BevelProfile::Flat, 0.0, w),
            middle_distance(BevelProfile::Round, 0.0, w),
            middle_distance(BevelProfile::Custom, 1.0, w),
            middle_distance(BevelProfile::Custom, 0.0, w),
        );
        assert!((flat - w / 2f32.sqrt()).abs() < 1e-4, "the chord's middle");
        assert!(flat > round && round > sharp, "{flat} {round} {sharp}");
        assert!((soft - flat).abs() < 1e-4, "tension 0 is the chord");
    }

    #[test]
    fn segments_are_clamped() {
        let c = cube();
        let m = bevel_edges_with(&c, &[0], &BevelOptions { width: 0.2, segments: 1000, profile: BevelProfile::Round, tension: 0.5 }).unwrap();
        assert_eq!(m.face_count(), 6 + MAX_SEGMENTS as usize);
        let none = bevel_edges_with(&c, &[0], &BevelOptions { width: f32::NAN, ..Default::default() }).unwrap();
        assert_eq!(none, c);
    }

    #[test]
    fn bevel_keeps_uvs_and_patches() {
        let mut c = cube();
        let uvs: Vec<Vec2> = (0..c.corner_count()).map(|i| Vec2::new(i as f32 * 0.01, 0.5)).collect();
        c.set_uvs(&uvs).unwrap();
        let m = bevel_edges_with(&c, &[0, 1], &BevelOptions { width: 0.2, segments: 3, profile: BevelProfile::Round, tension: 0.5 }).unwrap();
        assert!(m.has_uvs());
        assert!(closed(&m));
    }

    #[test]
    fn vertex_bevel_cuts_the_corner() {
        let c = cube();
        let m = bevel_vertices(&c, &[0], 0.4).unwrap();
        m.validate().unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        // The corner is a triangle; the three faces around it each gain a corner.
        assert_eq!(m.face_count(), 7);
        assert_eq!(m.vertex_count(), 8 - 1 + 3);
        let all: Vec<u32> = (0..8).collect();
        let m = bevel_vertices(&c, &all, 0.3).unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        assert_eq!(m.face_count(), 6 + 8);
        assert_eq!(bevel_vertices(&c, &[500], 0.3).unwrap(), c);
        assert_eq!(bevel_vertices(&c, &[0], -1.0).unwrap(), c);
    }

    #[test]
    fn vertex_bevel_skips_the_border() {
        let g = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
        let border = (0..g.vertex_count() as u32).find(|v| g.is_boundary_vertex(*v)).unwrap();
        assert_eq!(bevel_vertices(&g, &[border], 0.1).unwrap(), g);
    }
}
