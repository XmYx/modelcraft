//! Seams: the edges along which a surface is cut open before it is flattened.
//!
//! A seam is stored as a pair of vertex indices, so a set of seams survives UV edits and is
//! independent of how a mesh numbers its edges. Edges that are already on the mesh boundary are
//! cuts anyway; marking them does nothing.

use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap};

use modelcraft_mesh::{NONE, PolyMesh};
use serde::{Deserialize, Serialize};

/// The most seams a set accepts from outside (a file or a command).
pub const MAX_SEAMS: usize = 20_000_000;

/// A set of seam edges, keyed by their two vertices.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SeamSet {
    edges: BTreeSet<(u32, u32)>,
}

fn key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

impl SeamSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// From vertex pairs, as stored in a history step. Pairs with equal ends are ignored and the
    /// count is capped at [`MAX_SEAMS`].
    pub fn from_vertex_pairs(pairs: &[[u32; 2]]) -> Self {
        let edges = pairs.iter().filter(|p| p[0] != p[1]).take(MAX_SEAMS).map(|p| key(p[0], p[1])).collect();
        SeamSet { edges }
    }

    /// The vertex pairs, sorted.
    pub fn to_vertex_pairs(&self) -> Vec<[u32; 2]> {
        self.edges.iter().map(|(a, b)| [*a, *b]).collect()
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    pub fn clear_all(&mut self) {
        self.edges.clear();
    }

    pub fn contains_vertices(&self, a: u32, b: u32) -> bool {
        self.edges.contains(&key(a, b))
    }

    /// Whether edge `e` of `mesh` is a seam.
    pub fn contains(&self, mesh: &PolyMesh, e: u32) -> bool {
        if (e as usize) >= mesh.edge_count() {
            return false;
        }
        let (a, b) = mesh.edge_vertices(e);
        self.contains_vertices(a, b)
    }

    /// Mark edge `e`. Returns whether the set changed (false for a missing edge or one that is
    /// already marked).
    pub fn mark(&mut self, mesh: &PolyMesh, e: u32) -> bool {
        if (e as usize) >= mesh.edge_count() || self.edges.len() >= MAX_SEAMS {
            return false;
        }
        let (a, b) = mesh.edge_vertices(e);
        self.edges.insert(key(a, b))
    }

    /// Clear edge `e`. Returns whether the set changed.
    pub fn clear(&mut self, mesh: &PolyMesh, e: u32) -> bool {
        if (e as usize) >= mesh.edge_count() {
            return false;
        }
        let (a, b) = mesh.edge_vertices(e);
        self.edges.remove(&key(a, b))
    }

    /// Mark every listed edge; returns how many were newly marked.
    pub fn mark_edges(&mut self, mesh: &PolyMesh, edges: &[u32]) -> usize {
        edges.iter().filter(|e| self.mark(mesh, **e)).count()
    }

    /// Clear every listed edge; returns how many were cleared.
    pub fn clear_edges(&mut self, mesh: &PolyMesh, edges: &[u32]) -> usize {
        edges.iter().filter(|e| self.clear(mesh, **e)).count()
    }

    /// Mark the edges that are not all marked, or clear them when they all are (the usual toggle
    /// on a selection). Returns whether the edges are marked afterwards.
    pub fn toggle_edges(&mut self, mesh: &PolyMesh, edges: &[u32]) -> bool {
        let valid: Vec<u32> = edges.iter().copied().filter(|e| (*e as usize) < mesh.edge_count()).collect();
        if !valid.is_empty() && valid.iter().all(|e| self.contains(mesh, *e)) {
            self.clear_edges(mesh, &valid);
            false
        } else {
            self.mark_edges(mesh, &valid);
            !valid.is_empty()
        }
    }

    /// The ids of the marked edges that exist in `mesh`, in increasing order.
    pub fn edge_ids(&self, mesh: &PolyMesh) -> Vec<u32> {
        let mut out: Vec<u32> = self.edges.iter().filter_map(|(a, b)| mesh.find_edge(*a, *b)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Drop the seams that name a vertex pair that is not an edge of `mesh`; returns how many.
    pub fn retain_existing(&mut self, mesh: &PolyMesh) -> usize {
        let before = self.edges.len();
        let n = mesh.vertex_count() as u32;
        self.edges.retain(|(a, b)| *a < n && *b < n && mesh.find_edge(*a, *b).is_some());
        before - self.edges.len()
    }

    /// Mark every interior edge whose two faces meet at more than `degrees` (the dihedral
    /// angle between their normals). Returns how many were newly marked.
    pub fn mark_by_angle(&mut self, mesh: &PolyMesh, degrees: f32) -> usize {
        if !degrees.is_finite() {
            return 0;
        }
        let limit = degrees.clamp(0.0, 180.0).to_radians().cos();
        let normals: Vec<_> = (0..mesh.face_count() as u32).map(|f| mesh.face_normal(f)).collect();
        let mut n = 0;
        for e in 0..mesh.edge_count() as u32 {
            let h = mesh.edge_half_edge(e);
            let (f, g) = (mesh.face_of(h), mesh.face_of(mesh.twin(h)));
            if f == NONE || g == NONE {
                continue;
            }
            let (Some(a), Some(b)) = (normals.get(f as usize), normals.get(g as usize)) else { continue };
            if a.length_squared() == 0.0 || b.length_squared() == 0.0 {
                continue;
            }
            if a.dot(*b) < limit && self.mark(mesh, e) {
                n += 1;
            }
        }
        n
    }

    /// Mark every interior edge between two faces of different patches (polygon groups): the
    /// seams that make each patch an island. Returns how many edges were newly marked.
    pub fn mark_patch_borders(&mut self, mesh: &PolyMesh) -> usize {
        let mut n = 0;
        for e in 0..mesh.edge_count() as u32 {
            let h = mesh.edge_half_edge(e);
            let t = mesh.twin(h);
            if mesh.face_of(h) == NONE || mesh.face_of(t) == NONE {
                continue;
            }
            if mesh.patch(mesh.face_of(h)) != mesh.patch(mesh.face_of(t)) && self.mark(mesh, e) {
                n += 1;
            }
        }
        n
    }

    /// The seams implied by the UVs a mesh already has: every interior edge whose two sides
    /// carry different UVs at either end. Empty for a mesh without UVs.
    pub fn from_uv_discontinuities(mesh: &PolyMesh) -> Self {
        let mut set = SeamSet::new();
        if !mesh.has_uvs() {
            return set;
        }
        for e in 0..mesh.edge_count() as u32 {
            let h = mesh.edge_half_edge(e);
            let t = mesh.twin(h);
            if mesh.face_of(h) == NONE || mesh.face_of(t) == NONE {
                continue;
            }
            // h runs a -> b; t runs b -> a. The UV of a is at h and at next(t); of b at next(h) and t.
            let same = |p: u32, q: u32| (mesh.uv(p) - mesh.uv(q)).abs().max_element() <= 1e-6;
            if !(same(h, mesh.next(t)) && same(mesh.next(h), t)) {
                set.mark(mesh, e);
            }
        }
        set
    }

    /// Mark the shortest path of edges (by length) from vertex `from` to vertex `to`. Returns the
    /// number of edges marked, or `None` when there is no path.
    pub fn mark_path(&mut self, mesh: &PolyMesh, from: u32, to: u32) -> Option<usize> {
        let path = shortest_path(mesh, from, to)?;
        let mut n = 0;
        for w in path.windows(2) {
            if let [a, b] = w
                && self.edges.len() < MAX_SEAMS
                && self.edges.insert(key(*a, *b))
            {
                n += 1;
            }
        }
        Some(n)
    }
}

#[derive(PartialEq)]
struct Item(f64, u32);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.partial_cmp(&self.0).unwrap_or(Ordering::Equal).then(o.1.cmp(&self.1))
    }
}

