//! Extrude with options: faces (offset, steps along the wall, seeded variation, with or without
//! caps, region or one by one) and border edges.

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};
use modelcraft_mesh::NONE;

use crate::edit::{polys_of, push};
use crate::{OpError, OpResult, PolyMesh, Polygon, clean_selection, rebuild};

/// Most rows a wall may be divided into.
pub const MAX_SUBDIVISIONS: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExtrudeOptions {
    /// How far the new faces move along their normals.
    pub distance: f32,
    /// Slide the new border towards (positive) or away from (negative) the middle of the
    /// extruded faces, in model units.
    pub offset: f32,
    /// Rows of quads on each wall, 1 to [`MAX_SUBDIVISIONS`].
    pub subdivisions: u32,
    /// Random change of the distance, 0 to 1 of `distance` (seeded, so it repeats exactly).
    pub variation: f32,
    pub seed: u32,
    /// Keep the faces at the end of the extrusion; without them the wall is left open.
    pub caps: bool,
    /// Extrude each face on its own instead of the selection as one region.
    pub individual: bool,
}

impl Default for ExtrudeOptions {
    fn default() -> Self {
        ExtrudeOptions { distance: 0.0, offset: 0.0, subdivisions: 1, variation: 0.0, seed: 0, caps: true, individual: false }
    }
}

