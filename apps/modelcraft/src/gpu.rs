//! The app side of the GPU path: offscreen targets shown as egui textures, a cache of uploaded
//! meshes, and the sculpt mesh with chunked updates.

use std::collections::HashMap;
use std::sync::Arc;

use modelcraft_engine::SculptSession;
use modelcraft_engine::mesh::PolyMesh;
use modelcraft_render::buffers::LineVertex;
use modelcraft_render::buffers::{self, MeshVertex};
use modelcraft_render::gpu::{DrawItem, Frame, GpuMesh, Renderer, Target};

/// One object to draw: a cached mesh (`key`) or the sculpt mesh (`None`).
pub struct DrawSpec {
    pub key: Option<usize>,
    pub model: glam::Mat4,
    pub color: [f32; 4],
    pub selected: bool,
    pub show_mask: bool,
    pub patch_tint: bool,
    pub uv_checker: bool,
    pub roughness: f32,
}

pub struct GpuState {
    pub rs: egui_wgpu::RenderState,
    pub renderer: Renderer,
    targets: HashMap<usize, (Target, egui::TextureId)>,
    meshes: HashMap<usize, (Arc<PolyMesh>, GpuMesh, u64)>,
    sculpt: Option<(u64, u64, usize, GpuMesh)>,
    /// The sculpt mesh's patch and visibility revision last uploaded, and the patch each vertex
    /// shows (empty while every triangle is in patch 0).
    sculpt_view: (u64, Vec<f32>),
    frame: u64,
    /// Time of the last sculpt upload, milliseconds (status bar and benchmarks).
    pub upload_ms: f32,
    pub uploaded_vertices: usize,
}

impl GpuState {
    pub fn new(rs: egui_wgpu::RenderState) -> Self {
        let renderer = Renderer::new(&rs.device);
        GpuState {
            rs,
            renderer,
            targets: HashMap::new(),
            meshes: HashMap::new(),
            sculpt: None,
            sculpt_view: (0, Vec::new()),
            frame: 0,
            upload_ms: 0.0,
            uploaded_vertices: 0,
        }
    }

    pub fn begin_frame(&mut self) {
        self.frame += 1;
        let f = self.frame;
        // Drop meshes no view drew for a while.
        self.meshes.retain(|_, (_, _, used)| f.saturating_sub(*used) < 30);
    }

    /// The target of a view, (re)created at this size, and its egui texture.
    pub fn target(&mut self, view: usize, size: [u32; 2]) -> Option<(&Target, egui::TextureId)> {
        let device = self.rs.device.clone();
        let stale = self.targets.get(&view).is_none_or(|(t, _)| t.size != size);
        if stale {
            let t = Target::new(&device, size);
            let mut r = self.rs.renderer.write();
            let id = match self.targets.get(&view) {
                Some((_, id)) => {
                    r.update_egui_texture_from_wgpu_texture(&device, &t.view, wgpu::FilterMode::Linear, *id);
                    *id
                }
                None => r.register_native_texture(&device, &t.view, wgpu::FilterMode::Linear),
            };
            drop(r);
            self.targets.insert(view, (t, id));
        }
        self.targets.get(&view).map(|(t, id)| (t, *id))
    }

    /// Upload (or reuse) an evaluated mesh. Keyed by the mesh's allocation, kept alive here so
    /// the key cannot be reused while cached.
    pub fn mesh(&mut self, m: &Arc<PolyMesh>) -> usize {
        let key = Arc::as_ptr(m) as usize;
        let f = self.frame;
        if let Some(e) = self.meshes.get_mut(&key) {
            e.2 = f;
            return key;
        }
        // A mesh not seen before: triangles, normals and the upload. A deforming mesh comes
        // through here every frame, so this is what playback of a skinned mesh pays.
        let t0 = std::time::Instant::now();
        let g = GpuMesh::new(&self.rs.device, &buffers::mesh_data(m));
        self.upload_ms = t0.elapsed().as_secs_f32() * 1000.0;
        self.uploaded_vertices = m.vertex_count();
        self.meshes.insert(key, (m.clone(), g, f));
        key
    }

    /// Render a view into its target; returns the texture to show.
    pub fn draw(
        &mut self,
        view: usize,
        size: [u32; 2],
        view_proj: glam::Mat4,
        eye: glam::Vec3,
        clear: [f32; 4],
        specs: &[DrawSpec],
        lines: &[LineVertex],
        overlay: &[LineVertex],
    ) -> Option<egui::TextureId> {
        self.target(view, size)?;
        let GpuState { renderer, targets, meshes, sculpt, rs, .. } = self;
        let (target, id) = targets.get(&view)?;
        let draws = specs
            .iter()
            .filter_map(|s| {
                let mesh = match s.key {
                    Some(k) => meshes.get(&k).map(|e| &e.1),
                    None => sculpt.as_ref().map(|x| &x.3),
                }?;
                Some(DrawItem {
                    mesh,
                    model: s.model,
                    color: s.color,
                    selected: s.selected,
                    show_mask: s.show_mask,
                    patch_tint: s.patch_tint,
                    uv_checker: s.uv_checker,
                    roughness: s.roughness,
                })
            })
            .collect();
        renderer.render(&rs.device, &rs.queue, target, &Frame { view_proj, eye, clear, draws, lines, overlay });
        Some(*id)
    }

