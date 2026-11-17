//! ModelCraft's polygon mesh: a half-edge structure built from a polygon list and checked by
//! [`PolyMesh::validate`].
//!
//! The mesh is immutable once built. Modeling operations (`modelcraft-ops`) read it through the
//! adjacency queries here, produce a new polygon list and rebuild, so every result goes through
//! the same constructor and the same validation: an operation cannot leave a broken topology
//! behind. Building never panics: bad indices, degenerate polygons and non-manifold edges are
//! repaired (the polygon is dropped, flipped or detached) and listed in a [`BuildReport`].
//!
//! Conventions: Y up, right-handed, counter-clockwise front faces. A *corner* is a half-edge
//! that belongs to a face; its UV (if the mesh has UVs) is stored per corner. Each face carries a
//! *patch* id (a user-defined group of faces used for selection, masking and colouring).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::collections::HashMap;

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// "No element" in the index arrays.
pub const NONE: u32 = u32::MAX;

/// Upper bound on the corners of one polygon (hostile input guard).
pub const MAX_POLYGON_CORNERS: usize = 4096;

/// Upper bound on the elements of one mesh (vertices or faces).
pub const MAX_ELEMENTS: usize = 50_000_000;

/// One polygon of a polygon list: vertex indices in counter-clockwise order, optional per-corner
/// UVs (same length as `verts`) and a patch id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Polygon {
    pub verts: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uvs: Option<Vec<Vec2>>,
    #[serde(default)]
    pub patch: u32,
}

impl Polygon {
    pub fn new(verts: Vec<u32>) -> Self {
        Polygon { verts, uvs: None, patch: 0 }
    }
    pub fn with_uvs(verts: Vec<u32>, uvs: Vec<Vec2>) -> Self {
        Polygon { verts, uvs: Some(uvs), patch: 0 }
    }
}

/// What [`PolyMesh::build`] had to repair.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Polygons dropped: out-of-range index, fewer than three distinct corners, or a repeated vertex.
    pub dropped: usize,
    /// Polygons turned around to agree with their neighbours.
    pub flipped: usize,
    /// Polygons given their own copies of their vertices because an edge was shared by more than two faces.
    pub detached: usize,
    /// Non-finite positions replaced by the origin.
    pub fixed_positions: usize,
}

impl BuildReport {
    pub fn is_clean(&self) -> bool {
        *self == BuildReport::default()
    }
}

/// Why a mesh failed [`PolyMesh::validate`] or could not be built.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MeshError {
    #[error("mesh is too large ({0} elements)")]
    TooLarge(usize),
    #[error("broken topology: {0}")]
    Topology(String),
    #[error("non-finite position at vertex {0}")]
    NonFinite(u32),
}

/// A validated half-edge polygon mesh. See the crate documentation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolyMesh {
    positions: Vec<Vec3>,
    vert_he: Vec<u32>,
    he_origin: Vec<u32>,
    he_next: Vec<u32>,
    he_prev: Vec<u32>,
    he_twin: Vec<u32>,
    he_face: Vec<u32>,
    he_edge: Vec<u32>,
    he_uv: Vec<Vec2>,
    face_he: Vec<u32>,
    face_patch: Vec<u32>,
    edge_he: Vec<u32>,
}

fn idx(i: u32) -> usize {
    i as usize
}

fn get(v: &[u32], i: u32) -> u32 {
    v.get(idx(i)).copied().unwrap_or(NONE)
}

fn to_u32(n: usize) -> Result<u32, MeshError> {
    u32::try_from(n).ok().filter(|v| *v != NONE).ok_or(MeshError::TooLarge(n))
}

fn set(v: &mut [u32], i: usize, value: u32) {
    if let Some(s) = v.get_mut(i) {
        *s = value;
    }
}

