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
#![cfg_attr(not(feature = "hw-ray-query"), forbid(unsafe_code))]
#![cfg_attr(feature = "hw-ray-query", deny(unsafe_code))]

#[cfg(feature = "hw-ray-query")]
mod hw;

/// Whether this build can use hardware ray queries (the `hw-ray-query` feature).
pub const HARDWARE_RAYS_BUILT: bool = cfg!(feature = "hw-ray-query");

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
    /// Storage buffers one shader stage may bind (the resampled direct light kernels need 10).
    pub max_storage_buffers: u32,
    /// The device was opened with ray queries ([`GpuContext::new_hardware`]).
    pub hardware_rays: bool,
    /// Most triangles one acceleration structure may hold (0 without ray queries).
    pub max_blas_primitives: u32,
}

/// Options of one render that are not part of the scene or the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuOptions {
    /// Trace through the GPU's ray-query hardware instead of the shader's own BVH traversal, when
    /// the build and the device have it and the scene suits it (no cut-outs, small enough). Off by
    /// default; anything that does not fit falls back to software without an error.
    pub hardware_rays: bool,
}

/// A device and queue with their capabilities.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub caps: Caps,
}

impl GpuContext {
    /// A device that can run ray queries; always `None` unless this build has the `hw-ray-query` feature.
    #[cfg(not(feature = "hw-ray-query"))]
    pub fn new_hardware() -> Option<GpuContext> {
        None
    }

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
                // Resampled direct lighting binds two more buffers than the portable eight.
                max_storage_buffers_per_shader_stage: have.max_storage_buffers_per_shader_stage.clamp(8, 12),
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
            max_storage_buffers: limits.max_storage_buffers_per_shader_stage,
            hardware_rays: false,
            max_blas_primitives: 0,
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
    /// The previous frame's camera, for the motion feature image.
    prev_origin: [f32; 4],
    prev_right: [f32; 4],
    prev_up: [f32; 4],
    prev_forward: [f32; 4],
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
    /// Index bits of the z-curve pixel order; 255 for independent pixels.
    z_bits: u32,
    light_tree: u32,
    positional: u32,
    positional_mass: f32,
    tree_nodes: u32,
    tree_paths: u32,
    env_w: u32,
    env_h: u32,
    env_pixels: u32,
    env_marginal: u32,
    env_conditional: u32,
    env_turn: f32,
    textures: u32,
    texture_count: u32,
    has_cutouts: u32,
    has_env: u32,
    restir_mode: u32,
    restir_candidates: u32,
    restir_neighbors: u32,
    restir_radius: f32,
    restir_history: f32,
    pad: [u32; 3],
}

// The uniform block is padded to 16 bytes in WGSL; keep both sides the same size.
const _: () = assert!(std::mem::size_of::<Params>().is_multiple_of(16));

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
    /// The triangle's index in the scene.
    prim: u32,
    uv: [[f32; 2]; 3],
    pad: [f32; 2],
}

const FLAG_TRANSMISSIVE: u32 = 1;
const FLAG_CUTOUTS: u32 = 2;
/// The shading frame depends on the texture axes (normal map, bump map or anisotropy).
const FLAG_AXES: u32 = 4;

/// A sanitized [`craft_render_core::Material`] as the kernel reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuMaterial {
    base: [f32; 3],
    metalness: f32,
    specular_color: [f32; 3],
    roughness: f32,
    transmission_tint: [f32; 3],
    ior: f32,
    coat_color: [f32; 3],
    coat: f32,
    fuzz_color: [f32; 3],
    fuzz: f32,
    absorption: [f32; 3],
    specular_weight: f32,
    base_color: [f32; 3],
    opacity: f32,
    diffuse_roughness: f32,
    anisotropy: f32,
    transmission: f32,
    coat_roughness: f32,
    coat_ior: f32,
    fuzz_roughness: f32,
    thin_film: f32,
    thin_film_thickness: f32,
    thin_film_ior: f32,
    normal_scale: f32,
    bump_scale: f32,
    flags: u32,
    /// Base colour, roughness, metalness, normal, opacity and bump textures (−1: none), then padding.
    textures: [i32; 8],
}

