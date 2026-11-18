//! Progressive rendering: a film that accumulates samples and the parallel pass that fills it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec2;
use rayon::prelude::*;

use crate::camera::PreparedCamera;
use crate::guided::{GuideContext, radiance_guided};
use crate::integrator::{IntegratorSettings, radiance_and_features};
use crate::sampler::Sampler;
use crate::scene::{Scene, SceneError};
use craft_render_guide::{Guide, GuideConfig};

/// Largest image side.
pub const MAX_IMAGE_SIDE: u32 = 16_384;
/// Most samples per pixel one film accumulates.
pub const MAX_SAMPLES: u32 = 1 << 24;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub integrator: IntegratorSettings,
    /// Changes every random choice of the render; the same seed gives the same image.
    pub seed: u32,
    /// How pixels share the sample sequence.
    pub pixel_order: PixelOrder,
    /// The camera of the previous frame, for the motion feature image (`None`: no motion).
    pub previous_camera: Option<crate::camera::Camera>,
}

/// How pixels draw from the sample sequence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelOrder {
    /// One sequence shared along the z-curve: neighbouring pixels get complementary samples.
    #[default]
    ZCurve,
    /// Every pixel scrambles the sequence on its own (white-noise error).
    Independent,
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings { width: 640, height: 360, integrator: IntegratorSettings::default(), seed: 0, pixel_order: PixelOrder::ZCurve, previous_camera: None }
    }
}

/// Accumulated radiance per pixel, rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Film {
    pub width: u32,
    pub height: u32,
    /// Sum of the samples of each pixel (linear RGB).
    pub sum: Vec<[f32; 3]>,
    /// Samples accumulated in every pixel.
    pub samples: u32,
    /// Sums of the first-hit features, for denoising and compositing: base colour, world-space
    /// shading normal, distance from the camera, and screen motion in pixels since the previous
    /// frame's camera.
    pub albedo: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
    pub depth: Vec<f32>,
    pub motion: Vec<[f32; 2]>,
    /// Sum of the squared luminance of the samples (with `sum`, the per-pixel variance).
    pub luminance_sq: Vec<f32>,
}

/// The transfer function of sRGB displays (IEC 61966-2-1).
#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

impl Film {
    pub fn new(width: u32, height: u32) -> Result<Film, SceneError> {
        if width == 0 || height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
            return Err(SceneError::BadImageSize(width, height));
        }
        let n = width as usize * height as usize;
        Ok(Film {
            width,
            height,
            sum: vec![[0.0; 3]; n],
            samples: 0,
            albedo: vec![[0.0; 3]; n],
            normal: vec![[0.0; 3]; n],
            depth: vec![0.0; n],
            motion: vec![[0.0; 2]; n],
            luminance_sq: vec![0.0; n],
        })
    }

    /// Mean radiance per pixel (linear RGB).
    pub fn mean(&self) -> Vec<[f32; 3]> {
        let k = if self.samples > 0 { 1.0 / self.samples as f32 } else { 0.0 };
        self.sum.iter().map(|p| [p[0] * k, p[1] * k, p[2] * k]).collect()
    }

    /// Variance of each pixel's mean luminance, estimated from its samples (zero below two samples).
    pub fn variance(&self) -> Vec<f32> {
        let n = self.samples as f32;
        if self.samples < 2 {
            return vec![0.0; self.sum.len()];
        }
        self.sum
            .iter()
            .zip(&self.luminance_sq)
            .map(|(s, sq)| {
                let mean = crate::math::luminance(glam::Vec3A::from_array(*s)) / n;
                ((sq / n - mean * mean).max(0.0)) / (n - 1.0)
            })
            .collect()
    }

    /// A feature image divided by the sample count.
    pub fn mean_of(&self, sums: &[[f32; 3]]) -> Vec<[f32; 3]> {
        let k = if self.samples > 0 { 1.0 / self.samples as f32 } else { 0.0 };
        sums.iter().map(|p| [p[0] * k, p[1] * k, p[2] * k]).collect()
    }

    /// The image as 8-bit sRGB with opaque alpha, after scaling by `2^exposure`.
    pub fn to_srgb8(&self, exposure: f32) -> Vec<u8> {
        let gain = if exposure.is_finite() { 2f32.powf(exposure.clamp(-32.0, 32.0)) } else { 1.0 };
        let k = if self.samples > 0 { gain / self.samples as f32 } else { 0.0 };
        let mut out = Vec::with_capacity(self.sum.len() * 4);
        for p in &self.sum {
            for c in p {
                out.push((linear_to_srgb(c * k) * 255.0 + 0.5) as u8);
            }
            out.push(255);
        }
        out
    }
}

