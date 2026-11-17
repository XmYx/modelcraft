//! The sculpt engine. A [`SculptMesh`] holds flat arrays (positions, normals, mask) built from a
//! [`PolyMesh`], a triangle BVH for picking and radius queries, and per-vertex neighbour lists.
//! Brushes ([`brush`]) move vertices dab by dab; every change marks fixed-size vertex *chunks*
//! dirty so a renderer can upload only those ranges ([`SculptMesh::take_dirty_chunks`]).
//!
//! A stroke records what it changed ([`StrokeUndo`]) for undo, and the stroke itself (brush and
//! dabs, [`Stroke`]) is plain data, so replaying it on the same mesh gives the same result:
//! that is what the document journal stores.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod brush;
mod bvh;
pub mod detail;
pub mod live;

use glam::Vec3;
use modelcraft_mesh::PolyMesh;

pub use brush::{BrushKind, BrushSettings, Dab, Falloff, Stamp, Stroke, Symmetry};
pub use bvh::Bvh;

/// Vertices per dirty chunk (a GPU upload unit).
pub const CHUNK: usize = 4096;

/// A sculptable mesh. See the crate documentation.
pub struct SculptMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub mask: Vec<f32>,
    tris: Vec<[u32; 3]>,
    tri_patch: Vec<u32>,
    adj: Adj,
    bvh: Bvh,
    /// Live-detail strokes applied minus undone since the session began.
    topo_edits: i64,
    /// Counts every change of the triangles (strokes, undo, redo).
    topo_rev: u64,
    /// Largest distance any vertex moved since the BVH was last refitted.
    drift: f32,
    dirty_chunks: Vec<bool>,
    stamp: Vec<u32>,
    stamp_gen: u32,
    stroke: Option<StrokeState>,
}

/// Who is next to whom: packed lists while the triangles are fixed, growable ones once live
/// detail starts changing them.
pub(crate) enum Adj {
    Csr { nbr_start: Vec<u32>, nbr: Vec<u32>, vtri_start: Vec<u32>, vtri: Vec<u32> },
    Lists { nbr: Vec<Vec<u32>>, vtri: Vec<Vec<u32>> },
}

fn span<'a>(start: &[u32], data: &'a [u32], v: u32) -> &'a [u32] {
    let a = start.get(v as usize).copied().unwrap_or(0) as usize;
    let b = start.get(v as usize + 1).copied().unwrap_or(0) as usize;
    data.get(a..b).unwrap_or_default()
}

impl Adj {
    fn neighbors(&self, v: u32) -> &[u32] {
        match self {
            Adj::Csr { nbr_start, nbr, .. } => span(nbr_start, nbr, v),
            Adj::Lists { nbr, .. } => nbr.get(v as usize).map(Vec::as_slice).unwrap_or_default(),
        }
    }
    fn vertex_tris(&self, v: u32) -> &[u32] {
        match self {
            Adj::Csr { vtri_start, vtri, .. } => span(vtri_start, vtri, v),
            Adj::Lists { vtri, .. } => vtri.get(v as usize).map(Vec::as_slice).unwrap_or_default(),
        }
    }
    fn heap_bytes(&self) -> usize {
        match self {
            Adj::Csr { nbr_start, nbr, vtri_start, vtri } => (nbr_start.len() + nbr.len() + vtri_start.len() + vtri.len()) * 4,
            Adj::Lists { nbr, vtri } => nbr.iter().chain(vtri).map(|l| 24 + l.capacity() * 4).sum(),
        }
    }
}

/// What a finished stroke changed, for undo/redo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StrokeUndo {
    pub indices: Vec<u32>,
    pub before: Vec<Vec3>,
    pub after: Vec<Vec3>,
    pub mask_before: Vec<f32>,
    pub mask_after: Vec<f32>,
    /// Triangles and vertices added by live detail (applied before the values above on redo,
    /// after them on undo).
    pub topo: Option<Box<live::TopoChange>>,
}

impl StrokeUndo {
    pub fn heap_bytes(&self) -> usize {
        self.indices.len() * (4 + 24 + 8) + self.topo.as_ref().map(|t| t.heap_bytes()).unwrap_or(0)
    }
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty() && self.topo.is_none()
    }
}