impl GpuMaterial {
    fn new(m: &craft_render_core::Material, texture_count: usize) -> GpuMaterial {
        // A texture the scene does not have is no texture, as on the CPU.
        let tex = |id: Option<u32>| id.filter(|t| (*t as usize) < texture_count).map_or(-1, |t| t as i32);
        let textures = [
            tex(m.base_color_texture),
            tex(m.roughness_texture),
            tex(m.metalness_texture),
            tex(m.normal_texture),
            tex(m.opacity_texture),
            tex(m.bump_texture),
            0,
            0,
        ];
        let mut flags = 0;
        if m.is_transmissive() {
            flags |= FLAG_TRANSMISSIVE;
        }
        if m.has_cutouts() {
            flags |= FLAG_CUTOUTS;
        }
        if m.normal_texture.is_some() || m.bump_texture.is_some() || m.specular_roughness_anisotropy > 0.0 {
            flags |= FLAG_AXES;
        }
        GpuMaterial {
            base: (m.base_color * m.base_weight).to_array(),
            metalness: m.base_metalness,
            specular_color: m.specular_color.to_array(),
            roughness: m.specular_roughness,
            transmission_tint: if m.transmission_depth > 0.0 { [1.0; 3] } else { m.transmission_color.to_array() },
            ior: m.specular_ior,
            coat_color: m.coat_color.to_array(),
            coat: m.coat_weight,
            fuzz_color: m.fuzz_color.to_array(),
            fuzz: m.fuzz_weight,
            absorption: glam::Vec3::from(m.absorption()).to_array(),
            specular_weight: m.specular_weight,
            base_color: m.base_color.to_array(),
            opacity: m.geometry_opacity,
            diffuse_roughness: m.base_diffuse_roughness,
            anisotropy: m.specular_roughness_anisotropy,
            transmission: m.transmission_weight,
            coat_roughness: m.coat_roughness,
            coat_ior: m.coat_ior,
            fuzz_roughness: m.fuzz_roughness,
            thin_film: m.thin_film_weight,
            thin_film_thickness: m.thin_film_thickness,
            thin_film_ior: m.thin_film_ior,
            normal_scale: m.normal_scale,
            bump_scale: m.bump_scale,
            flags,
            textures,
        }
    }
}

/// What one pixel accumulates on the GPU: radiance and samples, base colour and depth, normal and
/// squared luminance, motion, the radiance of the odd samples.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuPixel {
    radiance: [f32; 4],
    albedo: [f32; 4],
    normal: [f32; 4],
    motion: [f32; 4],
    odd: [f32; 4],
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
const KIND_SPOT: u32 = 5;
const KIND_SUN: u32 = 6;

/// Samples per pixel times pixels one dispatch may cover, so a single submission stays short
/// enough for the driver's watchdog and for the interface to stay responsive.
const WORK_PER_DISPATCH: u64 = 1 << 26;

/// The kernels of resampled direct lighting, which run one sample at a time.
struct RestirKernels {
    initial: wgpu::ComputePipeline,
    temporal: wgpu::ComputePipeline,
    spatial: wgpu::ComputePipeline,
    shade: wgpu::ComputePipeline,
}

/// What runs a sample: the one kernel that renders it whole, or the stages of resampled lighting.
enum Kernels {
    Plain(wgpu::ComputePipeline),
    Restir(RestirKernels),
}

/// A progressive render on the GPU. Create one per scene and image size.
pub struct GpuRenderer {
    kernels: Kernels,
    bind_group: wgpu::BindGroup,
    params_buf: wgpu::Buffer,
    accum: wgpu::Buffer,
    params: Params,
    samples: u32,
    hardware: bool,
    /// Keeps the acceleration structures of the hardware path alive.
    _accel: Option<Box<dyn std::any::Any>>,
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

/// What answers the kernel's two ray queries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Traversal {
    Binary,
    Wide,
    RayQuery,
}

/// The shader: the path tracer with one implementation of the ray queries.
fn shader_source(traversal: Traversal) -> String {
    match traversal {
        Traversal::Binary => format!("{}\n{}", include_str!("pathtrace.wgsl"), include_str!("bvh_binary.wgsl")),
        Traversal::Wide => format!("{}\n{}", include_str!("pathtrace.wgsl"), include_str!("bvh_wide.wgsl")),
        // The directive must come before every declaration.
        Traversal::RayQuery => format!("enable wgpu_ray_query;\n{}\n{}", include_str!("pathtrace.wgsl"), include_str!("bvh_rayquery.wgsl")),
    }
}

#[cfg(feature = "hw-ray-query")]
type HardwareAccel = hw::Accel;
#[cfg(not(feature = "hw-ray-query"))]
type HardwareAccel = ();

