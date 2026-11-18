//! ModelCraft files.
//!
//! - [`project`]: the native `.modelcraft` project, a ZIP with `project.json` (the scene) and
//!   binary blobs for stored meshes and sculpt layers. Saves are atomic (write a temporary file,
//!   sync, rename) through `craft-atomic`; a crash mid-save leaves the old file intact.
//! - [`journal`]: an append-only JSON-lines log of every command since the last save, next to
//!   the project. After a crash, opening the project replays it (autosave recovery).
//! - [`exchange`]: glTF 2.0 / GLB, OBJ, STL and PLY through the shared `craft-mesh` crate.
//! - [`pointcache`]: baked vertex positions per frame (`.mcpc`).
//! - [`blob`]: the bounded binary mesh encoding used inside projects.
//! - [`uvlayout`]: a mesh's UV layout as SVG or PNG.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod blob;
pub mod exchange;
pub mod fbx_write;
pub mod journal;
pub mod pointcache;
pub mod project;
pub mod uvlayout;

/// Name and version written into files.
pub const GENERATOR: &str = concat!("ModelCraft ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("{path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("not a valid file: {0}")]
    Malformed(String),
    #[error("the file was written by a newer ModelCraft (schema {found}, this version reads up to {supported})")]
    NewerSchema { found: u32, supported: u32 },
    #[error("unsupported format: {0}")]
    Unsupported(String),
}

impl FormatError {
    pub fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        FormatError::Io { path: path.display().to_string(), source }
    }
}

pub type Result<T> = std::result::Result<T, FormatError>;