impl PolyMesh {
    /// Build from positions and polygons, repairing what it must (see [`BuildReport`]).
    pub fn build(positions: Vec<Vec3>, polygons: Vec<Polygon>) -> Result<(PolyMesh, BuildReport), MeshError> {
        let mut report = BuildReport::default();
        if positions.len() > MAX_ELEMENTS || polygons.len() > MAX_ELEMENTS {
            return Err(MeshError::TooLarge(positions.len().max(polygons.len())));
        }
        let mut positions = positions;
        for p in &mut positions {
            if !p.is_finite() {
                *p = Vec3::ZERO;
                report.fixed_positions += 1;
            }
        }
        let has_uvs = !polygons.is_empty() && polygons.iter().all(|p| p.uvs.as_ref().is_some_and(|u| u.len() == p.verts.len()));
        let nverts = positions.len();

        // 1. Clean each polygon: indices in range, no consecutive duplicates, no repeats.
        let mut clean: Vec<Polygon> = Vec::with_capacity(polygons.len());
        for mut poly in polygons {
            if poly.verts.len() > MAX_POLYGON_CORNERS || poly.verts.iter().any(|v| idx(*v) >= nverts) {
                report.dropped += 1;
                continue;
            }
            let mut uvs = if has_uvs { poly.uvs.take().unwrap_or_default() } else { Vec::new() };
            let mut i = 0;
            while poly.verts.len() >= 2 && i < poly.verts.len() {
                let j = (i + 1) % poly.verts.len();
                if poly.verts.get(i) == poly.verts.get(j) {
                    poly.verts.remove(j);
                    if j < uvs.len() {
                        uvs.remove(j);
                    }
                } else {
                    i += 1;
                }
            }
            let mut sorted = poly.verts.clone();
            sorted.sort_unstable();
            if poly.verts.len() < 3 || sorted.windows(2).any(|w| w.first() == w.get(1)) {
                report.dropped += 1;
                continue;
            }
            poly.uvs = has_uvs.then_some(uvs);
            clean.push(poly);
        }

        // 2. Orientation and manifoldness: every directed edge used at most once, every
        //    undirected edge by at most two faces.
        let total_corners: usize = clean.iter().map(|p| p.verts.len()).sum();
        let mut used: HashMap<(u32, u32), ()> = HashMap::with_capacity(total_corners);
        let edges_of = |v: &[u32]| -> Vec<(u32, u32)> { (0..v.len()).filter_map(|i| Some((*v.get(i)?, *v.get((i + 1) % v.len())?))).collect() };
        let fits = |used: &HashMap<(u32, u32), ()>, es: &[(u32, u32)]| es.iter().all(|e| !used.contains_key(e));
        for poly in &mut clean {
            let fwd = edges_of(&poly.verts);
            if fits(&used, &fwd) {
                for e in fwd {
                    used.insert(e, ());
                }
                continue;
            }
            let rev: Vec<(u32, u32)> = fwd.iter().map(|(a, b)| (*b, *a)).collect();
            if fits(&used, &rev) {
                poly.verts.reverse();
                if let Some(u) = poly.uvs.as_mut() {
                    u.reverse();
                }
                report.flipped += 1;
                for e in edges_of(&poly.verts) {
                    used.insert(e, ());
                }
                continue;
            }
            // Detach: the polygon gets fresh vertices.
            for v in &mut poly.verts {
                let p = positions.get(idx(*v)).copied().unwrap_or(Vec3::ZERO);
                *v = to_u32(positions.len())?;
                positions.push(p);
            }
            report.detached += 1;
            for e in edges_of(&poly.verts) {
                used.insert(e, ());
            }
        }
        drop(used);
        if positions.len() > MAX_ELEMENTS {
            return Err(MeshError::TooLarge(positions.len()));
        }

        // 3. Interior half-edges.
        let mut m = PolyMesh {
            vert_he: vec![NONE; positions.len()],
            positions,
            he_origin: Vec::with_capacity(total_corners * 2),
            he_next: Vec::with_capacity(total_corners * 2),
            he_prev: Vec::with_capacity(total_corners * 2),
            he_twin: Vec::with_capacity(total_corners * 2),
            he_face: Vec::with_capacity(total_corners * 2),
            he_edge: Vec::new(),
            he_uv: Vec::new(),
            face_he: Vec::with_capacity(clean.len()),
            face_patch: Vec::with_capacity(clean.len()),
            edge_he: Vec::new(),
        };
        let mut directed: HashMap<(u32, u32), u32> = HashMap::with_capacity(total_corners);
        for (f, poly) in clean.iter().enumerate() {
            let f = to_u32(f)?;
            let first = to_u32(m.he_origin.len())?;
            let n = poly.verts.len() as u32;
            for (i, v) in poly.verts.iter().enumerate() {
                let i = i as u32;
                let h = first + i;
                m.he_origin.push(*v);
                m.he_next.push(first + (i + 1) % n);
                m.he_prev.push(first + (i + n - 1) % n);
                m.he_twin.push(NONE);
                m.he_face.push(f);
                let w = poly.verts.get(idx((i + 1) % n)).copied().unwrap_or(*v);
                directed.insert((*v, w), h);
            }
            if has_uvs {
                m.he_uv.extend(poly.uvs.iter().flatten().copied());
            }
            m.face_he.push(first);
            m.face_patch.push(poly.patch);
        }
        let interior = m.he_origin.len();
        for h in 0..interior {
            let a = m.he_origin.get(h).copied().unwrap_or(NONE);
            let b = get(&m.he_origin, get(&m.he_next, h as u32));
            if let Some(t) = directed.get(&(b, a)) {
                set(&mut m.he_twin, h, *t);
            }
        }
        drop(directed);

        // 4. Boundary half-edges, one per interior half-edge without a twin.
        for h in 0..interior {
            if m.he_twin.get(h).copied() != Some(NONE) {
                continue;
            }
            let b = to_u32(m.he_origin.len())?;
            let dest = get(&m.he_origin, get(&m.he_next, h as u32));
            m.he_origin.push(dest);
            m.he_next.push(NONE);
            m.he_prev.push(NONE);
            m.he_twin.push(h as u32);
            m.he_face.push(NONE);
            if has_uvs {
                m.he_uv.push(Vec2::ZERO);
            }
            set(&mut m.he_twin, h, b);
        }
        // Link the boundary loops: boundary b runs d → o; next(b) is the boundary half-edge
        // leaving o, found by turning around o starting from twin(b).
        for b in interior..m.he_origin.len() {
            let b = b as u32;
            let mut c = get(&m.he_twin, b);
            let mut found = NONE;
            for _ in 0..interior.saturating_add(1) {
                let t = get(&m.he_twin, get(&m.he_prev, c));
                if t == NONE {
                    break;
                }
                if get(&m.he_face, t) == NONE {
                    found = t;
                    break;
                }
                c = t;
            }
            if found == NONE {
                return Err(MeshError::Topology(format!("boundary half-edge {b} has no successor")));
            }
            set(&mut m.he_next, idx(b), found);
            set(&mut m.he_prev, idx(found), b);
        }

        // 5. Vertex → outgoing half-edge (a boundary one when there is one), edges.
        for h in 0..m.he_origin.len() {
            let v = m.he_origin.get(h).copied().unwrap_or(NONE);
            let boundary = m.he_face.get(h).copied() == Some(NONE);
            if let Some(slot) = m.vert_he.get_mut(idx(v))
                && (*slot == NONE || boundary)
            {
                *slot = h as u32;
            }
        }
        m.he_edge = vec![NONE; m.he_origin.len()];
        for h in 0..m.he_origin.len() {
            let t = get(&m.he_twin, h as u32);
            // The canonical half-edge of an edge is the lower index, which is always an
            // interior one (boundary half-edges are numbered after all interior ones).
            if (h as u32) < t {
                let e = to_u32(m.edge_he.len())?;
                m.edge_he.push(h as u32);
                set(&mut m.he_edge, h, e);
                set(&mut m.he_edge, idx(t), e);
            }
        }
        m.validate()?;
        Ok((m, report))
    }

