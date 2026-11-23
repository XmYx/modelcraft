//! A queue of frames: render each frame of the project's range and write numbered files.
//!
//! The queue works on its own copy of the scene (`Scene` is plain data), so the window stays
//! live while it runs on a worker thread, and it can be cancelled between passes. The same code
//! serves the app and `modelcraft-cli render --frames`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use craft_render_core::integrator::IntegratorSettings;
use craft_render_core::{DenoiseMode, Film, RenderSettings, Renderer};
use craft_render_gpu::{GpuContext, GpuRenderer};
use modelcraft_scene::stage::RenderConfig;
use modelcraft_scene::{Evaluator, Scene};

use crate::camera::Camera;
use crate::output::{self, Picture};
use crate::trace::{ExportOptions, Lighting, export_builder};

/// Where a render runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    /// A hardware GPU when there is one, otherwise the CPU.
    Auto,
    Cpu,
    Gpu,
}

/// Render a scene to a film with the device asked for; the label says what ran. `cancel` stops
/// the render early: the film then holds the samples done so far, or `Err("cancelled")` when
/// none were.
pub fn render_film(
    scene: craft_render_core::Scene,
    settings: RenderSettings,
    samples: u32,
    device: Device,
    cancel: &AtomicBool,
) -> Result<(Film, String), String> {
    let gpu = match device {
        Device::Cpu => None,
        Device::Gpu => Some(GpuContext::new().ok_or("no graphics card is available; use the processor instead")?),
        Device::Auto => GpuContext::new().filter(|c| !c.caps.software),
    };
    if let Some(ctx) = gpu {
        let attempt = (|| -> Result<(Film, String), String> {
            let mut r = GpuRenderer::new(&ctx, &scene, settings).map_err(|e| e.to_string())?;
            while r.samples() < samples && !cancel.load(Ordering::Relaxed) {
                let n = r.samples().clamp(1, 64).min(samples - r.samples());
                r.render(&ctx, n).map_err(|e| e.to_string())?;
            }
            if r.samples() == 0 {
                return Err("cancelled".into());
            }
            Ok((r.film(&ctx).map_err(|e| e.to_string())?, format!("GPU: {}", ctx.caps.adapter)))
        })();
        match attempt {
            Ok(done) => return Ok(done),
            Err(e) if device == Device::Gpu || e == "cancelled" => return Err(e),
            Err(e) => log::warn!("the GPU could not render this ({e}); using the CPU"),
        }
    }
    let mut r = Renderer::new(Arc::new(scene), settings).map_err(|e| e.to_string())?;
    while r.film().samples < samples {
        let n = r.film().samples.clamp(1, 16).min(samples - r.film().samples);
        if !r.render(n, cancel) {
            break;
        }
    }
    if r.film().samples == 0 {
        return Err("cancelled".into());
    }
    Ok((r.film().clone(), format!("CPU: {} threads", std::thread::available_parallelism().map_or(1, usize::from))))
}

/// What a queue renders: the project as it was when the queue started.
#[derive(Clone)]
pub struct Job {
    pub scene: Scene,
    /// The view renders use when the project has no render camera.
    pub viewport: Camera,
    pub lighting: Lighting,
    pub config: RenderConfig,
    pub device: Device,
    /// Write files (the render queue) or only keep the pictures.
    pub write: bool,
}

/// One finished frame.
#[derive(Clone, Debug)]
pub struct Frame {
    pub frame: i32,
    pub path: Option<PathBuf>,
    pub picture: Picture,
}

/// What a running queue tells the window; read from any thread.
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU32,
    pub total: AtomicU32,
}

