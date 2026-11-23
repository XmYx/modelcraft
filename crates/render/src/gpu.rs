//! The wgpu draw path. See the crate documentation.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::buffers::{LineVertex, MeshData, MeshVertex};
use crate::hair_gpu::HairDraw;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;

/// Largest viewport target (per side).
pub const MAX_TARGET: u32 = 8192;

/// A mesh on the GPU.
pub struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    index_count: u32,
    pub vertex_count: usize,
}

impl GpuMesh {
    pub fn new(device: &wgpu::Device, data: &MeshData) -> GpuMesh {
        let verts: &[MeshVertex] = if data.vertices.is_empty() {
            &[MeshVertex { pos: [0.0; 3], normal: [0.0, 1.0, 0.0], mask: 0.0, patch: 0.0, uv: [0.0; 2], color: crate::buffers::WHITE }]
        } else {
            &data.vertices
        };
        let idx: &[u32] = if data.indices.is_empty() { &[0, 0, 0] } else { &data.indices };
        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modelcraft mesh vertices"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modelcraft mesh indices"),
            contents: bytemuck::cast_slice(idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        GpuMesh { vbuf, ibuf, index_count: data.indices.len() as u32, vertex_count: data.vertices.len() }
    }

    /// Overwrite vertices `first..first + verts.len()` (sculpt updates; out-of-range is ignored).
    pub fn update(&self, queue: &wgpu::Queue, first: usize, verts: &[MeshVertex]) {
        if verts.is_empty() || first.saturating_add(verts.len()) > self.vertex_count {
            return;
        }
        let offset = (first * std::mem::size_of::<MeshVertex>()) as u64;
        queue.write_buffer(&self.vbuf, offset, bytemuck::cast_slice(verts));
    }
}

/// Offscreen colour/depth target, resolved into `texture` for display.
pub struct Target {
    pub size: [u32; 2],
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl Target {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Target {
        let size = [size[0].clamp(1, MAX_TARGET), size[1].clamp(1, MAX_TARGET)];
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let tex = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = tex("modelcraft msaa", COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let depth = tex("modelcraft depth", DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let texture = tex(
            "modelcraft viewport",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Target {
            size,
            msaa: msaa.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            texture,
            view,
        }
    }
}

/// One mesh to draw.
pub struct DrawItem<'a> {
    pub mesh: &'a GpuMesh,
    pub model: Mat4,
    pub color: [f32; 4],
    pub selected: bool,
    pub show_mask: bool,
    pub patch_tint: bool,
    /// Draw a checker from the UVs instead of the colour (needs UVs on the mesh).
    pub uv_checker: bool,
    pub roughness: f32,
}

pub struct Frame<'a> {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub clear: [f32; 4],
    pub draws: Vec<DrawItem<'a>>,
    /// Line list in world space.
    pub lines: &'a [LineVertex],
    /// Triangle list in world space, alpha blended over the meshes (selected faces).
    pub overlay: &'a [LineVertex],
    /// Hair strands, drawn as depth-tested lines over the meshes.
    pub hair: Vec<HairDraw<'a>>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraU {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawU {
    model: [[f32; 4]; 4],
    color: [f32; 4],
    flags: [f32; 4],
}

pub struct Renderer {
    mesh_pipe: wgpu::RenderPipeline,
    line_pipe: wgpu::RenderPipeline,
    overlay_pipe: wgpu::RenderPipeline,
    hair_pipe: wgpu::RenderPipeline,
    cam_buf: wgpu::Buffer,
    cam_bg: wgpu::BindGroup,
    draw_layout: wgpu::BindGroupLayout,
    draw_buf: wgpu::Buffer,
    draw_bg: wgpu::BindGroup,
    draw_stride: u64,
    draw_cap: u64,
    line_buf: Option<(wgpu::Buffer, u64)>,
    overlay_buf: Option<(wgpu::Buffer, u64)>,
}

fn uniform_layout(device: &wgpu::Device, dynamic: bool, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: None },
            count: None,
        }],
    })
}