/// What a renderer keeps so that the acceleration structures outlive its bind group.
fn keep_alive(hardware: Option<HardwareAccel>) -> Option<Box<dyn std::any::Any>> {
    hardware.map(|a| Box::new(a) as Box<dyn std::any::Any>)
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
        GpuRenderer::with_options(ctx, scene, settings, GpuOptions::default())
    }

    /// [`GpuRenderer::new`] with options.
    pub fn with_options(ctx: &GpuContext, scene: &Scene, settings: RenderSettings, options: GpuOptions) -> Result<GpuRenderer, GpuError> {
        if !scene.curves.is_empty() {
            return Err(GpuError::Unsupported("hair curves"));
        }
        let (w, h) = (settings.width, settings.height);
        let restir = settings.integrator.restir.active(&settings.integrator, w as usize * h as usize) && !scene.lights.is_empty();
        if (settings.integrator.guiding || settings.integrator.adaptive_splitting) && !restir {
            return Err(GpuError::Unsupported("path guiding and adaptive splitting"));
        }
        if restir && ctx.caps.max_storage_buffers < 10 {
            return Err(GpuError::Unsupported("resampled direct lighting on a device with fewer than ten storage buffers per stage"));
        }
        let max_side = ctx.caps.max_compute_workgroups_per_dimension.saturating_mul(8).min(craft_render_core::render::MAX_IMAGE_SIDE);
        if w == 0 || h == 0 || w > max_side || h > max_side {
            return Err(GpuError::BadImageSize(w, h));
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
                    prim: *i,
                    uv: scene.uvs.get(*i as usize).map_or([[0.0; 2]; 3], |t| t.map(|v| v.to_array())),
                    pad: [0.0; 2],
                }
            })
            .collect();
        let materials: Vec<GpuMaterial> = scene.materials.iter().map(|m| GpuMaterial::new(m, scene.textures.len())).collect();
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
            // `e1` carries a spot's axis, `e2.x` the cosine where its fall-off starts, and `area`
            // the cosine of its cone (spot) or the tangent of its angular radius (sun).
            let (kind, p0, radiance, e1, e2, area) = match *l {
                Light::Point { position, intensity } => (KIND_POINT, position, intensity, glam::Vec3::ZERO, 0.0, 0.0),
                Light::Spot { position, direction, intensity, cone_angle, blend } => {
                    let outer = cone_angle.clamp(1e-3, std::f32::consts::PI);
                    (KIND_SPOT, position, intensity, direction.normalize_or_zero(), (outer * (1.0 - blend.clamp(0.0, 1.0))).cos(), outer.cos())
                }
                Light::Directional { direction, irradiance } => (KIND_DIRECTIONAL, direction.normalize_or_zero(), irradiance, glam::Vec3::ZERO, 0.0, 0.0),
                Light::Sun { direction, irradiance, angular_radius } => {
                    (KIND_SUN, direction.normalize_or_zero(), irradiance, glam::Vec3::ZERO, 0.0, angular_radius.clamp(0.0, 0.5).tan())
                }
            };
            entries.push(GpuLight {
                p0: p0.to_array(),
                kind,
                e1: e1.to_array(),
                cdf: cdf.get(k).copied().unwrap_or(1.0),
                e2: [e2, 0.0, 0.0],
                area,
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

        // One buffer of four-float rows (the albedo table, then texture and environment pixels)
        // and one of plain words (texture table, light tree, environment sampling tables), so
        // the kernel stays within the eight storage buffers every device offers.
        let limit = ctx.caps.max_storage_buffer_binding_size.min(ctx.caps.max_buffer_size);
        let mut blob: Vec<[f32; 4]> = AlbedoTable::get().ab.clone();
        let mut words: Vec<u32> = Vec::new();
        let fits = |what: &'static str, have: usize, more: usize, each: u64| -> Result<(), GpuError> {
            let needed = (have as u64).saturating_add(more as u64).saturating_mul(each);
            if needed > limit || needed / each > u64::from(u32::MAX) { Err(GpuError::TooLarge { what, needed, allowed: limit }) } else { Ok(()) }
        };
        let push_image = |blob: &mut Vec<[f32; 4]>, image: &craft_render_core::texture::Image| -> Result<u32, GpuError> {
            let at = blob.len();
            let data = image.data();
            if image.is_gray() {
                fits("textures", at, data.len(), 16)?;
                blob.extend(data.iter().map(|v| [*v, *v, *v, 1.0]));
            } else {
                fits("textures", at, data.len() / 4, 16)?;
                blob.extend(data.as_chunks::<4>().0.iter().copied());
            }
            Ok(at as u32)
        };
        let texture_table = words.len() as u32;
        for image in &scene.textures {
            let at = push_image(&mut blob, image)?;
            words.extend([at, image.width(), image.height(), u32::from(image.is_gray())]);
        }
        let (tree, paths) = lights.tree_words();
        fits("light tree", words.len(), tree.len() + paths.len() * 2, 4)?;
        let tree_nodes = words.len() as u32;
        words.extend(tree);
        let tree_paths = words.len() as u32;
        words.extend(paths.iter().flat_map(|p| [*p as u32, (*p >> 32) as u32]));
        let (mut env_w, mut env_h, mut env_pixels, mut env_marginal, mut env_conditional, mut env_turn) = (0, 0, 0, 0, 0, 0.0);
        if let Some(map) = &lights.env_map {
            (env_w, env_h, env_turn) = (map.image.width(), map.image.height(), map.turn);
            env_pixels = push_image(&mut blob, &map.image)?;
            let (marginal, conditional) = map.sampling_tables();
            fits("environment tables", words.len(), marginal.len() + conditional.len(), 4)?;
            env_marginal = words.len() as u32;
            words.extend(marginal.iter().map(|v| v.to_bits()));
            env_conditional = words.len() as u32;
            words.extend(conditional.iter().map(|v| v.to_bits()));
        }

        // What answers the ray queries: the GPU's own ray-query hardware when asked for and available
        // (anything that does not fit falls back), else the compressed 8-wide hierarchy if the scene
        // has it, else the binary one.
        #[cfg(feature = "hw-ray-query")]
        let hardware = if options.hardware_rays && ctx.caps.hardware_rays {
            match hw::build(ctx, scene) {
                Ok(a) => Some(a),
                Err(e) => {
                    log::info!("craft-render: hardware rays are not used for this scene ({e}); tracing in software");
                    None
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "hw-ray-query"))]
        let hardware: Option<()> = {
            let _ = options;
            None
        };
        let wide = scene.accel.wide.as_ref();
        let traversal = if hardware.is_some() {
            Traversal::RayQuery
        } else if wide.is_some() {
            Traversal::Wide
        } else {
            Traversal::Binary
        };
        let nodes_buf = match (&hardware, wide) {
            (Some(_), _) => storage(ctx, "unused", &[0u32], 0u32)?,
            (None, Some(w)) => storage(ctx, "bvh nodes (8-wide)", bytemuck::cast_slice::<_, u32>(&w.nodes), 0u32)?,
            (None, None) => storage(ctx, "bvh nodes", &scene.accel.bvh.nodes, Zeroable::zeroed())?,
        };
        let node_count = wide.map_or(scene.accel.bvh.nodes.len(), |w| w.nodes.len()).max(1) as u32;
        let tris_buf = storage(ctx, "triangles", &tris, Zeroable::zeroed())?;
        let attrs_buf = storage(ctx, "triangle attributes", &attrs, Zeroable::zeroed())?;
        let mats_buf = storage(ctx, "materials", &materials, Zeroable::zeroed())?;
        let lights_buf = storage(ctx, "lights", &entries, Zeroable::zeroed())?;
        let blob_buf = storage(ctx, "tables and textures", &blob, [0.0f32; 4])?;
        let words_buf = storage(ctx, "light tree and sampling tables", &words, 0u32)?;
        let accum_size = u64::from(w) * u64::from(h) * std::mem::size_of::<GpuPixel>() as u64;
        let allowed = ctx.caps.max_storage_buffer_binding_size.min(ctx.caps.max_buffer_size);
        if accum_size > allowed {
            return Err(GpuError::TooLarge { what: "image", needed: accum_size, allowed });
        }

        let restir_cfg = settings.integrator.restir.clamped();
        let cam = scene.camera.prepare(w as f32 / h as f32).data();
        let with = |v: [f32; 3], w: f32| [v[0], v[1], v[2], w];
        let i = settings.integrator;
        let env = lights.environment;
        let has_env = lights.has_environment();
        let env_pmf = if has_env { lights.pmf(cdf.len().saturating_sub(1)) } else { 0.0 };
        let aspect = w as f32 / h as f32;
        let prev = settings.previous_camera.map(|c| c.prepare(aspect).data());
        let prev_or = |f: fn(&craft_render_core::camera::CameraData) -> [f32; 3], w: f32| prev.as_ref().map_or([0.0; 4], |c| with(f(c), w));
        let params = Params {
            cam_origin: with(cam.origin, cam.half_width),
            cam_right: with(cam.right, cam.half_height),
            cam_up: with(cam.up, cam.aperture_radius),
            cam_forward: with(cam.forward, cam.focus_distance),
            prev_origin: prev_or(|c| c.origin, prev.as_ref().map_or(1.0, |c| c.half_width)),
            prev_right: prev_or(|c| c.right, prev.as_ref().map_or(1.0, |c| c.half_height)),
            prev_up: prev_or(|c| c.up, 1.0),
            prev_forward: prev_or(|c| c.forward, prev.as_ref().map_or(0.0, |c| f32::from(u8::from(c.orthographic)))),
            env: [env.x, env.y, env.z, env_pmf],
            size: [w, h],
            first_sample: 0,
            samples: 0,
            seed: settings.seed,
            max_depth: i.max_depth.min(MAX_DEPTH),
            roulette_start: i.roulette_start,
            light_count,
            ortho: u32::from(cam.orthographic),
            node_count,
            clamp_indirect: if i.clamp_indirect.is_finite() && i.clamp_indirect > 0.0 { i.clamp_indirect } else { 0.0 },
            strategy: match i.strategy {
                Strategy::Mis => 0,
                Strategy::LightOnly => 1,
                Strategy::BsdfOnly => 2,
            },
            z_bits: match settings.pixel_order {
                craft_render_core::PixelOrder::ZCurve => craft_render_core::sampler::z_order_bits(w, h),
                craft_render_core::PixelOrder::Independent => 255,
            },
            light_tree: u32::from(i.light_sampling == craft_render_core::LightSampling::Tree),
            positional: lights.positional_count() as u32,
            positional_mass: lights.positional_mass(),
            tree_nodes,
            tree_paths,
            env_w,
            env_h,
            env_pixels,
            env_marginal,
            env_conditional,
            env_turn,
            textures: texture_table,
            texture_count: scene.textures.len() as u32,
            has_cutouts: u32::from(scene.has_cutouts),
            has_env: u32::from(has_env),
            restir_mode: if restir { restir_cfg.mode as u32 } else { 0 },
            restir_candidates: restir_cfg.candidates,
            restir_neighbors: restir_cfg.neighbors,
            restir_radius: restir_cfg.radius,
            restir_history: restir_cfg.history,
            pad: [0; 3],
        };

        let pixels = u64::from(w) * u64::from(h);
        // Two samples of first surfaces (48 bytes) and four planes of reservoirs (32 bytes).
        let (gbuf_size, resv_size) = (pixels * 48 * 2, pixels * 32 * 4);
        if restir && gbuf_size.max(resv_size) > allowed {
            return Err(GpuError::TooLarge { what: "reservoirs", needed: gbuf_size.max(resv_size), allowed });
        }

        scoped(ctx, || {
            let device = &ctx.device;
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("craft path tracer"),
                source: wgpu::ShaderSource::Wgsl(shader_source(traversal).into()),
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
            let mut buffers = vec![&params_buf, &nodes_buf, &tris_buf, &attrs_buf, &mats_buf, &lights_buf, &blob_buf, &words_buf, &accum];
            let rs_buffers;
            if restir {
                let make = |label: &'static str, size: u64| {
                    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false })
                };
                rs_buffers = [make("first surfaces", gbuf_size), make("reservoirs", resv_size)];
                buffers.extend(rs_buffers.iter());
            }
            #[cfg(feature = "hw-ray-query")]
            let tlas = hardware.as_ref().map(|a| &a.tlas);
            let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    #[cfg(feature = "hw-ray-query")]
                    if i == 1
                        && let Some(t) = tlas
                    {
                        return wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::AccelerationStructure(t) };
                    }
                    wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() }
                })
                .collect();
            let is_hardware = hardware.is_some();
            if !restir {
                let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("craft path tracer"),
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("craft path tracer"),
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                });
                return GpuRenderer {
                    kernels: Kernels::Plain(pipeline),
                    bind_group,
                    params_buf,
                    accum,
                    params,
                    samples: 0,
                    hardware: is_hardware,
                    _accel: keep_alive(hardware),
                };
            }
            // The stages share one layout: the kernels use different subsets of the bindings.
            let layout_entries: Vec<wgpu::BindGroupLayoutEntry> = (0..buffers.len() as u32)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: if binding == 1 && is_hardware {
                        wgpu::BindingType::AccelerationStructure { vertex_return: false }
                    } else {
                        wgpu::BindingType::Buffer {
                            ty: if binding == 0 {
                                wgpu::BufferBindingType::Uniform
                            } else {
                                wgpu::BufferBindingType::Storage { read_only: (1..=7).contains(&binding) }
                            },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        }
                    },
                    count: None,
                })
                .collect();
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("craft restir"), entries: &layout_entries });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("craft restir"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let kernel = |name: &'static str| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(name),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(name),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let kernels = RestirKernels {
                initial: kernel("restir_initial"),
                temporal: kernel("restir_temporal"),
                spatial: kernel("restir_spatial"),
                shade: kernel("restir_shade"),
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("craft restir"), layout: &layout, entries: &entries });
            GpuRenderer {
                kernels: Kernels::Restir(kernels),
                bind_group,
                params_buf,
                accum,
                params,
                samples: 0,
                hardware: is_hardware,
                _accel: keep_alive(hardware),
            }
        })
    }

    /// Whether the ray queries run on the GPU's ray-query hardware (see [`GpuOptions`]).
    pub fn uses_hardware(&self) -> bool {
        self.hardware
    }

    /// Samples accumulated in every pixel so far.
    pub fn samples(&self) -> u32 {
        self.samples
    }

    /// Add `samples` samples to every pixel and wait for the GPU to finish them.
    pub fn render(&mut self, ctx: &GpuContext, samples: u32) -> Result<(), GpuError> {
        let mut left = samples.min(MAX_SAMPLES - self.samples);
        let pixels = u64::from(self.params.size[0]) * u64::from(self.params.size[1]);
        let (gx, gy) = (self.params.size[0].div_ceil(8), self.params.size[1].div_ceil(8));
        if let Kernels::Restir(k) = &self.kernels {
            // One sample at a time: reuse needs the reservoirs of the whole image for one sample.
            // The queue orders each parameter write before its dispatches, so several samples go
            // in flight before the CPU waits.
            while left > 0 {
                let batch = left.min(16);
                scoped(ctx, || {
                    for _ in 0..batch {
                        self.params.first_sample = self.samples;
                        self.params.samples = 1;
                        ctx.queue.write_buffer(&self.params_buf, 0, bytemuck::bytes_of(&self.params));
                        let mut enc = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("craft restir") });
                        let viewport = self.params.restir_mode == 2;
                        let stages = [Some(&k.initial), viewport.then_some(&k.temporal), Some(&k.spatial), Some(&k.shade)];
                        for pipeline in stages.into_iter().flatten() {
                            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("craft restir"), timestamp_writes: None });
                            pass.set_pipeline(pipeline);
                            pass.set_bind_group(0, &self.bind_group, &[]);
                            pass.dispatch_workgroups(gx, gy, 1);
                        }
                        ctx.queue.submit([enc.finish()]);
                        self.samples += 1;
                    }
                    ctx.device.poll(wgpu::PollType::wait_indefinitely()).map(|_| ()).map_err(|e| GpuError::Device(e.to_string()))
                })??;
                left -= batch;
            }
            return Ok(());
        }
        let Kernels::Plain(pipeline) = &self.kernels else { return Ok(()) };
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
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &self.bind_group, &[]);
                    pass.dispatch_workgroups(gx, gy, 1);
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
        let pixels: &[GpuPixel] = bytemuck::try_cast_slice(&data).map_err(|e| GpuError::Device(e.to_string()))?;
        for (i, src) in pixels.iter().enumerate().take(film.sum.len()) {
            film.sum[i] = [src.radiance[0], src.radiance[1], src.radiance[2]];
            film.albedo[i] = [src.albedo[0], src.albedo[1], src.albedo[2]];
            film.normal[i] = [src.normal[0], src.normal[1], src.normal[2]];
            film.depth[i] = src.albedo[3];
            film.luminance_sq[i] = src.normal[3];
            film.motion[i] = [src.motion[0], src.motion[1]];
            film.odd[i] = [src.odd[0], src.odd[1], src.odd[2]];
        }
        film.samples = self.samples;
        Ok(film)
    }
}