    /// Build from plain index lists (no UVs, patch 0).
    pub fn from_faces(positions: Vec<Vec3>, faces: &[Vec<u32>]) -> Result<PolyMesh, MeshError> {
        Ok(Self::build(positions, faces.iter().map(|f| Polygon::new(f.clone())).collect())?.0)
    }

    /// Check every invariant of the structure. Cost is linear in the mesh size.
    pub fn validate(&self) -> Result<(), MeshError> {
        let nh = self.he_origin.len();
        let bad = |m: String| Err(MeshError::Topology(m));
        if [self.he_next.len(), self.he_prev.len(), self.he_twin.len(), self.he_face.len(), self.he_edge.len()].iter().any(|l| *l != nh) {
            return bad("half-edge arrays differ in length".into());
        }
        if !self.he_uv.is_empty() && self.he_uv.len() != nh {
            return bad("UV array does not match the half-edges".into());
        }
        if self.face_he.len() != self.face_patch.len() || self.vert_he.len() != self.positions.len() {
            return bad("face or vertex arrays differ in length".into());
        }
        if let Some(v) = self.positions.iter().position(|p| !p.is_finite()) {
            return Err(MeshError::NonFinite(v as u32));
        }
        for h in 0..nh as u32 {
            let (n, p, t) = (get(&self.he_next, h), get(&self.he_prev, h), get(&self.he_twin, h));
            if idx(n) >= nh || idx(p) >= nh || idx(t) >= nh {
                return bad(format!("half-edge {h} links out of range"));
            }
            if get(&self.he_prev, n) != h || get(&self.he_next, p) != h {
                return bad(format!("half-edge {h}: next/prev disagree"));
            }
            if get(&self.he_twin, t) != h || t == h {
                return bad(format!("half-edge {h}: twin is not mutual"));
            }
            if get(&self.he_origin, t) != get(&self.he_origin, n) {
                return bad(format!("half-edge {h}: twin does not start where it ends"));
            }
            if get(&self.he_face, n) != get(&self.he_face, h) {
                return bad(format!("half-edge {h}: next is on another face"));
            }
            let f = get(&self.he_face, h);
            if f != NONE && idx(f) >= self.face_he.len() {
                return bad(format!("half-edge {h}: face out of range"));
            }
            if f == NONE && get(&self.he_face, t) == NONE {
                return bad(format!("edge of half-edge {h} has no face"));
            }
            let o = get(&self.he_origin, h);
            if idx(o) >= self.positions.len() || get(&self.vert_he, o) == NONE {
                return bad(format!("half-edge {h}: bad origin"));
            }
            let e = get(&self.he_edge, h);
            if idx(e) >= self.edge_he.len() || get(&self.he_edge, t) != e {
                return bad(format!("half-edge {h}: bad edge"));
            }
        }
        for (f, h) in self.face_he.iter().enumerate() {
            if get(&self.he_face, *h) != f as u32 {
                return bad(format!("face {f}: first half-edge is not on it"));
            }
            let mut c = *h;
            let mut n = 0usize;
            loop {
                n += 1;
                c = get(&self.he_next, c);
                if c == *h {
                    break;
                }
                if n > MAX_POLYGON_CORNERS + 1 {
                    return bad(format!("face {f}: loop does not close"));
                }
            }
            if n < 3 {
                return bad(format!("face {f} has {n} corners"));
            }
        }
        for (v, h) in self.vert_he.iter().enumerate() {
            if *h != NONE && get(&self.he_origin, *h) != v as u32 {
                return bad(format!("vertex {v}: outgoing half-edge starts elsewhere"));
            }
        }
        Ok(())
    }