/// Render every frame of the job's range. `on_frame` gets each finished frame (with its file, if
/// written); a failure to render or write stops the queue with the reason, naming the frame.
/// Returns the files written. Cancelling stops quietly: what is done stays done.
pub fn run(job: &Job, cancel: &AtomicBool, progress: &Progress, on_frame: &mut dyn FnMut(Frame)) -> Result<Vec<PathBuf>, String> {
    let config = job.config.sanitized();
    let frames = config.frames();
    progress.total.store(frames.len() as u32, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    if job.write && config.output.trim().is_empty() {
        return Err("choose where the frames are written first".into());
    }
    let mut written = Vec::new();
    for frame in frames {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let start = Instant::now();
        let mut scene = job.scene.clone();
        scene.set_frame(f64::from(frame));
        let options = ExportOptions { lighting: job.lighting.clone(), aspect: config.width as f32 / config.height as f32, ..ExportOptions::default() };
        let mut eval = Evaluator::new();
        let built = export_builder(&scene, &mut eval, &job.viewport, &options).map_err(|e| format!("frame {frame}: {e}"))?.build();
        let mut settings = RenderSettings { width: config.width, height: config.height, ..RenderSettings::default() };
        settings.integrator = IntegratorSettings { max_depth: config.bounces, ..IntegratorSettings::default() };
        let (film, label) = match render_film(built, settings, config.samples, job.device, cancel) {
            Ok(done) => done,
            Err(e) if e == "cancelled" => break,
            Err(e) => return Err(format!("frame {frame}: {e}")),
        };
        let filtered = if config.denoise { film.denoise(DenoiseMode::Nfor).ok() } else { None };
        let path = if job.write {
            let path = output::numbered(Path::new(&config.output), frame, config.format);
            output::write_picture(&path, &film, filtered.as_deref(), &config).map_err(|e| format!("frame {frame}: {e}"))?;
            written.push(path.clone());
            Some(path)
        } else {
            None
        };
        let rect = output::crop_rect(film.width, film.height, config.region);
        let pixels = output::display_pixels(&film, filtered.as_deref(), config.exposure);
        let picture = Picture {
            label: format!("Frame {frame} ({label})"),
            size: [rect.2, rect.3],
            pixels: crop_rgba(&pixels, film.width, rect),
            samples: film.samples,
            seconds: start.elapsed().as_secs_f64(),
        };
        progress.done.fetch_add(1, Ordering::Relaxed);
        on_frame(Frame { frame, path, picture });
    }
    Ok(written)
}

/// Cut a rectangle out of RGBA bytes.
pub fn crop_rgba(pixels: &[u8], width: u32, rect: (u32, u32, u32, u32)) -> Vec<u8> {
    let (x, y, w, h) = rect;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let start = (row as usize * width as usize + x as usize) * 4;
        if let Some(line) = pixels.get(start..start + w as usize * 4) {
            out.extend_from_slice(line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_scene::{ObjectKind, Primitive, Source, Transform};

    fn cube_scene() -> Scene {
        let mut s = Scene::new();
        let id = s
            .add(
                "Cube",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: glam::Vec3::splat(2.0), segments: 1 } }, steps: vec![] },
                None,
            )
            .unwrap();
        s.edit(id).unwrap().transform = Transform::default();
        s
    }

    fn job(config: RenderConfig, write: bool) -> Job {
        Job { scene: cube_scene(), viewport: Camera::default(), lighting: Lighting::Studio, config, device: Device::Cpu, write }
    }

    fn small(frames: (i32, i32), output: &str) -> RenderConfig {
        RenderConfig { width: 32, height: 24, samples: 2, frame_start: frames.0, frame_end: frames.1, output: output.into(), ..Default::default() }
    }

    #[test]
    fn a_frame_range_renders_numbered_files_and_reports_each_frame() {
        let dir = std::env::temp_dir().join(format!("mc-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("shot.png");
        let j = job(small((3, 5), &base.display().to_string()), true);
        let progress = Progress::default();
        let mut seen = Vec::new();
        let files = run(&j, &AtomicBool::new(false), &progress, &mut |f| seen.push((f.frame, f.picture.size))).unwrap();
        assert_eq!(seen, vec![(3, [32, 24]), (4, [32, 24]), (5, [32, 24])]);
        let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["shot_0003.png", "shot_0004.png", "shot_0005.png"]);
        assert!(files.iter().all(|f| std::fs::metadata(f).unwrap().len() > 0));
        assert_eq!((progress.done.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed)), (3, 3));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_queue_can_be_cancelled_between_frames_and_needs_a_place_to_write() {
        let j = job(small((0, 9), ""), true);
        let progress = Progress::default();
        assert!(run(&j, &AtomicBool::new(false), &progress, &mut |_| {}).unwrap_err().contains("where the frames are written"));
        // Cancelled at once, nothing renders; cancelled after the first frame, one is done.
        let cancel = AtomicBool::new(true);
        let none = run(&job(small((0, 9), ""), false), &cancel, &progress, &mut |_| {}).unwrap();
        assert!(none.is_empty() && progress.done.load(Ordering::Relaxed) == 0);
        let cancel = AtomicBool::new(false);
        let mut count = 0;
        run(&job(small((0, 9), ""), false), &cancel, &progress, &mut |_| {
            count += 1;
            cancel.store(true, Ordering::Relaxed);
        })
        .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn keys_change_the_picture_from_frame_to_frame_and_a_region_cuts_it() {
        let mut j = job(RenderConfig { region: Some([0.0, 0.0, 0.5, 0.5]), ..small((0, 10), "") }, false);
        let id = j.scene.objects()[0].id;
        j.scene.set_key(id, "material.base_color.0", 0.0, Some(0.05)).unwrap();
        j.scene.set_key(id, "material.base_color.0", 10.0, Some(0.95)).unwrap();
        let mut pictures = Vec::new();
        let config = RenderConfig { frame_step: 10, ..j.config.clone() };
        j.config = config;
        run(&j, &AtomicBool::new(false), &Progress::default(), &mut |f| pictures.push(f.picture)).unwrap();
        assert_eq!(pictures.len(), 2);
        assert_eq!(pictures[0].size, [16, 12], "the region is a quarter of the picture");
        let red = |p: &Picture| p.pixels.chunks(4).map(|c| u64::from(c[0])).sum::<u64>();
        assert_ne!(red(&pictures[0]), red(&pictures[1]), "frame 0 and frame 10 differ");
    }

    #[test]
    fn cropping_bytes_is_bounded_by_the_picture() {
        let px: Vec<u8> = (0..4 * 4 * 4).map(|i| i as u8).collect();
        assert_eq!(crop_rgba(&px, 4, (1, 1, 2, 2)).len(), 16);
        assert!(crop_rgba(&px, 4, (3, 3, 5, 5)).is_empty(), "rows that do not fit the picture are left out");
    }
}