struct StrokeState {
    brush: BrushSettings,
    /// First value of every touched vertex (index → (position, mask)).
    original: std::collections::HashMap<u32, (Vec3, f32)>,
    /// Vertices captured by a grab brush at the first dab, with their weights, per symmetry pass.
    grabbed: Vec<Vec<(u32, f32)>>,
    dabs: Vec<Dab>,
    /// Set once live detail has split something in this stroke.
    topo: Option<live::TopoStart>,
}

fn csr(n: usize, lists: impl Iterator<Item = (u32, u32)>) -> (Vec<u32>, Vec<u32>) {
    let pairs: Vec<(u32, u32)> = lists.collect();
    let mut count = vec![0u32; n + 1];
    for (a, _) in &pairs {
        if let Some(c) = count.get_mut(*a as usize + 1) {
            *c += 1;
        }
    }
    for i in 0..n {
        let prev = count.get(i).copied().unwrap_or(0);
        if let Some(c) = count.get_mut(i + 1) {
            *c += prev;
        }
    }
    let mut fill = count.clone();
    let mut out = vec![0u32; pairs.len()];
    for (a, b) in pairs {
        if let Some(f) = fill.get_mut(a as usize) {
            if let Some(slot) = out.get_mut(*f as usize) {
                *slot = b;
            }
            *f += 1;
        }
    }
    (count, out)
}

