//! ModelCraft hair and fur kernel, UI-free and built on [`modelcraft_mesh::PolyMesh`] only.
//!
//! - [`Surface`]: the triangles hair grows from, with [`SurfacePoint`] roots that follow the
//!   surface when it deforms, and per-vertex [`SurfaceMaps`] (density, length).
//! - [`grow_guides`]: a seeded set of [`Guides`] (a few thousand control curves) with a density, a
//!   length, a jitter and a direction, rooted on the surface.
//! - [`interpolate`]: [`HairCurves`], the render hairs (up to millions) blended from the nearby
//!   guides, either by barycentric weights over the three nearest guides or by inverse-distance
//!   weights over the nearest `k`, with clumping toward the guide, frizz noise, per-strand width
//!   and a root-to-tip taper.
//! - [`apply_stroke`]: grooming brushes (comb, cut, length, puff, smooth, clump, frizz) as plain-data
//!   [`Stroke`]s of [`Dab`]s, with symmetry and a per-vertex mask, which is what makes them commands.
//! - [`Limits`]: hard caps on guide counts, strand counts, points per strand and total memory, checked
//!   before anything is allocated.
//!
//! Every random number is a pure function of `(seed, strand index, purpose)` (see `rng`), so a
//! groom is reproducible, independent of thread scheduling, and strand `i` does not change when the
//! strand count does: any prefix of the strand list is a uniform random subset of the surface, which
//! is what a percentage display uses.
//!
//! Methods follow the published literature on guide-based hair (for example Ward et al. 2007, *A
//! Survey on Hair Modeling: Styling, Simulation, and Rendering*); nothing is taken from any
//! product's code.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod brush;
mod guides;
mod interp;
mod kdtree;
mod rng;
mod surface;

#[cfg(test)]
mod brush_tests;
#[cfg(test)]
mod tests;

pub use brush::{Brush, BrushKind, Dab, MAX_DABS, Stroke, StrokeReport, Symmetry, apply_dabs, apply_stroke, paint_map};
pub use guides::{Amount, GrowSpec, Guides, grow_guides};
pub use interp::{Blend, Clump, Frizz, HairCurves, InterpSpec, Width, interpolate};
pub use kdtree::MAX_K;
pub use surface::{Surface, SurfaceMaps, SurfacePoint};

/// Most triangles a [`Surface`] accepts.
pub const MAX_SURFACE_TRIANGLES: usize = 10_000_000;
/// Absolute ceilings no [`Limits`] can exceed, whatever the caller asks for.
pub const ABSOLUTE_MAX_GUIDES: usize = 1_000_000;
pub const ABSOLUTE_MAX_STRANDS: usize = 20_000_000;
pub const ABSOLUTE_MAX_POINTS_PER_STRAND: usize = 256;
pub const ABSOLUTE_MAX_TOTAL_POINTS: usize = 200_000_000;
/// Longest strand a spec may ask for, in scene units.
pub const MAX_LENGTH: f32 = 1.0e4;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum HairError {
    #[error("the surface has no area to grow hair from")]
    NoSurface,
    #[error("the surface has {0} triangles, more than hair accepts")]
    SurfaceTooLarge(usize),
    #[error("a triangle refers to a vertex that does not exist")]
    BadIndex,
    #[error("{0} is not a finite number")]
    NotFinite(&'static str),
    #[error("{what} must be between {min} and {max}, found {found}")]
    OutOfRange { what: &'static str, min: f64, max: f64, found: f64 },
    #[error("{requested} {what} is more than the limit of {max}")]
    TooMany { what: &'static str, requested: usize, max: usize },
    #[error("{what} has {found} entries, expected {expected}")]
    LengthMismatch { what: &'static str, expected: usize, found: usize },
    #[error("interpolation needs at least one guide")]
    NoGuides,
    #[error("a root is not on the surface")]
    BadRoot,
    #[error("out of memory for {0} bytes of hair")]
    OutOfMemory(usize),
}

/// Hard caps on what one operation may create. [`Limits::default`] suits interactive use; a caller
/// can lower any of them, and [`Limits::clamped`] pulls them under the absolute ceilings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Limits {
    pub max_guides: usize,
    pub max_strands: usize,
    pub max_points_per_strand: usize,
    /// Points over all strands of one operation (each point is 12 bytes, plus per-strand data).
    pub max_total_points: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_guides: 200_000, max_strands: 5_000_000, max_points_per_strand: 64, max_total_points: 40_000_000 }
    }
}

impl Limits {
    /// These limits, each pulled under its absolute ceiling.
    pub fn clamped(self) -> Limits {
        Limits {
            max_guides: self.max_guides.min(ABSOLUTE_MAX_GUIDES),
            max_strands: self.max_strands.min(ABSOLUTE_MAX_STRANDS),
            max_points_per_strand: self.max_points_per_strand.clamp(2, ABSOLUTE_MAX_POINTS_PER_STRAND),
            max_total_points: self.max_total_points.min(ABSOLUTE_MAX_TOTAL_POINTS),
        }
    }

    /// Check `strands` strands of `points_per_strand` points against the limits (`guides` picks the
    /// guide cap, otherwise the strand cap applies).
    pub fn check(&self, strands: usize, points_per_strand: usize, guides: bool) -> Result<(), HairError> {
        let l = self.clamped();
        let (what, max) = if guides { ("guides", l.max_guides) } else { ("strands", l.max_strands) };
        if strands > max {
            return Err(HairError::TooMany { what, requested: strands, max });
        }
        if points_per_strand < 2 || points_per_strand > l.max_points_per_strand {
            return Err(HairError::OutOfRange { what: "points per strand", min: 2.0, max: l.max_points_per_strand as f64, found: points_per_strand as f64 });
        }
        let points = strands.saturating_mul(points_per_strand);
        if points > l.max_total_points {
            return Err(HairError::TooMany { what: "points", requested: points, max: l.max_total_points });
        }
        Ok(())
    }
}

/// Bytes of hair data for `strands` strands of `points_per_strand` points: points, roots and widths.
pub fn estimate_bytes(strands: usize, points_per_strand: usize) -> usize {
    let per = points_per_strand.saturating_mul(12).saturating_add(std::mem::size_of::<SurfacePoint>() + 8);
    strands.saturating_mul(per)
}

/// A vector of `n` copies of `value`, or an error rather than an abort when memory is short.
pub(crate) fn filled<T: Clone>(n: usize, value: T) -> Result<Vec<T>, HairError> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| HairError::OutOfMemory(n.saturating_mul(std::mem::size_of::<T>())))?;
    v.resize(n, value);
    Ok(v)
}

pub(crate) fn finite(what: &'static str, v: f32) -> Result<f32, HairError> {
    if v.is_finite() { Ok(v) } else { Err(HairError::NotFinite(what)) }
}

pub(crate) fn in_range(what: &'static str, v: f32, min: f32, max: f32) -> Result<f32, HairError> {
    finite(what, v)?;
    if v < min || v > max { Err(HairError::OutOfRange { what, min: f64::from(min), max: f64::from(max), found: f64::from(v) }) } else { Ok(v) }
}