impl Renderer {
    pub fn new(device: &wgpu::Device) -> Renderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("modelcraft shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let cam_layout = uniform_layout(device, false, "camera layout");
        let draw_layout = uniform_layout(device, true, "draw layout");
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh layout"),
            bind_group_layouts: &[Some(&cam_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line layout"),
            bind_group_layouts: &[Some(&cam_layout)],
            immediate_size: 0,
        });
        let depth = |write: bool| wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(write),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let ms = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let mesh_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32, 4 => Float32x2, 5 => Float32x4];
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
        let mesh_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&mesh_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &mesh_attrs,
                })],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth(true)),
            multisample: ms,
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: COLOR_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let line_like = |label, topology, write| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&line_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_line"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<LineVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &line_attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(depth(write)),
                multisample: ms,
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_line"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let line_pipe = line_like("line pipeline", wgpu::PrimitiveTopology::LineList, false);
        let overlay_pipe = line_like("overlay pipeline", wgpu::PrimitiveTopology::TriangleList, false);
        let hair_pipe = crate::hair_gpu::pipeline(device, &shader, &mesh_layout, depth(true), ms, COLOR_FORMAT);
        let cam_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &cam_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: cam_buf.as_entire_binding() }],
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1));
        let draw_stride = (std::mem::size_of::<DrawU>() as u64).div_ceil(align) * align;
        let (draw_buf, draw_bg) = Self::draw_buffer(device, &draw_layout, draw_stride, 64);
        Renderer {
            mesh_pipe,
            line_pipe,
            overlay_pipe,
            hair_pipe,
            cam_buf,
            cam_bg,
            draw_layout,
            draw_buf,
            draw_bg,
            draw_stride,
            draw_cap: 64,
            line_buf: None,
            overlay_buf: None,
        }
    }

    fn draw_buffer(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, stride: u64, cap: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draws"),
            size: stride * cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draws"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64),
                }),
            }],
        });
        (buf, bg)
    }

    fn upload_lines(device: &wgpu::Device, queue: &wgpu::Queue, slot: &mut Option<(wgpu::Buffer, u64)>, data: &[LineVertex]) {
        if data.is_empty() {
            return;
        }
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let need = bytes.len() as u64;
        if slot.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two().max(4096);
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("lines"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            *slot = Some((buf, cap));
        }
        if let Some((buf, _)) = slot {
            queue.write_buffer(buf, 0, bytes);
        }
    }

    /// Render a frame into `target` and submit it.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, target: &Target, frame: &Frame<'_>) {
        let cam = CameraU { view_proj: frame.view_proj.to_cols_array_2d(), eye: frame.eye.extend(1.0).to_array() };
        queue.write_buffer(&self.cam_buf, 0, bytemuck::bytes_of(&cam));
        let n = (frame.draws.len() + frame.hair.len()) as u64;
        if n > self.draw_cap {
            self.draw_cap = n.next_power_of_two();
            let (b, g) = Self::draw_buffer(device, &self.draw_layout, self.draw_stride, self.draw_cap);
            self.draw_buf = b;
            self.draw_bg = g;
        }
        if n > 0 {
            let mut bytes = vec![0u8; (self.draw_stride * n) as usize];
            let uniforms = frame.draws.iter().map(|d| DrawU {
                model: d.model.to_cols_array_2d(),
                color: d.color,
                flags: [
                    f32::from(u8::from(d.selected)),
                    f32::from(u8::from(d.show_mask)),
                    if d.uv_checker { 2.0 } else { f32::from(u8::from(d.patch_tint)) },
                    d.roughness,
                ],
            });
            let hair_uniforms = frame.hair.iter().map(|h| DrawU { model: h.model.to_cols_array_2d(), color: h.color, flags: [0.0; 4] });
            for (i, u) in uniforms.chain(hair_uniforms).enumerate() {
                let at = i * self.draw_stride as usize;
                if let Some(slot) = bytes.get_mut(at..at + std::mem::size_of::<DrawU>()) {
                    slot.copy_from_slice(bytemuck::bytes_of(&u));
                }
            }
            queue.write_buffer(&self.draw_buf, 0, &bytes);
        }
        Self::upload_lines(device, queue, &mut self.line_buf, frame.lines);
        Self::upload_lines(device, queue, &mut self.overlay_buf, frame.overlay);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("modelcraft viewport") });
        {
            let [r, g, b, a] = frame.clear.map(f64::from);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.msaa,
                    depth_slice: None,
                    resolve_target: Some(&target.view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.cam_bg, &[]);
            pass.set_pipeline(&self.mesh_pipe);
            for (i, d) in frame.draws.iter().enumerate() {
                if d.mesh.index_count == 0 {
                    continue;
                }
                pass.set_bind_group(1, &self.draw_bg, &[(i as u64 * self.draw_stride) as u32]);
                pass.set_vertex_buffer(0, d.mesh.vbuf.slice(..));
                pass.set_index_buffer(d.mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..d.mesh.index_count, 0, 0..1);
            }
            if !frame.hair.is_empty() {
                pass.set_pipeline(&self.hair_pipe);
                for (i, h) in frame.hair.iter().enumerate() {
                    pass.set_bind_group(1, &self.draw_bg, &[((frame.draws.len() + i) as u64 * self.draw_stride) as u32]);
                    h.hair.draw(&mut pass, h.fraction);
                }
            }
            if let Some((buf, _)) = &self.overlay_buf
                && !frame.overlay.is_empty()
            {
                pass.set_pipeline(&self.overlay_pipe);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..frame.overlay.len() as u32, 0..1);
            }
            if let Some((buf, _)) = &self.line_buf
                && !frame.lines.is_empty()
            {
                pass.set_pipeline(&self.line_pipe);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..frame.lines.len() as u32, 0..1);
            }
        }
        queue.submit([enc.finish()]);
    }

    /// Read the target back as RGBA8 rows (screenshots, tests). Blocks until the GPU is done.
    pub fn readback(device: &wgpu::Device, queue: &wgpu::Queue, target: &Target) -> Option<Vec<u8>> {
        let [w, h] = target.size;
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let s = (y * row) as usize;
            out.extend_from_slice(data.get(s..s + (w * 4) as usize)?);
        }
        Some(out)
    }
}

