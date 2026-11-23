//! Cutting tools: slice by a plane (a whole loop at once, or only a stretch of it, which is what
//! a knife line is), connect vertices across faces, and the polygon pen.
//!
//! Every tool rebuilds the mesh through [`PolyMesh::build`]. New points on edges are shared by the
//! two faces of the edge, and a face that only touches a cut edge takes the new point as an extra
//! corner, so the result has no T-junctions.

use std::collections::{BTreeSet, HashMap, HashSet};

use glam::{Vec2, Vec3};
use modelcraft_mesh::{MAX_POLYGON_CORNERS, NONE};
use serde::{Deserialize, Serialize};

use crate::edit::{polys_of, push};
use crate::{OpError, OpResult, PolyMesh, Polygon, clean_selection, rebuild};

/// Most points one polygon pen face may have.
pub const MAX_PEN_POINTS: usize = 256;

/// One corner while a face is being cut: a vertex and the UV of the face corner.
#[derive(Clone, Copy)]
struct Corner {
    v: u32,
    uv: Vec2,
}

/// What a slice does.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    /// A point on the plane.
    pub point: Vec3,
    /// The plane's normal (any length).
    pub normal: Vec3,
    /// Further half-spaces `(point, normal)`: a cut only reaches where `(p - point) · normal >= 0`
    /// holds for each. A knife line uses two, one at each end.
    pub limits: Vec<(Vec3, Vec3)>,
    /// The faces that may be cut (all when empty).
    pub faces: Vec<u32>,
}

impl Slice {
    pub fn plane(point: Vec3, normal: Vec3) -> Slice {
        Slice { point, normal, limits: Vec::new(), faces: Vec::new() }
    }
}

/// How a cut leaves a face boundary: through a new point on an edge or through an existing vertex.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Key {
    Edge(u32),
    Vertex(u32),
}

