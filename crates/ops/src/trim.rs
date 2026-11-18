//! Trim by plane: cut a mesh with a plane, drop the side the normal points to and (optionally)
//! close the cut with a cap.
//!
//! Every face is clipped against the plane on its own (the polygon clipping of Sutherland and
//! Hodgman, 1974); the two faces on a cut edge share one new vertex, so the result stays one
//! surface. The cut segments are chained into loops, and each loop is closed with a fan around
//! its centre. A fan is exact for a convex outline and still watertight for any other.

use std::collections::HashMap;

use glam::{Vec2, Vec3};

use crate::{OpError, OpResult, PolyMesh, Polygon, rebuild};

/// Signed distances closer to the plane than this share of the mesh size count as on it.
const ON_PLANE: f32 = 1e-6;

/// Cut `m` with the plane through `point` with `normal`; what lies on the normal's side goes.
/// With `cap` the cut is closed, so a closed mesh stays closed. A plane that misses the mesh
/// returns it unchanged, or an error when nothing would be left.
pub fn trim_plane(m: &PolyMesh, point: Vec3, normal: Vec3, cap: bool) -> OpResult {
    let n = normal.normalize_or_zero();
    if !point.is_finite() || n == Vec3::ZERO {
        return Err(OpError::Invalid("the plane needs a finite point and a normal with a length".into()));
    }
    let (lo, hi) = m.bounds();
    let eps = (hi - lo).length().max(1e-12) * ON_PLANE;
    let dist: Vec<f32> = m
        .positions()
        .iter()
        .map(|p| {
            let d = (*p - point).dot(n);
            if d.abs() <= eps { 0.0 } else { d }
        })
        .collect();
    if dist.iter().all(|d| *d <= 0.0) {
        return Ok(m.clone());
    }
    if dist.iter().all(|d| *d >= 0.0) {
        return Err(OpError::Invalid("the plane would trim the whole mesh away".into()));
    }
    let d = |v: u32| dist.get(v as usize).copied().unwrap_or(0.0);
    let uv_mode = m.has_uvs();
    let mut pos = m.positions().to_vec();
    let mut polys: Vec<Polygon> = Vec::with_capacity(m.face_count());
    // The new vertex on a cut edge, keyed by the edge's (inside, outside) vertices.
    let mut cut: HashMap<(u32, u32), u32> = HashMap::new();
    // For the cap: where the boundary left behind by a clipped face goes next.
    let mut next: HashMap<u32, u32> = HashMap::new();
    for f in 0..m.face_count() as u32 {
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let outside = hs.iter().filter(|h| d(m.origin(**h)) > 0.0).count();
        if outside == 0 {
            let mut p = Polygon::new(hs.iter().map(|h| m.origin(*h)).collect());
            p.uvs = uv_mode.then(|| hs.iter().map(|h| m.uv(*h)).collect());
            p.patch = m.patch(f);
            polys.push(p);
            continue;
        }
        if outside == hs.len() {
            continue;
        }
        let mut verts: Vec<u32> = Vec::with_capacity(hs.len() + 2);
        let mut uvs: Vec<Vec2> = Vec::with_capacity(hs.len() + 2);
        let (mut exit, mut entry) = (None, None);
        for (i, h) in hs.iter().enumerate() {
            let Some(hn) = hs.get((i + 1) % hs.len()) else { continue };
            let (a, b) = (m.origin(*h), m.origin(*hn));
            let (da, db) = (d(a), d(b));
            if da <= 0.0 {
                verts.push(a);
                uvs.push(m.uv(*h));
            }
            if (da <= 0.0) == (db <= 0.0) {
                continue;
            }
            // The edge crosses: `inside` is the end that stays.
            let (inside, out, di, do_) = if da <= 0.0 { (a, b, da, db) } else { (b, a, db, da) };
            let t = (di / (di - do_)).clamp(0.0, 1.0);
            let v = if di == 0.0 {
                inside
            } else {
                *cut.entry((inside, out)).or_insert_with(|| {
                    pos.push(m.position(inside).lerp(m.position(out), t));
                    (pos.len() - 1) as u32
                })
            };
            let (ui, uo) = if da <= 0.0 { (m.uv(*h), m.uv(*hn)) } else { (m.uv(*hn), m.uv(*h)) };
            if da <= 0.0 {
                exit = Some(v);
            } else {
                entry = Some(v);
            }
            if verts.last() != Some(&v) {
                verts.push(v);
                uvs.push(ui.lerp(uo, t));
            }
        }
        if let (Some(a), Some(b)) = (exit, entry)
            && a != b
        {
            // The face runs exit → entry along the cut; the cap runs the other way.
            next.insert(b, a);
        }
        if verts.len() < 3 {
            continue;
        }
        let mut p = Polygon::new(verts);
        p.uvs = uv_mode.then_some(uvs);
        p.patch = m.patch(f);
        polys.push(p);
    }
    if cap {
        let patch = (0..m.face_count() as u32).map(|f| m.patch(f)).max().unwrap_or(0).saturating_add(1);
        let mut starts: Vec<u32> = next.keys().copied().collect();
        starts.sort_unstable();
        let mut seen: HashMap<u32, ()> = HashMap::with_capacity(next.len());
        for s in starts {
            if seen.contains_key(&s) {
                continue;
            }
            let mut ring = vec![s];
            seen.insert(s, ());
            let mut v = s;
            let mut closed = false;
            while let Some(w) = next.get(&v).copied() {
                if w == s {
                    closed = true;
                    break;
                }
                if seen.insert(w, ()).is_some() {
                    break;
                }
                ring.push(w);
                v = w;
            }
            if !closed || ring.len() < 3 {
                continue;
            }
            let mut add = |verts: Vec<u32>| {
                let k = verts.len();
                let mut p = Polygon::new(verts);
                p.uvs = uv_mode.then(|| vec![Vec2::ZERO; k]);
                p.patch = patch;
                polys.push(p);
            };
            if ring.len() <= 4 {
                add(ring);
                continue;
            }
            let centre = ring.iter().map(|v| pos.get(*v as usize).copied().unwrap_or_default()).sum::<Vec3>() / ring.len() as f32;
            pos.push(centre);
            let c = (pos.len() - 1) as u32;
            for i in 0..ring.len() {
                if let (Some(a), Some(b)) = (ring.get(i), ring.get((i + 1) % ring.len())) {
                    add(vec![*a, *b, c]);
                }
            }
        }
    }
    // Drop the vertices that went with the trimmed side.
    let mut used = vec![false; pos.len()];
    for p in &polys {
        for v in &p.verts {
            if let Some(u) = used.get_mut(*v as usize) {
                *u = true;
            }
        }
    }
    let mut remap = vec![u32::MAX; pos.len()];
    let mut kept = Vec::with_capacity(pos.len());
    for (i, p) in pos.iter().enumerate() {
        if used.get(i).copied().unwrap_or(false) {
            if let Some(r) = remap.get_mut(i) {
                *r = kept.len() as u32;
            }
            kept.push(*p);
        }
    }
    for p in &mut polys {
        for v in &mut p.verts {
            *v = remap.get(*v as usize).copied().unwrap_or(u32::MAX);
        }
    }
    rebuild(kept, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::{closed, euler};

    fn volume(m: &PolyMesh) -> f32 {
        let (tris, _) = m.triangulate();
        tris.iter().map(|t| m.position(t[0]).dot(m.position(t[1]).cross(m.position(t[2]))) / 6.0).sum()
    }

    fn ball() -> PolyMesh {
        Primitive::default_of("round_cube").unwrap().build().unwrap()
    }

    #[test]
    fn a_capped_trim_stays_closed_and_halves_a_ball() {
        let m = ball();
        let full = volume(&m);
        let out = trim_plane(&m, Vec3::ZERO, Vec3::Y, true).unwrap();
        out.validate().unwrap();
        assert!(closed(&out));
        assert_eq!(euler(&out), 2);
        assert!(out.positions().iter().all(|p| p.y <= 1e-5));
        let half = volume(&out);
        assert!((half / full - 0.5).abs() < 0.01, "{half} of {full}");
        // The cap got a patch of its own.
        let cap = (0..out.face_count() as u32).filter(|f| out.face_normal(*f).y > 0.99).count();
        assert!(cap >= 3);
    }

    #[test]
    fn an_open_trim_leaves_one_hole() {
        let out = trim_plane(&ball(), Vec3::new(0.0, 0.13, 0.0), Vec3::new(0.3, 1.0, 0.1), false).unwrap();
        out.validate().unwrap();
        assert!(!closed(&out));
        assert_eq!(euler(&out), 1, "a disc");
    }

    #[test]
    fn a_plane_through_vertices_makes_no_double_points() {
        // A cube cut exactly through its corners' plane x = 0 after a loop of vertices sits there.
        let m = Primitive::default_of("cube").unwrap().build().unwrap();
        let cutm = trim_plane(&m, Vec3::ZERO, Vec3::X, true).unwrap();
        let again = trim_plane(&cutm, Vec3::ZERO, Vec3::X, true).unwrap();
        assert_eq!(again.vertex_count(), cutm.vertex_count(), "a second cut on the same plane changes nothing");
        assert!(closed(&cutm));
        // And a cut that removes whole faces lying beyond a plane through existing vertices.
        let slab = trim_plane(&cutm, Vec3::ZERO, Vec3::NEG_X, false);
        assert!(slab.is_err(), "everything is on or beyond the plane");
    }

    #[test]
    fn missing_or_bad_planes() {
        let m = ball();
        assert_eq!(trim_plane(&m, Vec3::new(0.0, 50.0, 0.0), Vec3::Y, true).unwrap(), m);
        assert!(trim_plane(&m, Vec3::new(0.0, -50.0, 0.0), Vec3::Y, true).is_err());
        assert!(trim_plane(&m, Vec3::ZERO, Vec3::ZERO, true).is_err());
        assert!(trim_plane(&m, Vec3::splat(f32::NAN), Vec3::Y, true).is_err());
    }

    #[test]
    fn uvs_and_patches_survive() {
        let m = ball();
        let out = trim_plane(&m, Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0), true).unwrap();
        assert_eq!(out.has_uvs(), m.has_uvs());
        let most = (0..m.face_count() as u32).map(|f| m.patch(f)).max().unwrap();
        assert_eq!((0..out.face_count() as u32).map(|f| out.patch(f)).max().unwrap(), most + 1);
    }
}
