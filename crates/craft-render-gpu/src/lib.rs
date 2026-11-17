//! The Craft render engine on a GPU.
//!
//! [`GpuRenderer`] runs the path tracer of `craft-render-core` as a wgpu compute pipeline: the
//! same BVH bytes, the same sample sequence, the same surface model and estimator, so its images
//! converge to the CPU reference (the tests compare them).
//!
//! It follows RFC-0022 (the shared GPU layer) ahead of the shared `craft-gpu` crates:
//! - capabilities ([`Caps`]) are read from the *created device*, never from the adapter;
//! - no adapter is a defined answer: [`GpuContext::new`] returns `None` and callers use the CPU
//!   renderer;
//! - a scene that does not fit the device's limits is an error ([`GpuError::TooLarge`]), and
//!   validation or device errors come back as [`GpuError::Device`]; nothing here panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use bytemuck::{Pod, Zeroable};
use craft_render_core::bsdf::AlbedoTable;
use craft_render_core::integrator::{MAX_DEPTH, Strategy};
use craft_render_core::render::{MAX_SAMPLES, RenderSettings};
use craft_render_core::scene::Light;
use craft_render_core::{Film, Scene};
use wgpu::util::DeviceExt;

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("the scene needs a {what} of {needed} bytes; this GPU allows {allowed}")]
    TooLarge { what: &'static str, needed: u64, allowed: u64 },
    #[error("the image size {0}×{1} is not renderable")]
    BadImageSize(u32, u32),
    #[error("the GPU path does not render {0} yet; the CPU path does")]
    Unsupported(&'static str),
    #[error("GPU error: {0}")]
    Device(String),
}

/// What the created device can do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caps {
    pub adapter: String,
    pub backend: String,
    /// A software rasterizer (for example Mesa's lavapipe) rather than hardware.
    pub software: bool,
    pub max_buffer_size: u64,
    pub max_storage_buffer_binding_size: u64,
    pub max_compute_workgroups_per_dimension: u32,
}

/// A device and queue with their capabilities.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub caps: Caps,
}

