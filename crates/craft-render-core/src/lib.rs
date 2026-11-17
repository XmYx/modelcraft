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
pub mod integrator;
pub mod material;
pub mod math;
pub mod render;
pub mod sampler;
pub mod scene;
pub mod scenes;
pub mod shapes;

pub use camera::Camera;
pub use craft_render_bvh as bvh;
pub use integrator::{IntegratorSettings, Strategy};
pub use material::Material;
pub use render::{Film, RenderSettings, Renderer, mean_rgb, mse, relative_mse, render_image};
pub use scene::{Light, Scene, SceneBuilder, SceneError};
pub use shapes::MeshData;

#[cfg(test)]
mod tests;
