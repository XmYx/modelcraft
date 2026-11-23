//! Mesh booleans: union, subtraction and intersection of two closed meshes.
//!
//! The method is an arrangement of the two surfaces with exact decisions. Which edge of one mesh
//! passes through which triangle of the other is decided by exact orientation determinants on
//! integer-snapped coordinates, with the degenerate cases (coplanar faces, a vertex on a face)
//! resolved consistently by symbolic perturbation ([`exact`]); every crossing is a vertex shared
//! by name, so the cut pieces fit together exactly; each cut triangle is retriangulated
//! ([`faces`]); and a piece is inside or outside the other mesh by following the surface from
//! the cuts, or by a winding number where no cut reaches ([`core`]).
//!
//! The inputs must be closed, manifold and not self-intersecting; anything else is reported as
//! an error, as is a result that does not close. Nothing here panics on odd input: sizes are
//! limited by [`Limits`] and the work is bounded by them.
//!
//! Sources: Edelsbrunner and Mücke, "Simulation of Simplicity" (1990) for the perturbation;
//! Jacobson, Kavan and Sorkine-Hornung, "Robust Inside-Outside Segmentation using Generalized
//! Winding Numbers" (2013) for the winding number idea; Zhou, Grinspun, Zorin and Jacobson,
//! "Mesh Arrangements for Solid Geometry" (2016) for the overall arrangement approach.

mod arrange;
pub mod core;
pub mod exact;
mod faces;
mod solid;

use modelcraft_mesh::{PolyMesh, Polygon};

pub use self::core::{Outcome, Stats, boolean, volume};
pub use self::solid::TriMesh;
use crate::{MAX_RESULT_FACES, OpError, OpResult};

/// Which combination to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    /// Everything in either.
    Union,
    /// What is in the first and not in the second.
    Subtract,
    /// What is in both.
    Intersect,
}

pub const OPS: &[&str] = &["union", "subtract", "intersect"];

impl BoolOp {
    pub fn name(self) -> &'static str {
        match self {
            BoolOp::Union => "union",
            BoolOp::Subtract => "subtract",
            BoolOp::Intersect => "intersect",
        }
    }
    pub fn from_name(n: &str) -> Option<BoolOp> {
        [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect].into_iter().find(|o| o.name() == n)
    }
}

/// Why a boolean could not be made.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum BoolError {
    #[error("{0} has no faces")]
    Empty(String),
    #[error("{0}")]
    Open(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    TooLarge(String),
    #[error("the boolean could not be completed: {0}")]
    Inconsistent(String),
    #[error("the boolean could not be completed: {0}")]
    NotClosed(String),
}

impl From<BoolError> for OpError {
    fn from(e: BoolError) -> OpError {
        match e {
            BoolError::TooLarge(_) => OpError::TooLarge(0),
            other => OpError::Invalid(other.to_string()),
        }
    }
}

/// Bounds on the work one boolean may do.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Most triangles in either input.
    pub max_triangles: usize,
    /// Most overlapping triangle pairs examined.
    pub max_pairs: usize,
    /// Most grid cells entries made while indexing the second mesh.
    pub max_grid_entries: usize,
    /// Most triangle evaluations of the winding number.
    pub max_winding_work: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_triangles: 2_000_000, max_pairs: 40_000_000, max_grid_entries: 60_000_000, max_winding_work: 4_000_000_000 }
    }
}

/// Fan-triangulate a polygon mesh, keeping per-corner UVs and the patch of each face.
pub fn to_tris(m: &PolyMesh) -> TriMesh {
    let mut out = TriMesh { positions: m.positions().to_vec(), tris: Vec::new(), uvs: m.has_uvs().then(Vec::new), patches: Vec::new() };
    for f in 0..m.face_count() as u32 {
        let verts: Vec<u32> = m.face_vertices(f).collect();
        let uvs: Vec<glam::Vec2> = m.face_half_edges(f).map(|h| m.uv(h)).collect();
        for j in 1..verts.len().saturating_sub(1) {
            out.tris.push([verts[0], verts[j], verts[j + 1]]);
            out.patches.push(m.patch(f));
            if let (Some(u), true) = (&mut out.uvs, uvs.len() == verts.len()) {
                u.push([uvs[0], uvs[j], uvs[j + 1]]);
            }
        }
    }
    out
}

/// Rebuild a polygon mesh from triangles.
pub fn from_tris(t: &TriMesh) -> OpResult {
    if t.tris.len() > MAX_RESULT_FACES {
        return Err(OpError::TooLarge(t.tris.len()));
    }
    let polys: Vec<Polygon> = t
        .tris
        .iter()
        .enumerate()
        .map(|(i, tri)| {
            let mut p = match t.uvs.as_ref().and_then(|u| u.get(i)) {
                Some(uv) => Polygon::with_uvs(tri.to_vec(), uv.to_vec()),
                None => Polygon::new(tri.to_vec()),
            };
            p.patch = t.patches.get(i).copied().unwrap_or(0);
            p
        })
        .collect();
    let (mesh, _) = PolyMesh::build(t.positions.clone(), polys)?;
    Ok(mesh)
}

/// The boolean of two polygon meshes. UVs are kept when both have them; patches of the second
/// mesh are renumbered after the first's so the two stay distinct.
pub fn combine(a: &PolyMesh, b: &PolyMesh, op: BoolOp) -> OpResult {
    combine_with(a, b, op, &Limits::default()).map(|(m, _)| m)
}

/// [`combine`] with explicit limits, also returning the run's counters.
pub fn combine_with(a: &PolyMesh, b: &PolyMesh, op: BoolOp, limits: &Limits) -> Result<(PolyMesh, Stats), OpError> {
    let out = boolean(&to_tris(a), &to_tris(b), op, limits)?;
    let mesh = from_tris(&out.mesh)?;
    if !(0..mesh.edge_count() as u32).all(|e| !mesh.is_boundary_edge(e)) {
        return Err(OpError::Invalid("the boolean result is not a closed surface".into()));
    }
    Ok((mesh, out.stats))
}

#[cfg(test)]
mod tests;
