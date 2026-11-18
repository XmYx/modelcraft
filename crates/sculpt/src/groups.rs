//! Patches made while sculpting, what is hidden, screen regions (rectangle and lasso), clipping
//! to a plane, occlusion masks and shells from a mask.
//!
//! The grouping functions work on the faces of a [`PolyMesh`] and return one value per face;
//! [`SculptMesh::set_face_patches`] and [`SculptMesh::set_face_hidden`] bring a sculpt session
//! in line. Hidden triangles are skipped by picking and by every brush, and nothing about them
//! is removed: showing them again gives back exactly what was there.

use glam::{Mat4, Vec2, Vec3, Vec4};
use modelcraft_mesh::{NONE, PolyMesh, Polygon};
use serde::{Deserialize, Serialize};

use crate::{SculptMesh, StrokeUndo};

/// Most corners a screen region's outline may have.
pub const MAX_REGION_POINTS: usize = 4096;
/// Rays cast from all vertices together by one occlusion mask (the count per vertex follows).
pub const OCCLUSION_RAY_BUDGET: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GroupError {
    #[error("the mesh has no UVs")]
    NoUvs,
    #[error("nothing is masked")]
    NothingMasked,
    #[error("the thickness must be a finite number other than zero")]
    BadThickness,
    #[error(transparent)]
    Mesh(#[from] modelcraft_mesh::MeshError),
}

/// An outline drawn on the screen, with the view it was drawn in. `points` are in normalized
/// device coordinates (x right, y up, both −1..1); `view_proj` takes object space to clip space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenRegion {
    pub view_proj: Mat4,
    pub points: Vec<Vec2>,
    /// Only what faces the viewer (`eye`, in object space) is inside.
    #[serde(default)]
    pub front_only: bool,
    #[serde(default)]
    pub eye: Vec3,
}

impl ScreenRegion {
    /// A rectangle between two corners in normalized device coordinates.
    pub fn rect(view_proj: Mat4, a: Vec2, b: Vec2) -> ScreenRegion {
        let (lo, hi) = (a.min(b), a.max(b));
        ScreenRegion { view_proj, points: vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)], front_only: false, eye: Vec3::ZERO }
    }

    /// False for a region nothing can be tested against (too few or too many corners, or
    /// numbers that are not finite).
    pub fn is_valid(&self) -> bool {
        (3..=MAX_REGION_POINTS).contains(&self.points.len()) && self.points.iter().all(|p| p.is_finite()) && self.view_proj.is_finite() && self.eye.is_finite()
    }

    /// Is the object-space point `p` (with surface normal `n`) inside the outline?
    pub fn contains(&self, p: Vec3, n: Vec3) -> bool {
        if self.front_only && n.dot(self.eye - p) <= 0.0 {
            return false;
        }
        let c: Vec4 = self.view_proj * p.extend(1.0);
        if !(c.w.is_finite() && c.w > 1e-12) {
            return false;
        }
        let q = Vec2::new(c.x / c.w, c.y / c.w);
        // Even-odd rule: count the outline's edges a ray to the right of `q` crosses.
        let mut inside = false;
        let k = self.points.len();
        for i in 0..k {
            let (Some(a), Some(b)) = (self.points.get(i), self.points.get((i + 1) % k)) else { continue };
            if (a.y > q.y) != (b.y > q.y) && q.x < a.x + (q.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                inside = !inside;
            }
        }
        inside
    }
}

/// The faces around `f` across its edges.
fn face_neighbours(m: &PolyMesh, f: u32) -> impl Iterator<Item = (u32, u32)> + '_ {
    m.face_half_edges(f).filter_map(move |h| {
        let g = m.face_of(m.twin(h));
        (g != NONE && (g as usize) < m.face_count()).then_some((h, g))
    })
}

/// Number the groups of faces joined through edges where `joined(half-edge, face, neighbour)`
/// holds: `(group of every face, group count)`. Groups are numbered from `first` in face order.
fn flood(m: &PolyMesh, first: u32, joined: impl Fn(u32, u32, u32) -> bool) -> (Vec<u32>, u32) {
    let nf = m.face_count();
    let mut group = vec![NONE; nf];
    let mut next = first;
    let mut stack = Vec::new();
    for seed in 0..nf as u32 {
        if group.get(seed as usize).copied() != Some(NONE) {
            continue;
        }
        if let Some(g) = group.get_mut(seed as usize) {
            *g = next;
        }
        stack.push(seed);
        while let Some(f) = stack.pop() {
            for (h, g) in face_neighbours(m, f) {
                if group.get(g as usize).copied() == Some(NONE)
                    && joined(h, f, g)
                    && let Some(x) = group.get_mut(g as usize)
                {
                    *x = next;
                    stack.push(g);
                }
            }
        }
        next = next.saturating_add(1);
    }
    (group, next.saturating_sub(first))
}

