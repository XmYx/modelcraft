//! ModelCraft UV packing and projections, UI-free and built on [`modelcraft_mesh::PolyMesh`] only,
//! so the crate can move to a shared library unchanged.
//!
//! - [`pack`]: place islands in the unit square (MaxRects, margin, rotation, even texel density).
//! - [`pack_mesh`], [`uv_islands`]: the same for the islands a mesh's UVs already form.
//! - [`pack_udim`], [`pack_udim_mesh`]: the same into several tiles (UDIM numbers 1001 and up), with one
//!   texel density across them.
//! - [`project`]: planar, box, cylindrical, spherical and camera projections.
//!
//! UV space follows glTF: origin top left, v down. Only published papers were used (see each
//! module).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod islands;
mod pack;
mod project;
mod udim;

pub use islands::{island_densities, pack_islands, pack_mesh, uv_islands};
pub use pack::{
    Density, MAX_ISLANDS, MAX_POINTS, MAX_RECTS_ISLANDS, MAX_WORK, PackIsland, PackOptions, PackResult, Placement, Rotation, pack, pack_with_limit,
};
pub use project::{Axis, Fit, ProjectOptions, Projection, project, project_mesh};
pub use udim::{MAX_TILES, TILE_COLUMNS, UdimOptions, UdimResult, UdimTiles, pack_udim, pack_udim_mesh, tile_of, tile_of_number, udim_number, udim_tiles};

use modelcraft_mesh::MeshError;

/// Most faces a projection or pack accepts.
pub const MAX_FACES: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum PackError {
    #[error("too many islands or faces ({0})")]
    TooLarge(usize),
    #[error("the mesh has no faces")]
    NoFaces,
    #[error("the mesh has no UVs to pack")]
    NoUvs,
    #[error("a coordinate is not a finite number")]
    NonFinite,
    #[error("{0}")]
    Invalid(String),
    #[error("the islands do not fit at any scale with this margin")]
    NoRoom,
    #[error(transparent)]
    Mesh(#[from] MeshError),
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod udim_tests;
