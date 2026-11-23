//! `modelcraft-cli render`: a path-traced image of a project, model file or built-in scene.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use modelcraft_engine::Document;
use modelcraft_render::pathtrace::integrator::IntegratorSettings;
use modelcraft_render::pathtrace::{DenoiseMode, RestirMode, RestirSettings};
use modelcraft_render::pathtrace::{Film, RenderSettings, Renderer, Scene, scenes};
use modelcraft_render::pathtrace_gpu::{GpuContext, GpuOptions, GpuRenderer};
use modelcraft_render::trace::{ExportOptions, Lighting, export_scene, load_panorama, scene_bounds};
use modelcraft_render::{Camera, Projection};

pub const USAGE: &str = "  render <file|builtin:NAME> -o <out.png> [options]
                                    path-trace an image. Built-in scenes: cornell, materials,
                                    interior, night-street, studio, showcase, furnace. A .exr output also holds
                                    the feature images (albedo, normal, depth, motion, variance)
      --width N --height N          image size (default 960×540)
      --samples N                   samples per pixel (default 64)
      --depth N                     bounces (default 12)
      --device auto|cpu|gpu         auto uses a hardware GPU when there is one
      --yaw DEG --pitch DEG --fov DEG   camera around the framed scene; --ortho for no perspective
      --exposure EV                 brightness in stops (default 0)
      --guiding --splitting         learn where light comes from and sample that way; end or
                                    multiply paths by their expected contribution (CPU only)
      --restir MODE                 resampled direct light at the first surface (ReSTIR DI):
                                    unbiased (same expected image, for finals) or viewport
                                    (adds temporal reuse, faster to clean, slightly biased)
      --restir-candidates N         light samples per pixel and pass (default 8)
      --restir-neighbors N          neighbouring pixels merged per pass (default 4)
      --denoise [MODE]              remove the noise from the picture: nfor (the default; regression on
                                    the two half renderings, for finals) or svgf (the interactive
                                    filter). A .exr then holds the filtered image as R, G, B and the
                                    original as noisy.R/G/B
      --hardware-rays               trace through the GPU's ray-query hardware (needs a build with the
                                    hw-ray-query feature and a device that has it; otherwise software)
      --bvh binary|wide             traversal structure: the binary tree (default) or its compressed 8-wide
                                    form (faster on the CPU, not on the GPU; see docs/performance.md)
      --sky R,G,B                   uniform sky light instead of the studio light; --no-ground
      --sun ELEVATION[,AZIMUTH]     daylight: a physical sky with the sun at these angles (degrees)
      --env FILE.hdr [--env-rotation DEG]  light the scene with a Radiance HDR panorama
      --variant N                   render scene variant N (see `variant` in `modelcraft-cli commands`); the
                                    file is not changed
      --seed N";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Auto,
    Cpu,
    Gpu,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderArgs {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub depth: u32,
    pub seed: u32,
    pub device: Device,
    pub exposure: f32,
    pub yaw: Option<f32>,
    pub pitch: Option<f32>,
    pub fov: Option<f32>,
    pub ortho: bool,
    pub sky: Option<[f32; 3]>,
    /// Sun elevation and azimuth in degrees.
    pub sun: Option<[f32; 2]>,
    pub env: Option<String>,
    pub env_rotation: f32,
    pub ground: bool,
    /// Path guiding and adaptive roulette/splitting (CPU only).
    pub guiding: bool,
    pub splitting: bool,
    pub restir: RestirSettings,
    pub denoise: DenoiseMode,
    pub wide_bvh: bool,
    pub hardware_rays: bool,
    /// Render this scene variant (the file is not changed).
    pub variant: Option<u32>,
}

impl Default for RenderArgs {
    fn default() -> Self {
        RenderArgs {
            width: 960,
            height: 540,
            samples: 64,
            depth: 12,
            seed: 0,
            device: Device::Auto,
            exposure: 0.0,
            yaw: None,
            pitch: None,
            fov: None,
            ortho: false,
            sky: None,
            sun: None,
            env: None,
            env_rotation: 0.0,
            ground: true,
            guiding: false,
            splitting: false,
            restir: RestirSettings::default(),
            denoise: DenoiseMode::Off,
            wide_bvh: false,
            variant: None,
            hardware_rays: false,
        }
    }
}

fn value<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).map(String::as_str)
}

fn number<T: std::str::FromStr>(a: &[String], name: &str) -> Result<Option<T>, String> {
    value(a, name).map(|v| v.parse::<T>().map_err(|_| format!("{name}: {v:?} is not a number"))).transpose()
}