impl GpuContext {
    /// Open the most capable adapter without a window. `None` when there is none (the caller
    /// renders on the CPU instead).
    pub fn new() -> Option<GpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        // Every adapter, best first: hardware before software (Mesa's lavapipe on CI runners is
        // still a real backend that compiles the shader and runs the kernel), and Vulkan, Metal
        // or DX12 before GL. The first one that opens a device wins: an adapter that is listed
        // but cannot open one (a GL context without a display, say) is passed over.
        let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
        let rank = |a: &wgpu::Adapter| {
            let info = a.get_info();
            let kind = match info.device_type {
                wgpu::DeviceType::DiscreteGpu => 0,
                wgpu::DeviceType::IntegratedGpu => 1,
                wgpu::DeviceType::VirtualGpu => 2,
                wgpu::DeviceType::Other => 3,
                wgpu::DeviceType::Cpu => 4,
            };
            (kind, u8::from(info.backend == wgpu::Backend::Gl))
        };
        adapters.sort_by_key(rank);
        for adapter in adapters {
            let info = adapter.get_info();
            let have = adapter.limits();
            // Ask for the adapter's buffer sizes (large scenes) on top of the portable defaults;
            // if that is refused, take the defaults.
            let wanted = wgpu::Limits {
                max_buffer_size: have.max_buffer_size,
                max_storage_buffer_binding_size: have.max_storage_buffer_binding_size,
                ..wgpu::Limits::default()
            };
            let open = |limits: wgpu::Limits| {
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("craft-render"),
                    required_limits: limits,
                    ..Default::default()
                }))
                .ok()
            };
            if let Some((device, queue)) = open(wanted).or_else(|| open(wgpu::Limits::default())) {
                return Some(GpuContext::from_device(device, queue, &info));
            }
            log::debug!("craft-render: {} ({:?}) is listed but did not open a device", info.name, info.backend);
        }
        None
    }

    /// Wrap a device the application already has (the viewport's, for example).
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, info: &wgpu::AdapterInfo) -> GpuContext {
        let limits = device.limits();
        let caps = Caps {
            adapter: info.name.clone(),
            backend: format!("{:?}", info.backend),
            software: info.device_type == wgpu::DeviceType::Cpu,
            max_buffer_size: limits.max_buffer_size,
            max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
            max_compute_workgroups_per_dimension: limits.max_compute_workgroups_per_dimension,
        };
        GpuContext { device, queue, caps }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    cam_origin: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_forward: [f32; 4],
    env: [f32; 4],
    size: [u32; 2],
    first_sample: u32,
    samples: u32,
    seed: u32,
    max_depth: u32,
    roulette_start: u32,
    light_count: u32,
    ortho: u32,
    node_count: u32,
    clamp_indirect: f32,
    strategy: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuTri {
    p0: [f32; 3],
    pad0: u32,
    e1: [f32; 3],
    pad1: u32,
    e2: [f32; 3],
    pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuAttr {
    n0: [f32; 3],
    material: u32,
    n1: [f32; 3],
    light: i32,
    n2: [f32; 3],
    pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMaterial {
    base: [f32; 3],
    metalness: f32,
    tint: [f32; 3],
    roughness: f32,
    d0: f32,
    d90: f32,
    ior: f32,
    pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuLight {
    p0: [f32; 3],
    kind: u32,
    e1: [f32; 3],
    cdf: f32,
    e2: [f32; 3],
    area: f32,
    radiance: [f32; 3],
    pmf: f32,
}

const KIND_TRI: u32 = 0;
const KIND_TRI_TWO_SIDED: u32 = 1;
const KIND_POINT: u32 = 2;
const KIND_DIRECTIONAL: u32 = 3;
const KIND_ENVIRONMENT: u32 = 4;

/// Samples per pixel times pixels one dispatch may cover, so a single submission stays short
/// enough for the driver's watchdog and for the interface to stay responsive.
const WORK_PER_DISPATCH: u64 = 1 << 26;

/// A progressive render on the GPU. Create one per scene and image size.
pub struct GpuRenderer {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    params_buf: wgpu::Buffer,
    accum: wgpu::Buffer,
    params: Params,
    samples: u32,
}

/// The first feature of a material that only the CPU path renders so far.
fn unsupported(m: &craft_render_core::Material) -> Option<&'static str> {
    let white = m.specular_color == glam::Vec3::ONE;
    if m.coat_weight > 0.0 {
        Some("coats")
    } else if m.fuzz_weight > 0.0 {
        Some("fuzz")
    } else if m.is_transmissive() {
        Some("transmission")
    } else if m.thin_film_weight > 0.0 && m.base_metalness < 1.0 {
        Some("thin films")
    } else if m.specular_roughness_anisotropy > 0.0 {
        Some("anisotropic highlights")
    } else if m.base_diffuse_roughness > 0.0 && m.base_metalness < 1.0 {
        Some("rough diffuse bases")
    } else if m.base_metalness > 0.0 && !white {
        Some("tinted metal edges")
    } else if m.has_cutouts() {
        Some("cut-outs")
    } else if m.base_color_texture.is_some()
        || m.roughness_texture.is_some()
        || m.metalness_texture.is_some()
        || m.normal_texture.is_some()
        || m.bump_texture.is_some()
    {
        Some("textures")
    } else {
        None
    }
}

fn storage<T: Pod>(ctx: &GpuContext, label: &'static str, data: &[T], fallback: T) -> Result<wgpu::Buffer, GpuError> {
    let one = [fallback];
    let data = if data.is_empty() { &one[..] } else { data };
    let bytes: &[u8] = bytemuck::cast_slice(data);
    let needed = bytes.len() as u64;
    let allowed = ctx.caps.max_storage_buffer_binding_size.min(ctx.caps.max_buffer_size);
    if needed > allowed {
        return Err(GpuError::TooLarge { what: label, needed, allowed });
    }
    Ok(ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage: wgpu::BufferUsages::STORAGE }))
}

fn scoped<T>(ctx: &GpuContext, f: impl FnOnce() -> T) -> Result<T, GpuError> {
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let oom = ctx.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let out = f();
    let e1 = pollster::block_on(oom.pop());
    let e2 = pollster::block_on(scope.pop());
    match e1.or(e2) {
        Some(e) => Err(GpuError::Device(e.to_string())),
        None => Ok(out),
    }
}

impl GpuRenderer {
    /// Upload `scene` and prepare to render it at the size in `settings`.
    pub fn new(ctx: &GpuContext, scene: &Scene, settings: RenderSettings) -> Result<GpuRenderer, GpuError> {
        let (w, h) = (settings.width, settings.height);
        let max_side = ctx.caps.max_compute_workgroups_per_dimension.saturating_mul(8).min(craft_render_core::render::MAX_IMAGE_SIDE);
        if w == 0 || h == 0 || w > max_side || h > max_side {
            return Err(GpuError::BadImageSize(w, h));
        }
        if let Some(feature) = scene.materials.iter().find_map(unsupported) {
            return Err(GpuError::Unsupported(feature));
        }
        let order = &scene.accel.bvh.prim_order;
        let lights = &scene.lights;
        let emissive_count = lights.emissive.len();
        let tris: Vec<GpuTri> =
            scene.accel.tris.iter().map(|t| GpuTri { p0: t.p0.to_array(), pad0: 0, e1: t.e1.to_array(), pad1: 0, e2: t.e2.to_array(), pad2: 0 }).collect();
        let attrs: Vec<GpuAttr> = order
            .iter()
            .map(|i| {
                let n = scene.normals.get(*i as usize).copied().unwrap_or([glam::Vec3::Z; 3]);
                GpuAttr {
                    n0: n[0].to_array(),
                    material: scene.tri_material.get(*i as usize).copied().unwrap_or(0),
                    n1: n[1].to_array(),
                    light: lights.emissive_slot(*i).map_or(-1, |s| s as i32),
                    n2: n[2].to_array(),
                    pad: 0,
                }
            })
            .collect();
        let materials: Vec<GpuMaterial> = scene
            .materials
            .iter()
            .map(|m| {
                let r = (m.specular_ior - 1.0) / (m.specular_ior + 1.0);
                GpuMaterial {
                    base: (m.base_color * m.base_weight).to_array(),
                    metalness: m.base_metalness,
                    tint: m.specular_color.to_array(),
                    roughness: m.specular_roughness,
                    d0: r * r * m.specular_weight,
                    d90: m.specular_weight,
                    ior: m.specular_ior,
                    pad: 0.0,
                }
            })
            .collect();
        let cdf = lights.selection_cdf();
        let mut entries: Vec<GpuLight> = Vec::with_capacity(cdf.len());
        for (k, e) in lights.emissive.iter().enumerate() {
            let t = scene.positions.get(e.tri as usize).copied().unwrap_or([glam::Vec3::ZERO; 3]);
            entries.push(GpuLight {
                p0: t[0].to_array(),
                kind: if e.two_sided { KIND_TRI_TWO_SIDED } else { KIND_TRI },
                e1: (t[1] - t[0]).to_array(),
                cdf: cdf.get(k).copied().unwrap_or(1.0),
                e2: (t[2] - t[0]).to_array(),
                area: e.area,
                radiance: e.radiance.to_array(),
                pmf: lights.pmf(k),
            });
        }
        for (j, l) in lights.analytic.iter().enumerate() {
            let k = emissive_count + j;
            let (kind, p0, radiance) = match *l {
                Light::Point { position, intensity } => (KIND_POINT, position, intensity),
                Light::Directional { direction, irradiance } => (KIND_DIRECTIONAL, direction.normalize_or_zero(), irradiance),
            };
            entries.push(GpuLight {
                p0: p0.to_array(),
                kind,
                e1: [0.0; 3],
                cdf: cdf.get(k).copied().unwrap_or(1.0),
                e2: [0.0; 3],
                area: 0.0,
                radiance: radiance.to_array(),
                pmf: lights.pmf(k),
            });
        }
        if entries.len() < cdf.len() {
            let k = cdf.len() - 1;
            entries.push(GpuLight {
                p0: [0.0; 3],
                kind: KIND_ENVIRONMENT,
                e1: [0.0; 3],
                cdf: 1.0,
                e2: [0.0; 3],
                area: 0.0,
                radiance: lights.environment.to_array(),
                pmf: lights.pmf(k),
            });
        }
        let light_count = entries.len() as u32;

        let nodes_buf = storage(ctx, "bvh nodes", &scene.accel.bvh.nodes, Zeroable::zeroed())?;
        let tris_buf = storage(ctx, "triangles", &tris, Zeroable::zeroed())?;
        let attrs_buf = storage(ctx, "triangle attributes", &attrs, Zeroable::zeroed())?;
        let mats_buf = storage(ctx, "materials", &materials, Zeroable::zeroed())?;
        let lights_buf = storage(ctx, "lights", &entries, Zeroable::zeroed())?;
        let table_buf = storage(ctx, "albedo table", &AlbedoTable::get().ab, [0.0f32; 4])?;
        let accum_size = u64::from(w) * u64::from(h) * 16;
        let allowed = ctx.caps.max_storage_buffer_binding_size.min(ctx.caps.max_buffer_size);
        if accum_size > allowed {
            return Err(GpuError::TooLarge { what: "image", needed: accum_size, allowed });
        }

        let cam = scene.camera.prepare(w as f32 / h as f32).data();
        let with = |v: [f32; 3], w: f32| [v[0], v[1], v[2], w];
        let i = settings.integrator;
        let env = lights.environment;
        let params = Params {
            cam_origin: with(cam.origin, cam.half_width),
            cam_right: with(cam.right, cam.half_height),
            cam_up: with(cam.up, cam.aperture_radius),
            cam_forward: with(cam.forward, cam.focus_distance),
            env: [env.x, env.y, env.z, lights.pdf_environment()],
            size: [w, h],
            first_sample: 0,
            samples: 0,
            seed: settings.seed,
            max_depth: i.max_depth.min(MAX_DEPTH),
            roulette_start: i.roulette_start,
            light_count,
            ortho: u32::from(cam.orthographic),
            node_count: scene.accel.bvh.nodes.len() as u32,
            clamp_indirect: if i.clamp_indirect.is_finite() && i.clamp_indirect > 0.0 { i.clamp_indirect } else { 0.0 },
            strategy: match i.strategy {
                Strategy::Mis => 0,
                Strategy::LightOnly => 1,
                Strategy::BsdfOnly => 2,
            },
        };

        scoped(ctx, || {
            let device = &ctx.device;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("craft path tracer"),
                source: wgpu::ShaderSource::Wgsl(include_str!("pathtrace.wgsl").into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("craft path tracer"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let accum = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("accumulation"),
                size: accum_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let buffers = [&params_buf, &nodes_buf, &tris_buf, &attrs_buf, &mats_buf, &lights_buf, &table_buf, &accum];
            let entries: Vec<wgpu::BindGroupEntry<'_>> =
                buffers.iter().enumerate().map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }).collect();
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("craft path tracer"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &entries,
            });
            GpuRenderer { pipeline, bind_group, params_buf, accum, params, samples: 0 }
        })
    }

    /// Samples accumulated in every pixel so far.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Add `samples` samples to every pixel and wait for the GPU to finish them.
    pub fn render(&mut self, ctx: &GpuContext, samples: u32) -> Result<(), GpuError> {
        let mut left = samples.min(MAX_SAMPLES - self.samples);
        let pixels = u64::from(self.params.size[0]) * u64::from(self.params.size[1]);
        let per_dispatch = (WORK_PER_DISPATCH / pixels.max(1)).clamp(1, 64) as u32;
        while left > 0 {
            let n = left.min(per_dispatch);
            self.params.first_sample = self.samples;
            self.params.samples = n;
            scoped(ctx, || {
                ctx.queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&self.params));
                let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("craft path tracer") });
                {
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("craft path tracer"), timestamp_writes: None });
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &self.bind_group, &[]);
                    pass.dispatch_workgroups(self.params.size[0].div_ceil(8), self.params.size[1].div_ceil(8), 1);
                }
                ctx.queue.submit([enc.finish()]);
                ctx.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|e| GpuError::Device(e.to_string()))
            })??;
            self.samples += n;
            left -= n;
        }
        Ok(())
    }

    /// Copy the accumulated image back.
    pub fn film(&self, ctx: &GpuContext) -> Result<Film, GpuError> {
        let (w, h) = (self.params.size[0], self.params.size[1]);
        let mut film = Film::new(w, h).map_err(|_| GpuError::BadImageSize(w, h))?;
        let size = self.accum.size();
        let data = scoped(ctx, || -> Result<Vec<u8>, GpuError> {
            let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("film readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("film readback") });
            enc.copy_buffer_to_buffer(&self.accum, 0, &staging, 0, size);
            ctx.queue.submit([enc.finish()]);
            let slice = staging.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| GpuError::Device(e.to_string()))?;
            let view = slice.get_mapped_range().map_err(|e| GpuError::Device(e.to_string()))?;
            Ok(view.to_vec())
        })??;
        let pixels: &[[f32; 4]] = bytemuck::try_cast_slice(&data).map_err(|e| GpuError::Device(e.to_string()))?;
        for (dst, src) in film.sum.iter_mut().zip(pixels) {
            *dst = [src[0], src[1], src[2]];
        }
        film.samples = self.samples;
        Ok(film)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_render_core::integrator::IntegratorSettings;
    use craft_render_core::{Material, mean_rgb, render_image, scenes};
    use std::sync::Arc;

    /// The GPU for a test, or `None` (with a note) on machines without an adapter.
    fn gpu() -> Option<GpuContext> {
        let ctx = GpuContext::new();
        match &ctx {
            Some(c) => eprintln!("GPU test on {} ({}, software: {})", c.caps.adapter, c.caps.backend, c.caps.software),
            None => eprintln!("skipped: no GPU adapter on this machine; the CPU reference covers this path"),
        }
        ctx
    }

    fn both(scene: Scene, size: u32, spp: u32, integrator: IntegratorSettings) -> Option<(Film, Film)> {
        let ctx = gpu()?;
        let settings = RenderSettings { width: size, height: size, integrator, seed: 5 };
        let mut r = GpuRenderer::new(&ctx, &scene, settings).unwrap();
        r.render(&ctx, spp).unwrap();
        let g = r.film(&ctx).unwrap();
        let c = render_image(Arc::new(scene), settings, spp).unwrap();
        Some((g, c))
    }

    #[test]
    fn gpu_matches_the_cpu_reference_on_the_cornell_box() {
        let Some((g, c)) = both(scenes::cornell_box().unwrap(), 32, 64, IntegratorSettings { max_depth: 5, ..IntegratorSettings::default() }) else { return };
        assert_eq!(g.samples, 64);
        let (gm, cm) = (mean_rgb(&g.mean()), mean_rgb(&c.mean()));
        for k in 0..3 {
            assert!((gm[k] - cm[k]).abs() < 0.02 * cm[k], "gpu {gm:?} cpu {cm:?}");
        }
        // The two run the same samples, so the images agree far better than two independent
        // renders would (differences come from floating-point order only).
        let err = craft_render_core::relative_mse(&g.mean(), &c.mean());
        assert!(err < 2e-3, "relative MSE between GPU and CPU: {err}");
    }

    #[test]
    fn gpu_passes_the_furnace_and_matches_analytic_lights() {
        let Some((g, c)) = both(scenes::furnace(Material::metal(glam::Vec3::ONE, 0.6)).unwrap(), 32, 64, IntegratorSettings::default()) else { return };
        let (gm, cm) = (mean_rgb(&g.mean()), mean_rgb(&c.mean()));
        assert!((gm[0] - 1.0).abs() < 0.02 && (gm[0] - cm[0]).abs() < 0.01, "gpu {gm:?} cpu {cm:?}");
        let Some((g, c)) = both(scenes::material_row().unwrap(), 32, 32, IntegratorSettings::default()) else { return };
        let (gm, cm) = (mean_rgb(&g.mean()), mean_rgb(&c.mean()));
        for k in 0..3 {
            assert!((gm[k] - cm[k]).abs() < 0.05 * cm[k], "gpu {gm:?} cpu {cm:?}");
        }
    }

    #[test]
    fn gpu_handles_empty_scenes_progressive_passes_and_limits() {
        let Some(ctx) = gpu() else { return };
        let empty = craft_render_core::SceneBuilder::new().build();
        let settings = RenderSettings { width: 9, height: 5, ..RenderSettings::default() };
        let mut r = GpuRenderer::new(&ctx, &empty, settings).unwrap();
        r.render(&ctx, 2).unwrap();
        r.render(&ctx, 3).unwrap();
        let f = r.film(&ctx).unwrap();
        assert_eq!((f.samples, f.sum.len()), (5, 45));
        assert!(f.sum.iter().all(|p| *p == [0.0; 3]));
        assert!(matches!(GpuRenderer::new(&ctx, &empty, RenderSettings { width: 0, ..settings }), Err(GpuError::BadImageSize(0, 5))));
        // A device with tiny limits refuses a scene instead of crashing.
        let small = GpuContext { device: ctx.device.clone(), queue: ctx.queue.clone(), caps: Caps { max_storage_buffer_binding_size: 64, ..ctx.caps.clone() } };
        assert!(matches!(GpuRenderer::new(&small, &scenes::cornell_box().unwrap(), settings), Err(GpuError::TooLarge { .. })));
    }

    #[test]
    fn gpu_declines_materials_it_cannot_render() {
        // No adapter needed: the check comes before any GPU work, but a context is required to call it.
        let Some(ctx) = gpu() else { return };
        let settings = RenderSettings { width: 16, height: 16, ..RenderSettings::default() };
        for (material, word) in [
            (Material::glass(1.5, 0.0), "transmission"),
            (Material { coat_weight: 1.0, ..Material::default() }, "coats"),
            (Material { fuzz_weight: 0.5, ..Material::default() }, "fuzz"),
            (Material { geometry_opacity: 0.5, ..Material::default() }, "cut-outs"),
            (Material { base_color_texture: Some(0), ..Material::default() }, "textures"),
        ] {
            let scene = scenes::furnace(material).unwrap();
            match GpuRenderer::new(&ctx, &scene, settings) {
                Err(GpuError::Unsupported(feature)) => assert_eq!(feature, word),
                other => panic!("expected Unsupported({word}), got {:?}", other.map(|_| ()).map_err(|e| e.to_string())),
            }
        }
    }
}
