//! Polygon edits: extrude, inset, bevel, loop cut, merge, weld, delete, move, flip and
//! triangulate. Selections are element indices into the input mesh; invalid ones are ignored.

use std::collections::{HashMap, HashSet};

use glam::{Quat, Vec2, Vec3};
use modelcraft_mesh::NONE;

use crate::{OpError, OpResult, PolyMesh, Polygon, clean_selection, rebuild};

fn polys_of(m: &PolyMesh) -> (Vec<Vec3>, Vec<Polygon>) {
    (m.positions().to_vec(), m.to_polygons())
}

fn push(pos: &mut Vec<Vec3>, p: Vec3) -> u32 {
    pos.push(p);
    (pos.len() - 1) as u32
}

/// Extrude a region of faces along the averaged normals by `distance`, adding side walls along
/// the region's border. Faces that share edges move together.
pub fn extrude_faces(m: &PolyMesh, faces: &[u32], distance: f32) -> OpResult {
    let sel = clean_selection(faces, m.face_count());
    if sel.is_empty() || !distance.is_finite() {
        return Ok(m.clone());
    }
    let in_sel: HashSet<u32> = sel.iter().copied().collect();
    let (mut pos, mut polys) = polys_of(m);
    // Direction per vertex: sum of the selected faces' normals around it.
    let mut dir: HashMap<u32, Vec3> = HashMap::new();
    for &f in &sel {
        let n = m.face_normal(f);
        for v in m.face_vertices(f) {
            *dir.entry(v).or_default() += n;
        }
    }
    let mut copy: HashMap<u32, u32> = HashMap::new();
    let mut verts: Vec<u32> = dir.keys().copied().collect();
    verts.sort_unstable();
    for v in verts {
        let d = dir.get(&v).copied().unwrap_or_default();
        // Keep the offset distance for corners (scale by 1/cos of the spread).
        let n = d.normalize_or_zero();
        let mean = sel.iter().filter(|f| m.face_vertices(**f).any(|x| x == v)).map(|f| m.face_normal(*f).dot(n)).fold(1.0f32, f32::min).max(0.35);
        let p = m.position(v) + n * distance / mean;
        copy.insert(v, push(&mut pos, p));
    }
    for &f in &sel {
        if let Some(poly) = polys.get_mut(f as usize) {
            for v in &mut poly.verts {
                *v = copy.get(v).copied().unwrap_or(*v);
            }
        }
    }
    let uv_mode = m.has_uvs();
    for &f in &sel {
        for h in m.face_half_edges(f) {
            let t = m.twin(h);
            let other = m.face_of(t);
            if other != NONE && in_sel.contains(&other) {
                continue;
            }
            let (a, b) = (m.origin(h), m.dest(h));
            let (a2, b2) = (copy.get(&a).copied().unwrap_or(a), copy.get(&b).copied().unwrap_or(b));
            let mut p = Polygon::new(vec![a, b, b2, a2]);
            if uv_mode {
                let (ua, ub) = (m.uv(h), m.uv(m.next(h)));
                let up = Vec2::new(0.0, 0.05);
                p.uvs = Some(vec![ua, ub, ub - up, ua - up]);
            }
            p.patch = m.patch(f);
            polys.push(p);
        }
    }
    rebuild(pos, polys)
}

/// Inset each selected face on its own: the face shrinks towards its centre by `amount`
/// (0..1, fraction of the way to the centre) and a ring of quads fills the gap.
pub fn inset_faces(m: &PolyMesh, faces: &[u32], amount: f32) -> OpResult {
    let sel = clean_selection(faces, m.face_count());
    let t = if amount.is_finite() { amount.clamp(0.0, 0.99) } else { 0.0 };
    if sel.is_empty() || t == 0.0 {
        return Ok(m.clone());
    }
    let (mut pos, mut polys) = polys_of(m);
    let uv_mode = m.has_uvs();
    for &f in &sel {
        let c = m.face_center(f);
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let outer: Vec<u32> = hs.iter().map(|h| m.origin(*h)).collect();
        let inner: Vec<u32> = outer.iter().map(|v| push(&mut pos, m.position(*v).lerp(c, t))).collect();
        let uv_out: Vec<Vec2> = hs.iter().map(|h| m.uv(*h)).collect();
        let uv_c = uv_out.iter().copied().sum::<Vec2>() / uv_out.len().max(1) as f32;
        let uv_in: Vec<Vec2> = uv_out.iter().map(|u| u.lerp(uv_c, t)).collect();
        let n = outer.len();
        let patch = m.patch(f);
        if let Some(poly) = polys.get_mut(f as usize) {
            poly.verts = inner.clone();
            poly.uvs = uv_mode.then(|| uv_in.clone());
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let (Some(&a), Some(&b), Some(&bi), Some(&ai)) = (outer.get(i), outer.get(j), inner.get(j), inner.get(i)) else { continue };
            let mut p = Polygon::new(vec![a, b, bi, ai]);
            if uv_mode {
                let g = |v: &Vec<Vec2>, k: usize| v.get(k).copied().unwrap_or_default();
                p.uvs = Some(vec![g(&uv_out, i), g(&uv_out, j), g(&uv_in, j), g(&uv_in, i)]);
            }
            p.patch = patch;
            polys.push(p);
        }
    }
    rebuild(pos, polys)
}

