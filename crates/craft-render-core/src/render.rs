//! Progressive rendering: a film that accumulates samples and the parallel pass that fills it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec2;
use rayon::prelude::*;

use crate::camera::PreparedCamera;
use crate::integrator::{IntegratorSettings, radiance};
use crate::sampler::Sampler;
use crate::scene::{Scene, SceneError};

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
}

impl Default for RenderSettings {
    fn default() -> Self {
        RenderSettings { width: 640, height: 360, integrator: IntegratorSettings::default(), seed: 0 }
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
        Ok(Film { width, height, sum: vec![[0.0; 3]; width as usize * height as usize], samples: 0 })
    }

    /// Mean radiance per pixel (linear RGB).
    pub fn mean(&self) -> Vec<[f32; 3]> {
        let k = if self.samples > 0 { 1.0 / self.samples as f32 } else { 0.0 };
        self.sum.iter().map(|p| [p[0] * k, p[1] * k, p[2] * k]).collect()
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
    scratch: Vec<[f32; 3]>,
}

impl Renderer {
    pub fn new(scene: Arc<Scene>, settings: RenderSettings) -> Result<Renderer, SceneError> {
        let film = Film::new(settings.width, settings.height)?;
        let camera = scene.camera.prepare(settings.width as f32 / settings.height as f32);
        let scratch = vec![[0.0; 3]; film.sum.len()];
        Ok(Renderer { scene, settings, camera, film, scratch })
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

    /// Add `samples` samples to every pixel. Returns `false` (and leaves the film as it was)
    /// when `cancel` was set before the pass finished.
    pub fn render(&mut self, samples: u32, cancel: &AtomicBool) -> bool {
        let samples = samples.min(MAX_SAMPLES - self.film.samples);
        if samples == 0 {
            return true;
        }
        let (w, h) = (self.film.width as usize, self.film.height as usize);
        let first = self.film.samples;
        let (scene, camera, settings) = (&*self.scene, &self.camera, &self.settings);
        self.scratch.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            for (x, px) in row.iter_mut().enumerate() {
                let seed = Sampler::pixel_seed(x as u32, y as u32, settings.seed);
                let mut acc = glam::Vec3A::ZERO;
                for s in first..first + samples {
                    let mut sampler = Sampler::new(seed, s);
                    let jitter = sampler.next2();
                    let lens = sampler.next2();
                    let ndc = Vec2::new((x as f32 + jitter.x) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + jitter.y) / h as f32 * 2.0);
                    acc += radiance(scene, camera.ray(ndc, lens), &mut sampler, &settings.integrator);
                }
                *px = acc.to_array();
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        for (sum, add) in self.film.sum.iter_mut().zip(&self.scratch) {
            for k in 0..3 {
                sum[k] += add[k];
            }
        }
        self.film.samples += samples;
        true
    }
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
