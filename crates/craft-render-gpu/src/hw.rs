//! Hardware ray queries, behind the `hw-ray-query` cargo feature and the runtime option
//! [`crate::GpuOptions::hardware_rays`] (both off by default).
//!
//! wgpu 30 offers ray queries (acceleration structures built by the GPU and `rayQuery*` calls in
//! compute shaders) as an *experimental* feature. Enabling any experimental feature needs
//! `ExperimentalFeatures::enabled()`, an `unsafe fn` whose contract is only that the caller
//! accepts that wgpu may still have bugs in these APIs. This module holds that one call, and it
//! is the only place in the crate with `#[allow(unsafe_code)]` (named as an exception in
//! AGENTS.md and docs/architecture.md); the default build does not compile it and the crate then
//! forbids `unsafe`.
//!
//! The backend answers the same two queries as the software traversal (`trace_closest` and
//! `trace_any`, through `intersect` in `bvh_rayquery.wgsl`); the triangles are the ones the
//! software BVH holds, in the same slot order, so hits mean the same thing. A scene with cut-outs
//! (which need a decision per candidate hit) or one too large for the device is left to the
//! software path, as is a device without the feature.

use craft_render_core::Scene;
use wgpu::util::DeviceExt;

use crate::{GpuContext, GpuError};

/// The one place `unsafe` appears in the crate.
#[allow(unsafe_code)]
fn experimental_features() -> wgpu::ExperimentalFeatures {
    // SAFETY: `enabled` records that this program accepts that wgpu's experimental features may
    // contain bugs that wgpu has not fixed yet. This function runs only when the opt-in
    // `hw-ray-query` cargo feature is built and the user asks for hardware rays; the backend is
    // checked against the software path by `hw_tests`, and every failure to create the
    // structures falls back to software traversal.
    unsafe { wgpu::ExperimentalFeatures::enabled() }
}

impl GpuContext {
    /// Open a device that can run ray queries, or `None` when no adapter has them.
    pub fn new_hardware() -> Option<GpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        for adapter in adapters {
            let info = adapter.get_info();
            if info.device_type == wgpu::DeviceType::Cpu || !adapter.features().contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
                continue;
            }
            let have = adapter.limits();
            let limits = wgpu::Limits {
                max_buffer_size: have.max_buffer_size,
                max_storage_buffer_binding_size: have.max_storage_buffer_binding_size,
                max_storage_buffers_per_shader_stage: have.max_storage_buffers_per_shader_stage.clamp(8, 12),
                max_blas_primitive_count: have.max_blas_primitive_count,
                max_blas_geometry_count: have.max_blas_geometry_count,
                max_tlas_instance_count: have.max_tlas_instance_count,
                max_acceleration_structures_per_shader_stage: have.max_acceleration_structures_per_shader_stage,
                ..wgpu::Limits::default()
            };
            let opened = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("craft-render hardware rays"),
                required_features: wgpu::Features::EXPERIMENTAL_RAY_QUERY,
                required_limits: limits,
                experimental_features: experimental_features(),
                ..Default::default()
            }));
            if let Ok((device, queue)) = opened {
                let mut ctx = GpuContext::from_device(device, queue, &info);
                ctx.caps.hardware_rays = true;
                ctx.caps.max_blas_primitives = have.max_blas_primitive_count;
                return Some(ctx);
            }
        }
        None
    }
}

/// The scene's triangles as a bottom-level and a top-level acceleration structure.
pub(crate) struct Accel {
    pub tlas: wgpu::Tlas,
    _blas: wgpu::Blas,
    _vertices: wgpu::Buffer,
}

/// Build the structures for `scene`'s triangles (slot order), or say why not.
pub(crate) fn build(ctx: &GpuContext, scene: &Scene) -> Result<Accel, GpuError> {
    let tris = &scene.accel.tris;
    let n = tris.len();
    if n == 0 {
        return Err(GpuError::Unsupported("hardware ray queries on an empty scene"));
    }
    if scene.has_cutouts {
        return Err(GpuError::Unsupported("hardware ray queries on a scene with cut-outs"));
    }
    if n as u64 > u64::from(ctx.caps.max_blas_primitives) || (n as u64) * 3 > u64::from(u32::MAX) {
        return Err(GpuError::TooLarge { what: "acceleration structure primitives", needed: n as u64, allowed: u64::from(ctx.caps.max_blas_primitives) });
    }
    let mut vertices: Vec<[f32; 3]> = Vec::with_capacity(n * 3);
    for t in tris {
        vertices.extend([t.p0.to_array(), (t.p0 + t.e1).to_array(), (t.p0 + t.e2).to_array()]);
    }
    crate::scoped(ctx, || {
        let device = &ctx.device;
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hardware rays: vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::BLAS_INPUT,
        });
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: vertices.len() as u32,
            index_format: None,
            index_count: None,
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor {
                label: Some("scene"),
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            },
            wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: vec![size.clone()] },
        );
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some("scene"),
            max_instances: 1,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        tlas[0] = Some(wgpu::TlasInstance::new(&blas, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], 0, 0xff));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("hardware rays: build") });
        enc.build_acceleration_structures(
            [&wgpu::BlasBuildEntry {
                blas: &blas,
                geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                    size: &size,
                    vertex_buffer: &buffer,
                    first_vertex: 0,
                    vertex_stride: 12,
                    index_buffer: None,
                    first_index: None,
                    transform_buffer: None,
                    transform_buffer_offset: None,
                }]),
            }],
            [&tlas],
        );
        ctx.queue.submit([enc.finish()]);
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Device(e.to_string()))?;
        Ok(Accel { tlas, _blas: blas, _vertices: buffer })
    })?
}
