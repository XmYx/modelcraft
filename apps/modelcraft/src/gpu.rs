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
    pub roughness: f32,
}

pub struct GpuState {
    pub rs: egui_wgpu::RenderState,
    pub renderer: Renderer,
    targets: HashMap<usize, (Target, egui::TextureId)>,
    meshes: HashMap<usize, (Arc<PolyMesh>, GpuMesh, u64)>,
    sculpt: Option<(u64, u64, usize, GpuMesh)>,
    frame: u64,
    /// Time of the last sculpt upload, milliseconds (status bar and benchmarks).
    pub upload_ms: f32,
    pub uploaded_vertices: usize,
}

impl GpuState {
    pub fn new(rs: egui_wgpu::RenderState) -> Self {
        let renderer = Renderer::new(&rs.device);
        GpuState { rs, renderer, targets: HashMap::new(), meshes: HashMap::new(), sculpt: None, frame: 0, upload_ms: 0.0, uploaded_vertices: 0 }
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
        let g = GpuMesh::new(&self.rs.device, &buffers::mesh_data(m));
        self.meshes.insert(key, (m.clone(), g, f));
        key
    }

    /// Like [`GpuState::mesh`], with one normal per face (flat shading). The key is the mesh's
    /// plus one: allocations are aligned, so the two never collide.
    pub fn mesh_flat(&mut self, m: &Arc<PolyMesh>) -> usize {
        let key = (Arc::as_ptr(m) as usize) | 1;
        let f = self.frame;
        if let Some(e) = self.meshes.get_mut(&key) {
            e.2 = f;
            return key;
        }
        let g = GpuMesh::new(&self.rs.device, &buffers::flat_data(m));
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
                    roughness: s.roughness,
                })
            })
            .collect();
        renderer.render(&rs.device, &rs.queue, target, &Frame { view_proj, eye, clear, draws, lines, overlay });
        Some(*id)
    }

    /// Bring the sculpt mesh up to date: a full upload when the session changed, otherwise only
    /// the dirty vertex chunks.
    pub fn sync_sculpt(&mut self, s: &mut SculptSession) {
        let t0 = std::time::Instant::now();
        let n = s.mesh.vertex_count();
        let fresh = self.sculpt.as_ref().is_none_or(|(id, step, count, _)| *id != s.id.0 || *step != s.step_id || *count != n);
        if fresh {
            let data = buffers::shared_data(&s.mesh.positions, &s.mesh.normals, Some(&s.mesh.mask), s.mesh.triangles());
            let g = GpuMesh::new(&self.rs.device, &data);
            let _ = s.mesh.take_dirty_chunks();
            self.sculpt = Some((s.id.0, s.step_id, n, g));
            self.uploaded_vertices = n;
        } else if let Some((_, _, _, g)) = &self.sculpt {
            let mut total = 0;
            for (start, len) in s.mesh.take_dirty_chunks() {
                let verts: Vec<MeshVertex> =
                    (start..start + len).map(|i| buffers::vertex_at(&s.mesh.positions, &s.mesh.normals, Some(&s.mesh.mask), i)).collect();
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
