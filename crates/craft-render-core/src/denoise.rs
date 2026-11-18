//! Denoising a [`Film`]: the filters of `craft-render-denoise` fed with the film's own images.

use craft_render_denoise::nfor::{Halves, NforSettings, nfor};
use craft_render_denoise::svgf::{Svgf, SvgfSettings};
use craft_render_denoise::{DenoiseError, Frame};

use crate::render::Film;

/// Which filter to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DenoiseMode {
    #[default]
    Off,
    /// The spatiotemporal variance-guided filter, for the interactive view (needs one render).
    Svgf,
    /// First-order regression on the two half renderings, for final frames (needs two samples;
    /// with one, the interactive filter is used).
    Nfor,
}

/// The film's images as the filters want them: per-pixel means.
#[derive(Clone, Debug)]
pub struct FrameData {
    pub width: usize,
    pub height: usize,
    pub color: Vec<[f32; 3]>,
    pub albedo: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
    pub depth: Vec<f32>,
    /// The variance of each pixel's mean luminance; `None` below two samples.
    pub variance: Option<Vec<f32>>,
    /// Screen motion since the previous camera, in pixels.
    pub motion: Vec<[f32; 2]>,
}

impl FrameData {
    pub fn frame(&self) -> Frame<'_> {
        Frame {
            width: self.width,
            height: self.height,
            color: &self.color,
            albedo: &self.albedo,
            normal: &self.normal,
            depth: &self.depth,
            variance: self.variance.as_deref(),
        }
    }
}

impl Film {
    /// The film's images as the filters want them.
    pub fn frame_data(&self) -> FrameData {
        let k = if self.samples > 0 { 1.0 / self.samples as f32 } else { 0.0 };
        FrameData {
            width: self.width as usize,
            height: self.height as usize,
            color: self.mean(),
            albedo: self.mean_of(&self.albedo),
            normal: self.mean_of(&self.normal),
            depth: self.depth.iter().map(|d| d * k).collect(),
            variance: (self.samples >= 2).then(|| self.variance()),
            motion: self.motion.iter().map(|m| [m[0] * k, m[1] * k]).collect(),
        }
    }

    /// The film's image with its noise filtered out, as mean radiance (what [`Film::mean`]
    /// returns for the unfiltered one).
    pub fn denoise(&self, mode: DenoiseMode) -> Result<Vec<[f32; 3]>, DenoiseError> {
        let data = self.frame_data();
        match mode {
            DenoiseMode::Off => Ok(data.color),
            DenoiseMode::Svgf => Svgf::new(SvgfSettings::default()).filter(&data.frame(), None),
            DenoiseMode::Nfor => match self.halves() {
                Some((a, b)) => nfor(&data.frame(), &Halves { a: &a, b: &b }, &NforSettings::default()),
                None => Svgf::new(SvgfSettings::default()).filter(&data.frame(), None),
            },
        }
    }
}
