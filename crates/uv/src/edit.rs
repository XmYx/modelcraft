//! Editing UVs by hand: move, rotate and scale chosen corners, weld the two sides of seams, and
//! the selection helpers the UV editor uses.
//!
//! A *UV vertex* (see [`Charts`]) is the group of corners of one vertex that share a place in UV
//! space; selecting a UV vertex in the editor means all of its corners, so that the surface stays
//! connected in the layout. These functions take explicit corner lists, which is also what the
//! history steps record.

use std::collections::{BTreeSet, HashMap};

use glam::Vec2;
use modelcraft_mesh::{NONE, PolyMesh};

use crate::UvError;
use crate::charts::Charts;
use crate::seams::SeamSet;

/// A transform of UV points about a pivot: scale, then rotate, then translate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvTransform {
    pub translate: Vec2,
    /// Rotation in degrees, counter-clockwise as the layout is drawn (v down, so clockwise in the
    /// numbers; the editor shows the layout the way it is stored).
    pub rotate_deg: f32,
    pub scale: Vec2,
    pub pivot: Vec2,
}

impl Default for UvTransform {
    fn default() -> Self {
        UvTransform { translate: Vec2::ZERO, rotate_deg: 0.0, scale: Vec2::ONE, pivot: Vec2::ZERO }
    }
}

impl UvTransform {
    pub fn is_finite(&self) -> bool {
        self.translate.is_finite() && self.rotate_deg.is_finite() && self.scale.is_finite() && self.pivot.is_finite()
    }

    pub fn is_identity(&self) -> bool {
        self.translate == Vec2::ZERO && self.rotate_deg == 0.0 && self.scale == Vec2::ONE
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        let (s, c) = self.rotate_deg.to_radians().sin_cos();
        let d = (p - self.pivot) * self.scale;
        self.pivot + Vec2::new(c * d.x - s * d.y, s * d.x + c * d.y) + self.translate
    }
}

/// Move the listed corners by `t`. Corners that do not exist are ignored; a non-finite transform
/// or a mesh without UVs is an error.
pub fn transform_corners(mesh: &PolyMesh, corners: &[u32], t: &UvTransform) -> Result<PolyMesh, UvError> {
    if !mesh.has_uvs() {
        return Err(UvError::NoUvs);
    }
    if !t.is_finite() {
        return Err(UvError::Invalid("the transform is not a finite number".into()));
    }
    let mut uvs = mesh.corner_uvs().to_vec();
    for &c in corners {
        if let Some(u) = uvs.get_mut(c as usize) {
            let q = t.apply(*u);
            if q.is_finite() {
                *u = q;
            }
        }
    }
    let mut out = mesh.clone();
    out.set_uvs(&uvs)?;
    Ok(out)
}