impl SculptMesh {
    /// Build from a mesh; `positions`/`mask` override the mesh's own (a sculpt layer) when
    /// their length matches.
    pub fn new(mesh: &PolyMesh, positions: Option<&[Vec3]>, mask: Option<&[f32]>) -> SculptMesh {
        let n = mesh.vertex_count();
        let positions = positions.filter(|p| p.len() == n).map(<[Vec3]>::to_vec).unwrap_or_else(|| mesh.positions().to_vec());
        let mask = mask.filter(|m| m.len() == n).map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; n]);
        let (tris, owner) = mesh.triangulate();
        let tri_patch = owner.iter().map(|f| mesh.patch(*f)).collect();
        let (nbr_start, nbr) = csr(n, (0..n as u32).flat_map(|v| mesh.vertex_neighbors(v).map(move |w| (v, w))));
        let (vtri_start, vtri) = csr(n, tris.iter().enumerate().flat_map(|(t, tri)| tri.iter().map(move |v| (*v, t as u32))));
        let bvh = Bvh::build(&positions, &tris);
        let mut s = SculptMesh {
            normals: vec![Vec3::Y; n],
            positions,
            mask,
            tris,
            tri_patch,
            adj: Adj::Csr { nbr_start, nbr, vtri_start, vtri },
            bvh,
            topo_edits: 0,
            topo_rev: 0,
            drift: 0.0,
            dirty_chunks: vec![true; n.div_ceil(CHUNK)],
            stamp: vec![0; n],
            stamp_gen: 0,
            stroke: None,
        };
        s.recompute_all_normals();
        s
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.tris
    }
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }
    pub fn neighbors(&self, v: u32) -> &[u32] {
        self.adj.neighbors(v)
    }
    fn vertex_tris(&self, v: u32) -> &[u32] {
        self.adj.vertex_tris(v)
    }

    pub fn heap_bytes(&self) -> usize {
        self.positions.len() * (12 + 12 + 4 + 4) + self.tris.len() * (12 + 4) + self.adj.heap_bytes() + self.bvh.heap_bytes()
    }

    fn tri_normal(&self, t: u32) -> Vec3 {
        let Some([a, b, c]) = self.tris.get(t as usize) else { return Vec3::ZERO };
        let p = |i: &u32| self.positions.get(*i as usize).copied().unwrap_or_default();
        (p(b) - p(a)).cross(p(c) - p(a))
    }

    pub fn recompute_all_normals(&mut self) {
        let mut n = vec![Vec3::ZERO; self.positions.len()];
        for t in 0..self.tris.len() as u32 {
            let fnorm = self.tri_normal(t);
            if let Some(tri) = self.tris.get(t as usize) {
                for v in tri {
                    if let Some(x) = n.get_mut(*v as usize) {
                        *x += fnorm;
                    }
                }
            }
        }
        for x in &mut n {
            *x = x.normalize_or(Vec3::Y);
        }
        self.normals = n;
    }

    /// Recompute the normals of `verts` and of every vertex sharing a triangle with them.
    fn update_normals(&mut self, verts: &[u32]) {
        self.stamp_gen = self.stamp_gen.wrapping_add(1);
        if self.stamp_gen == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.stamp_gen = 1;
        }
        let gen_ = self.stamp_gen;
        let mut ring = Vec::with_capacity(verts.len() * 2);
        for v in verts {
            for t in self.adj.vertex_tris(*v) {
                if let Some(tri) = self.tris.get(*t as usize) {
                    for w in tri {
                        if let Some(s) = self.stamp.get_mut(*w as usize)
                            && *s != gen_
                        {
                            *s = gen_;
                            ring.push(*w);
                        }
                    }
                }
            }
        }
        for w in ring {
            let mut n = Vec3::ZERO;
            for t in self.vertex_tris(w) {
                n += self.tri_normal(*t);
            }
            if let Some(x) = self.normals.get_mut(w as usize) {
                *x = n.normalize_or(Vec3::Y);
            }
            self.mark_dirty(w);
        }
    }

    fn mark_dirty(&mut self, v: u32) {
        if let Some(c) = self.dirty_chunks.get_mut(v as usize / CHUNK) {
            *c = true;
        }
    }

    /// Dirty vertex ranges since the last call, coalesced: `(first vertex, count)`.
    pub fn take_dirty_chunks(&mut self) -> Vec<(usize, usize)> {
        let n = self.positions.len();
        let mut out: Vec<(usize, usize)> = Vec::new();
        for (i, d) in self.dirty_chunks.iter_mut().enumerate() {
            if !*d {
                continue;
            }
            *d = false;
            let start = i * CHUNK;
            let len = CHUNK.min(n.saturating_sub(start));
            match out.last_mut() {
                Some((s, l)) if *s + *l == start => *l += len,
                _ => out.push((start, len)),
            }
        }
        out
    }

    /// Replace every position (same topology). Returns false, changing nothing, when the count
    /// differs or a stroke is in progress.
    pub fn set_positions(&mut self, positions: &[Vec3]) -> bool {
        if positions.len() != self.positions.len() || self.stroke.is_some() {
            return false;
        }
        for (dst, src) in self.positions.iter_mut().zip(positions) {
            if src.is_finite() {
                *dst = *src;
            }
        }
        self.recompute_all_normals();
        self.refit();
        self.mark_all_dirty();
        true
    }

    /// Mark everything dirty (after an undo that replaced positions wholesale).
    pub fn mark_all_dirty(&mut self) {
        self.dirty_chunks.iter_mut().for_each(|d| *d = true);
    }

    /// Nearest hit of a ray (object space): `(distance, point, triangle)`.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3, u32)> {
        self.bvh.raycast(&self.positions, &self.tris, origin, dir, self.drift)
    }

    /// Vertices within `radius` of `center`.
    pub fn query_sphere(&mut self, center: Vec3, radius: f32, out: &mut Vec<u32>) {
        out.clear();
        if !(radius.is_finite() && radius > 0.0 && center.is_finite()) {
            return;
        }
        self.stamp_gen = self.stamp_gen.wrapping_add(1);
        if self.stamp_gen == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.stamp_gen = 1;
        }
        let gen_ = self.stamp_gen;
        let r2 = radius * radius;
        let positions = &self.positions;
        let stamp = &mut self.stamp;
        let tris = &self.tris;
        self.bvh.visit_sphere(center, radius + self.drift, |t| {
            if let Some(tri) = tris.get(t as usize) {
                for v in tri {
                    let Some(s) = stamp.get_mut(*v as usize) else { continue };
                    if *s == gen_ {
                        continue;
                    }
                    *s = gen_;
                    if positions.get(*v as usize).is_some_and(|p| p.distance_squared(center) <= r2) {
                        out.push(*v);
                    }
                }
            }
        });
    }

    /// Refit the BVH to the moved vertices (end of a stroke; cost linear in the triangles).
    pub fn refit(&mut self) {
        self.bvh.refit(&self.positions, &self.tris);
        self.drift = 0.0;
    }

    pub fn in_stroke(&self) -> bool {
        self.stroke.is_some()
    }

    /// Start a stroke with these brush settings.
    pub fn begin_stroke(&mut self, brush: BrushSettings) {
        self.stroke = Some(StrokeState { brush, original: Default::default(), grabbed: Vec::new(), dabs: Vec::new(), topo: None });
    }

    /// Apply one dab of the current stroke (does nothing outside a stroke).
    pub fn dab(&mut self, dab: Dab) {
        let Some(mut st) = self.stroke.take() else { return };
        if dab.center.is_finite() && dab.view_dir.is_finite() && dab.grab_delta.is_finite() {
            brush::apply(self, &mut st, &dab);
            st.dabs.push(dab);
        }
        self.stroke = Some(st);
    }

    /// Finish the stroke: refit, return the undo record and the replayable stroke.
    pub fn end_stroke(&mut self) -> Option<(StrokeUndo, Stroke)> {
        let st = self.stroke.take()?;
        let mut u = StrokeUndo::default();
        let old_verts = st.topo.as_ref().map(|t| t.verts).unwrap_or(usize::MAX);
        if let Some(change) = st.topo.as_ref().and_then(|t| self.topo_change(t)) {
            // The triangles changed: the picking tree is rebuilt rather than refitted.
            self.topo_edits += 1;
            self.topo_rev += 1;
            self.bvh = Bvh::build(&self.positions, &self.tris);
            self.drift = 0.0;
            u.topo = Some(Box::new(change));
        } else {
            self.refit();
        }
        let mut idx: Vec<u32> = st.original.keys().copied().filter(|i| (*i as usize) < old_verts).collect();
        idx.sort_unstable();
        for i in idx {
            let (p0, m0) = st.original.get(&i).copied().unwrap_or_default();
            let (p1, m1) = (self.positions.get(i as usize).copied().unwrap_or(p0), self.mask.get(i as usize).copied().unwrap_or(m0));
            if p0 == p1 && m0 == m1 {
                continue;
            }
            u.indices.push(i);
            u.before.push(p0);
            u.after.push(p1);
            u.mask_before.push(m0);
            u.mask_after.push(m1);
        }
        Some((u, Stroke { brush: st.brush, dabs: st.dabs }))
    }

    /// Replay a recorded stroke.
    pub fn replay(&mut self, stroke: &Stroke) -> StrokeUndo {
        self.begin_stroke(stroke.brush.clone());
        for d in &stroke.dabs {
            self.dab(*d);
        }
        self.end_stroke().map(|(u, _)| u).unwrap_or_default()
    }

    /// Put the `before` (undo) or `after` (redo) values back.
    pub fn apply_undo(&mut self, u: &StrokeUndo, redo: bool) {
        if let Some(t) = &u.topo
            && !self.apply_topo(t, redo)
        {
            return;
        }
        let (pos, mask) = if redo { (&u.after, &u.mask_after) } else { (&u.before, &u.mask_before) };
        for (k, i) in u.indices.iter().enumerate() {
            if let (Some(dst), Some(src)) = (self.positions.get_mut(*i as usize), pos.get(k)) {
                *dst = *src;
            }
            if let (Some(dst), Some(src)) = (self.mask.get_mut(*i as usize), mask.get(k)) {
                *dst = *src;
            }
        }
        self.update_normals(&u.indices);
        self.refit();
    }

    /// Mask helpers: clear, invert, fill, or mask the patch under a triangle.
    pub fn mask_all(&mut self, value: f32) -> StrokeUndo {
        self.mask_edit(|_, _, _| value)
    }
    pub fn mask_invert(&mut self) -> StrokeUndo {
        self.mask_edit(|_, _, m| 1.0 - m)
    }
    pub fn mask_patch(&mut self, tri: u32) -> StrokeUndo {
        let Some(&patch) = self.tri_patch.get(tri as usize) else { return StrokeUndo::default() };
        let mut inside = vec![false; self.positions.len()];
        for (t, p) in self.tris.iter().zip(&self.tri_patch) {
            if *p == patch {
                for v in t {
                    if let Some(x) = inside.get_mut(*v as usize) {
                        *x = true;
                    }
                }
            }
        }
        self.mask_edit(|i, _, m| if inside.get(i as usize).copied().unwrap_or(false) { 1.0 } else { m })
    }

    fn mask_edit(&mut self, f: impl Fn(u32, Vec3, f32) -> f32) -> StrokeUndo {
        let mut u = StrokeUndo::default();
        for i in 0..self.positions.len() {
            let (Some(p), Some(m)) = (self.positions.get(i).copied(), self.mask.get(i).copied()) else { continue };
            let nm = f(i as u32, p, m).clamp(0.0, 1.0);
            if nm != m {
                u.indices.push(i as u32);
                u.before.push(p);
                u.after.push(p);
                u.mask_before.push(m);
                u.mask_after.push(nm);
                if let Some(x) = self.mask.get_mut(i) {
                    *x = nm;
                }
            }
        }
        self.mark_all_dirty();
        u
    }
}

#[cfg(test)]
mod tests;
