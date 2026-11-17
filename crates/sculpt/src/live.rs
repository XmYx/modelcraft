//! Live detail: triangles under the brush are split until their edges are no longer than the
//! brush's detail size, so a stroke adds resolution exactly where it works.
//!
//! An edge is split at its midpoint; every triangle on a split edge is re-cut along the new
//! points (into two, three or four), so no vertex ever sits in the middle of a neighbour's edge.
//! Splitting only ever subdivides: children lie inside their parent, which lets the picking
//! tree keep its boxes until the stroke ends. Everything is done in index order, so replaying
//! the stroke gives the same mesh (the crash journal and redo depend on that).

use std::collections::HashMap;

use glam::Vec3;
use modelcraft_mesh::{PolyMesh, Polygon};

use crate::{Adj, Bvh, SculptMesh, StrokeState};

/// Most triangles a live-detail mesh may reach.
pub const MAX_LIVE_TRIANGLES: usize = 16_000_000;
/// The detail size is never finer than the brush radius divided by this.
pub const FINEST_PER_RADIUS: f32 = 48.0;
/// Split rounds per dab (each round halves the longest edges under the brush).
const ROUNDS: usize = 4;

/// The topology part of a stroke's undo record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopoChange {
    pub verts_before: u32,
    pub tris_before: u32,
    /// Triangles that existed before and were re-cut: `(index, before, after)`.
    pub changed: Vec<(u32, [u32; 3], [u32; 3])>,
    pub new_positions: Vec<Vec3>,
    pub new_mask: Vec<f32>,
    pub new_tris: Vec<[u32; 3]>,
    pub new_patch: Vec<u32>,
}

impl TopoChange {
    pub fn heap_bytes(&self) -> usize {
        self.changed.len() * 28 + self.new_positions.len() * 16 + self.new_tris.len() * 16
    }
}

/// What a stroke needs to remember about the topology it started from.
#[derive(Default)]
pub(crate) struct TopoStart {
    pub verts: usize,
    pub tris: usize,
    /// First value of every pre-existing triangle that was re-cut.
    pub changed: HashMap<u32, [u32; 3]>,
}

fn lists_from(tris: &[[u32; 3]], n: usize) -> Adj {
    let mut vtri: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (t, tri) in tris.iter().enumerate() {
        for v in tri {
            if let Some(l) = vtri.get_mut(*v as usize) {
                l.push(t as u32);
            }
        }
    }
    let mut nbr: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (v, l) in nbr.iter_mut().enumerate() {
        neighbours_of(v as u32, vtri.get(v).map(Vec::as_slice).unwrap_or_default(), tris, l);
    }
    Adj::Lists { nbr, vtri }
}

fn neighbours_of(v: u32, vtri: &[u32], tris: &[[u32; 3]], out: &mut Vec<u32>) {
    out.clear();
    for t in vtri {
        if let Some(tri) = tris.get(*t as usize) {
            out.extend(tri.iter().copied().filter(|w| *w != v));
        }
    }
    out.sort_unstable();
    out.dedup();
}

impl SculptMesh {
    /// True once live detail (or its undo) has changed the triangles since the session began.
    pub fn topology_changed(&self) -> bool {
        self.topo_edits != 0
    }

    /// A number that changes whenever the triangles do.
    pub fn topology_rev(&self) -> u64 {
        self.topo_rev
    }

    /// The triangles as a mesh (patches kept, no UVs), for storing a live-detail sculpt.
    pub fn to_poly_mesh(&self) -> Result<PolyMesh, modelcraft_mesh::MeshError> {
        let polys = self
            .tris
            .iter()
            .zip(&self.tri_patch)
            .map(|(t, p)| {
                let mut poly = Polygon::new(t.to_vec());
                poly.patch = *p;
                poly
            })
            .collect();
        Ok(PolyMesh::build(self.positions.clone(), polys)?.0)
    }

    fn make_live(&mut self) {
        if matches!(self.adj, Adj::Csr { .. }) {
            self.adj = lists_from(&self.tris, self.positions.len());
        }
    }

    /// After triangles or vertices were replaced wholesale (undo, redo): rebuild everything
    /// derived from them.
    pub(crate) fn rebuild_topology(&mut self) {
        let n = self.positions.len();
        self.adj = lists_from(&self.tris, n);
        self.bvh = Bvh::build(&self.positions, &self.tris);
        self.drift = 0.0;
        self.stamp = vec![0; n];
        self.stamp_gen = 0;
        self.dirty_chunks = vec![true; n.div_ceil(crate::CHUNK)];
        self.normals.resize(n, Vec3::Y);
        self.recompute_all_normals();
    }

    fn push_vertex(&mut self, p: Vec3, n: Vec3, mask: f32) -> u32 {
        let i = self.positions.len() as u32;
        self.positions.push(p);
        self.normals.push(n.normalize_or(Vec3::Y));
        self.mask.push(mask);
        self.stamp.push(0);
        let chunks = self.positions.len().div_ceil(crate::CHUNK);
        if self.dirty_chunks.len() < chunks {
            self.dirty_chunks.resize(chunks, true);
        }
        if let Adj::Lists { nbr, vtri } = &mut self.adj {
            nbr.push(Vec::new());
            vtri.push(Vec::new());
        }
        self.mark_dirty(i);
        i
    }