/// Cut the mesh along a plane (optionally only a stretch of it), keeping both sides.
pub fn slice(m: &PolyMesh, s: &Slice) -> OpResult {
    let n = s.normal.normalize_or_zero();
    if !s.point.is_finite() || !s.normal.is_finite() || n == Vec3::ZERO || m.face_count() == 0 {
        return Ok(m.clone());
    }
    let (lo, hi) = m.bounds();
    let eps = ((hi - lo).length() * 1e-6).max(1e-7);
    let dist: Vec<f32> = m.positions().iter().map(|p| (*p - s.point).dot(n)).collect();
    let sign = |v: u32| -> i8 {
        let d = dist.get(v as usize).copied().unwrap_or(0.0);
        if d > eps {
            1
        } else if d < -eps {
            -1
        } else {
            0
        }
    };
    let in_range = |p: Vec3| s.limits.iter().all(|(q, l)| !(q.is_finite() && l.is_finite()) || (p - *q).dot(*l) >= -eps);
    // The point where an edge meets the plane, the same bits for both of its faces.
    let cross_point = |e: u32| -> Vec3 {
        let (a, b) = m.edge_vertices(e);
        let (da, db) = (dist.get(a as usize).copied().unwrap_or(0.0), dist.get(b as usize).copied().unwrap_or(0.0));
        let t = if (da - db).abs() > 1e-30 { da / (da - db) } else { 0.5 };
        m.position(a).lerp(m.position(b), t.clamp(0.0, 1.0))
    };
    let allowed: Option<HashSet<u32>> = (!s.faces.is_empty()).then(|| clean_selection(&s.faces, m.face_count()).into_iter().collect());

    // Pass 1: for each face that may be cut, the pairs of boundary points the cut joins.
    let mut pairs: Vec<(u32, Key, Key)> = Vec::new();
    let mut needed: BTreeSet<u32> = BTreeSet::new();
    for f in 0..m.face_count() as u32 {
        if allowed.as_ref().is_some_and(|a| !a.contains(&f)) {
            continue;
        }
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let signs: Vec<i8> = hs.iter().map(|h| sign(m.origin(*h))).collect();
        if signs.iter().all(|x| *x >= 0) || signs.iter().all(|x| *x <= 0) {
            continue; // the face does not reach both sides
        }
        let fnorm = m.face_normal(f);
        let mut along = n.cross(fnorm);
        if along.length_squared() < 1e-12 {
            along = n.any_orthonormal_vector();
        }
        let mut pts: Vec<(f32, Key, bool)> = Vec::new();
        let k = hs.len();
        for (i, h) in hs.iter().enumerate() {
            let (sa, sb) = (signs.get(i).copied().unwrap_or(0), signs.get((i + 1) % k).copied().unwrap_or(0));
            if sa * sb < 0 {
                let p = cross_point(m.edge_of(*h));
                pts.push((p.dot(along), Key::Edge(m.edge_of(*h)), in_range(p)));
            } else if sa == 0 {
                // A vertex on the plane cuts only if the boundary passes from one side to the other
                // through it, and not along an edge in the plane.
                let (sp, sn) = (signs.get((i + k - 1) % k).copied().unwrap_or(0), sb);
                if sp * sn < 0 {
                    let p = m.position(m.origin(*h));
                    pts.push((p.dot(along), Key::Vertex(m.origin(*h)), in_range(p)));
                }
            }
        }
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        for [(_, a, ia), (_, b, ib)] in pts.as_chunks::<2>().0 {
            if *ia && *ib {
                pairs.push((f, *a, *b));
                for key in [a, b] {
                    if let Key::Edge(e) = key {
                        needed.insert(*e);
                    }
                }
            }
        }
    }
    if pairs.is_empty() {
        return Ok(m.clone());
    }
    if pairs.len().saturating_mul(2) > crate::MAX_RESULT_FACES {
        return Err(OpError::TooLarge(pairs.len()));
    }
    // New vertices on the cut edges.
    let mut pos = m.positions().to_vec();
    let mut new_on: HashMap<u32, u32> = HashMap::new();
    for e in &needed {
        new_on.insert(*e, push(&mut pos, cross_point(*e)));
    }
    let cut_pairs: HashMap<u32, Vec<(Key, Key)>> = pairs.iter().fold(HashMap::new(), |mut acc, (f, a, b)| {
        acc.entry(*f).or_default().push((*a, *b));
        acc
    });
    // Pass 2: every face gets the new points on its edges; cut faces are split.
    let uv_mode = m.has_uvs();
    let mut out: Vec<Polygon> = Vec::with_capacity(m.face_count() + pairs.len());
    for f in 0..m.face_count() as u32 {
        let mut corners: Vec<Corner> = Vec::new();
        for h in m.face_half_edges(f) {
            let (a, b) = (m.origin(h), m.dest(h));
            let (ua, ub) = (m.uv(h), m.uv(m.next(h)));
            corners.push(Corner { v: a, uv: ua });
            let e = m.edge_of(h);
            if let Some(&nv) = new_on.get(&e) {
                let (da, db) = (dist.get(a as usize).copied().unwrap_or(0.0), dist.get(b as usize).copied().unwrap_or(0.0));
                let t = if (da - db).abs() > 1e-30 { (da / (da - db)).clamp(0.0, 1.0) } else { 0.5 };
                corners.push(Corner { v: nv, uv: ua.lerp(ub, t) });
            }
        }
        let patch = m.patch(f);
        let resolve = |k: &Key| match k {
            Key::Edge(e) => new_on.get(e).copied(),
            Key::Vertex(v) => Some(*v),
        };
        let mut pieces: Vec<Vec<Corner>> = vec![corners];
        for (a, b) in cut_pairs.get(&f).into_iter().flatten() {
            let (Some(va), Some(vb)) = (resolve(a), resolve(b)) else { continue };
            // The piece that holds both ends.
            let Some(at) = pieces.iter().position(|p| p.iter().any(|c| c.v == va) && p.iter().any(|c| c.v == vb)) else { continue };
            let piece = pieces.swap_remove(at);
            let (Some(i), Some(j)) = (piece.iter().position(|c| c.v == va), piece.iter().position(|c| c.v == vb)) else { continue };
            let (i, j) = (i.min(j), i.max(j));
            let first: Vec<Corner> = piece.get(i..=j).map(<[Corner]>::to_vec).unwrap_or_default();
            let mut second: Vec<Corner> = piece.get(j..).map(<[Corner]>::to_vec).unwrap_or_default();
            second.extend(piece.get(..=i).unwrap_or(&[]));
            pieces.push(first);
            pieces.push(second);
        }
        for p in pieces {
            let mut poly = Polygon::new(p.iter().map(|c| c.v).collect());
            poly.uvs = uv_mode.then(|| p.iter().map(|c| c.uv).collect());
            poly.patch = patch;
            out.push(poly);
        }
    }
    rebuild(pos, out)
}