    // ---- sizes and raw access ----

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
    pub fn face_count(&self) -> usize {
        self.face_he.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edge_he.len()
    }
    pub fn half_edge_count(&self) -> usize {
        self.he_origin.len()
    }
    pub fn positions(&self) -> &[Vec3] {
        &self.positions
    }
    pub fn has_uvs(&self) -> bool {
        !self.he_uv.is_empty()
    }
    /// Number of face corners: the interior half-edges, which come first, so a corner id is also
    /// its half-edge id. Boundary half-edges (no face) are not corners.
    pub fn corner_count(&self) -> usize {
        self.he_face.partition_point(|f| *f != NONE)
    }
    /// The UV of every corner, in corner order (empty when the mesh has no UVs).
    pub fn corner_uvs(&self) -> &[Vec2] {
        self.he_uv.get(..self.corner_count()).unwrap_or(&[])
    }
    /// Replace the UVs: one per corner, in corner order (see [`PolyMesh::corner_count`]).
    /// Non-finite values are rejected and the mesh is left unchanged.
    pub fn set_uvs(&mut self, corner_uvs: &[Vec2]) -> Result<(), MeshError> {
        let n = self.corner_count();
        if corner_uvs.len() != n {
            return Err(MeshError::Topology(format!("{} UVs for {n} corners", corner_uvs.len())));
        }
        if let Some(c) = corner_uvs.iter().position(|u| !u.is_finite()) {
            return Err(MeshError::NonFinite(c as u32));
        }
        let mut uv = Vec::with_capacity(self.he_origin.len());
        uv.extend_from_slice(corner_uvs);
        uv.resize(self.he_origin.len(), Vec2::ZERO);
        self.he_uv = uv;
        Ok(())
    }
    /// Remove the UVs.
    pub fn clear_uvs(&mut self) {
        self.he_uv = Vec::new();
    }
    pub fn position(&self, v: u32) -> Vec3 {
        self.positions.get(idx(v)).copied().unwrap_or(Vec3::ZERO)
    }

