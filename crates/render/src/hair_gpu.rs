//! Hair strands on the GPU: one position buffer and one index buffer of line strips, drawn in a
//! single call.
//!
//! Strands are stored in random order, so the first `n` of them are a uniform random subset of the
//! groom. A percentage display therefore draws a prefix of the index buffer: changing the percentage
//! costs nothing and never re-uploads. Each strand is `points_per_strand` indices followed by a
//! primitive-restart index.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

/// The index that ends a line strip.
pub const RESTART: u32 = u32::MAX;

/// Hair on the GPU.
pub struct GpuHair {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    /// Strands uploaded (fewer than offered when the device's buffer limit cut the list).
    pub strands: usize,
    pub points_per_strand: usize,
}

/// One hair draw: the uploaded strands, the emitter's world matrix, a colour, and the share of the
/// strands to draw (0 to 1, a prefix).
pub struct HairDraw<'a> {
    pub hair: &'a GpuHair,
    pub model: Mat4,
    pub color: [f32; 4],
    pub fraction: f32,
}

/// How many of `strands` strands of `points_per_strand` points fit in buffers of at most
/// `max_buffer` bytes (positions are 12 bytes, indices 4).
pub fn fit_strands(max_buffer: u64, strands: usize, points_per_strand: usize) -> usize {
    let per_vertices = points_per_strand as u64 * 12;
    let per_indices = (points_per_strand as u64 + 1) * 4;
    let per = per_vertices.max(per_indices).max(1);
    let fit = (max_buffer / per) as usize;
    strands.min(fit)
}

/// The index list for `strands` strands: each strand's points, then [`RESTART`].
pub fn index_data(strands: usize, points_per_strand: usize) -> Vec<u32> {
    let mut idx = Vec::with_capacity(strands * (points_per_strand + 1));
    for s in 0..strands {
        let base = (s * points_per_strand) as u32;
        idx.extend(base..base + points_per_strand as u32);
        idx.push(RESTART);
    }
    idx
}

impl GpuHair {
    /// Upload strands (`points` is `points_per_strand` points per strand, back to back). `None` for
    /// nothing to draw.
    pub fn new(device: &wgpu::Device, points: &[Vec3], points_per_strand: usize) -> Option<GpuHair> {
        if points_per_strand < 2 || points.len() < points_per_strand {
            return None;
        }
        let offered = points.len() / points_per_strand;
        let strands = fit_strands(device.limits().max_buffer_size, offered, points_per_strand);
        if strands == 0 {
            return None;
        }
        let used = strands * points_per_strand;
        // Written straight into the mapped buffer: no second copy of millions of points.
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modelcraft hair points"),
            size: used as u64 * 12,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        {
            let mut view = vbuf.slice(..).get_mapped_range_mut().ok()?;
            // A write-only view: fill it in chunks of 64k points.
            let mut chunk = Vec::with_capacity(65_536 * 12);
            let mut at = 0usize;
            for run in points.get(..used)?.chunks(65_536) {
                chunk.clear();
                for p in run {
                    chunk.extend_from_slice(bytemuck::bytes_of(&p.to_array()));
                }
                view.slice(at..at + chunk.len()).copy_from_slice(&chunk);
                at += chunk.len();
            }
        }
        vbuf.unmap();
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modelcraft hair strips"),
            contents: bytemuck::cast_slice(&index_data(strands, points_per_strand)),
            usage: wgpu::BufferUsages::INDEX,
        });
        Some(GpuHair { vbuf, ibuf, strands, points_per_strand })
    }

    /// Bytes of GPU memory held.
    pub fn bytes(&self) -> u64 {
        (self.strands * self.points_per_strand) as u64 * 12 + (self.strands * (self.points_per_strand + 1)) as u64 * 4
    }

    /// Strands a draw of this fraction covers (at least one when there is any).
    pub fn strands_for(&self, fraction: f32) -> usize {
        let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 1.0 };
        ((self.strands as f64 * f64::from(f)).ceil() as usize).min(self.strands)
    }

    pub(crate) fn draw<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, fraction: f32) {
        let n = self.strands_for(fraction);
        if n == 0 {
            return;
        }
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..(n * (self.points_per_strand + 1)) as u32, 0, 0..1);
    }
}