/// A knife line: a cut along the plane through `a` and `b` that contains the viewing direction
/// `view`, between the two points only. With `front_only`, faces that turn away from the viewer
/// are left alone.
pub fn cut_line(m: &PolyMesh, a: Vec3, b: Vec3, view: Vec3, front_only: bool) -> OpResult {
    if !(a.is_finite() && b.is_finite() && view.is_finite()) {
        return Ok(m.clone());
    }
    let line = b - a;
    let normal = line.cross(view);
    if line.length_squared() < 1e-18 || normal.length_squared() < 1e-18 {
        return Err(OpError::Invalid("the line is a point, or points straight along the view".into()));
    }
    let dir = line.normalize();
    let faces: Vec<u32> = if front_only { (0..m.face_count() as u32).filter(|f| m.face_normal(*f).dot(view) <= 1e-6).collect() } else { Vec::new() };
    if front_only && faces.is_empty() {
        return Ok(m.clone());
    }
    let out = slice(m, &Slice { point: a, normal, limits: vec![(a, dir), (b, -dir)], faces })?;
    if out == *m {
        return Err(OpError::Invalid("the line crosses no face".into()));
    }
    Ok(out)
}

/// Split every face that has two or more of the selected vertices as corners, joining them with
/// edges. With exactly two (not neighbours) the face splits in two; with three or more they are
/// joined in order around the face, which also leaves a polygon between them.
pub fn connect(m: &PolyMesh, verts: &[u32]) -> OpResult {
    let sel: HashSet<u32> = clean_selection(verts, m.vertex_count()).into_iter().collect();
    if sel.len() < 2 {
        return Err(OpError::Invalid("pick two or more vertices".into()));
    }
    let uv_mode = m.has_uvs();
    let (pos, mut polys) = polys_of(m);
    let mut extra: Vec<Polygon> = Vec::new();
    let mut changed = false;
    for f in 0..m.face_count() as u32 {
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let k = hs.len();
        let picked: Vec<usize> = (0..k).filter(|i| hs.get(*i).is_some_and(|h| sel.contains(&m.origin(*h)))).collect();
        if picked.len() < 2 {
            continue;
        }
        let corner = |i: usize| -> (u32, Vec2) {
            let h = hs.get(i % k).copied().unwrap_or(0);
            (m.origin(h), m.uv(h))
        };
        let make = |idx: Vec<usize>| -> Polygon {
            let cs: Vec<(u32, Vec2)> = idx.iter().map(|i| corner(*i)).collect();
            let mut p = Polygon::new(cs.iter().map(|c| c.0).collect());
            p.uvs = uv_mode.then(|| cs.iter().map(|c| c.1).collect());
            p.patch = m.patch(f);
            p
        };
        let mut pieces: Vec<Polygon> = Vec::new();
        if picked.len() == 2 {
            let (i, j) = (picked.first().copied().unwrap_or(0), picked.get(1).copied().unwrap_or(0));
            if j - i < 2 || (i + k) - j < 2 {
                continue; // neighbours: nothing to join
            }
            pieces.push(make((i..=j).collect()));
            pieces.push(make((j..=i + k).collect()));
        } else {
            for w in 0..picked.len() {
                let (i, j) = (picked.get(w).copied().unwrap_or(0), picked.get((w + 1) % picked.len()).copied().unwrap_or(0));
                let j = if j <= i { j + k } else { j };
                if j - i >= 2 {
                    pieces.push(make((i..=j).collect()));
                }
            }
            pieces.push(make(picked.clone()));
        }
        if pieces.len() < 2 {
            continue;
        }
        changed = true;
        let mut it = pieces.into_iter();
        if let (Some(first), Some(slot)) = (it.next(), polys.get_mut(f as usize)) {
            *slot = first;
        }
        extra.extend(it);
    }
    if !changed {
        return Err(OpError::Invalid("no face has two of the vertices as corners that are not neighbours".into()));
    }
    polys.extend(extra);
    rebuild(pos, polys)
}