/// The vertices of the shortest edge path from `from` to `to` (Dijkstra over edge lengths).
pub fn shortest_path(mesh: &PolyMesh, from: u32, to: u32) -> Option<Vec<u32>> {
    let n = mesh.vertex_count();
    if (from as usize) >= n || (to as usize) >= n {
        return None;
    }
    let mut dist = vec![f64::INFINITY; n];
    let mut prev = vec![NONE; n];
    let mut heap = BinaryHeap::new();
    *dist.get_mut(from as usize)? = 0.0;
    heap.push(Item(0.0, from));
    while let Some(Item(d, v)) = heap.pop() {
        if v == to {
            break;
        }
        if d > dist.get(v as usize).copied().unwrap_or(f64::INFINITY) {
            continue;
        }
        let pv = mesh.position(v);
        for w in mesh.vertex_neighbors(v) {
            let nd = d + f64::from(pv.distance(mesh.position(w)));
            if let Some(slot) = dist.get_mut(w as usize)
                && nd < *slot
            {
                *slot = nd;
                if let Some(p) = prev.get_mut(w as usize) {
                    *p = v;
                }
                heap.push(Item(nd, w));
            }
        }
    }
    if !dist.get(to as usize)?.is_finite() {
        return None;
    }
    let mut path = vec![to];
    let mut c = to;
    while c != from {
        c = *prev.get(c as usize)?;
        if c == NONE || path.len() > n {
            return None;
        }
        path.push(c);
    }
    path.reverse();
    Some(path)
}
