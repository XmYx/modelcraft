//! Smooth-cage subdivision: each level splits every n-gon into n quads and moves the points
//! towards a smooth limit surface (the Catmull–Clark rules, with boundary edges kept as curves).
//! UVs are split linearly; patches are kept.

use glam::{Vec2, Vec3};
use modelcraft_mesh::NONE;

use crate::{MAX_RESULT_FACES, OpError, OpResult, PolyMesh, Polygon, rebuild};

/// Highest level accepted (each level multiplies the faces by about four).
pub const MAX_LEVELS: u32 = 8;

/// Faces after `levels` subdivisions of `m` (exact for level 1, then ×4 per level).
pub fn face_count_after(m: &PolyMesh, levels: u32) -> usize {
    if levels == 0 {
        return m.face_count();
    }
    let first: usize = (0..m.face_count() as u32).map(|f| m.face_len(f)).sum();
    (1..levels).fold(first, |n, _| n.saturating_mul(4))
}

pub fn catmull_clark(m: &PolyMesh, levels: u32) -> OpResult {
    let levels = levels.min(MAX_LEVELS);
    let n = face_count_after(m, levels);
    if n > MAX_RESULT_FACES {
        return Err(OpError::TooLarge(n));
    }
    let mut out = m.clone();
    for _ in 0..levels {
        out = once(&out)?;
    }
    Ok(out)
}

fn once(m: &PolyMesh) -> OpResult {
    let (nv, ne, nf) = (m.vertex_count(), m.edge_count(), m.face_count());
    let face_pts: Vec<Vec3> = (0..nf as u32).map(|f| m.face_center(f)).collect();
    let fp = |f: u32| face_pts.get(f as usize).copied().unwrap_or_default();
    let edge_pts: Vec<Vec3> = (0..ne as u32)
        .map(|e| {
            let h = m.edge_half_edge(e);
            let (a, b) = (m.position(m.origin(h)), m.position(m.dest(h)));
            let (f1, f2) = (m.face_of(h), m.face_of(m.twin(h)));
            if f1 == NONE || f2 == NONE { (a + b) * 0.5 } else { (a + b + fp(f1) + fp(f2)) * 0.25 }
        })
        .collect();
    let mut pos = Vec::with_capacity(nv + ne + nf);
    for v in 0..nv as u32 {
        let p = m.position(v);
        let mut q = Vec3::ZERO;
        let mut r = Vec3::ZERO;
        let mut k = 0.0f32;
        let mut boundary: Vec<Vec3> = Vec::new();
        for h in m.vertex_out(v) {
            let w = m.position(m.dest(h));
            r += (p + w) * 0.5;
            k += 1.0;
            let f = m.face_of(h);
            if f != NONE {
                q += fp(f);
            }
            if m.face_of(h) == NONE || m.face_of(m.twin(h)) == NONE {
                boundary.push(w);
            }
        }
        let np = if k == 0.0 {
            p
        } else if !boundary.is_empty() {
            match boundary.as_slice() {
                [a, b] => (*a + *b + p * 6.0) / 8.0,
                _ => p,
            }
        } else {
            (q / k + r / k * 2.0 + p * (k - 3.0)) / k
        };
        pos.push(np);
    }
    pos.extend(edge_pts);
    pos.extend(face_pts.iter().copied());
    let uv_mode = m.has_uvs();
    let mut polys = Vec::with_capacity(nf * 4);
    for f in 0..nf as u32 {
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let k = hs.len();
        let centre = (nv + ne) as u32 + f;
        let uvs: Vec<Vec2> = hs.iter().map(|h| m.uv(*h)).collect();
        let uvc = uvs.iter().copied().sum::<Vec2>() / k.max(1) as f32;
        let g = |i: usize| uvs.get(i % k.max(1)).copied().unwrap_or_default();
        for i in 0..k {
            let (Some(&h), Some(&hp)) = (hs.get(i), hs.get((i + k - 1) % k)) else { continue };
            let eo = nv as u32 + m.edge_of(h);
            let ei = nv as u32 + m.edge_of(hp);
            let mut p = Polygon::new(vec![m.origin(h), eo, centre, ei]);
            if uv_mode {
                p.uvs = Some(vec![g(i), (g(i) + g(i + 1)) * 0.5, uvc, (g(i + k - 1) + g(i)) * 0.5]);
            }
            p.patch = m.patch(f);
            polys.push(p);
        }
    }
    rebuild(pos, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;

    #[test]
    fn cube_converges_towards_a_sphere() {
        let c = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
        let s = catmull_clark(&c, 3).unwrap();
        assert_eq!(s.face_count(), 6 * 64);
        assert!(closed(&s));
        assert_eq!(euler(&s), 2);
        let r: Vec<f32> = s.positions().iter().map(|p| p.length()).collect();
        let (lo, hi) = r.iter().fold((f32::MAX, 0.0f32), |(a, b), x| (a.min(*x), b.max(*x)));
        assert!(hi / lo < 1.25, "not round: {lo}..{hi}");
        assert!(s.has_uvs());
    }

    #[test]
    fn open_grid_keeps_its_border_and_count() {
        let g = Primitive::Plane { width: 2.0, depth: 2.0, segments: 1 }.build().unwrap();
        let s = catmull_clark(&g, 2).unwrap();
        assert_eq!(s.face_count(), 16);
        // The corner stays at the corner of a single quad (boundary vertex rule with 2 boundary neighbours).
        assert!(s.positions().iter().all(|p| p.y.abs() < 1e-6));
    }

    #[test]
    fn too_many_levels_is_an_error_not_an_allocation() {
        let c = Primitive::Cube { size: Vec3::ONE, segments: 8 }.build().unwrap();
        assert!(matches!(catmull_clark(&c, 8), Err(OpError::TooLarge(_))));
    }
}