    /// Replace the vertex positions (same count, finite values); topology is unchanged.
    pub fn set_positions(&mut self, positions: &[Vec3]) -> Result<(), MeshError> {
        if positions.len() != self.positions.len() {
            return Err(MeshError::Topology(format!("{} positions for {} vertices", positions.len(), self.positions.len())));
        }
        if let Some(v) = positions.iter().position(|p| !p.is_finite()) {
            return Err(MeshError::NonFinite(v as u32));
        }
        self.positions.copy_from_slice(positions);
        Ok(())
    }

    /// Move each vertex to `f(index, position)`; a non-finite result keeps the old position.
    pub fn map_positions(&mut self, mut f: impl FnMut(u32, Vec3) -> Vec3) {
        for (i, p) in self.positions.iter_mut().enumerate() {
            let q = f(i as u32, *p);
            if q.is_finite() {
                *p = q;
            }
        }
    }

    // ---- half-edge navigation (out-of-range input gives NONE, never a panic) ----

    pub fn next(&self, h: u32) -> u32 {
        get(&self.he_next, h)
    }
    pub fn prev(&self, h: u32) -> u32 {
        get(&self.he_prev, h)
    }
    pub fn twin(&self, h: u32) -> u32 {
        get(&self.he_twin, h)
    }
    pub fn origin(&self, h: u32) -> u32 {
        get(&self.he_origin, h)
    }
    pub fn dest(&self, h: u32) -> u32 {
        self.origin(self.next(h))
    }
    /// The face of a half-edge, [`NONE`] on the boundary.
    pub fn face_of(&self, h: u32) -> u32 {
        get(&self.he_face, h)
    }
    pub fn edge_of(&self, h: u32) -> u32 {
        get(&self.he_edge, h)
    }
    pub fn face_half_edge(&self, f: u32) -> u32 {
        get(&self.face_he, f)
    }
    pub fn vertex_half_edge(&self, v: u32) -> u32 {
        get(&self.vert_he, v)
    }
    /// The canonical half-edge of an edge; it always lies on a face.
    pub fn edge_half_edge(&self, e: u32) -> u32 {
        get(&self.edge_he, e)
    }
    pub fn uv(&self, h: u32) -> Vec2 {
        self.he_uv.get(idx(h)).copied().unwrap_or(Vec2::ZERO)
    }
    pub fn patch(&self, f: u32) -> u32 {
        self.face_patch.get(idx(f)).copied().unwrap_or(0)
    }
    pub fn is_boundary_edge(&self, e: u32) -> bool {
        let h = self.edge_half_edge(e);
        self.face_of(h) == NONE || self.face_of(self.twin(h)) == NONE
    }
    pub fn edge_vertices(&self, e: u32) -> (u32, u32) {
        let h = self.edge_half_edge(e);
        (self.origin(h), self.dest(h))
    }
    /// The edge between two vertices, if any.
    pub fn find_edge(&self, a: u32, b: u32) -> Option<u32> {
        self.vertex_out(a).find(|h| self.dest(*h) == b).map(|h| self.edge_of(h))
    }