/// One patch per region of faces that bend less than `angle_deg` from one to the next (so a
/// crease, a hard edge or a corner starts a new patch).
pub fn patches_by_normal(m: &PolyMesh, angle_deg: f32) -> (Vec<u32>, u32) {
    let angle = if angle_deg.is_finite() { angle_deg.clamp(0.5, 180.0) } else { 30.0 };
    let cos = angle.to_radians().cos();
    let normals: Vec<Vec3> = (0..m.face_count() as u32).map(|f| m.face_normal(f)).collect();
    let n = |f: u32| normals.get(f as usize).copied().unwrap_or(Vec3::Y);
    flood(m, 1, |_, f, g| n(f).dot(n(g)) >= cos)
}

/// One patch per connected part of the mesh.
pub fn patches_by_part(m: &PolyMesh) -> (Vec<u32>, u32) {
    flood(m, 1, |_, _, _| true)
}

/// One patch per UV island: faces are joined where both ends of their shared edge have the
/// same UV on either side.
pub fn patches_by_uv_island(m: &PolyMesh) -> Result<(Vec<u32>, u32), GroupError> {
    if !m.has_uvs() {
        return Err(GroupError::NoUvs);
    }
    let same = |a: Vec2, b: Vec2| a.distance_squared(b) <= 1e-12;
    Ok(flood(m, 1, |h, _, _| {
        let t = m.twin(h);
        same(m.uv(h), m.uv(m.next(t))) && same(m.uv(m.next(h)), m.uv(t))
    }))
}

/// Is every corner of the face masked more than half?
fn face_masked(m: &PolyMesh, mask: &[f32], f: u32) -> bool {
    m.face_vertices(f).all(|v| mask.get(v as usize).copied().unwrap_or(0.0) > 0.5)
}

/// Put the masked faces into a new patch of their own. Returns the patches and the number of
/// faces that moved.
pub fn patch_from_mask(m: &PolyMesh, mask: &[f32]) -> Result<(Vec<u32>, usize), GroupError> {
    let mut patches: Vec<u32> = (0..m.face_count() as u32).map(|f| m.patch(f)).collect();
    let fresh = patches.iter().copied().max().unwrap_or(0).saturating_add(1);
    let mut moved = 0;
    for (f, p) in patches.iter_mut().enumerate() {
        if face_masked(m, mask, f as u32) {
            *p = fresh;
            moved += 1;
        }
    }
    if moved == 0 {
        return Err(GroupError::NothingMasked);
    }
    Ok((patches, moved))
}

/// The faces whose corners all lie inside the region.
pub fn faces_in_region(m: &PolyMesh, region: &ScreenRegion) -> Vec<bool> {
    if !region.is_valid() {
        return vec![false; m.face_count()];
    }
    let normals = m.vertex_normals();
    let inside: Vec<bool> = m.positions().iter().enumerate().map(|(v, p)| region.contains(*p, normals.get(v).copied().unwrap_or(Vec3::Y))).collect();
    (0..m.face_count() as u32).map(|f| m.face_len(f) >= 3 && m.face_vertices(f).all(|v| inside.get(v as usize).copied().unwrap_or(false))).collect()
}

/// The fully masked faces of a mesh.
pub fn faces_masked(m: &PolyMesh, mask: &[f32]) -> Vec<bool> {
    (0..m.face_count() as u32).map(|f| face_masked(m, mask, f)).collect()
}