/// A repeatable number in -1..1 for a key and a seed (an integer hash, no state).
fn jitter(seed: u32, key: u32) -> f32 {
    let mut x = u64::from(key) ^ (u64::from(seed) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
}

/// A slide towards the middle: positive amounts stop short of it, negative ones go outwards.
fn slide(offset: f32, to_middle: f32) -> f32 {
    if offset >= 0.0 { offset.min(to_middle * 0.9) } else { offset }
}

fn clean(o: &ExtrudeOptions) -> Option<ExtrudeOptions> {
    if !o.distance.is_finite() || !o.offset.is_finite() || !o.variation.is_finite() {
        return None;
    }
    Some(ExtrudeOptions { subdivisions: o.subdivisions.clamp(1, MAX_SUBDIVISIONS), variation: o.variation.clamp(0.0, 1.0), ..*o })
}

/// Extrude faces. With `individual` each face goes on its own (walls between neighbours stay);
/// otherwise faces that share edges move together and only the border of the region gets walls.
pub fn extrude_faces_with(m: &PolyMesh, faces: &[u32], opts: &ExtrudeOptions) -> OpResult {
    let sel = clean_selection(faces, m.face_count());
    let Some(o) = clean(opts) else { return Ok(m.clone()) };
    if sel.is_empty() {
        return Ok(m.clone());
    }
    let rows = o.subdivisions as usize;
    if sel.len().saturating_mul(rows.saturating_mul(4)) > crate::MAX_RESULT_FACES {
        return Err(OpError::TooLarge(sel.len().saturating_mul(rows)));
    }
    let in_sel: HashSet<u32> = sel.iter().copied().collect();
    let (mut pos, mut polys) = polys_of(m);
    let uv_mode = m.has_uvs();
    // Ring r (0 = the original, rows = the end) of every extruded vertex: `rings[v][r]`.
    // Individual mode keys by (face, corner) instead, so it builds its own table.
    if o.individual {
        let mut cap_polys: Vec<(u32, Polygon)> = Vec::new();
        let mut walls: Vec<Polygon> = Vec::new();
        for &f in &sel {
            let n = m.face_normal(f);
            let c = m.face_center(f);
            let k = 1.0 + o.variation * jitter(o.seed, f);
            let hs: Vec<u32> = m.face_half_edges(f).collect();
            // Ring points per corner.
            let table: Vec<Vec<u32>> = hs
                .iter()
                .map(|h| {
                    let v = m.origin(*h);
                    let p = m.position(v);
                    let inward = (c - p).normalize_or_zero() * slide(o.offset, (c - p).length());
                    let end = p + n * o.distance * k + inward;
                    let mut col = vec![v];
                    for r in 1..=rows {
                        col.push(push(&mut pos, p.lerp(end, r as f32 / rows as f32)));
                    }
                    col
                })
                .collect();
            let top: Vec<u32> = table.iter().map(|c| c.last().copied().unwrap_or(0)).collect();
            let mut cap = Polygon::new(top);
            if uv_mode {
                cap.uvs = Some(hs.iter().map(|h| m.uv(*h)).collect());
            }
            cap.patch = m.patch(f);
            cap_polys.push((f, cap));
            let nn = hs.len();
            for i in 0..nn {
                let j = (i + 1) % nn;
                let (Some(ci), Some(cj)) = (table.get(i), table.get(j)) else { continue };
                for r in 0..rows {
                    let (Some(&a0), Some(&b0), Some(&b1), Some(&a1)) = (ci.get(r), cj.get(r), cj.get(r + 1), ci.get(r + 1)) else { continue };
                    let mut w = Polygon::new(vec![a0, b0, b1, a1]);
                    if uv_mode {
                        w.uvs = Some(wall_uvs(m, hs.get(i).copied().unwrap_or(0), r, rows));
                    }
                    w.patch = m.patch(f);
                    walls.push(w);
                }
            }
        }
        for (f, cap) in cap_polys {
            if let Some(slot) = polys.get_mut(f as usize) {
                if o.caps {
                    *slot = cap;
                } else {
                    slot.verts.clear();
                }
            }
        }
        polys.retain(|p| p.verts.len() >= 3);
        polys.extend(walls);
        return rebuild(pos, polys);
    }

    // Region mode.
    let mut dir: HashMap<u32, Vec3> = HashMap::new();
    let mut centers: HashMap<u32, (Vec3, f32)> = HashMap::new();
    for &f in &sel {
        let n = m.face_normal(f);
        let c = m.face_center(f);
        for v in m.face_vertices(f) {
            *dir.entry(v).or_default() += n;
            let e = centers.entry(v).or_default();
            e.0 += c;
            e.1 += 1.0;
        }
    }
    let mut verts: Vec<u32> = dir.keys().copied().collect();
    verts.sort_unstable();
    let mut table: HashMap<u32, Vec<u32>> = HashMap::new();
    for v in verts {
        let d = dir.get(&v).copied().unwrap_or_default();
        // Keep the offset distance for corners (scale by 1/cos of the spread).
        let n = d.normalize_or_zero();
        let mean = sel.iter().filter(|f| m.face_vertices(**f).any(|x| x == v)).map(|f| m.face_normal(*f).dot(n)).fold(1.0f32, f32::min).max(0.35);
        let p = m.position(v);
        let k = 1.0 + o.variation * jitter(o.seed, v);
        // Slide inwards along the plane of the region by the offset.
        let inward = match centers.get(&v) {
            Some((sum, count)) if *count > 0.0 && o.offset != 0.0 => {
                let to = *sum / *count - p;
                let flat = to - n * to.dot(n);
                let len = flat.length();
                if len > 1e-9 { flat / len * slide(o.offset, len) } else { Vec3::ZERO }
            }
            _ => Vec3::ZERO,
        };
        let end = p + n * o.distance * k / mean + inward;
        let mut col = vec![v];
        for r in 1..=rows {
            col.push(push(&mut pos, p.lerp(end, r as f32 / rows as f32)));
        }
        table.insert(v, col);
    }
    let end_of = |v: u32| table.get(&v).and_then(|c| c.last().copied()).unwrap_or(v);
    for &f in &sel {
        if let Some(poly) = polys.get_mut(f as usize) {
            if o.caps {
                for v in &mut poly.verts {
                    *v = end_of(*v);
                }
            } else {
                poly.verts.clear();
            }
        }
    }
    polys.retain(|p| p.verts.len() >= 3);
    for &f in &sel {
        for h in m.face_half_edges(f) {
            let other = m.face_of(m.twin(h));
            if other != NONE && in_sel.contains(&other) {
                continue;
            }
            let (a, b) = (m.origin(h), m.dest(h));
            let (Some(ca), Some(cb)) = (table.get(&a), table.get(&b)) else { continue };
            for r in 0..rows {
                let (Some(&a0), Some(&b0), Some(&b1), Some(&a1)) = (ca.get(r), cb.get(r), cb.get(r + 1), ca.get(r + 1)) else { continue };
                let mut p = Polygon::new(vec![a0, b0, b1, a1]);
                if uv_mode {
                    p.uvs = Some(wall_uvs(m, h, r, rows));
                }
                p.patch = m.patch(f);
                polys.push(p);
            }
        }
    }
    rebuild(pos, polys)
}

/// UVs of wall row `r` of `rows` along half-edge `h`: the edge's own UVs, stepping down.
fn wall_uvs(m: &PolyMesh, h: u32, r: usize, rows: usize) -> Vec<Vec2> {
    let (ua, ub) = (m.uv(h), m.uv(m.next(h)));
    let step = Vec2::new(0.0, 0.05 / rows as f32);
    let (lo, hi) = (step * r as f32, step * (r + 1) as f32);
    vec![ua - lo, ub - lo, ub - hi, ua - hi]
}

/// Extrude edges on the border of an open mesh: each gets a new quad wall and the edge moves
/// outwards by `distance` (away from its face, in the plane of the face) and by `lift` along
/// the face normal. Edges that are not on the border are skipped, because a closed surface has no
/// free side to pull them to; no usable edge is an error.
pub fn extrude_edges(m: &PolyMesh, edges: &[u32], distance: f32, lift: f32) -> OpResult {
    if !distance.is_finite() || !lift.is_finite() {
        return Ok(m.clone());
    }
    let sel = clean_selection(edges, m.edge_count());
    if sel.is_empty() {
        return Ok(m.clone());
    }
    // (boundary half-edge a -> b with no face, the face on its other side)
    let mut walls: Vec<(u32, u32, u32, u32)> = Vec::new();
    for e in sel {
        let h = m.edge_half_edge(e);
        let (hb, hf) = if m.face_of(h) == NONE { (h, m.twin(h)) } else { (m.twin(h), h) };
        if m.face_of(hb) != NONE || m.face_of(hf) == NONE {
            continue;
        }
        walls.push((m.origin(hb), m.dest(hb), m.face_of(hf), hf));
    }
    if walls.is_empty() {
        return Err(OpError::Invalid("pick edges on the border of the mesh: the edges inside a closed surface have no free side to extrude into".into()));
    }
    if walls.len() > crate::MAX_RESULT_FACES {
        return Err(OpError::TooLarge(walls.len()));
    }
    // Direction per vertex: the outward pushes of the walls that meet there, mitred.
    let outward = |hf: u32, f: u32| -> (Vec3, Vec3) {
        let (a, b) = (m.position(m.origin(hf)), m.position(m.dest(hf)));
        let n = m.face_normal(f);
        let mut out = n.cross(b - a).normalize_or_zero();
        if out.dot(m.face_center(f) - (a + b) * 0.5) > 0.0 {
            out = -out;
        }
        (out, n)
    };
    let mut push_sum: HashMap<u32, (Vec3, Vec3)> = HashMap::new();
    for (a, b, f, hf) in &walls {
        let (out, n) = outward(*hf, *f);
        for v in [*a, *b] {
            let e = push_sum.entry(v).or_default();
            e.0 += out;
            e.1 += n;
        }
    }
    let (mut pos, mut polys) = polys_of(m);
    let mut copy: HashMap<u32, u32> = HashMap::new();
    let mut vs: Vec<u32> = push_sum.keys().copied().collect();
    vs.sort_unstable();
    for v in vs {
        let (o, n) = push_sum.get(&v).copied().unwrap_or_default();
        let d = o.normalize_or_zero();
        // Walls that meet at a corner push along the bisector, scaled so each keeps its distance.
        let cos = walls.iter().filter(|w| w.0 == v || w.1 == v).map(|w| outward(w.3, w.2).0.dot(d)).fold(1.0f32, f32::min).max(0.35);
        let p = m.position(v) + d * distance / cos + n.normalize_or_zero() * lift;
        copy.insert(v, push(&mut pos, p));
    }
    for (a, b, f, hf) in walls {
        let (Some(&a2), Some(&b2)) = (copy.get(&a), copy.get(&b)) else { continue };
        let mut p = Polygon::new(vec![a, b, b2, a2]);
        if m.has_uvs() {
            let (ub, ua) = (m.uv(hf), m.uv(m.next(hf)));
            // hf runs b → a; the wall runs a → b.
            let down = Vec2::new(0.0, 0.05);
            p.uvs = Some(vec![ua, ub, ub - down, ua - down]);
        }
        p.patch = m.patch(f);
        polys.push(p);
    }
    rebuild(pos, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
    }
    fn grid() -> PolyMesh {
        Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap()
    }

    #[test]
    fn the_defaults_are_the_plain_extrude() {
        let m = extrude_faces_with(&cube(), &[0], &ExtrudeOptions { distance: 1.0, ..Default::default() }).unwrap();
        assert_eq!((m.vertex_count(), m.face_count()), (12, 10));
        assert_eq!(m, crate::edit::extrude_faces(&cube(), &[0], 1.0).unwrap());
        assert!(closed(&m));
    }

    #[test]
    fn subdivisions_add_rows_to_every_wall() {
        for rows in 1..=6u32 {
            let m = extrude_faces_with(&cube(), &[0], &ExtrudeOptions { distance: 1.0, subdivisions: rows, ..Default::default() }).unwrap();
            assert!(closed(&m));
            assert_eq!(euler(&m), 2);
            assert_eq!(m.face_count(), 6 + 4 * rows as usize);
        }
    }

    #[test]
    fn without_caps_the_end_stays_open() {
        let m = extrude_faces_with(&cube(), &[0], &ExtrudeOptions { distance: 1.0, caps: false, ..Default::default() }).unwrap();
        assert!(!closed(&m));
        let open = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).count();
        assert_eq!(open, 4);
        m.validate().unwrap();
    }

    #[test]
    fn offset_slides_the_border_in_and_out() {
        // The box is 2 wide; the extruded face makes it 3 tall. Its narrowest extent shows
        // whether the cap stayed inside the box or grew past it.
        let at = |offset: f32| {
            let m = extrude_faces_with(&cube(), &[0], &ExtrudeOptions { distance: 1.0, offset, ..Default::default() }).unwrap();
            let (lo, hi) = m.bounds();
            (hi - lo).min_element()
        };
        assert!((at(0.0) - 2.0).abs() < 1e-4);
        assert!((at(0.4) - 2.0).abs() < 1e-4, "a positive offset pulls the cap inside");
        // The corners slide 0.5 along the diagonal: 0.5 / sqrt 2 on each axis, on both sides.
        assert!((at(-0.5) - (2.0 + 2.0 * 0.5 / 2f32.sqrt())).abs() < 1e-3, "a negative offset grows the cap beyond the box");
        let inset = extrude_faces_with(&cube(), &[0], &ExtrudeOptions { distance: 1.0, offset: 0.4, ..Default::default() }).unwrap();
        assert!(closed(&inset));
        assert_eq!(euler(&inset), 2);
    }

    #[test]
    fn region_interior_vertices_stay_put_with_an_offset() {
        let g = grid();
        let m = extrude_faces_with(&g, &[0, 1, 2, 3], &ExtrudeOptions { distance: 0.5, offset: 0.2, ..Default::default() }).unwrap();
        // The middle vertex of the 2 x 2 grid has all four faces around it: it moves only up.
        let centre = m.positions().iter().filter(|p| p.x.abs() < 1e-5 && p.z.abs() < 1e-5 && p.y > 0.4).count();
        assert_eq!(centre, 1);
        m.validate().unwrap();
    }

    #[test]
    fn individual_extrusion_keeps_neighbouring_walls() {
        let g = grid();
        let region = extrude_faces_with(&g, &[0, 1, 2, 3], &ExtrudeOptions { distance: 0.5, ..Default::default() }).unwrap();
        let each = extrude_faces_with(&g, &[0, 1, 2, 3], &ExtrudeOptions { distance: 0.5, individual: true, ..Default::default() }).unwrap();
        assert_eq!(region.face_count(), 12);
        // Four caps and 4 * 4 walls.
        assert_eq!(each.face_count(), 4 + 16);
        each.validate().unwrap();
        let c = extrude_faces_with(&cube(), &[0, 1, 2, 3, 4, 5], &ExtrudeOptions { distance: 0.3, individual: true, ..Default::default() }).unwrap();
        assert!(closed(&c));
        assert_eq!(euler(&c), 2, "every shared edge now has a wall on each side");
        assert_eq!(c.face_count(), 6 + 24);
    }

    #[test]
    fn variation_is_seeded_and_bounded() {
        let o = ExtrudeOptions { distance: 1.0, variation: 0.5, seed: 7, individual: true, ..Default::default() };
        let a = extrude_faces_with(&cube(), &[0, 1, 2, 3, 4, 5], &o).unwrap();
        let b = extrude_faces_with(&cube(), &[0, 1, 2, 3, 4, 5], &o).unwrap();
        assert_eq!(a, b, "same seed, same result");
        let c = extrude_faces_with(&cube(), &[0, 1, 2, 3, 4, 5], &ExtrudeOptions { seed: 8, ..o }).unwrap();
        assert_ne!(a, c);
        let (lo, hi) = a.bounds();
        // Faces start at 1.0 from the centre and move 0.5..1.5 outwards.
        assert!(hi.max_element() <= 2.5 + 1e-4 && lo.min_element() >= -2.5 - 1e-4);
        for seed in 0..50 {
            for key in 0..50 {
                let j = jitter(seed, key);
                assert!((-1.0..=1.0).contains(&j));
            }
        }
    }

    #[test]
    fn hostile_numbers_change_nothing_or_are_clamped() {
        let c = cube();
        assert_eq!(extrude_faces_with(&c, &[0], &ExtrudeOptions { distance: f32::NAN, ..Default::default() }).unwrap(), c);
        assert_eq!(extrude_faces_with(&c, &[99], &ExtrudeOptions { distance: 1.0, ..Default::default() }).unwrap(), c);
        let m = extrude_faces_with(&c, &[0], &ExtrudeOptions { distance: 1.0, subdivisions: 9999, variation: 50.0, ..Default::default() }).unwrap();
        assert_eq!(m.face_count(), 6 + 4 * MAX_SUBDIVISIONS as usize);
    }

    #[test]
    fn border_edges_extrude_and_inner_edges_do_not() {
        let g = grid();
        let border: Vec<u32> = (0..g.edge_count() as u32).filter(|e| g.is_boundary_edge(*e)).collect();
        assert_eq!(border.len(), 8);
        let m = extrude_edges(&g, &border, 0.5, 0.0).unwrap();
        m.validate().unwrap();
        assert_eq!(m.face_count(), 4 + 8);
        let (lo, hi) = m.bounds();
        assert!((hi.x - 1.5).abs() < 1e-4 && (lo.z + 1.5).abs() < 1e-4, "the border moved outwards: {lo:?} {hi:?}");
        // Lift raises the new edge.
        let up = extrude_edges(&g, &border[..1], 0.0, 0.7).unwrap();
        assert!(up.bounds().1.y > 0.69);
        // A closed cube has no border.
        assert!(extrude_edges(&cube(), &[0], 1.0, 0.0).is_err());
        assert_eq!(extrude_edges(&g, &[], 1.0, 0.0).unwrap(), g);
    }

    #[test]
    fn extruding_a_whole_border_ring_makes_a_clean_rim() {
        let g = grid();
        let border: Vec<u32> = (0..g.edge_count() as u32).filter(|e| g.is_boundary_edge(*e)).collect();
        let m = extrude_edges(&g, &border, 0.3, 0.0).unwrap();
        let open = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).count();
        assert_eq!(open, 8, "the new border has the same number of edges");
    }
}