    /// Split the edges longer than `edge` whose midpoint lies within `radius` of `center`.
    /// Returns true when anything was split.
    pub(crate) fn refine_sphere(&mut self, st: &mut StrokeState, center: Vec3, radius: f32, edge: f32) -> bool {
        if !(center.is_finite() && radius.is_finite() && radius > 0.0 && edge.is_finite() && edge > 0.0) {
            return false;
        }
        let edge = edge.max(radius / FINEST_PER_RADIUS).max(1e-6);
        let (e2, r2) = (edge * edge, radius * radius);
        self.make_live();
        if st.topo.is_none() {
            st.topo = Some(TopoStart { verts: self.positions.len(), tris: self.tris.len(), changed: HashMap::new() });
        }
        let mut any = false;
        let mut cand: Vec<u32> = Vec::new();
        for _ in 0..ROUNDS {
            cand.clear();
            self.bvh.visit_sphere(center, radius + self.drift, |t| cand.push(t));
            cand.sort_unstable();
            // 1. Choose the edges to split and make their midpoints.
            let mut mids: HashMap<(u32, u32), u32> = HashMap::new();
            let mut split: Vec<(u32, u32)> = Vec::new();
            for t in &cand {
                let Some(tri) = self.tris.get(*t as usize).copied() else { continue };
                for c in 0..3 {
                    let (a, b) = (tri[c], tri[(c + 1) % 3]);
                    let key = (a.min(b), a.max(b));
                    if mids.contains_key(&key) {
                        continue;
                    }
                    let (Some(pa), Some(pb)) = (self.positions.get(a as usize).copied(), self.positions.get(b as usize).copied()) else { continue };
                    let mid = (pa + pb) * 0.5;
                    if pa.distance_squared(pb) <= e2 || mid.distance_squared(center) > r2 {
                        continue;
                    }
                    let (ma, mb) = (self.mask.get(a as usize).copied().unwrap_or(0.0), self.mask.get(b as usize).copied().unwrap_or(0.0));
                    if ma.min(mb) >= 0.999 {
                        continue;
                    }
                    if self.tris.len() + split.len() * 2 + 4 > MAX_LIVE_TRIANGLES {
                        break;
                    }
                    let n = self.normals.get(a as usize).copied().unwrap_or(Vec3::Y) + self.normals.get(b as usize).copied().unwrap_or(Vec3::Y);
                    let v = self.push_vertex(mid, n, (ma + mb) * 0.5);
                    mids.insert(key, v);
                    split.push(key);
                }
            }
            if split.is_empty() {
                break;
            }
            any = true;
            // 2. Every triangle on a split edge is re-cut.
            let mut affected: Vec<u32> = Vec::new();
            for (a, b) in &split {
                for t in self.vertex_tris(*a) {
                    if self.tris.get(*t as usize).is_some_and(|tri| tri.contains(b)) {
                        affected.push(*t);
                    }
                }
            }
            affected.sort_unstable();
            affected.dedup();
            let mut touched: Vec<u32> = Vec::new();
            for t in affected {
                let Some(old) = self.tris.get(t as usize).copied() else { continue };
                let patch = self.tri_patch.get(t as usize).copied().unwrap_or(0);
                let m = [0, 1, 2].map(|c| {
                    let (a, b) = (old[c], old[(c + 1) % 3]);
                    mids.get(&(a.min(b), a.max(b))).copied()
                });
                let children = cut(old, m, &self.positions);
                let Some(first) = children.first().copied() else { continue };
                if let Some(start) = &mut st.topo
                    && (t as usize) < start.tris
                {
                    start.changed.entry(t).or_insert(old);
                }
                // Adjacency: the parent leaves its corners, the children join theirs.
                if let Adj::Lists { vtri, .. } = &mut self.adj {
                    for v in old {
                        if let Some(l) = vtri.get_mut(v as usize) {
                            l.retain(|x| *x != t);
                        }
                    }
                }
                touched.extend(old);
                for (k, child) in children.iter().enumerate() {
                    let id = if k == 0 {
                        if let Some(slot) = self.tris.get_mut(t as usize) {
                            *slot = first;
                        }
                        t
                    } else {
                        let id = self.tris.len() as u32;
                        self.tris.push(*child);
                        self.tri_patch.push(patch);
                        self.bvh.add_child(t, id);
                        id
                    };
                    if let Adj::Lists { vtri, .. } = &mut self.adj {
                        for v in child {
                            if let Some(l) = vtri.get_mut(*v as usize) {
                                l.push(id);
                            }
                        }
                    }
                    touched.extend(child);
                }
            }
            touched.sort_unstable();
            touched.dedup();
            if let Adj::Lists { nbr, vtri } = &mut self.adj {
                for v in &touched {
                    if let (Some(l), Some(ts)) = (nbr.get_mut(*v as usize), vtri.get(*v as usize)) {
                        neighbours_of(*v, ts, &self.tris, l);
                    }
                }
            }
            self.update_normals(&touched);
        }
        any
    }