/// Renders a scene progressively on the CPU. This is the reference implementation.
pub struct Renderer {
    scene: Arc<Scene>,
    settings: RenderSettings,
    camera: PreparedCamera,
    film: Film,
    previous: Option<PreparedCamera>,
    scratch: Vec<PixelSums>,
    /// The learned sampling state, when the settings ask for guiding or adaptive splitting.
    guide: Option<Guide>,
    /// Per pixel, the root of the mean squared luminance of its samples so far, blurred (what a
    /// path's expected contribution is compared with); empty until the first learning pass ends.
    reference: Vec<f32>,
    /// Sample count at which the guide is next updated (learning passes double in length).
    next_update: u32,
}

/// What one pass adds to one pixel.
#[derive(Clone, Copy, Default)]
struct PixelSums {
    radiance: [f32; 3],
    albedo: [f32; 3],
    normal: [f32; 3],
    depth: f32,
    motion: [f32; 2],
    luminance_sq: f32,
}

impl Renderer {
    pub fn new(scene: Arc<Scene>, settings: RenderSettings) -> Result<Renderer, SceneError> {
        let film = Film::new(settings.width, settings.height)?;
        let camera = scene.camera.prepare(settings.width as f32 / settings.height as f32);
        let scratch = vec![PixelSums::default(); film.sum.len()];
        let previous = settings.previous_camera.map(|c| c.prepare(settings.width as f32 / settings.height as f32));
        let learns = settings.integrator.guiding || settings.integrator.adaptive_splitting;
        let guide = learns.then(|| Guide::new(scene.bounds.min, scene.bounds.max, GuideConfig::default()));
        Ok(Renderer { scene, settings, camera, film, previous, scratch, guide, reference: Vec::new(), next_update: 1 })
    }

    pub fn film(&self) -> &Film {
        &self.film
    }
    pub fn scene(&self) -> &Arc<Scene> {
        &self.scene
    }
    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// The learned sampling state, if this render has one.
    pub fn guide(&self) -> Option<&Guide> {
        self.guide.as_ref()
    }

    /// Add `samples` samples to every pixel. Returns `false` when `cancel` was set before the
    /// pass finished; the film then holds what it held before (with guiding or adaptive
    /// splitting: plus the learning passes that had finished).
    pub fn render(&mut self, samples: u32, cancel: &AtomicBool) -> bool {
        let mut left = samples.min(MAX_SAMPLES - self.film.samples);
        if self.guide.is_none() {
            return left == 0 || self.pass(left, cancel);
        }
        // Learning passes end at 1, 2, 4, 8, … samples; each samples from what the passes before
        // it recorded.
        while left > 0 {
            let n = self.next_update.saturating_sub(self.film.samples).clamp(1, left);
            if !self.pass(n, cancel) {
                return false;
            }
            left -= n;
            if self.film.samples >= self.next_update {
                self.next_update = self.next_update.saturating_mul(2);
                if let Some(g) = &mut self.guide {
                    g.update();
                }
                self.reference = sample_magnitude(&self.film);
            }
        }
        true
    }

    fn pass(&mut self, samples: u32, cancel: &AtomicBool) -> bool {
        let (w, h) = (self.film.width as usize, self.film.height as usize);
        let first = self.film.samples;
        let (scene, camera, settings, previous) = (&*self.scene, &self.camera, &self.settings, &self.previous);
        let (guide, reference) = (self.guide.as_ref(), &self.reference);
        let z_bits = crate::sampler::z_order_bits(self.film.width, self.film.height);
        self.scratch.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            for (x, px) in row.iter_mut().enumerate() {
                let seed = Sampler::pixel_seed(x as u32, y as u32, settings.seed);
                let mut acc = PixelSums::default();
                let mut rgb = glam::Vec3A::ZERO;
                for s in first..first + samples {
                    let ordered =
                        if settings.pixel_order == PixelOrder::ZCurve { Sampler::z_ordered(x as u32, y as u32, s, z_bits, settings.seed) } else { None };
                    let mut sampler = ordered.unwrap_or_else(|| Sampler::new(seed, s));
                    let jitter = sampler.next2();
                    let lens = sampler.next2();
                    let ndc = Vec2::new((x as f32 + jitter.x) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + jitter.y) / h as f32 * 2.0);
                    let (l, f) = match guide {
                        Some(guide) => {
                            let ctx = GuideContext { guide, reference: reference.get(y * w + x).copied().unwrap_or(0.0), learn: true };
                            radiance_guided(scene, camera.ray(ndc, lens), &mut sampler, &settings.integrator, &ctx)
                        }
                        None => radiance_and_features(scene, camera.ray(ndc, lens), &mut sampler, &settings.integrator),
                    };
                    rgb += l;
                    let lum = crate::math::luminance(l);
                    acc.luminance_sq += lum * lum;
                    for k in 0..3 {
                        acc.albedo[k] += f.albedo[k];
                        acc.normal[k] += f.normal[k];
                    }
                    acc.depth += f.depth;
                    if let (Some(p), Some(prev)) = (f.position, previous)
                        && let Some(before) = prev.project(p)
                    {
                        acc.motion[0] += (ndc.x - before.x) * 0.5 * w as f32;
                        acc.motion[1] += (before.y - ndc.y) * 0.5 * h as f32;
                    }
                }
                acc.radiance = rgb.to_array();
                *px = acc;
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let film = &mut self.film;
        for (i, add) in self.scratch.iter().enumerate() {
            for k in 0..3 {
                film.sum[i][k] += add.radiance[k];
                film.albedo[i][k] += add.albedo[k];
                film.normal[i][k] += add.normal[k];
            }
            film.depth[i] += add.depth;
            film.motion[i][0] += add.motion[0];
            film.motion[i][1] += add.motion[1];
            film.luminance_sq[i] += add.luminance_sq;
        }
        self.film.samples += samples;
        true
    }
}

