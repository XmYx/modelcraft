//! The animation kernel: values over time, with no scene, file or UI dependency.
//!
//! - [`curve`]: a [`Curve`] of [`Key`]s with stepped, linear and Bézier segments, automatic
//!   clamped tangents, weighted handles, extrapolation, baking and simplifying.
//! - [`euler`]: removing whole-turn and flip jumps from keyed Euler rotations.
//! - [`deform`]: blend shapes with in-betweens, lattice (free-form) deformation, clusters.
//! - [`ik`]: two-bone, FABRIK and CCD inverse kinematics on a chain of points.
//! - [`expr`]: a tiny, safe expression language for numbers.
//! - [`skeleton`]: aiming a joint along its bone, mirroring joints, the bone shape.
//! - [`skin`]: joint weights, linear-blend and dual-quaternion skinning, automatic weights.
//! - [`clock`]: frame rate, playback range and a [`Playback`] that advances the current frame from
//!   wall-clock time.
//!
//! Time is measured in frames (`f64`, fractional frames allowed); [`TimeSettings::fps`] converts
//! to seconds. Nothing here panics on hostile input: non-finite times and values are dropped or
//! replaced, and every list is capped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod clock;
pub mod curve;
pub mod deform;
pub mod euler;
pub mod expr;
pub mod ik;
pub mod skeleton;
pub mod skin;

pub use clock::{LoopMode, Playback, TimeSettings};
pub use curve::{Curve, Extrapolation, Interp, Key, Tangent};
