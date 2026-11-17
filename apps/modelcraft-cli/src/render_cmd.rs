//! `modelcraft-cli render`: a path-traced image of a project, model file or built-in scene.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use modelcraft_engine::Document;
use modelcraft_render::pathtrace::integrator::IntegratorSettings;
use modelcraft_render::pathtrace::{Film, RenderSettings, Renderer, Scene, scenes};
use modelcraft_render::pathtrace_gpu::{GpuContext, GpuRenderer};
use modelcraft_render::trace::{ExportOptions, Lighting, export_scene, scene_bounds};
use modelcraft_render::{Camera, Projection};

pub const USAGE: &str = "  render <file|builtin:NAME> -o <out.png> [options]
                                    path-trace an image. Built-in scenes: cornell, materials, furnace
      --width N --height N          image size (default 960×540)
      --samples N                   samples per pixel (default 64)
      --depth N                     bounces (default 12)
      --device auto|cpu|gpu         auto uses a hardware GPU when there is one
      --yaw DEG --pitch DEG --fov DEG   camera around the framed scene; --ortho for no perspective
      --exposure EV                 brightness in stops (default 0)
      --sky R,G,B                   uniform sky light instead of the studio light; --no-ground
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
    pub ground: bool,
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
            ground: true,
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
    Ok(RenderArgs {
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
    })
}

/// A built-in scene by name.
pub fn builtin(name: &str) -> Result<Scene, String> {
    match name {
        "cornell" => scenes::cornell_box(),
        "materials" => scenes::material_row(),
        "furnace" => scenes::furnace(modelcraft_render::pathtrace::Material::metal(glam::Vec3::ONE, 0.5)),
        other => return Err(format!("unknown built-in scene {other:?} (cornell, materials, furnace)")),
    }
    .map_err(|e| e.to_string())
}

/// The path tracer's scene for a document, seen by a camera that frames everything.
pub fn document_scene(d: &mut Document, args: &RenderArgs) -> Result<Scene, String> {
    d.sync_sculpt();
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
    let lighting = args.sky.map_or(Lighting::Studio, |s| Lighting::Sky(glam::Vec3::from_array(s)));
    export_scene(&d.scene, &mut d.eval, &cam, &ExportOptions { lighting, ground: args.ground }).map_err(|e| e.to_string())
}

/// What a render did, for the JSON line the command prints.
pub struct Report {
    pub film: Film,
    pub device: String,
    pub seconds: f64,
}

/// Render `scene` with the device the arguments ask for.
pub fn render(scene: Scene, args: &RenderArgs) -> Result<Report, String> {
    let settings = RenderSettings {
        width: args.width,
        height: args.height,
        integrator: IntegratorSettings { max_depth: args.depth, ..IntegratorSettings::default() },
        seed: args.seed,
    };
    let gpu = match args.device {
        Device::Cpu => None,
        Device::Gpu => Some(GpuContext::new().ok_or("no GPU adapter is available; use --device cpu")?),
        Device::Auto => GpuContext::new().filter(|c| !c.caps.software),
    };
    if let Some(ctx) = gpu {
        let attempt = (|| -> Result<Report, String> {
            let mut r = GpuRenderer::new(&ctx, &scene, settings).map_err(|e| e.to_string())?;
            let t = Instant::now();
            r.render(&ctx, args.samples).map_err(|e| e.to_string())?;
            let film = r.film(&ctx).map_err(|e| e.to_string())?;
            Ok(Report { film, device: format!("gpu: {} ({})", ctx.caps.adapter, ctx.caps.backend), seconds: t.elapsed().as_secs_f64() })
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
    Ok(Report { film: r.film().clone(), device: format!("cpu: {threads} threads"), seconds: t.elapsed().as_secs_f64() })
}

pub fn write_png(path: &Path, film: &Film, exposure: f32) -> Result<(), String> {
    use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
    let image = Image::from_raw(film.width, film.height, ChannelLayout::Rgba, SampleType::U8, film.to_srgb8(exposure)).map_err(|e| e.to_string())?;
    let bytes = craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// The whole command: load or build the scene, render, write the image, describe the run.
pub fn run(source: &str, out: &str, args: &RenderArgs, load: impl FnOnce(&str) -> Result<Document, String>) -> Result<serde_json::Value, String> {
    if !out.to_ascii_lowercase().ends_with(".png") {
        return Err(format!("{out}: the output must be a .png file"));
    }
    let t = Instant::now();
    let scene = match source.strip_prefix("builtin:") {
        Some(name) => builtin(name)?,
        None => document_scene(&mut load(source)?, args)?,
    };
    let build_seconds = t.elapsed().as_secs_f64();
    let triangles = scene.triangle_count();
    let report = render(scene, args)?;
    write_png(Path::new(out), &report.film, args.exposure)?;
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
        assert!(builtin("teapot").is_err());
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
        assert!(run("builtin:cornell", "out.jpg", &small, |_| Err("not used".into())).is_err());
        assert!(run("missing.glb", out.to_str().unwrap(), &small, |p| Err(format!("{p}: no such file"))).is_err());
        // An impossible size is an error, not a crash.
        assert!(run("builtin:cornell", out.to_str().unwrap(), &RenderArgs { width: 0, ..small.clone() }, |_| Err("not used".into())).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
