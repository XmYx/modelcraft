//! Splines: curves in 3D made of one or more *segments*, each a list of knots that is open or
//! closed and is a polyline, a cubic Bezier curve (with tangent handles) or a uniform cubic
//! B-spline (with control points).
//!
//! The crate knows nothing about meshes, the scene or the UI, so it can be shared: a font
//! outline, a generator profile or a path for a sweep are all a [`Spline`]. What it offers:
//!
//! - the data model ([`Spline`], [`Segment`], [`Knot`]) with serde support and a [`PathBuilder`]
//!   for move/line/curve/close style outlines (glyphs, SVG-like paths);
//! - exact evaluation: every segment is turned into cubic Bezier pieces ([`Segment::pieces`]),
//!   so a position and a tangent come from one formula whatever the curve kind;
//! - tessellation into polylines with fixed, uniform-length or adaptive subdivision
//!   ([`Tessellation`]);
//! - point editing: move knots, edit handles under the handle rules, insert a knot without
//!   changing the shape, remove a knot ([`edit`]).
//!
//! Everything is total and bounded. A spline with a non-finite number, more than
//! [`MAX_SEGMENTS`] segments or [`MAX_KNOTS`] knots is rejected by [`Spline::validate`] with a
//! [`SplineError`], and tessellation stops with [`SplineError::TooLarge`] before it produces more
//! than [`MAX_POLYLINE_POINTS`] points (the output size is checked as it grows, not only the
//! input size).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod curve;
pub mod edit;
mod path;
mod tess;

pub use curve::{CurveKind, HandleMode, Knot, Piece, Segment, Side, Spline};
pub use path::PathBuilder;
pub use tess::{MODES, Polyline, Tessellation, tessellate_pieces};

#[cfg(test)]
mod tests;

/// Most segments in one spline.
pub const MAX_SEGMENTS: usize = 1_024;
/// Most knots in one spline, over all its segments.
pub const MAX_KNOTS: usize = 100_000;
/// Most points one tessellation may produce, over all segments.
pub const MAX_POLYLINE_POINTS: usize = 1_000_000;
/// Most subdivisions of one curve piece in the fixed and uniform modes.
pub const MAX_STEPS: u32 = 256;
/// Deepest recursion of the adaptive mode (at most `2^MAX_DEPTH` points per piece).
pub const MAX_DEPTH: u32 = 12;

/// Why a spline was refused or could not be tessellated.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SplineError {
    #[error("the spline has {0} segments; at most {MAX_SEGMENTS} are allowed")]
    TooManySegments(usize),
    #[error("the spline has {0} points; at most {MAX_KNOTS} are allowed")]
    TooManyKnots(usize),
    #[error("point {knot} of segment {segment} is not a finite number")]
    NonFinite { segment: usize, knot: usize },
    #[error("the curve would need more than {MAX_POLYLINE_POINTS} points; use fewer steps or a larger tolerance")]
    TooLarge,
    #[error("there is no point {0}")]
    NoPoint(usize),
    #[error("{0}")]
    Invalid(String),
}
