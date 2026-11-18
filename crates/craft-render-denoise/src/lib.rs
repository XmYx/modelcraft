//! Denoising for Monte Carlo renderings, without trained models.
//!
//! Two filters share one input, a [`Frame`] of the image and the feature images the renderer
//! writes with it (albedo, normal, depth, and the variance of each pixel's mean):
//!
//! - [`svgf::Svgf`] for the interactive view: the spatiotemporal variance-guided filter of
//!   Schied et al., "Spatiotemporal Variance-Guided Filtering: Real-Time Reconstruction for
//!   Path-Traced Global Illumination" (HPG 2017). Frames are accumulated through motion vectors,
//!   the variance of the result is tracked, and an edge-avoiding à-trous wavelet filter, whose
//!   luminance edge-stopping scales with that variance, smooths what is left.
//! - [`nfor::nfor`] for final frames: weighted first-order regression after Bitterli et al.,
//!   "Nonlinearly Weighted First-order Regression for Denoising Monte Carlo Renderings" (EGSR
//!   2016). Each pixel fits a plane through its neighbourhood (in the features: normal, depth and
//!   screen position), weighted by how similar the neighbours' patches are (the variance-aware
//!   distance of Rousselle et al. 2012), and the two independent half renderings filter each
//!   other so that the noise of the weights is not correlated with the noise they average.
//!
//! Both divide the image by its albedo first and multiply it back after, so texture detail is not
//! smoothed away, and neither needs anything but the renderer's own images. They work on plain
//! slices so any renderer can feed them.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod nfor;
pub mod svgf;

use std::fmt;

/// Largest image (in pixels) either filter accepts.
pub const MAX_PIXELS: usize = 1 << 26;

/// Luminance of the albedo below which a pixel is not divided by it by default. Emitters and
/// very dark materials are not modulated by their base colour, and a small divisor would turn
/// their radiance into a huge value that a filter then spreads over its neighbours.
pub const DEFAULT_ALBEDO_FLOOR: f32 = 0.1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DenoiseError {
    /// The image has no pixels or more than [`MAX_PIXELS`].
    BadSize(usize, usize),
    /// An input has the wrong number of pixels.
    Mismatch(&'static str),
}

impl fmt::Display for DenoiseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DenoiseError::BadSize(w, h) => write!(f, "the image size {w}×{h} cannot be denoised"),
            DenoiseError::Mismatch(what) => write!(f, "the {what} image does not match the size of the frame"),
        }
    }
}

impl std::error::Error for DenoiseError {}

/// One rendering and what the renderer knew about its first surfaces. Every image has
/// `width × height` pixels, rows from the top, and holds per-pixel means over all samples.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    /// Linear radiance.
    pub color: &'a [[f32; 3]],
    /// Base colour of the first surface.
    pub albedo: &'a [[f32; 3]],
    /// World-space shading normal (any length; zero where nothing was hit).
    pub normal: &'a [[f32; 3]],
    /// Distance from the camera (zero where nothing was hit).
    pub depth: &'a [f32],
    /// Variance of each pixel's mean luminance, if the renderer estimated it.
    pub variance: Option<&'a [f32]>,
}

impl Frame<'_> {
    pub fn pixels(&self) -> usize {
        self.width * self.height
    }

    /// Check sizes.
    pub fn check(&self) -> Result<(), DenoiseError> {
        let n = self.width.saturating_mul(self.height);
        if self.width == 0 || self.height == 0 || n > MAX_PIXELS {
            return Err(DenoiseError::BadSize(self.width, self.height));
        }
        for (len, what) in [(self.color.len(), "colour"), (self.albedo.len(), "albedo"), (self.normal.len(), "normal"), (self.depth.len(), "depth")] {
            if len != n {
                return Err(DenoiseError::Mismatch(what));
            }
        }
        if self.variance.is_some_and(|v| v.len() != n) {
            return Err(DenoiseError::Mismatch("variance"));
        }
        Ok(())
    }
}