/// A new closed mesh from the masked faces, thickened by `thickness` along the surface normals
/// (negative: inwards). The rim between the two skins is closed with quads.
pub fn extract_shell(m: &PolyMesh, mask: &[f32], thickness: f32) -> Result<PolyMesh, GroupError> {
    if !thickness.is_finite() || thickness == 0.0 {
        return Err(GroupError::BadThickness);
    }
    let picked = faces_masked(m, mask);
    if !picked.iter().any(|p| *p) {
        return Err(GroupError::NothingMasked);
    }
    let normals = m.vertex_normals();
    // Two points per used vertex: on the surface and offset from it.
    let mut at = vec![NONE; m.vertex_count()];
    let mut pos: Vec<Vec3> = Vec::new();
    for (f, on) in picked.iter().enumerate() {
        if !*on {
            continue;
        }
        for v in m.face_vertices(f as u32) {
            let Some(slot) = at.get_mut(v as usize) else { continue };
            if *slot == NONE {
                *slot = pos.len() as u32;
                let p = m.position(v);
                pos.push(p);
                pos.push(p + normals.get(v as usize).copied().unwrap_or(Vec3::Y) * thickness);
            }
        }
    }
    // The skin that faces outwards keeps the winding; which of the two it is depends on the sign.
    let (outer, inner) = if thickness > 0.0 { (1, 0) } else { (0, 1) };
    let idx = |v: u32, side: u32| at.get(v as usize).copied().unwrap_or(0).saturating_add(side);
    let mut polys = Vec::new();
    for (f, on) in picked.iter().enumerate() {
        if !*on {
            continue;
        }
        let f = f as u32;
        let hs: Vec<u32> = m.face_half_edges(f).collect();
        let uvs = m.has_uvs().then(|| hs.iter().map(|h| m.uv(*h)).collect::<Vec<Vec2>>());
        let mut top = Polygon::new(hs.iter().map(|h| idx(m.origin(*h), outer)).collect());
        top.uvs = uvs.clone();
        top.patch = m.patch(f);
        let mut bottom = Polygon::new(hs.iter().rev().map(|h| idx(m.origin(*h), inner)).collect());
        bottom.uvs = uvs.map(|u| u.into_iter().rev().collect());
        bottom.patch = m.patch(f);
        polys.push(top);
        polys.push(bottom);
        for h in &hs {
            let g = m.face_of(m.twin(*h));
            if g != NONE && picked.get(g as usize).copied().unwrap_or(false) {
                continue;
            }
            let (a, b) = (m.origin(*h), m.dest(*h));
            let mut rim = Polygon::new(vec![idx(b, outer), idx(a, outer), idx(a, inner), idx(b, inner)]);
            rim.uvs = m.has_uvs().then(|| vec![Vec2::ZERO; 4]);
            rim.patch = m.patch(f);
            polys.push(rim);
        }
    }
    Ok(PolyMesh::build(pos, polys)?.0)
}

/// Evenly spread directions over the hemisphere around +Z, denser towards the pole (so each
/// counts the same for light arriving at a surface).
fn hemisphere(k: usize) -> Vec<Vec3> {
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    (0..k)
        .map(|i| {
            let u = (i as f32 + 0.5) / k as f32;
            let (r, a) = (u.sqrt(), i as f32 * golden);
            Vec3::new(r * a.cos(), r * a.sin(), (1.0 - u).max(0.0).sqrt())
        })
        .collect()
}

impl SculptMesh {
    /// Faces of the mesh this session was built from, or triangles once live detail has
    /// changed them (every triangle is then a face of its own).
    pub fn face_count(&self) -> usize {
        if self.topology_changed() { self.tris.len() } else { self.faces }
    }

    /// The face a triangle belongs to.
    pub fn face_of_triangle(&self, t: u32) -> u32 {
        if self.topology_changed() { t } else { self.tri_face.get(t as usize).copied().unwrap_or(t) }
    }

    pub fn patch_of_triangle(&self, t: u32) -> Option<u32> {
        self.tri_patch.get(t as usize).copied()
    }

    /// The patch of every triangle.
    pub fn triangle_patches(&self) -> &[u32] {
        &self.tri_patch
    }

    /// The patch of every face.
    pub fn face_patches(&self) -> Vec<u32> {
        let mut out = vec![0; self.face_count()];
        for (t, p) in self.tri_patch.iter().enumerate() {
            if let Some(x) = out.get_mut(self.face_of_triangle(t as u32) as usize) {
                *x = *p;
            }
        }
        out
    }

    /// Give every face a patch (one value per face). Returns false, changing nothing, when the
    /// count is wrong.
    pub fn set_face_patches(&mut self, patches: &[u32]) -> bool {
        if patches.len() != self.face_count() {
            return false;
        }
        for t in 0..self.tris.len() {
            let p = patches.get(self.face_of_triangle(t as u32) as usize).copied().unwrap_or(0);
            if let Some(x) = self.tri_patch.get_mut(t) {
                *x = p;
            }
        }
        self.view_rev += 1;
        true
    }

    /// Hide faces (one flag per face; an empty slice shows everything). Returns false, changing
    /// nothing, when the count is wrong or a stroke is in progress.
    pub fn set_face_hidden(&mut self, hidden: &[bool]) -> bool {
        if self.stroke.is_some() || !(hidden.is_empty() || hidden.len() == self.face_count()) {
            return false;
        }
        self.hidden = if hidden.iter().any(|h| *h) {
            (0..self.tris.len() as u32).map(|t| hidden.get(self.face_of_triangle(t) as usize).copied().unwrap_or(false)).collect()
        } else {
            Vec::new()
        };
        self.view_rev += 1;
        true
    }