/// How large the samples of each pixel have been: the root of their mean squared luminance,
/// averaged over 5 × 5 pixels and kept above a tenth of the image's mean (so dark pixels do not
/// ask for endless splitting). Empty for an empty film.
fn sample_magnitude(film: &Film) -> Vec<f32> {
    let (w, h) = (film.width as usize, film.height as usize);
    if film.samples == 0 {
        return Vec::new();
    }
    let k = 1.0 / film.samples as f32;
    let lum: Vec<f32> = film.luminance_sq.iter().map(|sq| if sq.is_finite() { (sq * k).max(0.0) } else { 0.0 }).collect();
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; src.len()];
        out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            for (x, o) in row.iter_mut().enumerate() {
                let (mut acc, mut n) = (0.0f32, 0.0f32);
                for d in -2i64..=2 {
                    let (xx, yy) = if horizontal { (x as i64 + d, y as i64) } else { (x as i64, y as i64 + d) };
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        acc += src.get(yy as usize * w + xx as usize).copied().unwrap_or(0.0);
                        n += 1.0;
                    }
                }
                *o = acc / n.max(1.0);
            }
        });
        out
    };
    let mut out: Vec<f32> = blur(&blur(&lum, true), false).into_iter().map(f32::sqrt).collect();
    let mean = (out.iter().map(|v| f64::from(*v)).sum::<f64>() / out.len().max(1) as f64) as f32;
    if !(mean.is_finite() && mean > 0.0) {
        return Vec::new();
    }
    for v in &mut out {
        *v = if v.is_finite() { v.max(0.1 * mean) } else { mean };
    }
    out
}

/// Render `samples` samples per pixel in one call.
pub fn render_image(scene: Arc<Scene>, settings: RenderSettings, samples: u32) -> Result<Film, SceneError> {
    let mut r = Renderer::new(scene, settings)?;
    r.render(samples, &AtomicBool::new(false));
    Ok(r.film)
}

/// Mean squared error between two images of the same size (over all channels).
pub fn mse(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return f64::INFINITY;
    }
    let mut acc = 0.0f64;
    for (p, q) in a.iter().zip(b) {
        for k in 0..3 {
            let d = f64::from(p[k]) - f64::from(q[k]);
            acc += d * d;
        }
    }
    acc / (a.len() * 3) as f64
}

/// Relative mean squared error: squared error divided by the squared reference (plus a small
/// constant), so dark and bright regions count alike.
pub fn relative_mse(image: &[[f32; 3]], reference: &[[f32; 3]]) -> f64 {
    if image.len() != reference.len() || image.is_empty() {
        return f64::INFINITY;
    }
    let mut acc = 0.0f64;
    for (p, q) in image.iter().zip(reference) {
        for k in 0..3 {
            let (a, b) = (f64::from(p[k]), f64::from(q[k]));
            acc += (a - b) * (a - b) / (b * b + 1e-2);
        }
    }
    acc / (image.len() * 3) as f64
}

/// Mean of every channel of an image.
pub fn mean_rgb(image: &[[f32; 3]]) -> [f64; 3] {
    let mut acc = [0.0f64; 3];
    for p in image {
        for k in 0..3 {
            acc[k] += f64::from(p[k]);
        }
    }
    let n = image.len().max(1) as f64;
    [acc[0] / n, acc[1] / n, acc[2] / n]
}