#[inline]
pub(crate) fn luminance(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A finite non-negative number, or zero.
#[inline]
pub(crate) fn clean(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// The albedo a pixel is divided by: itself (each channel at least 0.02), or 1 where its luminance
/// is below `floor` or it is not a number.
#[inline]
pub(crate) fn albedo_floor(a: [f32; 3], floor: f32) -> [f32; 3] {
    if !a.iter().all(|v| v.is_finite()) || luminance(a) < floor {
        return [1.0; 3];
    }
    a.map(|v| v.clamp(0.02, 1.0e3))
}

/// The unit normal of a pixel, or zero where there is none.
#[inline]
pub(crate) fn unit(n: [f32; 3]) -> [f32; 3] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l.is_finite() && l > 1e-6 { [n[0] / l, n[1] / l, n[2] / l] } else { [0.0; 3] }
}

#[inline]
pub(crate) fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Relative mean squared error between two images (the benchmark's measure, ε = 0.01).
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

#[cfg(test)]
pub(crate) mod test_images {
    //! Synthetic renderings with a known answer.

    use super::*;

    pub struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
        /// Roughly normal (sum of four uniforms).
        pub fn normal(&mut self) -> f32 {
            (self.next() + self.next() + self.next() + self.next() - 2.0) * 1.732
        }
    }

    pub struct Scene {
        pub w: usize,
        pub h: usize,
        pub clean: Vec<[f32; 3]>,
        pub albedo: Vec<[f32; 3]>,
        pub normal: Vec<[f32; 3]>,
        pub depth: Vec<f32>,
    }

    /// Two walls meeting in a vertical edge, each lit with a gentle gradient and textured by a
    /// checker of albedo, in front of a sky.
    pub fn walls(w: usize, h: usize) -> Scene {
        let n = w * h;
        let (mut clean, mut albedo, mut normal, mut depth) = (vec![[0.0; 3]; n], vec![[0.0; 3]; n], vec![[0.0; 3]; n], vec![0.0; n]);
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if y < h / 6 {
                    albedo[i] = [0.4, 0.6, 1.0];
                    clean[i] = [0.4, 0.6, 1.0];
                    continue;
                }
                let left = x < w / 2;
                normal[i] = if left { [1.0, 0.0, 0.0] } else { [0.0, 0.0, 1.0] };
                depth[i] = if left { 3.0 + 0.01 * x as f32 } else { 5.0 };
                let check = if (x / 6 + y / 6) % 2 == 0 { 0.8 } else { 0.3 };
                albedo[i] = [check, check * 0.8, check * 0.6];
                let light = if left { 1.0 + 0.5 * (y as f32 / h as f32) } else { 0.6 + 0.8 * (x as f32 / w as f32) };
                clean[i] = [albedo[i][0] * light, albedo[i][1] * light, albedo[i][2] * light];
            }
        }
        Scene { w, h, clean, albedo, normal, depth }
    }

    impl Scene {
        pub fn frame<'a>(&'a self, color: &'a [[f32; 3]], variance: Option<&'a [f32]>) -> Frame<'a> {
            Frame { width: self.w, height: self.h, color, albedo: &self.albedo, normal: &self.normal, depth: &self.depth, variance }
        }

        /// The clean image plus noise of standard deviation `sigma × radiance` (a quarter of it
        /// independent of the radiance), and the variance image a renderer would report.
        pub fn noisy(&self, sigma: f32, seed: u64) -> (Vec<[f32; 3]>, Vec<f32>) {
            let mut rng = Rng(seed | 1);
            let mut var = Vec::with_capacity(self.clean.len());
            let img = self
                .clean
                .iter()
                .map(|c| {
                    let s = sigma * (0.25 + luminance(*c));
                    var.push(s * s);
                    [c[0] + s * rng.normal(), c[1] + s * rng.normal(), c[2] + s * rng.normal()]
                })
                .collect();
            (img, var)
        }
    }
}
