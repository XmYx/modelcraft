//! The viewport renderer.
//!
//! - [`camera`]: an orbit camera with perspective and orthographic views, screen↔world rays.
//! - [`buffers`]: CPU-side vertex data for meshes (with auto-smoothed corner normals), sculpt
//!   meshes (shared vertices, chunked updates), wireframes, the grid and overlays. Pure data,
//!   tested without a GPU.
//! - [`gpu`]: the wgpu draw path (offscreen colour + depth with 4× MSAA, resolved to a texture
//!   the UI shows). It follows RFC-0022's rules ahead of the shared GPU crates: capabilities come
//!   from the created device, a device that cannot be created means "no GPU" (the app falls back
//!   to its CPU wireframe view), and nothing here panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod buffers;
pub mod camera;
pub mod gpu;

pub use buffers::{LineVertex, MeshVertex};
pub use camera::{Camera, Projection, ViewPreset};
