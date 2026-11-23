//! Geometry operations on [`PolyMesh`]: parametric primitives ([`primitives`]), polygon edits
//! ([`edit`]), smooth-cage subdivision ([`subdivide`]) and deformers ([`deform`]).
//!
//! Every operation takes a mesh and returns a new one built through [`PolyMesh::build`], so the
//! result is always validated; bad selections (out-of-range indices) are ignored, sizes are
//! clamped, and an operation that would exceed [`MAX_RESULT_FACES`] returns [`OpError::TooLarge`]
//! instead of allocating without bound.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod array;
pub mod bevel;
pub mod bvh;
pub mod cut;
pub mod decimate;
pub mod deform;
pub mod edit;
pub mod extrude;
pub mod loopcut;
pub mod paint;
pub mod patches;
pub mod primitives;
mod primitives_more;
pub mod remesh;
pub mod select;
pub mod soft;
pub mod spline_gen;
pub mod subdivide;
pub mod trim;

pub use modelcraft_mesh::{MeshError, PolyMesh, Polygon};

/// Largest face count an operation may produce (about 1.5 GiB of half-edges).
pub const MAX_RESULT_FACES: usize = 16_000_000;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum OpError {
    #[error(transparent)]
    Mesh(#[from] MeshError),
    #[error("the result would have {0} faces, more than the limit")]
    TooLarge(usize),
    #[error("{0}")]
    Invalid(String),
}

pub type OpResult = Result<PolyMesh, OpError>;

/// Rebuild from a polygon list (the shared tail of every operation).
pub(crate) fn rebuild(positions: Vec<glam::Vec3>, polygons: Vec<Polygon>) -> OpResult {
    if polygons.len() > MAX_RESULT_FACES {
        return Err(OpError::TooLarge(polygons.len()));
    }
    Ok(PolyMesh::build(positions, polygons)?.0)
}

/// Keep only valid, distinct indices below `n`.
pub(crate) fn clean_selection(sel: &[u32], n: usize) -> Vec<u32> {
    let mut v: Vec<u32> = sel.iter().copied().filter(|i| (*i as usize) < n).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    /// Euler characteristic V − E + F.
    pub fn euler(m: &PolyMesh) -> i64 {
        m.vertex_count() as i64 - m.edge_count() as i64 + m.face_count() as i64
    }
    pub fn closed(m: &PolyMesh) -> bool {
        (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
    }
}