/// Weld the two sides of each listed edge in UV space: the corners at each end of the edge, on
/// both sides, and every corner that shares a UV vertex with them, move to their common centre.
/// Edges that are already continuous, on the boundary or missing are left alone. Welding is
/// transitive: edges that meet at a corner pull all their UV vertices to one point.
pub fn stitch_edges(mesh: &PolyMesh, edges: &[u32]) -> Result<PolyMesh, UvError> {
    if !mesh.has_uvs() {
        return Err(UvError::NoUvs);
    }
    let charts = Charts::build(mesh, &SeamSet::from_uv_discontinuities(mesh))?;
    let nv = charts.vertex_source.len();
    let mut dsu: Vec<u32> = (0..nv as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while let Some(&parent) = p.get(x as usize) {
            if parent == x {
                break;
            }
            let grand = p.get(parent as usize).copied().unwrap_or(parent);
            if let Some(s) = p.get_mut(x as usize) {
                *s = grand;
            }
            x = grand;
        }
        x
    }
    let uv_vertex = |c: u32| charts.corner_vertex.get(c as usize).copied();
    let mut touched = false;
    for &e in edges {
        if (e as usize) >= mesh.edge_count() {
            continue;
        }
        let h = mesh.edge_half_edge(e);
        let t = mesh.twin(h);
        if mesh.face_of(h) == NONE || mesh.face_of(t) == NONE {
            continue;
        }
        // h runs a → b and t runs b → a: the UV vertex of a is at h and at next(t), of b at next(h) and t.
        for (x, y) in [(h, mesh.next(t)), (mesh.next(h), t)] {
            if let (Some(p), Some(q)) = (uv_vertex(x), uv_vertex(y)) {
                let (rp, rq) = (find(&mut dsu, p), find(&mut dsu, q));
                if rp != rq {
                    if let Some(s) = dsu.get_mut(rp.max(rq) as usize) {
                        *s = rp.min(rq);
                    }
                    touched = true;
                }
            }
        }
    }
    if !touched {
        return Ok(mesh.clone());
    }
    // Size of each welded group (in UV vertices) and the sum of its corner UVs.
    let mut size: HashMap<u32, u32> = HashMap::new();
    for v in 0..nv as u32 {
        *size.entry(find(&mut dsu, v)).or_insert(0) += 1;
    }
    let mut sum: HashMap<u32, (Vec2, u32)> = HashMap::new();
    for c in 0..mesh.corner_count() as u32 {
        if let Some(v) = uv_vertex(c) {
            let r = find(&mut dsu, v);
            let e = sum.entry(r).or_insert((Vec2::ZERO, 0));
            e.0 += mesh.uv(c);
            e.1 += 1;
        }
    }
    let mut uvs = mesh.corner_uvs().to_vec();
    for c in 0..uvs.len() as u32 {
        let Some(v) = uv_vertex(c) else { continue };
        let r = find(&mut dsu, v);
        // Only groups that were joined move; a lone UV vertex keeps its exact position.
        if size.get(&r).copied().unwrap_or(1) > 1
            && let Some((s, n)) = sum.get(&r)
            && let Some(u) = uvs.get_mut(c as usize)
        {
            *u = *s / *n as f32;
        }
    }
    let mut out = mesh.clone();
    out.set_uvs(&uvs)?;
    Ok(out)
}

/// The corners that move together with `corner` in the editor: those of its UV vertex.
pub fn uv_vertex_corners(mesh: &PolyMesh, charts: &Charts, corner: u32) -> Vec<u32> {
    let Some(&v) = charts.corner_vertex.get(corner as usize) else { return Vec::new() };
    (0..mesh.corner_count() as u32).filter(|c| charts.corner_vertex.get(*c as usize) == Some(&v)).collect()
}

/// All corners of the island that contains `corner`.
pub fn island_corners(mesh: &PolyMesh, charts: &Charts, corner: u32) -> Vec<u32> {
    let f = mesh.face_of(corner);
    let Some(&isl) = charts.face_island.get(f as usize) else { return Vec::new() };
    let Some(island) = charts.islands.get(isl as usize) else { return Vec::new() };
    island.faces.iter().flat_map(|f| mesh.face_half_edges(*f)).collect()
}

/// Expand a set of corners to every corner of their UV vertices (what a vertex selection means).
pub fn expand_to_uv_vertices(mesh: &PolyMesh, charts: &Charts, corners: &BTreeSet<u32>) -> BTreeSet<u32> {
    let verts: BTreeSet<u32> = corners.iter().filter_map(|c| charts.corner_vertex.get(*c as usize).copied()).collect();
    (0..mesh.corner_count() as u32).filter(|c| charts.corner_vertex.get(*c as usize).is_some_and(|v| verts.contains(v))).collect()
}

/// Bounds of the UVs of some corners: `(min, max)`, `None` when there are none.
pub fn bounds(mesh: &PolyMesh, corners: &BTreeSet<u32>) -> Option<(Vec2, Vec2)> {
    let mut it = corners.iter().filter(|c| (**c as usize) < mesh.corner_count()).map(|c| mesh.uv(*c));
    let first = it.next()?;
    Some(it.fold((first, first), |(lo, hi), u| (lo.min(u), hi.max(u))))
}
