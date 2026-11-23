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
//! - [`hair_gpu`]: hair strands as line strips with a percentage display by index prefix.
//! - [`output`]: saving renders (PNG, EXR with passes, a region), numbered frame files and the
//!   picture history behind the picture viewer.
//! - [`batch`]: a queue of frames rendered to numbered files on a worker thread.
//! - [`trace`]: the scene graph exported to the Craft render engine (the path tracer in the
//!   `craft-render-*` crates, re-exported here as [`pathtrace`] and [`pathtrace_gpu`]).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod batch;
pub mod buffers;
pub mod camera;
pub mod gpu;
pub mod hair_gpu;
pub mod hair_trace;
pub mod output;
pub mod trace;

pub use craft_render_core as pathtrace;
pub use craft_render_gpu as pathtrace_gpu;

pub use buffers::{LineVertex, MeshVertex};
pub use camera::{Camera, Projection, ViewPreset};