pub fn parse(a: &[String]) -> Result<RenderArgs, String> {
    let d = RenderArgs::default();
    let sky = match value(a, "--sky") {
        None => None,
        Some(v) => {
            let parts: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            match parts.as_slice() {
                [g] => Some([*g; 3]),
                [r, g, b] => Some([*r, *g, *b]),
                _ => return Err(format!("--sky: expected one number or R,G,B, found {v:?}")),
            }
        }
    };
    let sun = match value(a, "--sun") {
        None => None,
        Some(v) => {
            let parts: Vec<f32> = v.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            match parts.as_slice() {
                [e] => Some([*e, 0.0]),
                [e, az] => Some([*e, *az]),
                _ => return Err(format!("--sun: expected ELEVATION or ELEVATION,AZIMUTH in degrees, found {v:?}")),
            }
        }
    };
    Ok(RenderArgs {
        sun,
        env: value(a, "--env").map(str::to_string),
        env_rotation: number(a, "--env-rotation")?.unwrap_or(0.0),
        width: number(a, "--width")?.unwrap_or(d.width),
        height: number(a, "--height")?.unwrap_or(d.height),
        samples: number::<u32>(a, "--samples")?.unwrap_or(d.samples).max(1),
        depth: number(a, "--depth")?.unwrap_or(d.depth),
        seed: number(a, "--seed")?.unwrap_or(d.seed),
        device: match value(a, "--device") {
            None | Some("auto") => Device::Auto,
            Some("cpu") => Device::Cpu,
            Some("gpu") => Device::Gpu,
            Some(other) => return Err(format!("--device: expected auto, cpu or gpu, found {other:?}")),
        },
        exposure: number(a, "--exposure")?.unwrap_or(d.exposure),
        yaw: number(a, "--yaw")?,
        pitch: number(a, "--pitch")?,
        fov: number(a, "--fov")?,
        ortho: a.iter().any(|x| x == "--ortho"),
        sky,
        ground: !a.iter().any(|x| x == "--no-ground"),
        guiding: a.iter().any(|x| x == "--guiding"),
        splitting: a.iter().any(|x| x == "--splitting"),
        restir: parse_restir(a)?,
        denoise: parse_denoise(a)?,
        wide_bvh: parse_bvh(a)?,
        hardware_rays: a.iter().any(|x| x == "--hardware-rays"),
        variant: number(a, "--variant")?,
    })
}

/// `--bvh binary|wide`: whether to trace through the 8-wide hierarchy.
pub fn parse_bvh(a: &[String]) -> Result<bool, String> {
    match value(a, "--bvh") {
        None | Some("binary") => Ok(false),
        Some("wide") => Ok(true),
        Some(other) => Err(format!("--bvh: expected binary or wide, found {other:?}")),
    }
}

/// `--denoise` alone means `nfor`; a mode may follow it.
pub fn parse_denoise(a: &[String]) -> Result<DenoiseMode, String> {
    let Some(i) = a.iter().position(|x| x == "--denoise") else { return Ok(DenoiseMode::Off) };
    match a.get(i + 1).map(String::as_str) {
        None => Ok(DenoiseMode::Nfor),
        Some(next) if next.starts_with('-') => Ok(DenoiseMode::Nfor),
        Some("off") => Ok(DenoiseMode::Off),
        Some("nfor") => Ok(DenoiseMode::Nfor),
        Some("svgf") => Ok(DenoiseMode::Svgf),
        Some(other) => Err(format!("--denoise: expected off, svgf or nfor, found {other:?}")),
    }
}

/// The `--restir`, `--restir-candidates` and `--restir-neighbors` options.
pub fn parse_restir(a: &[String]) -> Result<RestirSettings, String> {
    let mode = match value(a, "--restir") {
        None | Some("off") => RestirMode::Off,
        Some("unbiased") => RestirMode::Unbiased,
        Some("viewport") => RestirMode::Viewport,
        Some(other) => return Err(format!("--restir: expected off, unbiased or viewport, found {other:?}")),
    };
    let mut r = RestirSettings { mode, ..RestirSettings::default() };
    if let Some(n) = number::<u32>(a, "--restir-candidates")? {
        r.candidates = n.clamp(1, modelcraft_render::pathtrace::restir::MAX_CANDIDATES);
    }
    if let Some(n) = number::<u32>(a, "--restir-neighbors")? {
        r.neighbors = n.min(modelcraft_render::pathtrace::restir::MAX_NEIGHBORS);
    }
    Ok(r)
}