/// Chamfer the selected edges with a flat strip of the given width. Boundary edges are skipped.
/// Corners where beveled edges meet are closed with new polygons.
pub fn bevel_edges(m: &PolyMesh, edges: &[u32], width: f32) -> OpResult {
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
    let mut polys = Vec::with_capacity(m.face_count() + sel.len());
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
    // Strips.
    let mut sorted: Vec<u32> = sel.iter().copied().collect();
    sorted.sort_unstable();
    for e in sorted {
        let h1 = m.edge_half_edge(e);
        let h2 = m.twin(h1);
        let get = |map: &HashMap<u32, u32>, h: u32| map.get(&h).copied();
        let (Some(a1), Some(b1), Some(b2), Some(a2)) = (get(&corner_pt, h1), get(&corner_end, h1), get(&corner_pt, h2), get(&corner_end, h2)) else {
            continue;
        };
        let mut p = Polygon::new(vec![b1, a1, a2, b2]);
        if uv_mode {
            p.uvs = Some(vec![m.uv(m.next(h1)), m.uv(h1), m.uv(m.next(h2)), m.uv(h2)]);
        }
        p.patch = m.patch(m.face_of(h1));
        polys.push(p);
    }
    fill_new_holes(&mut polys, orig_verts, &touched, uv_mode);
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
    let mut seen = HashSet::new();
    for s in starts {
        if seen.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        let mut c = s;
        let mut ok = false;
        for _ in 0..64 {
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

/// The ring of edges across quads that starts at `edge`: each step crosses a quad to its
/// opposite edge. Stops at a non-quad, a boundary or when the ring closes.
pub fn edge_ring(m: &PolyMesh, edge: u32) -> Vec<u32> {
    if edge as usize >= m.edge_count() {
        return Vec::new();
    }
    let start = m.edge_half_edge(edge);
    let mut ring = vec![edge];
    let mut seen: HashSet<u32> = HashSet::from([edge]);
    for first in [start, m.twin(start)] {
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
            if first == start {
                ring.push(e);
            } else {
                ring.insert(0, e);
            }
            h = m.twin(opp);
        }
    }
    ring
}

/// Insert `cuts` evenly spaced edge loops across the ring of quads through `edge`.
pub fn loop_cut(m: &PolyMesh, edge: u32, cuts: u32) -> OpResult {
    let ring = edge_ring(m, edge);
    let n = cuts.clamp(1, 64);
    if ring.is_empty() {
        return Ok(m.clone());
    }
    let ring_set: HashSet<u32> = ring.iter().copied().collect();
    let mut pos = m.positions().to_vec();
    // New vertices per ring edge, along its canonical direction.
    let mut nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    for &e in &ring {
        let (a, b) = m.edge_vertices(e);
        let (pa, pb) = (m.position(a), m.position(b));
        let ids = (1..=n).map(|k| push(&mut pos, pa.lerp(pb, k as f32 / (n + 1) as f32))).collect();
        nodes.insert(e, ids);
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
        let (u0, u1) = (m.uv(h), m.uv(m.next(h)));
        (0..=n + 1).map(|k| u0.lerp(u1, k as f32 / (n + 1) as f32)).collect()
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
            for k in 0..=(n as usize) {
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

/// Merge the selected vertices into one at their centre.
pub fn merge_vertices(m: &PolyMesh, verts: &[u32]) -> OpResult {
    let sel = clean_selection(verts, m.vertex_count());
    let Some(&keep) = sel.first() else { return Ok(m.clone()) };
    if sel.len() < 2 {
        return Ok(m.clone());
    }
    let centre = sel.iter().map(|v| m.position(*v)).sum::<Vec3>() / sel.len() as f32;
    let set: HashSet<u32> = sel.iter().copied().collect();
    let (mut pos, mut polys) = polys_of(m);
    if let Some(p) = pos.get_mut(keep as usize) {
        *p = centre;
    }
    for p in &mut polys {
        for v in &mut p.verts {
            if set.contains(v) {
                *v = keep;
            }
        }
    }
    compact(pos, polys)
}

/// Weld vertices closer than `distance` (for imported triangle soups).
pub fn weld(m: &PolyMesh, distance: f32) -> OpResult {
    let d = if distance.is_finite() { distance.max(1e-7) } else { 1e-5 };
    let cell = |p: Vec3| ((p.x / d).floor() as i64, (p.y / d).floor() as i64, (p.z / d).floor() as i64);
    let mut grid: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut remap: Vec<u32> = (0..m.vertex_count() as u32).collect();
    for v in 0..m.vertex_count() as u32 {
        let p = m.position(v);
        let (cx, cy, cz) = cell(p);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &w in grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                        if m.position(w).distance(p) <= d {
                            found = Some(w);
                            break 'search;
                        }
                    }
                }
            }
        }
        match found {
            Some(w) => {
                if let Some(r) = remap.get_mut(v as usize) {
                    *r = w;
                }
            }
            None => grid.entry((cx, cy, cz)).or_default().push(v),
        }
    }
    let (pos, mut polys) = polys_of(m);
    for p in &mut polys {
        for v in &mut p.verts {
            *v = remap.get(*v as usize).copied().unwrap_or(*v);
        }
    }
    compact(pos, polys)
}

/// Drop unused vertices and renumber.
fn compact(pos: Vec<Vec3>, mut polys: Vec<Polygon>) -> OpResult {
    let mut used = vec![NONE; pos.len()];
    let mut out = Vec::new();
    for p in &mut polys {
        for v in &mut p.verts {
            let Some(slot) = used.get_mut(*v as usize) else { continue };
            if *slot == NONE {
                *slot = out.len() as u32;
                out.push(pos.get(*v as usize).copied().unwrap_or_default());
            }
            *v = *slot;
        }
    }
    rebuild(out, polys)
}

pub fn delete_faces(m: &PolyMesh, faces: &[u32]) -> OpResult {
    let sel: HashSet<u32> = clean_selection(faces, m.face_count()).into_iter().collect();
    let (pos, polys) = polys_of(m);
    let keep = polys.into_iter().enumerate().filter(|(i, _)| !sel.contains(&(*i as u32))).map(|(_, p)| p).collect();
    compact(pos, keep)
}

pub fn delete_vertices(m: &PolyMesh, verts: &[u32]) -> OpResult {
    let sel: HashSet<u32> = clean_selection(verts, m.vertex_count()).into_iter().collect();
    let (pos, polys) = polys_of(m);
    compact(pos, polys.into_iter().filter(|p| !p.verts.iter().any(|v| sel.contains(v))).collect())
}

/// Delete edges by deleting the faces on both sides.
pub fn delete_edges(m: &PolyMesh, edges: &[u32]) -> OpResult {
    let faces: Vec<u32> = clean_selection(edges, m.edge_count())
        .into_iter()
        .flat_map(|e| {
            let h = m.edge_half_edge(e);
            [m.face_of(h), m.face_of(m.twin(h))]
        })
        .filter(|f| *f != NONE)
        .collect();
    delete_faces(m, &faces)
}

/// Transform the selected vertices about their centre: scale, then rotate, then translate.
pub fn transform_vertices(m: &PolyMesh, verts: &[u32], translate: Vec3, rotate: Quat, scale: Vec3) -> OpResult {
    let sel = clean_selection(verts, m.vertex_count());
    if sel.is_empty() {
        return Ok(m.clone());
    }
    let pivot = sel.iter().map(|v| m.position(*v)).sum::<Vec3>() / sel.len() as f32;
    let set: HashSet<u32> = sel.into_iter().collect();
    let rot = if rotate.is_finite() && rotate.length() > 1e-6 { rotate.normalize() } else { Quat::IDENTITY };
    let mut out = m.clone();
    out.map_positions(|i, p| if set.contains(&i) { pivot + rot * ((p - pivot) * scale) + translate } else { p });
    Ok(out)
}

/// Turn the selected faces around (all faces when the selection is empty).
pub fn flip_faces(m: &PolyMesh, faces: &[u32]) -> OpResult {
    let sel: HashSet<u32> = clean_selection(faces, m.face_count()).into_iter().collect();
    let (pos, mut polys) = polys_of(m);
    for (i, p) in polys.iter_mut().enumerate() {
        if sel.is_empty() || sel.contains(&(i as u32)) {
            p.verts.reverse();
            if let Some(u) = p.uvs.as_mut() {
                u.reverse();
            }
        }
    }
    rebuild(pos, polys)
}

pub fn triangulate(m: &PolyMesh) -> OpResult {
    let (pos, polys) = polys_of(m);
    let mut out = Vec::new();
    for p in polys {
        let n = p.verts.len();
        for i in 1..n.saturating_sub(1) {
            let pick = |k: usize| p.verts.get(k).copied().unwrap_or(0);
            let mut t = Polygon::new(vec![pick(0), pick(i), pick(i + 1)]);
            t.uvs = p.uvs.as_ref().map(|u| [0, i, i + 1].iter().map(|k| u.get(*k).copied().unwrap_or_default()).collect());
            t.patch = p.patch;
            out.push(t);
        }
    }
    rebuild(pos, out)
}

/// Assign the selected faces to a patch.
pub fn set_patch(m: &PolyMesh, faces: &[u32], patch: u32) -> OpResult {
    let sel: HashSet<u32> = clean_selection(faces, m.face_count()).into_iter().collect();
    let (pos, mut polys) = polys_of(m);
    for (i, p) in polys.iter_mut().enumerate() {
        if sel.contains(&(i as u32)) {
            p.patch = patch;
        }
    }
    rebuild(pos, polys)
}

/// Check a selection fits the mesh (used by history steps to report stale selections).
pub fn check_selection(sel: &[u32], len: usize, what: &str) -> Result<(), OpError> {
    match sel.iter().find(|i| **i as usize >= len) {
        Some(i) => Err(OpError::Invalid(format!("{what} {i} no longer exists (the mesh has {len})"))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::*;
    use proptest::prelude::*;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
    }

    #[test]
    fn extrude_one_face_of_a_cube() {
        let m = extrude_faces(&cube(), &[0], 1.0).unwrap();
        assert_eq!((m.vertex_count(), m.face_count()), (12, 10));
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
    }

    #[test]
    fn extrude_region_keeps_inner_edges() {
        let g = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
        let m = extrude_faces(&g, &[0, 1, 2, 3], 0.5).unwrap();
        // 4 moved faces + 8 walls; the walls replace the open border.
        assert_eq!(m.face_count(), 12);
        assert_eq!(m.vertex_count(), 18);
    }

    #[test]
    fn inset_cube_face() {
        let m = inset_faces(&cube(), &[2], 0.3).unwrap();
        assert_eq!(m.face_count(), 10);
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
    }

    #[test]
    fn bevel_one_cube_edge() {
        let c = cube();
        let m = bevel_edges(&c, &[0], 0.2).unwrap();
        assert!(closed(&m), "bevel left a hole");
        assert_eq!(euler(&m), 2);
        assert_eq!((m.vertex_count(), m.face_count(), m.edge_count()), (10, 7, 15));
    }

    #[test]
    fn bevel_all_cube_edges_stays_closed() {
        let c = cube();
        let all: Vec<u32> = (0..12).collect();
        let m = bevel_edges(&c, &all, 0.2).unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        assert_eq!(m.face_count(), 6 + 12 + 8);
    }

    #[test]
    fn bevel_on_a_grid_fills_corner_holes() {
        let g = Primitive::Plane { width: 3.0, depth: 3.0, segments: 3 }.build().unwrap();
        // An interior edge of the grid.
        let e = (0..g.edge_count() as u32)
            .find(|e| {
                let (a, b) = g.edge_vertices(*e);
                !g.is_boundary_vertex(a) && !g.is_boundary_vertex(b)
            })
            .unwrap();
        let m = bevel_edges(&g, &[e], 0.1).unwrap();
        let boundary = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).count();
        assert_eq!(boundary, 12, "only the outer border stays open");
    }

    #[test]
    fn loop_cut_around_a_cube() {
        let c = cube();
        let ring = edge_ring(&c, 0);
        assert_eq!(ring.len(), 4, "a cube ring has four parallel edges");
        let m = loop_cut(&c, 0, 2).unwrap();
        assert_eq!(m.face_count(), 6 + 4 * 2);
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
    }

    #[test]
    fn loop_cut_stops_at_triangles_and_stays_conforming() {
        let s = Primitive::Sphere { radius: 1.0, segments: 8, rings: 6 }.build().unwrap();
        // A vertical edge in the quad band.
        let e = (0..s.edge_count() as u32)
            .find(|e| {
                let (a, b) = s.edge_vertices(*e);
                (s.position(a).y - s.position(b).y).abs() > 0.1 && s.position(a).y.abs() < 0.6 && s.position(b).y.abs() < 0.6
            })
            .unwrap();
        let m = loop_cut(&s, e, 1).unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        assert_eq!(m.face_count(), s.face_count() + 8);
    }

    #[test]
    fn merge_collapses_a_face() {
        let m = merge_vertices(&cube(), &[0, 1, 2, 3]).unwrap();
        m.validate().unwrap();
        assert!(m.vertex_count() < 8);
    }

    #[test]
    fn delete_and_weld() {
        let c = cube();
        let open = delete_faces(&c, &[0]).unwrap();
        assert_eq!(open.face_count(), 5);
        assert!(!closed(&open));
        let fewer = delete_vertices(&c, &[0]).unwrap();
        assert_eq!(fewer.face_count(), 3);
        assert_eq!(fewer.vertex_count(), 7);
        let tri = triangulate(&c).unwrap();
        assert_eq!(tri.face_count(), 12);
        let w = weld(&tri, 1e-4).unwrap();
        assert_eq!(w.vertex_count(), 8);
    }

    #[test]
    fn transform_and_flip() {
        let c = cube();
        let m = transform_vertices(&c, &[0, 1], Vec3::Y, Quat::IDENTITY, Vec3::ONE).unwrap();
        assert_eq!(m.position(0), c.position(0) + Vec3::Y);
        let f = flip_faces(&c, &[]).unwrap();
        assert!(f.face_normal(0).dot(f.face_center(0)) < 0.0);
    }

    #[test]
    fn stale_selections_are_ignored() {
        let c = cube();
        assert_eq!(extrude_faces(&c, &[99], 1.0).unwrap(), c);
        assert_eq!(bevel_edges(&c, &[1000], 0.1).unwrap(), c);
        assert_eq!(loop_cut(&c, 500, 1).unwrap(), c);
        assert!(check_selection(&[7], 6, "face").is_err());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// Random sequences of edits on random primitives always validate.
        #[test]
        fn random_edit_sequences_stay_valid(kind in 0usize..7, ops in prop::collection::vec((0u8..9, 0u32..200, -1.0f32..1.0), 1..6)) {
            let kind = crate::primitives::KINDS[kind];
            let mut m = Primitive::default_of(kind).unwrap();
            if let Primitive::Sphere { segments, rings, .. } = &mut m { *segments = 8; *rings = 5; }
            let mut m = m.build().unwrap();
            for (op, i, x) in ops {
                let f = if m.face_count() > 0 { i % m.face_count() as u32 } else { 0 };
                let e = if m.edge_count() > 0 { i % m.edge_count() as u32 } else { 0 };
                let r = match op {
                    0 => extrude_faces(&m, &[f], x),
                    1 => inset_faces(&m, &[f], x.abs()),
                    2 => bevel_edges(&m, &[e], x.abs() * 0.3),
                    3 => loop_cut(&m, e, 1 + i % 3),
                    4 => merge_vertices(&m, &[f, f + 1]),
                    5 => delete_faces(&m, &[f]),
                    6 => crate::subdivide::catmull_clark(&m, 1),
                    7 => bevel_edges(&m, &[e, e + 1, e + 2], x.abs() * 0.2),
                    _ => flip_faces(&m, &[f]),
                };
                m = r.unwrap();
                prop_assert!(m.validate().is_ok());
                if m.face_count() > 20_000 { break; }
            }
        }
    }
}