/// One point of the polygon pen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "at", rename_all = "snake_case")]
pub enum PenPoint {
    /// An existing vertex.
    Vertex { v: u32 },
    /// A new point in space (in the mesh's own coordinates).
    Position { p: Vec3 },
    /// A new point on the existing edge between two vertices, `t` of the way from `a` to `b`.
    /// Both faces of the edge take it as a corner.
    Edge { a: u32, b: u32, t: f32 },
}

/// Add one polygon made of the points. `toward`, when given, is the side the polygon should face
/// if it does not share an edge with the mesh (a polygon that does takes the direction of its
/// neighbours).
pub fn add_polygon(m: &PolyMesh, points: &[PenPoint], toward: Option<Vec3>) -> OpResult {
    if points.len() < 3 {
        return Err(OpError::Invalid("a polygon needs at least three points".into()));
    }
    if points.len() > MAX_PEN_POINTS.min(MAX_POLYGON_CORNERS) {
        return Err(OpError::Invalid(format!("a polygon can have at most {MAX_PEN_POINTS} points")));
    }
    let n = m.vertex_count() as u32;
    let (mut pos, mut polys) = polys_of(m);
    // Points on edges first: insert into the faces that have the edge.
    let mut ids: Vec<u32> = Vec::with_capacity(points.len());
    let mut splits: HashMap<(u32, u32), u32> = HashMap::new();
    for p in points {
        match *p {
            PenPoint::Vertex { v } => {
                if v >= n {
                    return Err(OpError::Invalid(format!("vertex {v} no longer exists (the mesh has {n})")));
                }
                ids.push(v);
            }
            PenPoint::Position { p } => {
                if !p.is_finite() {
                    return Err(OpError::Invalid("a point is not a number".into()));
                }
                ids.push(push(&mut pos, p));
            }
            PenPoint::Edge { a, b, t } => {
                if a >= n || b >= n || m.find_edge(a, b).is_none() {
                    return Err(OpError::Invalid(format!("there is no edge between vertices {a} and {b}")));
                }
                let key = (a.min(b), a.max(b));
                let t = if t.is_finite() { t.clamp(0.01, 0.99) } else { 0.5 };
                let t = if a <= b { t } else { 1.0 - t };
                let id = *splits.entry(key).or_insert_with(|| push(&mut pos, m.position(key.0).lerp(m.position(key.1), t)));
                ids.push(id);
            }
        }
    }
    if !splits.is_empty() {
        let uv_mode = m.has_uvs();
        for poly in &mut polys {
            let k = poly.verts.len();
            let mut verts = Vec::with_capacity(k + 1);
            let mut uvs: Vec<Vec2> = Vec::new();
            for i in 0..k {
                let (Some(&a), Some(&b)) = (poly.verts.get(i), poly.verts.get((i + 1) % k)) else { continue };
                verts.push(a);
                let ua = poly.uvs.as_ref().and_then(|u| u.get(i).copied()).unwrap_or_default();
                let ub = poly.uvs.as_ref().and_then(|u| u.get((i + 1) % k).copied()).unwrap_or_default();
                if uv_mode {
                    uvs.push(ua);
                }
                if let Some(&id) = splits.get(&(a.min(b), a.max(b))) {
                    verts.push(id);
                    if uv_mode {
                        uvs.push(ua.lerp(ub, 0.5));
                    }
                }
            }
            poly.verts = verts;
            poly.uvs = uv_mode.then_some(uvs);
        }
    }
    // Repeated vertices make no polygon.
    let distinct: HashSet<u32> = ids.iter().copied().collect();
    if distinct.len() != ids.len() {
        return Err(OpError::Invalid("a point is used twice".into()));
    }
    // Face the wanted way unless a neighbour decides.
    let area_normal = {
        let mut sum = Vec3::ZERO;
        for i in 0..ids.len() {
            let a = pos.get(ids.get(i).copied().unwrap_or(0) as usize).copied().unwrap_or_default();
            let b = pos.get(ids.get((i + 1) % ids.len()).copied().unwrap_or(0) as usize).copied().unwrap_or_default();
            sum += a.cross(b);
        }
        sum
    };
    let shares_edge = (0..ids.len()).any(|i| {
        let (a, b) = (ids.get(i).copied().unwrap_or(NONE), ids.get((i + 1) % ids.len()).copied().unwrap_or(NONE));
        a < n && b < n && m.find_edge(a, b).is_some()
    });
    if !shares_edge && toward.is_some_and(|t| t.is_finite() && area_normal.dot(t) < 0.0) {
        ids.reverse();
    }
    let mut poly = Polygon::new(ids);
    if m.has_uvs() {
        poly.uvs = Some(vec![Vec2::ZERO; poly.verts.len()]);
    }
    polys.push(poly);
    rebuild(pos, polys)
}

