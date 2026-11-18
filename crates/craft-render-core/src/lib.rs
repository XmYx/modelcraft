//! The Craft render engine: a physically based path tracer.
//!
//! This crate is the CPU reference: scene description, cameras, sampling, the surface model and
//! the integrator. It has no UI and no GPU dependency; `craft-render-gpu` runs the same transport
//! on a GPU and is checked against this crate.
//!
//! Everything here is written from published papers and open specifications (each module names
//! its sources). No other renderer's code was read.
//!
//! ```
//! use craft_render_core::{scenes, RenderSettings, render_image};
//! use std::sync::Arc;
//! let scene = Arc::new(scenes::cornell_box().unwrap());
//! let film = render_image(scene, RenderSettings { width: 32, height: 32, ..Default::default() }, 4).unwrap();
//! assert_eq!(film.samples, 4);
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod bsdf;
pub mod camera;
pub mod denoise;
pub mod flip;
pub mod guided;
pub mod hair;
pub mod integrator;
pub mod light;
pub mod material;
pub mod math;
pub mod render;
pub mod restir;
pub mod sampler;
pub mod scene;
pub mod scenes;
pub mod shapes;
pub mod sky;

pub use camera::Camera;
pub use craft_render_bvh as bvh;
pub use craft_render_denoise as filters;
pub use craft_render_guide as guide;
pub use craft_texture as texture;
pub use denoise::DenoiseMode;
pub use integrator::{IntegratorSettings, Strategy};
pub use light::{Light, LightSampling};
pub use material::Material;
pub use render::{Film, PixelOrder, RenderSettings, Renderer, mean_rgb, mse, relative_mse, render_image};
pub use restir::{RestirMode, RestirSettings};
pub use scene::{Scene, SceneBuilder, SceneError};
pub use shapes::MeshData;

#[cfg(test)]
mod denoise_tests;
#[cfg(test)]
mod hair_tests;
#[cfg(test)]
mod restir_tests;
#[cfg(test)]
mod tests;