    /// Bring the sculpt mesh up to date: a full upload when the session, its patches or what is
    /// hidden changed, otherwise only the dirty vertex chunks.
    pub fn sync_sculpt(&mut self, s: &mut SculptSession) {
        let t0 = std::time::Instant::now();
        let n = s.mesh.vertex_count();
        let fresh = self.sculpt.as_ref().is_none_or(|(id, step, count, _)| *id != s.id.0 || *step != s.step_id || *count != n)
            || self.sculpt_view.0 != s.mesh.view_rev();
        if fresh {
            let (indices, patch) = sculpt_view(&s.mesh);
            let mut data = buffers::shared_data(&s.mesh.positions, &s.mesh.normals, Some(&s.mesh.mask), &[]);
            for (v, p) in data.vertices.iter_mut().zip(&patch) {
                v.patch = *p;
            }
            data.indices = indices;
            let g = GpuMesh::new(&self.rs.device, &data);
            let _ = s.mesh.take_dirty_chunks();
            self.sculpt = Some((s.id.0, s.step_id, n, g));
            self.sculpt_view = (s.mesh.view_rev(), patch);
            self.uploaded_vertices = n;
        } else if let Some((_, _, _, g)) = &self.sculpt {
            let mut total = 0;
            let patch = &self.sculpt_view.1;
            for (start, len) in s.mesh.take_dirty_chunks() {
                let verts: Vec<MeshVertex> = (start..start + len)
                    .map(|i| MeshVertex {
                        patch: patch.get(i).copied().unwrap_or(0.0),
                        ..buffers::vertex_at(&s.mesh.positions, &s.mesh.normals, Some(&s.mesh.mask), i)
                    })
                    .collect();
                g.update(&self.rs.queue, start, &verts);
                total += len;
            }
            if total == 0 {
                return;
            }
            self.uploaded_vertices = total;
        }
        self.upload_ms = t0.elapsed().as_secs_f32() * 1000.0;
    }
}

/// The index list of the visible triangles and the patch each vertex shows. The shader takes a
/// triangle's patch from its first vertex, so every triangle is turned to start at a vertex
/// that shows its patch; where three patches meet at three shared vertices one triangle may
/// show its neighbour's tint.
pub fn sculpt_view(mesh: &modelcraft_engine::sculpt::SculptMesh) -> (Vec<u32>, Vec<f32>) {
    let (tris, patches, hidden) = (mesh.triangles(), mesh.triangle_patches(), mesh.hidden_triangles());
    let plain = patches.iter().all(|p| *p == 0);
    if plain && hidden.is_empty() {
        return (tris.iter().flatten().copied().collect(), Vec::new());
    }
    let mut claim: Vec<Option<u32>> = vec![None; if plain { 0 } else { mesh.vertex_count() }];
    let mut indices = Vec::with_capacity(tris.len() * 3);
    for (t, tri) in tris.iter().enumerate() {
        if hidden.get(t).copied().unwrap_or(false) {
            continue;
        }
        let p = patches.get(t).copied().unwrap_or(0);
        let owned = |k: &usize| tri.get(*k).and_then(|v| claim.get(*v as usize)).copied().flatten() == Some(p);
        let free = |k: &usize| tri.get(*k).and_then(|v| claim.get(*v as usize)).is_some_and(|c| c.is_none());
        let k = (0..3).find(owned).or_else(|| (0..3).find(free)).unwrap_or(0);
        if let Some(c) = tri.get(k).and_then(|v| claim.get_mut(*v as usize))
            && c.is_none()
        {
            *c = Some(p);
        }
        indices.extend((0..3).filter_map(|j| tri.get((k + j) % 3).copied()));
    }
    (indices, claim.iter().map(|c| c.unwrap_or(0) as f32).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_engine::ops::primitives::Primitive;
    use modelcraft_engine::sculpt::SculptMesh;

    #[test]
    fn the_first_vertex_of_a_triangle_carries_its_patch() {
        let m = Primitive::default_of("cube").and_then(|p| p.build().ok()).unwrap();
        let mut s = SculptMesh::new(&m, None, None);
        assert!(s.set_face_patches(&vec![0; m.face_count()]));
        let (idx, patch) = sculpt_view(&s);
        assert_eq!(idx.len(), s.triangle_count() * 3);
        assert!(patch.is_empty(), "no patches: nothing to carry");
        let faces: Vec<u32> = (0..m.face_count() as u32).collect();
        assert!(s.set_face_patches(&faces));
        let mut hidden = vec![false; m.face_count()];
        hidden[0] = true;
        assert!(s.set_face_hidden(&hidden));
        let (idx, patch) = sculpt_view(&s);
        let shown: Vec<usize> = (0..s.triangle_count()).filter(|t| !s.hidden_triangles()[*t]).collect();
        assert_eq!(idx.len(), shown.len() * 3);
        let mut right = 0;
        for (k, t) in shown.iter().enumerate() {
            let tri = s.triangles()[*t];
            let got = [idx[k * 3], idx[k * 3 + 1], idx[k * 3 + 2]];
            assert!((0..3).any(|r| (0..3).all(|j| got[j] == tri[(r + j) % 3])), "the triangle is only turned, never flipped");
            if patch[got[0] as usize] == s.triangle_patches()[*t] as f32 {
                right += 1;
            }
        }
        // A cube's eight corners are each shared by three patches, so not every triangle can
        // find a corner of its own; most do.
        assert!(right * 3 >= shown.len() * 2, "{right} of {}", shown.len());
    }
}