    /// The half-edges of a face, in order.
    pub fn face_half_edges(&self, f: u32) -> impl Iterator<Item = u32> + '_ {
        let start = self.face_half_edge(f);
        let mut c = start;
        let mut done = start == NONE;
        let mut budget = MAX_POLYGON_CORNERS + 1;
        std::iter::from_fn(move || {
            if done || budget == 0 {
                return None;
            }
            budget -= 1;
            let out = c;
            c = self.next(c);
            done = c == start || c == NONE;
            Some(out)
        })
    }
    /// The vertices of a face, in order.
    pub fn face_vertices(&self, f: u32) -> impl Iterator<Item = u32> + '_ {
        self.face_half_edges(f).map(|h| self.origin(h))
    }
    pub fn face_len(&self, f: u32) -> usize {
        self.face_half_edges(f).count()
    }

    /// Outgoing half-edges of a vertex (bounded by the half-edge count).
    pub fn vertex_out(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        let start = self.vertex_half_edge(v);
        let mut c = start;
        let mut done = start == NONE;
        let mut budget = self.he_origin.len();
        std::iter::from_fn(move || {
            if done || budget == 0 {
                return None;
            }
            budget -= 1;
            let out = c;
            c = self.twin(self.prev(c));
            done = c == start || c == NONE;
            Some(out)
        })
    }
    /// Neighbouring vertices of a vertex.
    pub fn vertex_neighbors(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        self.vertex_out(v).map(|h| self.dest(h))
    }
    /// Faces around a vertex.
    pub fn vertex_faces(&self, v: u32) -> impl Iterator<Item = u32> + '_ {
        self.vertex_out(v).map(|h| self.face_of(h)).filter(|f| *f != NONE)
    }
    pub fn is_boundary_vertex(&self, v: u32) -> bool {
        self.vertex_out(v).any(|h| self.face_of(h) == NONE)
    }

    // ---- geometry ----

    /// Face normal by Newell's method (robust for non-planar polygons); zero for degenerate faces.
    pub fn face_normal(&self, f: u32) -> Vec3 {
        let mut n = Vec3::ZERO;
        for h in self.face_half_edges(f) {
            let a = self.position(self.origin(h));
            let b = self.position(self.dest(h));
            n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        n.normalize_or_zero()
    }
    pub fn face_center(&self, f: u32) -> Vec3 {
        let (mut s, mut n) = (Vec3::ZERO, 0.0f32);
        for v in self.face_vertices(f) {
            s += self.position(v);
            n += 1.0;
        }
        if n > 0.0 { s / n } else { Vec3::ZERO }
    }
    /// Area-weighted vertex normals (straight up for isolated vertices).
    pub fn vertex_normals(&self) -> Vec<Vec3> {
        let mut out = vec![Vec3::ZERO; self.positions.len()];
        let mut verts = Vec::with_capacity(8);
        for f in 0..self.face_count() as u32 {
            verts.clear();
            verts.extend(self.face_vertices(f));
            let Some(&a) = verts.first() else { continue };
            let pa = self.position(a);
            let mut fnorm = Vec3::ZERO;
            for w in verts.get(1..).unwrap_or_default().windows(2) {
                if let [b, c] = w {
                    fnorm += (self.position(*b) - pa).cross(self.position(*c) - pa);
                }
            }
            for v in &verts {
                if let Some(o) = out.get_mut(idx(*v)) {
                    *o += fnorm;
                }
            }
        }
        for n in &mut out {
            *n = n.normalize_or(Vec3::Y);
        }
        out
    }
    /// Axis-aligned bounds (min, max); a zero box for an empty mesh.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        if self.positions.is_empty() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        self.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    // ---- conversion ----

    /// The faces as a polygon list (inverse of [`PolyMesh::build`]).
    pub fn to_polygons(&self) -> Vec<Polygon> {
        (0..self.face_count() as u32)
            .map(|f| Polygon {
                verts: self.face_vertices(f).collect(),
                uvs: self.has_uvs().then(|| self.face_half_edges(f).map(|h| self.uv(h)).collect()),
                patch: self.patch(f),
            })
            .collect()
    }

    /// Fan triangulation for drawing: `(triangles, face of each triangle)`.
    pub fn triangulate(&self) -> (Vec<[u32; 3]>, Vec<u32>) {
        let mut tris = Vec::with_capacity(self.face_count() * 2);
        let mut owner = Vec::with_capacity(self.face_count() * 2);
        for f in 0..self.face_count() as u32 {
            let mut it = self.face_vertices(f);
            let (Some(a), Some(mut b)) = (it.next(), it.next()) else { continue };
            for c in it {
                tris.push([a, b, c]);
                owner.push(f);
                b = c;
            }
        }
        (tris, owner)
    }

    pub fn triangle_count(&self) -> usize {
        (0..self.face_count() as u32).map(|f| self.face_len(f).saturating_sub(2)).sum()
    }

    /// Estimated heap size in bytes (for memory budgets).
    pub fn heap_bytes(&self) -> usize {
        self.positions.len() * 16 + self.he_origin.len() * 24 + self.he_uv.len() * 8 + self.face_he.len() * 8 + self.edge_he.len() * 4
    }
}

/// Serialized form: the polygon list, rebuilt (and validated) on load.
#[derive(Serialize, Deserialize)]
struct Soup {
    positions: Vec<Vec3>,
    polygons: Vec<Polygon>,
}

impl Serialize for PolyMesh {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Soup { positions: self.positions.clone(), polygons: self.to_polygons() }.serialize(s)
    }
}

impl<'de> Deserialize<'de> for PolyMesh {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let soup = Soup::deserialize(d)?;
        PolyMesh::build(soup.positions, soup.polygons).map(|(m, _)| m).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;