#[cfg(test)]
mod denoise_tests;

#[cfg(test)]
mod hw_tests;

#[cfg(test)]
mod wide_tests;

#[cfg(test)]
mod restir_tests;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use craft_render_core::integrator::IntegratorSettings;
    use craft_render_core::{Material, mean_rgb, render_image, scenes};
    use std::sync::Arc;

    /// One GPU test at a time: several devices opened at once from one process have crashed
    /// inside a vendor driver on a busy machine.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) struct TestGpu {
        ctx: GpuContext,
        _turn: std::sync::MutexGuard<'static, ()>,
    }

    impl std::ops::Deref for TestGpu {
        type Target = GpuContext;
        fn deref(&self) -> &GpuContext {
            &self.ctx
        }
    }

    /// The GPU for a test, or `None` (with a note) on machines without an adapter.
    /// The turn to hold while a test opens a device of its own.
    #[cfg(feature = "hw-ray-query")]
    pub(crate) fn gpu_turn() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn gpu() -> Option<TestGpu> {
        let turn = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
        let ctx = GpuContext::new();
        match &ctx {
            Some(c) => eprintln!("GPU test on {} ({}, software: {})", c.caps.adapter, c.caps.backend, c.caps.software),
            None => eprintln!("skipped: no GPU adapter on this machine; the CPU reference covers this path"),
        }
        ctx.map(|ctx| TestGpu { ctx, _turn: turn })
    }

    fn both(scene: Scene, size: u32, spp: u32, integrator: IntegratorSettings) -> Option<(Film, Film)> {
        let ctx = gpu()?;
        let settings = RenderSettings { width: size, height: size, integrator, seed: 5, ..RenderSettings::default() };
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
    fn gpu_renders_spots_and_suns_like_the_cpu() {
        use craft_render_core::shapes::quad;
        use craft_render_core::{Camera, Light, SceneBuilder};
        use glam::{Mat4, Vec3};
        let build = |light: Light| {
            let mut b = SceneBuilder::new();
            let grey = b.add_material(Material::diffuse(Vec3::splat(0.5))).unwrap();
            b.add_mesh(&quad(Vec3::new(-5.0, 0.0, -5.0), Vec3::Z * 10.0, Vec3::X * 10.0), Mat4::IDENTITY, grey).unwrap();
            b.add_mesh(&craft_render_core::shapes::uv_sphere(Vec3::new(0.0, 0.5, 0.0), 0.5, 24, 12), Mat4::IDENTITY, grey).unwrap();
            b.add_light(light).unwrap();
            b.set_camera(Camera { eye: Vec3::new(0.0, 4.0, 6.0), target: Vec3::ZERO, ..Camera::default() });
            b.build()
        };
        let spot = Light::Spot {
            position: Vec3::new(1.0, 3.0, 1.0),
            direction: Vec3::new(-0.3, -1.0, -0.3),
            intensity: Vec3::splat(20.0),
            cone_angle: 0.6,
            blend: 0.3,
        };
        let sun = Light::Sun { direction: Vec3::new(0.4, 0.8, 0.3), irradiance: Vec3::splat(2.0), angular_radius: 0.05 };
        for light in [spot, sun] {
            let Some((g, c)) = both(build(light), 32, 32, IntegratorSettings::default()) else { return };
            let err = craft_render_core::relative_mse(&g.mean(), &c.mean());
            assert!(err < 2e-3 && mean_rgb(&c.mean())[0] > 0.01, "{light:?}: relative MSE {err}");
        }
    }

    /// The largest difference a feature image shows between the two devices, as a mean.
    fn mean_abs(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
        a.iter().zip(b).map(|(p, q)| (0..3).map(|k| f64::from((p[k] - q[k]).abs())).sum::<f64>()).sum::<f64>() / (a.len() * 3) as f64
    }

    fn agree(name: &str, g: &Film, c: &Film) {
        let err = craft_render_core::relative_mse(&g.mean(), &c.mean());
        let (gm, cm) = (mean_rgb(&g.mean()), mean_rgb(&c.mean()));
        eprintln!("{name}: relative MSE between GPU and CPU {err:.6}, means {gm:?} {cm:?}");
        assert!(err < 2e-3, "{name}: relative MSE between GPU and CPU: {err}");
        assert!(cm[1] > 0.01, "{name}: the reference is black");
    }

    #[test]
    fn gpu_matches_the_cpu_on_layered_materials_and_textures() {
        // Glass (clear, rough, absorbing), coat, anisotropic metal, fuzz over a rough diffuse
        // base, a thin film, a bump map and a textured floor.
        let Some((g, c)) = both(scenes::showcase().unwrap(), 48, 64, IntegratorSettings::default()) else { return };
        agree("showcase", &g, &c);
    }

    #[test]
    fn gpu_matches_the_cpu_under_a_panorama() {
        let Some((g, c)) = both(scenes::studio().unwrap(), 48, 64, IntegratorSettings::default()) else { return };
        agree("studio", &g, &c);
        // The panorama alone, seen directly and reflected, with each estimator on its own.
        for strategy in [Strategy::Mis, Strategy::BsdfOnly] {
            let mut b = craft_render_core::SceneBuilder::new();
            let sky = craft_render_core::texture::Image::generate_uv(64, 32, craft_render_core::texture::Kind::Color, |u, v| {
                let patch = if (0.2..0.3).contains(&u) && (0.2..0.35).contains(&v) { 40.0 } else { 0.0 };
                [0.3 + patch, 0.4 + patch, 0.6 * (1.0 - v) + patch, 1.0]
            });
            b.set_environment_map(sky, 1.1);
            let grey = b.add_material(Material { base_color: glam::Vec3::splat(0.6), specular_roughness: 0.4, ..Material::default() }).unwrap();
            b.add_mesh(&craft_render_core::shapes::uv_sphere(glam::Vec3::ZERO, 1.0, 32, 16), glam::Mat4::IDENTITY, grey).unwrap();
            b.set_camera(craft_render_core::Camera { eye: glam::Vec3::new(0.0, 1.0, 4.0), target: glam::Vec3::ZERO, ..Default::default() });
            let Some((g, c)) = both(b.build(), 32, 64, IntegratorSettings { strategy, ..IntegratorSettings::default() }) else { return };
            agree("panorama", &g, &c);
        }
    }

    #[test]
    fn gpu_matches_the_cpu_with_the_light_tree_and_by_power() {
        for light_sampling in [craft_render_core::LightSampling::Tree, craft_render_core::LightSampling::Power] {
            let integrator = IntegratorSettings { light_sampling, ..IntegratorSettings::default() };
            let Some((g, c)) = both(scenes::night_street().unwrap(), 48, 64, integrator) else { return };
            agree("night-street", &g, &c);
        }
        let Some((g, c)) = both(scenes::interior().unwrap(), 32, 64, IntegratorSettings { clamp_indirect: 4.0, ..IntegratorSettings::default() }) else {
            return;
        };
        agree("interior", &g, &c);
    }

    #[test]
    fn gpu_matches_the_cpu_on_cutouts_and_normal_maps() {
        use craft_render_core::shapes::quad;
        use craft_render_core::texture::{Image, Kind};
        use craft_render_core::{Camera, Light, SceneBuilder};
        use glam::{Mat4, Vec3};
        let mut b = SceneBuilder::new();
        // A leaf-like card: holes from the alpha channel, over a floor with a normal map and
        // textured roughness and metalness; a half-transparent pane casts a partial shadow.
        let holes = b.add_texture(Image::generate(32, 32, Kind::Color, |x, y| [0.2, 0.7, 0.3, if (x / 4 + y / 4) % 2 == 0 { 1.0 } else { 0.0 }])).unwrap();
        let waves = b
            .add_texture(Image::generate_uv(64, 64, Kind::Color, |u, v| {
                let (x, y) = ((u * 25.0).sin() * 0.4, (v * 19.0).cos() * 0.4);
                [x * 0.5 + 0.5, y * 0.5 + 0.5, 0.9, 1.0]
            }))
            .unwrap();
        let packed = b.add_texture(Image::generate_uv(16, 16, Kind::Color, |u, v| [1.0, 0.3 + 0.7 * u, v, 1.0])).unwrap();
        let floor = b
            .add_material(Material {
                base_color: Vec3::new(0.7, 0.6, 0.5),
                specular_roughness: 0.8,
                base_metalness: 1.0,
                roughness_texture: Some(packed),
                metalness_texture: Some(packed),
                normal_texture: Some(waves),
                normal_scale: 1.0,
                ..Material::default()
            })
            .unwrap();
        let card = b.add_material(Material { base_color_texture: Some(holes), opacity_texture: Some(holes), ..Material::diffuse(Vec3::ONE) }).unwrap();
        let pane = b.add_material(Material { geometry_opacity: 0.5, ..Material::diffuse(Vec3::new(0.8, 0.2, 0.2)) }).unwrap();
        b.add_mesh(&quad(Vec3::new(-4.0, 0.0, -4.0), Vec3::Z * 8.0, Vec3::X * 8.0), Mat4::IDENTITY, floor).unwrap();
        b.add_mesh(&quad(Vec3::new(-1.5, 0.2, 0.0), Vec3::X * 1.5, Vec3::Y * 1.5), Mat4::IDENTITY, card).unwrap();
        b.add_mesh(&quad(Vec3::new(0.3, 0.2, 0.3), Vec3::X * 1.2, Vec3::Y * 1.2), Mat4::IDENTITY, pane).unwrap();
        b.add_light(Light::Point { position: Vec3::new(0.0, 3.0, 3.0), intensity: Vec3::splat(40.0) }).unwrap();
        b.set_environment(Vec3::splat(0.3));
        b.set_camera(Camera { eye: Vec3::new(0.0, 2.5, 4.5), target: Vec3::new(0.0, 0.5, 0.0), ..Camera::default() });
        let Some((g, c)) = both(b.build(), 48, 64, IntegratorSettings::default()) else { return };
        agree("cut-outs and maps", &g, &c);
    }

    #[test]
    fn gpu_fills_the_feature_images_like_the_cpu() {
        let Some(ctx) = gpu() else { return };
        let scene = scenes::showcase().unwrap();
        let moved = craft_render_core::Camera { eye: scene.camera.eye + glam::Vec3::new(0.3, 0.0, 0.0), ..scene.camera };
        let settings = RenderSettings { width: 40, height: 24, seed: 9, previous_camera: Some(moved), ..RenderSettings::default() };
        let mut r = GpuRenderer::new(&ctx, &scene, settings).unwrap();
        r.render(&ctx, 16).unwrap();
        r.render(&ctx, 16).unwrap();
        let g = r.film(&ctx).unwrap();
        let c = render_image(Arc::new(scene), settings, 32).unwrap();
        let albedo = mean_abs(&g.mean_of(&g.albedo), &c.mean_of(&c.albedo));
        let normal = mean_abs(&g.mean_of(&g.normal), &c.mean_of(&c.normal));
        assert!(albedo < 2e-3 && normal < 2e-3, "albedo {albedo}, normal {normal}");
        let n = g.depth.len() as f64;
        let depth = g.depth.iter().zip(&c.depth).map(|(a, b)| f64::from((a - b).abs())).sum::<f64>() / n / 32.0;
        let motion = g.motion.iter().zip(&c.motion).map(|(a, b)| f64::from((a[0] - b[0]).abs() + (a[1] - b[1]).abs())).sum::<f64>() / n / 32.0;
        assert!(depth < 2e-2 && motion < 2e-2, "depth {depth}, motion {motion}");
        assert!(c.motion.iter().any(|m| m[0].abs() > 1.0), "the camera moved, so something moved on screen");
        // The per-pixel variance estimate has the same total on both devices.
        let (gv, cv): (f64, f64) = (g.variance().iter().map(|v| f64::from(*v)).sum(), c.variance().iter().map(|v| f64::from(*v)).sum());
        assert!(cv > 0.0 && (gv - cv).abs() < 0.1 * cv, "variance {gv} against {cv}");
    }

    #[test]
    fn gpu_declines_learned_sampling() {
        // Path guiding and adaptive splitting exist on the CPU only; callers fall back to it.
        let Some(ctx) = gpu() else { return };
        let scene = scenes::cornell_box().unwrap();
        for (guiding, adaptive_splitting) in [(true, false), (false, true)] {
            let integrator = IntegratorSettings { guiding, adaptive_splitting, ..IntegratorSettings::default() };
            let settings = RenderSettings { width: 16, height: 16, integrator, ..RenderSettings::default() };
            assert!(matches!(GpuRenderer::new(&ctx, &scene, settings), Err(GpuError::Unsupported(_))));
        }
    }
}