    /// The topology part of the undo record for the stroke that just ended.
    pub(crate) fn topo_change(&self, start: &TopoStart) -> Option<TopoChange> {
        if self.positions.len() == start.verts && self.tris.len() == start.tris && start.changed.is_empty() {
            return None;
        }
        let mut changed: Vec<(u32, [u32; 3], [u32; 3])> =
            start.changed.iter().filter_map(|(t, before)| Some((*t, *before, *self.tris.get(*t as usize)?))).collect();
        changed.sort_unstable_by_key(|c| c.0);
        Some(TopoChange {
            verts_before: start.verts as u32,
            tris_before: start.tris as u32,
            changed,
            new_positions: self.positions.get(start.verts..).unwrap_or_default().to_vec(),
            new_mask: self.mask.get(start.verts..).unwrap_or_default().to_vec(),
            new_tris: self.tris.get(start.tris..).unwrap_or_default().to_vec(),
            new_patch: self.tri_patch.get(start.tris..).unwrap_or_default().to_vec(),
        })
    }

    /// Undo or redo the topology part of a stroke. Returns false (changing nothing) when the
    /// record does not fit this mesh.
    pub(crate) fn apply_topo(&mut self, c: &TopoChange, redo: bool) -> bool {
        let (vb, tb) = (c.verts_before as usize, c.tris_before as usize);
        let fits = if redo {
            self.positions.len() == vb && self.tris.len() == tb
        } else {
            self.positions.len() == vb + c.new_positions.len() && self.tris.len() == tb + c.new_tris.len()
        };
        if !fits || c.changed.iter().any(|(t, _, _)| *t as usize >= tb) || c.new_mask.len() != c.new_positions.len() || c.new_patch.len() != c.new_tris.len() {
            return false;
        }
        if redo {
            let total = (vb + c.new_positions.len()) as u32;
            if c.new_tris.iter().chain(c.changed.iter().map(|(_, _, after)| after)).flatten().any(|v| *v >= total) {
                return false;
            }
            self.positions.extend(c.new_positions.iter().map(|p| if p.is_finite() { *p } else { Vec3::ZERO }));
            self.mask.extend(c.new_mask.iter().map(|m| if m.is_finite() { m.clamp(0.0, 1.0) } else { 0.0 }));
            self.tris.extend_from_slice(&c.new_tris);
            self.tri_patch.extend_from_slice(&c.new_patch);
        } else {
            self.positions.truncate(vb);
            self.normals.truncate(vb);
            self.mask.truncate(vb);
            self.tris.truncate(tb);
            self.tri_patch.truncate(tb);
        }
        for (t, before, after) in &c.changed {
            if let Some(slot) = self.tris.get_mut(*t as usize) {
                *slot = if redo { *after } else { *before };
            }
        }
        self.topo_edits += if redo { 1 } else { -1 };
        self.topo_rev += 1;
        self.rebuild_topology();
        true
    }
}

/// Re-cut triangle `t` along the midpoints `m` of its edges (`m[c]` is on the edge from corner
/// `c` to the next). The first child takes the parent's place.
fn cut(t: [u32; 3], m: [Option<u32>; 3], p: &[Vec3]) -> Vec<[u32; 3]> {
    let d2 = |a: u32, b: u32| match (p.get(a as usize), p.get(b as usize)) {
        (Some(x), Some(y)) => x.distance_squared(*y),
        _ => f32::MAX,
    };
    match m {
        [None, None, None] => vec![],
        [Some(a), Some(b), Some(c)] => vec![[t[0], a, c], [a, t[1], b], [c, b, t[2]], [a, b, c]],
        _ => {
            // Turn the triangle so the pattern starts at corner 0.
            let k = m.iter().filter(|x| x.is_some()).count();
            let r = if k == 1 { m.iter().position(|x| x.is_some()).unwrap_or(0) } else { (m.iter().position(|x| x.is_none()).unwrap_or(0) + 1) % 3 };
            let (v0, v1, v2) = (t[r], t[(r + 1) % 3], t[(r + 2) % 3]);
            let (m0, m1) = (m[r], m[(r + 1) % 3]);
            match (k, m0, m1) {
                (1, Some(a), _) => vec![[v0, a, v2], [a, v1, v2]],
                (2, Some(a), Some(b)) => {
                    // Edges v0-v1 and v1-v2 are split; the quad v0, a, b, v2 takes its shorter diagonal.
                    if d2(a, v2) <= d2(v0, b) { vec![[a, v1, b], [v0, a, v2], [a, b, v2]] } else { vec![[a, v1, b], [v0, a, b], [v0, b, v2]] }
                }
                _ => vec![],
            }
        }
    }
}