/// A built-in scene by name.
pub fn builtin(name: &str) -> Result<Scene, String> {
    scenes::by_name(name).ok_or_else(|| format!("unknown built-in scene {name:?} ({}, furnace)", scenes::BENCHMARKS.join(", ")))?.map_err(|e| e.to_string())
}

/// The path tracer's scene for a document, seen by a camera that frames everything.
pub fn document_scene(d: &mut Document, args: &RenderArgs) -> Result<Scene, String> {
    d.sync_sculpt();
    if let Some(v) = args.variant {
        d.scene.switch_variant(Some(v)).map_err(|e| e.to_string())?;
    }
    let mut cam = Camera::default();
    if let Some(v) = args.fov.filter(|v| v.is_finite()) {
        cam.fov_y = v.clamp(1.0, 170.0).to_radians();
    }
    if let Some(v) = args.yaw.filter(|v| v.is_finite()) {
        cam.yaw = v.to_radians();
    }
    if let Some(v) = args.pitch.filter(|v| v.is_finite()) {
        cam.pitch = v.clamp(-89.0, 89.0).to_radians();
    }
    if let Some((lo, hi)) = scene_bounds(&d.scene, &mut d.eval) {
        cam.frame(lo, hi);
    }
    if args.ortho {
        cam.projection = Projection::Orthographic;
    }
    let lighting = if let Some(path) = &args.env {
        Lighting::Image { image: load_panorama(path)?, rotation: args.env_rotation.to_radians() }
    } else if let Some([elevation, azimuth]) = args.sun {
        Lighting::Daylight { elevation: elevation.to_radians(), azimuth: azimuth.to_radians() }
    } else {
        args.sky.map_or(Lighting::Studio, |s| Lighting::Sky(glam::Vec3::from_array(s)))
    };
    export_scene(&d.scene, &mut d.eval, &cam, &ExportOptions { lighting, ground: args.ground, ..ExportOptions::default() }).map_err(|e| e.to_string())
}

/// What a render did, for the JSON line the command prints.
pub struct Report {
    pub film: Film,
    pub device: String,
    pub seconds: f64,
    /// The filtered image (mean radiance) and the seconds the filter took, when asked for.
    pub denoised: Option<(Vec<[f32; 3]>, f64)>,
}

/// The GPU: one with ray-query hardware when asked for (and built for), else the usual one.
pub fn open_gpu(hardware_rays: bool) -> Option<GpuContext> {
    if hardware_rays {
        if let Some(ctx) = GpuContext::new_hardware() {
            return Some(ctx);
        }
        eprintln!(
            "note: hardware ray queries are not available (built with the hw-ray-query feature: {}); tracing in software",
            modelcraft_render::pathtrace_gpu::HARDWARE_RAYS_BUILT
        );
    }
    GpuContext::new()
}

/// Render `scene` with the device the arguments ask for.
pub fn render(mut scene: Scene, args: &RenderArgs) -> Result<Report, String> {
    if args.wide_bvh && !scene.enable_wide_bvh() {
        eprintln!("note: this scene's tree does not fit the 8-wide form; using the binary tree");
    }
    let settings = RenderSettings {
        width: args.width,
        height: args.height,
        integrator: IntegratorSettings {
            max_depth: args.depth,
            guiding: args.guiding,
            adaptive_splitting: args.splitting,
            restir: args.restir,
            ..IntegratorSettings::default()
        },
        seed: args.seed,
        ..RenderSettings::default()
    };
    let gpu = match args.device {
        Device::Cpu => None,
        Device::Gpu => Some(open_gpu(args.hardware_rays).ok_or("no GPU adapter is available; use --device cpu")?),
        Device::Auto => open_gpu(args.hardware_rays).filter(|c| !c.caps.software),
    };
    if let Some(ctx) = gpu {
        let attempt = (|| -> Result<Report, String> {
            let mut r = GpuRenderer::with_options(&ctx, &scene, settings, GpuOptions { hardware_rays: args.hardware_rays }).map_err(|e| e.to_string())?;
            let rays = if r.uses_hardware() { ", ray queries" } else { "" };
            let t = Instant::now();
            r.render(&ctx, args.samples).map_err(|e| e.to_string())?;
            let film = r.film(&ctx).map_err(|e| e.to_string())?;
            Ok(Report { film, device: format!("gpu: {} ({}{rays})", ctx.caps.adapter, ctx.caps.backend), seconds: t.elapsed().as_secs_f64(), denoised: None })
        })();
        match attempt {
            Ok(report) => return Ok(report),
            // An explicit request for the GPU fails loudly; `auto` falls back to the CPU.
            Err(e) if args.device == Device::Gpu => return Err(e),
            Err(e) => eprintln!("note: the GPU could not render this ({e}); using the CPU"),
        }
    }
    let mut r = Renderer::new(Arc::new(scene), settings).map_err(|e| e.to_string())?;
    let t = Instant::now();
    r.render(args.samples, &AtomicBool::new(false));
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    Ok(Report { film: r.film().clone(), device: format!("cpu: {threads} threads"), seconds: t.elapsed().as_secs_f64(), denoised: None })
}