/// The faces a slice with these settings would cut (for the preview).
pub fn faces_cut(m: &PolyMesh, s: &Slice) -> usize {
    match slice(m, s) {
        Ok(out) => out.face_count().saturating_sub(m.face_count()),
        Err(_) => 0,
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
    fn a_plane_through_a_cube_cuts_four_faces() {
        let c = cube();
        let m = slice(&c, &Slice::plane(Vec3::ZERO, Vec3::Y)).unwrap();
        m.validate().unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        assert_eq!(m.face_count(), 6 + 4, "four side faces split in two");
        assert_eq!(m.vertex_count(), 8 + 4);
        // Both halves stay.
        let (lo, hi) = m.bounds();
        assert!((lo.y + 1.0).abs() < 1e-5 && (hi.y - 1.0).abs() < 1e-5);
        // Every new point lies on the plane.
        assert_eq!(m.positions().iter().filter(|p| p.y.abs() < 1e-6).count(), 4);
    }

    #[test]
    fn a_plane_through_vertices_makes_no_double_points() {
        let c = cube();
        // The diagonal plane x = y passes through four of the cube's vertices.
        let m = slice(&c, &Slice::plane(Vec3::ZERO, Vec3::new(1.0, -1.0, 0.0))).unwrap();
        assert!(closed(&m));
        assert_eq!(euler(&m), 2);
        assert_eq!(m.vertex_count(), 8, "the plane runs through vertices only");
        assert_eq!(m.face_count(), 6 + 2, "two faces are split along their diagonal");
    }

    #[test]
    fn a_plane_that_misses_changes_nothing() {
        let c = cube();
        assert_eq!(slice(&c, &Slice::plane(Vec3::new(0.0, 5.0, 0.0), Vec3::Y)).unwrap(), c);
        assert_eq!(slice(&c, &Slice::plane(Vec3::ZERO, Vec3::ZERO)).unwrap(), c);
        assert_eq!(slice(&c, &Slice::plane(Vec3::NAN, Vec3::Y)).unwrap(), c);
        // A plane along a face touches the cube without cutting.
        assert_eq!(slice(&c, &Slice::plane(Vec3::new(0.0, 1.0, 0.0), Vec3::Y)).unwrap(), c);
    }

    #[test]
    fn the_grid_cuts_into_rows_and_keeps_uvs() {
        let mut g = Primitive::Plane { width: 4.0, depth: 4.0, segments: 4 }.build().unwrap();
        let uvs: Vec<Vec2> = (0..g.corner_count()).map(|i| Vec2::new(i as f32 * 0.01, 0.0)).collect();
        g.set_uvs(&uvs).unwrap();
        let m = slice(&g, &Slice::plane(Vec3::new(0.3, 0.0, 0.0), Vec3::X)).unwrap();
        m.validate().unwrap();
        assert!(m.has_uvs());
        assert_eq!(m.face_count(), 16 + 4);
        let open = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).count();
        assert_eq!(open, 16 + 2, "the cut adds one point to the top and one to the bottom border");
    }

    #[test]
    fn faces_option_limits_the_cut_and_neighbours_conform() {
        let c = cube();
        // Only one face may be cut: its neighbours across the cut edges take the new points as corners.
        let face = (0..6u32).find(|f| m_normal_is_x(&c, *f)).unwrap();
        let m = slice(&c, &Slice { faces: vec![face], ..Slice::plane(Vec3::ZERO, Vec3::Y) }).unwrap();
        m.validate().unwrap();
        assert!(closed(&m));
        assert_eq!(m.face_count(), 6 + 1);
    }

    fn m_normal_is_x(m: &PolyMesh, f: u32) -> bool {
        m.face_normal(f).x > 0.9
    }

    #[test]
    fn a_knife_line_cuts_only_between_its_ends() {
        let c = cube();
        let view = Vec3::new(0.0, 0.0, -1.0);
        // From left of the cube to its middle: only the left face has both its crossings in
        // reach. The front and back faces only gain a corner where the cut meets their edge.
        let m = cut_line(&c, Vec3::new(-2.0, 0.0, 2.0), Vec3::new(0.0, 0.0, 2.0), view, false).unwrap();
        m.validate().unwrap();
        assert!(closed(&m));
        assert_eq!(m.face_count(), 7);
        // Across the whole width the front, back and both sides are cut.
        let all = cut_line(&c, Vec3::new(-2.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 2.0), view, false).unwrap();
        assert!(closed(&all));
        assert_eq!(all.face_count(), 10);
        // A knife that stops short of the cube cuts nothing and says so.
        assert!(cut_line(&c, Vec3::new(5.0, 0.0, 2.0), Vec3::new(6.0, 0.0, 2.0), view, false).is_err());
        assert!(cut_line(&c, Vec3::ZERO, Vec3::ZERO, Vec3::Z, false).is_err());
        assert!(cut_line(&c, Vec3::ZERO, Vec3::X, Vec3::X, false).is_err(), "a line along the view has no plane");
    }

    #[test]
    fn visible_faces_only_leaves_the_back_alone() {
        let c = cube();
        let view = Vec3::new(0.0, 0.0, -1.0);
        let (a, b) = (Vec3::new(-2.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 2.0));
        let through = cut_line(&c, a, b, view, false).unwrap();
        let front = cut_line(&c, a, b, view, true).unwrap();
        assert!(closed(&through) && closed(&front));
        assert!(front.face_count() < through.face_count(), "{} {}", front.face_count(), through.face_count());
    }

    #[test]
    fn a_line_ending_inside_a_face_does_not_leave_a_stub() {
        // The knife ends in the middle of the front face: the face is not cut by it.
        let c = cube();
        let m = cut_line(&c, Vec3::new(-0.5, 0.0, 2.0), Vec3::new(0.5, 0.0, 2.0), Vec3::new(0.0, 0.0, -1.0), true);
        // Both ends are inside the face and the line crosses none of its edges: no cut.
        assert!(m.is_err());
    }

    #[test]
    fn connect_splits_a_face_between_two_corners() {
        let c = cube();
        let f = 0u32;
        let vs: Vec<u32> = c.face_vertices(f).collect();
        let m = connect(&c, &[vs[0], vs[2]]).unwrap();
        m.validate().unwrap();
        assert!(closed(&m));
        assert_eq!(m.face_count(), 7);
        // Neighbours have nothing between them.
        assert!(connect(&c, &[vs[0], vs[1]]).is_err());
        assert!(connect(&c, &[vs[0]]).is_err());
        // Three corners of a quad: a triangle between them and the remaining corner cut off.
        let m = connect(&c, &[vs[0], vs[1], vs[2]]).unwrap();
        assert!(closed(&m));
        assert_eq!(m.face_count(), 6 + 1);
    }

    #[test]
    fn connect_on_a_grid_joins_across_several_faces() {
        let g = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
        // A checkerboard of vertices: every quad has two opposite corners among them.
        let checker: Vec<u32> = (0..g.vertex_count() as u32).filter(|v| (g.position(*v).x.round() + g.position(*v).z.round()).rem_euclid(2.0) == 0.0).collect();
        let m = connect(&g, &checker).unwrap();
        m.validate().unwrap();
        assert_eq!(m.face_count(), g.face_count() * 2);
        // All four corners of a quad selected: they are already joined, so there is nothing to do.
        let all: Vec<u32> = (0..g.vertex_count() as u32).collect();
        assert!(connect(&g, &all).is_err());
    }

    #[test]
    fn the_pen_adds_a_face_to_an_open_mesh() {
        let c = crate::edit::delete_faces(&cube(), &[0]).unwrap();
        // Close the hole with a pen face through the four border vertices.
        let e = (0..c.edge_count() as u32).find(|e| c.is_boundary_edge(*e)).unwrap();
        let lp = crate::select::boundary_loop(&c, e).unwrap();
        let pts: Vec<PenPoint> = lp.iter().map(|v| PenPoint::Vertex { v: *v }).collect();
        let m = add_polygon(&c, &pts, None).unwrap();
        assert!(closed(&m), "the face took the direction of its neighbours");
        assert_eq!(euler(&m), 2);
        // And in the other order too.
        let mut rev = pts.clone();
        rev.reverse();
        let m2 = add_polygon(&c, &rev, None).unwrap();
        assert!(closed(&m2));
    }

    #[test]
    fn the_pen_makes_a_free_polygon_facing_the_view() {
        let m = PolyMesh::default();
        let pts = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0)].map(|p| PenPoint::Position { p });
        let up = add_polygon(&m, &pts, Some(Vec3::Z)).unwrap();
        assert!(up.face_normal(0).z > 0.9);
        let down = add_polygon(&m, &pts, Some(-Vec3::Z)).unwrap();
        assert!(down.face_normal(0).z < -0.9);
    }

    #[test]
    fn a_pen_point_on_an_edge_splits_it_for_both_faces() {
        let c = cube();
        let (a, b) = c.edge_vertices(0);
        let m = add_polygon(
            &c,
            &[PenPoint::Edge { a, b, t: 0.5 }, PenPoint::Position { p: Vec3::new(3.0, 3.0, 3.0) }, PenPoint::Position { p: Vec3::new(3.0, 3.0, 4.0) }],
            None,
        )
        .unwrap();
        m.validate().unwrap();
        // The cube's two faces at that edge gained a corner; the new triangle hangs from it.
        assert_eq!(m.face_count(), 7);
        assert_eq!(m.vertex_count(), 8 + 3);
    }

    #[test]
    fn the_pen_refuses_what_it_cannot_make() {
        let c = cube();
        assert!(add_polygon(&c, &[PenPoint::Vertex { v: 0 }, PenPoint::Vertex { v: 1 }], None).is_err());
        assert!(add_polygon(&c, &[PenPoint::Vertex { v: 0 }, PenPoint::Vertex { v: 1 }, PenPoint::Vertex { v: 99 }], None).is_err());
        assert!(add_polygon(&c, &[PenPoint::Vertex { v: 0 }, PenPoint::Vertex { v: 1 }, PenPoint::Vertex { v: 0 }], None).is_err());
        assert!(add_polygon(&c, &[PenPoint::Edge { a: 0, b: 6, t: 0.5 }, PenPoint::Vertex { v: 1 }, PenPoint::Vertex { v: 2 }], None).is_err());
        let nan = PenPoint::Position { p: Vec3::NAN };
        assert!(add_polygon(&c, &[nan, nan, nan], None).is_err());
        let many: Vec<PenPoint> = (0..MAX_PEN_POINTS + 1).map(|i| PenPoint::Position { p: Vec3::new(i as f32, 0.0, 0.0) }).collect();
        assert!(add_polygon(&c, &many, None).is_err());
    }
}