/// The pipeline: line strips with primitive restart, position only; colour and model matrix come
/// from the per-draw uniform the meshes use.
pub(crate) fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    depth: wgpu::DepthStencilState,
    multisample: wgpu::MultisampleState,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let attrs = wgpu::vertex_attr_array![0 => Float32x3];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hair pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_hair"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineStrip,
            strip_index_format: Some(wgpu::IndexFormat::Uint32),
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(depth),
        multisample,
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_line"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_lists_are_strips_with_restarts() {
        assert_eq!(index_data(2, 3), vec![0, 1, 2, RESTART, 3, 4, 5, RESTART]);
        assert!(index_data(0, 4).is_empty());
    }

    #[test]
    fn the_buffer_limit_cuts_the_strand_list_not_the_strands() {
        // 8 points: 96 bytes of positions and 36 of indices per strand, so 96 decides.
        assert_eq!(fit_strands(960, 100, 8), 10);
        assert_eq!(fit_strands(u64::MAX, 100, 8), 100);
        assert_eq!(fit_strands(0, 100, 8), 0);
        // Two points per strand: the indices (12 bytes) are not the larger.
        assert_eq!(fit_strands(240, 100, 2), 10);
    }

    #[test]
    fn a_fraction_is_a_prefix() {
        let Some((device, _, _)) = crate::gpu::headless() else { return };
        let pts: Vec<Vec3> = (0..40).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
        let h = GpuHair::new(&device, &pts, 4).unwrap();
        assert_eq!(h.strands, 10);
        assert_eq!((h.strands_for(1.0), h.strands_for(0.5), h.strands_for(0.01), h.strands_for(0.0), h.strands_for(f32::NAN)), (10, 5, 1, 0, 10));
        assert_eq!(h.bytes(), 40 * 12 + 10 * 5 * 4);
        assert!(GpuHair::new(&device, &pts, 1).is_none() && GpuHair::new(&device, &[], 4).is_none());
    }

    #[test]
    fn hair_is_drawn_and_a_fraction_draws_less() {
        use crate::camera::Camera;
        use crate::gpu::{Frame, Renderer, Target};
        let Some((device, queue, _)) = crate::gpu::headless() else { return };
        // 200 strands of 4 points standing on the xz plane in front of the camera.
        let mut pts = Vec::new();
        for s in 0..200 {
            let (x, z) = ((s % 20) as f32 * 0.1 - 1.0, (s / 20) as f32 * 0.1 - 0.5);
            for k in 0..4 {
                pts.push(Vec3::new(x, k as f32 * 0.25 - 0.4, z));
            }
        }
        let hair = GpuHair::new(&device, &pts, 4).unwrap();
        let target = Target::new(&device, [96, 96]);
        let mut r = Renderer::new(&device);
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 4.0, ..Default::default() };
        let mut lit = |fraction: f32| {
            let frame = Frame {
                view_proj: cam.view_proj(1.0),
                eye: cam.eye(),
                clear: [0.0, 0.0, 0.0, 1.0],
                draws: vec![],
                lines: &[],
                overlay: &[],
                hair: vec![HairDraw { hair: &hair, model: Mat4::IDENTITY, color: [1.0, 1.0, 1.0, 1.0], fraction }],
            };
            r.render(&device, &queue, &target, &frame);
            let px = Renderer::readback(&device, &queue, &target).unwrap();
            px.chunks(4).filter(|p| p[0] > 20).count()
        };
        let all = lit(1.0);
        let half = lit(0.5);
        let none = lit(0.0);
        assert!(all > 20, "the strands show: {all} pixels");
        assert!(half < all && half > 0, "a fraction draws fewer: {half} of {all}");
        assert_eq!(none, 0);
    }
}