/// The film's image, or the filtered one when there is one, as 8-bit sRGB.
fn display_pixels(film: &Film, denoised: Option<&[[f32; 3]]>, exposure: f32) -> Vec<u8> {
    match denoised {
        Some(image) => Film::image_to_srgb8(image, exposure),
        None => film.to_srgb8(exposure),
    }
}

pub fn write_png(path: &Path, film: &Film, denoised: Option<&[[f32; 3]]>, exposure: f32) -> Result<(), String> {
    use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
    let image =
        Image::from_raw(film.width, film.height, ChannelLayout::Rgba, SampleType::U8, display_pixels(film, denoised, exposure)).map_err(|e| e.to_string())?;
    let bytes = craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the film as one OpenEXR part of 32-bit float channels: `R G B` (linear radiance, not
/// exposed or tone-mapped), then the feature images in the customary layer names
/// (`albedo.R/G/B`, `N.X/Y/Z`, `Z`, `motion.X/Y`, `variance`).
pub fn write_exr(path: &Path, film: &Film, denoised: Option<&[[f32; 3]]>) -> Result<(), String> {
    use craft_exr::decode::{Channel, Compression, Samples};
    let k = if film.samples > 0 { 1.0 / film.samples as f32 } else { 0.0 };
    let plane = |name: &str, values: Vec<f32>| Channel { name: name.to_string(), samples: Samples::F32(values) };
    let of3 = |src: &[[f32; 3]], c: usize| src.iter().map(|p| p[c] * k).collect::<Vec<f32>>();
    let mut channels = Vec::new();
    for (c, name) in ["R", "G", "B"].into_iter().enumerate() {
        match denoised {
            Some(image) => {
                channels.push(plane(name, image.iter().map(|p| p[c]).collect()));
                channels.push(plane(&format!("noisy.{name}"), of3(&film.sum, c)));
            }
            None => channels.push(plane(name, of3(&film.sum, c))),
        }
        channels.push(plane(&format!("albedo.{name}"), of3(&film.albedo, c)));
    }
    for (c, name) in ["N.X", "N.Y", "N.Z"].into_iter().enumerate() {
        channels.push(plane(name, of3(&film.normal, c)));
    }
    channels.push(plane("Z", film.depth.iter().map(|d| d * k).collect()));
    channels.push(plane("motion.X", film.motion.iter().map(|m| m[0] * k).collect()));
    channels.push(plane("motion.Y", film.motion.iter().map(|m| m[1] * k).collect()));
    channels.push(plane("variance", film.variance()));
    let bytes = craft_exr::decode::write(film.width, film.height, &channels, Compression::Zip16).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The whole command: load or build the scene, render, write the image, describe the run.
pub fn run(source: &str, out: &str, args: &RenderArgs, load: impl FnOnce(&str) -> Result<Document, String>) -> Result<serde_json::Value, String> {
    let lower = out.to_ascii_lowercase();
    if !(lower.ends_with(".png") || lower.ends_with(".exr")) {
        return Err(format!("{out}: the output must be a .png or .exr file"));
    }
    let t = Instant::now();
    let scene = match source.strip_prefix("builtin:") {
        Some(name) => builtin(name)?,
        None => document_scene(&mut load(source)?, args)?,
    };
    let build_seconds = t.elapsed().as_secs_f64();
    let triangles = scene.triangle_count();
    let mut report = render(scene, args)?;
    if args.denoise != DenoiseMode::Off {
        let t = Instant::now();
        let image = report.film.denoise(args.denoise).map_err(|e| e.to_string())?;
        report.denoised = Some((image, t.elapsed().as_secs_f64()));
    }
    let filtered = report.denoised.as_ref().map(|(image, _)| image.as_slice());
    if lower.ends_with(".exr") {
        write_exr(Path::new(out), &report.film, filtered)?;
    } else {
        write_png(Path::new(out), &report.film, filtered, args.exposure)?;
    }
    let pixels = f64::from(report.film.width) * f64::from(report.film.height);
    Ok(serde_json::json!({
        "output": out,
        "device": report.device,
        "width": report.film.width,
        "height": report.film.height,
        "samples": report.film.samples,
        "triangles": triangles,
        "scene_seconds": (build_seconds * 1000.0).round() / 1000.0,
        "render_seconds": (report.seconds * 1000.0).round() / 1000.0,
        "denoise": format!("{:?}", args.denoise).to_lowercase(),
        "denoise_seconds": report.denoised.as_ref().map(|(_, s)| (s * 1000.0).round() / 1000.0),
        "million_samples_per_second": (pixels * f64::from(report.film.samples) / report.seconds.max(1e-9) / 1e6 * 100.0).round() / 100.0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_engine::Command;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_options_and_rejects_bad_ones() {
        let a =
            parse(&args(&["--width", "64", "--height", "48", "--samples", "0", "--device", "cpu", "--sky", "0.5", "--no-ground", "--yaw", "30", "--ortho"]))
                .unwrap();
        assert_eq!(
            (a.width, a.height, a.samples, a.device, a.sky, a.ground, a.yaw, a.ortho),
            (64, 48, 1, Device::Cpu, Some([0.5; 3]), false, Some(30.0), true)
        );
        assert_eq!(parse(&[]).unwrap(), RenderArgs::default());
        assert!(parse(&args(&["--width", "wide"])).is_err());
        assert!(parse(&args(&["--device", "abacus"])).is_err());
        assert!(parse(&args(&["--sky", "1,2"])).is_err());
        let lit = parse(&args(&["--sun", "35,120", "--env", "sky.hdr", "--env-rotation", "90"])).unwrap();
        assert_eq!((lit.sun, lit.env.as_deref(), lit.env_rotation), (Some([35.0, 120.0]), Some("sky.hdr"), 90.0));
        assert!(parse(&args(&["--sun", "high"])).is_err());
        assert!(load_panorama("/nonexistent/sky.hdr").is_err());
        assert!(builtin("teapot").is_err());
    }

    #[test]
    fn parses_the_denoise_option_and_writes_the_filtered_picture() {
        let one = |l: &[&str]| parse(&args(l)).unwrap().denoise;
        assert_eq!(
            (one(&[]), one(&["--denoise"]), one(&["--denoise", "svgf"]), one(&["--denoise", "--seed", "3"])),
            (DenoiseMode::Off, DenoiseMode::Nfor, DenoiseMode::Svgf, DenoiseMode::Nfor)
        );
        assert!(parse(&args(&["--denoise", "blur"])).is_err());
        let dir = std::env::temp_dir().join(format!("modelcraft-denoise-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = RenderArgs { width: 32, height: 24, samples: 4, device: Device::Cpu, denoise: DenoiseMode::Nfor, ..RenderArgs::default() };
        let exr = dir.join("c.exr");
        let report = run("builtin:cornell", exr.to_str().unwrap(), &small, |_| Err("no files".into())).unwrap();
        assert_eq!(report["denoise"], "nfor");
        assert!(report["denoise_seconds"].as_f64().is_some());
        assert!(std::fs::metadata(&exr).unwrap().len() > 0);
        let png = dir.join("c.png");
        run("builtin:cornell", png.to_str().unwrap(), &small, |_| Err("no files".into())).unwrap();
        assert!(std::fs::metadata(&png).unwrap().len() > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_the_bvh_option_and_renders_the_same_picture_either_way() {
        assert!(!parse(&args(&[])).unwrap().wide_bvh && parse(&args(&["--bvh", "wide"])).unwrap().wide_bvh);
        assert!(parse(&args(&["--bvh", "quad"])).is_err());
        let small = RenderArgs { width: 24, height: 18, samples: 4, device: Device::Cpu, ..RenderArgs::default() };
        let plain = render(builtin("showcase").unwrap(), &small).unwrap().film;
        let wide = render(builtin("showcase").unwrap(), &RenderArgs { wide_bvh: true, ..small }).unwrap().film;
        assert_eq!(plain.sum, wide.sum);
    }

    #[test]
    fn parses_the_resampling_options() {
        let a = parse(&args(&["--restir", "unbiased", "--restir-candidates", "100", "--restir-neighbors", "2"])).unwrap();
        assert_eq!((a.restir.mode, a.restir.candidates, a.restir.neighbors), (RestirMode::Unbiased, modelcraft_render::pathtrace::restir::MAX_CANDIDATES, 2));
        assert_eq!(parse(&args(&["--restir", "viewport"])).unwrap().restir.mode, RestirMode::Viewport);
        assert_eq!(parse(&[]).unwrap().restir.mode, RestirMode::Off);
        assert!(parse(&args(&["--restir", "always"])).is_err() && parse(&args(&["--restir-candidates", "many"])).is_err());
        // A render with it on produces a lit picture and says so.
        let dir = std::env::temp_dir().join(format!("modelcraft-restir-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = RenderArgs { width: 32, height: 24, samples: 4, device: Device::Cpu, restir: a.restir, ..RenderArgs::default() };
        let report = run("builtin:night-street", dir.join("n.png").to_str().unwrap(), &small, |_| Err("no files".into())).unwrap();
        assert_eq!(report["samples"], 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_a_document_and_a_builtin_scene_to_png() {
        let dir = std::env::temp_dir().join(format!("modelcraft-render-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let small = RenderArgs { width: 40, height: 30, samples: 8, device: Device::Cpu, ..RenderArgs::default() };
        let out = dir.join("cube.png");
        let report = run("scene", out.to_str().unwrap(), &small, |_| {
            let mut d = Document::new();
            d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).map_err(|e| e.to_string())?;
            Ok(d)
        })
        .unwrap();
        assert_eq!(report["triangles"], 16);
        assert_eq!(report["samples"], 8);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        let image = craft_image_codecs::decode(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (40, 30));

        let out = dir.join("cornell.png");
        let report = run("builtin:cornell", out.to_str().unwrap(), &small, |_| Err("not used".into())).unwrap();
        assert!(report["device"].as_str().unwrap().starts_with("cpu"));
        assert!(out.exists());
        // Daylight, and a panorama written here in the Radiance format (flat pixels, no run-length coding).
        let cube = |_: &str| {
            let mut d = Document::new();
            d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).map_err(|e| e.to_string())?;
            Ok(d)
        };
        let out = dir.join("day.png");
        run("scene", out.to_str().unwrap(), &RenderArgs { sun: Some([40.0, 30.0]), ..small.clone() }, cube).unwrap();
        let mut hdr = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 8\n".to_vec();
        for _ in 0..32 {
            hdr.extend_from_slice(&[128, 128, 128, 129]);
        }
        let pano = dir.join("flat.hdr");
        std::fs::write(&pano, hdr).unwrap();
        let image = load_panorama(pano.to_str().unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (8, 4));
        assert!((image.get(0, 0, false)[0] - 1.0).abs() < 0.02, "RGBE (128, 129) is 1.0");
        let out = dir.join("pano.png");
        run("scene", out.to_str().unwrap(), &RenderArgs { env: Some(pano.display().to_string()), ..small.clone() }, cube).unwrap();
        std::fs::write(&pano, b"not a picture").unwrap();
        assert!(run("scene", out.to_str().unwrap(), &RenderArgs { env: Some(pano.display().to_string()), ..small.clone() }, cube).is_err());
        // EXR: linear radiance plus the feature images, read back through craft-exr.
        let out = dir.join("cornell.exr");
        run("builtin:cornell", out.to_str().unwrap(), &small, |_| Err("not used".into())).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let part = craft_exr::decode::decode_part(&bytes, 0, &|_| true, &craft_exr::decode::Limits::none()).unwrap();
        for name in ["R", "G", "B", "albedo.R", "N.X", "Z", "motion.X", "variance"] {
            assert_eq!(part.channel(name).map(|c| c.samples.len()), Some(40 * 30), "{name}");
        }
        // The back wall's normal faces the camera (+z) and its depth is the room's.
        let centre = 8 * 40 + 20;
        assert!(part.channel("N.Z").unwrap().samples.get_f32(centre) > 0.9);
        assert!((part.channel("Z").unwrap().samples.get_f32(centre) - 5.6).abs() < 0.3);
        assert!(run("builtin:cornell", "out.jpg", &small, |_| Err("not used".into())).is_err());
        assert!(run("missing.glb", out.to_str().unwrap(), &small, |p| Err(format!("{p}: no such file"))).is_err());
        // An impossible size is an error, not a crash.
        assert!(run("builtin:cornell", out.to_str().unwrap(), &RenderArgs { width: 0, ..small.clone() }, |_| Err("not used".into())).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