/// A headless device for tests, benchmarks and the CLI; `None` without a usable adapter.
pub fn headless() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(
        instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() }),
    )
    .ok()?;
    let name = adapter.get_info().name;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some((device, queue, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use modelcraft_mesh::{PolyMesh, Polygon};

    #[test]
    fn renders_a_quad_when_a_gpu_is_present() {
        // CI machines without an adapter skip the GPU half (the CPU data path is tested in buffers).
        let Some((device, queue, _)) = headless() else { return };
        let m = PolyMesh::build(
            vec![Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(-1.0, 1.0, 0.0)],
            vec![Polygon::new(vec![0, 1, 2, 3])],
        )
        .unwrap()
        .0;
        let mesh = GpuMesh::new(&device, &crate::buffers::mesh_data(&m));
        let target = Target::new(&device, [64, 64]);
        let mut r = Renderer::new(&device);
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 3.0, ..Default::default() };
        let frame = Frame {
            view_proj: cam.view_proj(1.0),
            eye: cam.eye(),
            clear: [0.0, 0.0, 0.0, 1.0],
            draws: vec![DrawItem {
                mesh: &mesh,
                model: Mat4::IDENTITY,
                color: [0.8, 0.8, 0.8, 1.0],
                selected: false,
                show_mask: false,
                patch_tint: false,
                uv_checker: false,
                roughness: 0.5,
            }],
            lines: &[],
            overlay: &[],
            hair: Vec::new(),
        };
        r.render(&device, &queue, &target, &frame);
        let px = Renderer::readback(&device, &queue, &target).unwrap();
        let centre = (32 * 64 + 32) * 4;
        assert!(px[centre] > 40, "centre pixel is background");
        assert_eq!(px[0], 0, "corner should be background");
    }

    #[test]
    fn a_uv_checker_draws_two_tones_when_a_gpu_is_present() {
        let Some((device, queue, _)) = headless() else { return };
        let mut m = PolyMesh::build(
            vec![Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(-1.0, 1.0, 0.0)],
            vec![Polygon::new(vec![0, 1, 2, 3])],
        )
        .unwrap()
        .0;
        m.set_uvs(&[glam::Vec2::new(0.0, 1.0), glam::Vec2::new(1.0, 1.0), glam::Vec2::new(1.0, 0.0), glam::Vec2::new(0.0, 0.0)]).unwrap();
        let mesh = GpuMesh::new(&device, &crate::buffers::mesh_data(&m));
        let target = Target::new(&device, [128, 128]);
        let mut r = Renderer::new(&device);
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 3.0, ..Default::default() };
        let mut shoot = |checker: bool| {
            let frame = Frame {
                view_proj: cam.view_proj(1.0),
                eye: cam.eye(),
                clear: [0.0, 0.0, 0.0, 1.0],
                draws: vec![DrawItem {
                    mesh: &mesh,
                    model: Mat4::IDENTITY,
                    color: [0.8, 0.8, 0.8, 1.0],
                    selected: false,
                    show_mask: false,
                    patch_tint: false,
                    uv_checker: checker,
                    roughness: 0.5,
                }],
                lines: &[],
                overlay: &[],
                hair: Vec::new(),
            };
            r.render(&device, &queue, &target, &frame);
            Renderer::readback(&device, &queue, &target).unwrap()
        };
        let flat = shoot(false);
        let checked = shoot(true);
        // Sample a row through the quad: without the checker it is one tone, with it two.
        let row = |px: &[u8]| -> Vec<u8> { (40..90).map(|x| px[(64 * 128 + x) * 4]).collect() };
        let spread = |v: &[u8]| v.iter().max().copied().unwrap_or(0) - v.iter().min().copied().unwrap_or(0);
        assert!(spread(&row(&flat)) < 25, "{:?}", row(&flat));
        assert!(spread(&row(&checked)) > 60, "{:?}", row(&checked));
    }
}