    /// One flag per triangle, or nothing when everything shows.
    pub fn hidden_triangles(&self) -> &[bool] {
        &self.hidden
    }

    pub fn any_hidden(&self) -> bool {
        !self.hidden.is_empty()
    }

    /// A number that changes whenever patches or visibility do (a renderer rebuilds on it).
    pub fn view_rev(&self) -> u64 {
        self.view_rev
    }

    /// True for a vertex that only hidden triangles use.
    pub(crate) fn vertex_hidden(&self, v: u32) -> bool {
        if self.hidden.is_empty() {
            return false;
        }
        let ts = self.vertex_tris(v);
        !ts.is_empty() && ts.iter().all(|t| self.hidden.get(*t as usize).copied().unwrap_or(false))
    }

    /// Set the mask of every visible vertex inside a screen region to `value`.
    pub fn mask_region(&mut self, region: &ScreenRegion, value: f32) -> StrokeUndo {
        if !region.is_valid() || !value.is_finite() {
            return StrokeUndo::default();
        }
        let inside: Vec<bool> = (0..self.positions.len())
            .map(|v| {
                let (p, n) = (self.positions.get(v).copied().unwrap_or_default(), self.normals.get(v).copied().unwrap_or(Vec3::Y));
                region.contains(p, n) && !self.vertex_hidden(v as u32)
            })
            .collect();
        self.mask_edit(|i, _, m| if inside.get(i as usize).copied().unwrap_or(false) { value } else { m })
    }

    /// Clip to a plane: every visible vertex on the side the normal points to moves straight
    /// onto the plane, as far as its mask allows.
    pub fn clip_plane(&mut self, point: Vec3, normal: Vec3) -> StrokeUndo {
        let n = normal.normalize_or_zero();
        let mut u = StrokeUndo::default();
        if !point.is_finite() || n == Vec3::ZERO || self.stroke.is_some() {
            return u;
        }
        for v in 0..self.positions.len() {
            let (Some(p), Some(m)) = (self.positions.get(v).copied(), self.mask.get(v).copied()) else { continue };
            let d = (p - point).dot(n);
            let w = 1.0 - m.clamp(0.0, 1.0);
            if d <= 0.0 || w <= 0.0 || self.vertex_hidden(v as u32) {
                continue;
            }
            let q = p - n * d * w;
            if !q.is_finite() || q == p {
                continue;
            }
            if let Some(x) = self.positions.get_mut(v) {
                *x = q;
            }
            u.indices.push(v as u32);
            u.before.push(p);
            u.after.push(q);
            u.mask_before.push(m);
            u.mask_after.push(m);
        }
        self.update_normals(&u.indices);
        self.refit_around(&u.indices);
        u
    }

    /// Mask by how much of the sky each vertex cannot see: deep folds, the inside of holes and
    /// places where parts meet get masked, open surface stays clear. Rays per vertex follow
    /// [`OCCLUSION_RAY_BUDGET`] (between 4 and 24).
    pub fn mask_by_occlusion(&mut self) -> StrokeUndo {
        let n = self.positions.len();
        if n == 0 {
            return StrokeUndo::default();
        }
        let rays = (OCCLUSION_RAY_BUDGET / n).clamp(4, 24);
        let dirs = hemisphere(rays);
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &self.positions {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let lift = (hi - lo).length().max(1e-12) * 1e-4;
        let mut new = Vec::with_capacity(n);
        for v in 0..n {
            let (p, nrm) = (self.positions.get(v).copied().unwrap_or_default(), self.normals.get(v).copied().unwrap_or(Vec3::Y));
            // A frame around the normal, turned by a fixed amount per vertex so neighbours do
            // not all look the same way.
            let side = if nrm.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
            let (t, b) = (nrm.cross(side).normalize_or(Vec3::Z), nrm.cross(nrm.cross(side)).normalize_or(Vec3::X));
            let turn = (v as u32).wrapping_mul(2_654_435_761) as f32 / u32::MAX as f32 * std::f32::consts::TAU;
            let (s, c) = turn.sin_cos();
            let origin = p + nrm * lift;
            let mut hit = 0usize;
            for d in &dirs {
                let (x, y) = (d.x * c - d.y * s, d.x * s + d.y * c);
                let dir = t * x + b * y + nrm * d.z;
                if self.bvh.any_hit(&self.positions, &self.tris, origin, dir, self.drift, &self.hidden) {
                    hit += 1;
                }
            }
            new.push(hit as f32 / rays as f32);
        }
        self.mask_set(&new)
    }
}
