//! The simulation kernel: a fixed-step solver loop, a bounded frame cache and seeded runs, with no
//! scene, file or UI dependency.
//!
//! - [`Solver`] is what a simulation implements: reset to its rest state from a seed, step by a
//!   fixed `dt`, and write its points (the vertices of a cloth, the corners of a rigid body, the
//!   slots of a particle pool).
//! - [`bake`] runs a solver over a frame range: [`SimTiming`] gives the start and end frame, the
//!   frame rate and the substeps per frame; frame `start` is the rest state and every later frame
//!   is `substeps` fixed steps further. Each frame's points go into a [`FrameCache`], which plays
//!   back by scene frame ([`FrameCache::sample`]) and converts to a `.mcpc` point cache in
//!   `modelcraft-formats`.
//! - Runs are deterministic: the solver is reset from [`BakeConfig::seed`], steps are a fixed
//!   `dt` independent of wall-clock time, and [`Rng`] is the same on every platform, so the same
//!   input gives the same cache bit for bit ([`FrameCache::checksum`]).
//! - Everything is bounded: frames, substeps, total steps, points per frame and cache bytes are
//!   capped, a run can be cancelled between frames ([`Observer`]), and a solver that produces
//!   non-finite points stops the run with [`SimError::Diverged`] instead of filling the cache.
//!
//! Time is in frames (`f64`); [`SimTiming::fps`] converts to seconds.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod cache;
mod object;
mod rng;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;
use serde::{Deserialize, Serialize};

pub use cache::{DEAD, FrameCache, MAX_CACHE_BYTES, MAX_FRAMES, MAX_POINTS, is_live};
pub use object::{SimKind, SimObject};
pub use rng::Rng;

/// Most substeps per frame.
pub const MAX_SUBSTEPS: u32 = 64;
/// Most solver steps in one bake (frames × substeps).
pub const MAX_STEPS: u64 = 2_000_000;
/// The default bound on one bake's cache: 512 MiB of positions.
pub const DEFAULT_CACHE_BYTES: usize = 512 << 20;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SimError {
    #[error("{0}")]
    Invalid(String),
    #[error("the simulation was cancelled after {done} frames")]
    Cancelled { done: usize },
    #[error("the cache would hold more than {} MiB of positions; shorten the range, lower the point count or raise the limit", limit >> 20)]
    CacheFull { limit: usize },
    #[error("the simulation went wrong at frame {frame}: a point is no longer a finite position; lower the time step or the stiffness")]
    Diverged { frame: f64 },
}

/// When and how finely to simulate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimTiming {
    /// The frame the simulation starts at: the rest state, before any step.
    pub start_frame: f64,
    /// The last frame to simulate (inclusive).
    pub end_frame: f64,
    /// Frames per second.
    pub fps: f64,
    /// Fixed solver steps per frame; more is stiffer and slower.
    pub substeps: u32,
}

impl Default for SimTiming {
    fn default() -> Self {
        SimTiming { start_frame: 1.0, end_frame: 100.0, fps: 24.0, substeps: 4 }
    }
}

impl SimTiming {
    /// Check the numbers: finite, `start <= end`, a positive frame rate, 1 to [`MAX_SUBSTEPS`]
    /// substeps, at most [`MAX_FRAMES`] frames and [`MAX_STEPS`] steps.
    pub fn validate(&self) -> Result<(), SimError> {
        if ![self.start_frame, self.end_frame, self.fps].iter().all(|x| x.is_finite()) {
            return Err(SimError::Invalid("the start, end and frame rate must be finite numbers".into()));
        }
        if self.fps <= 0.0 || self.fps > 1000.0 {
            return Err(SimError::Invalid("the frame rate must be between 0 and 1000".into()));
        }
        if self.end_frame < self.start_frame {
            return Err(SimError::Invalid("the end frame comes before the start frame".into()));
        }
        if self.substeps == 0 || self.substeps > MAX_SUBSTEPS {
            return Err(SimError::Invalid(format!("substeps are 1 to {MAX_SUBSTEPS}")));
        }
        let frames = (self.end_frame - self.start_frame).floor() + 1.0;
        if frames > MAX_FRAMES as f64 {
            return Err(SimError::Invalid(format!("a simulation covers at most {MAX_FRAMES} frames")));
        }
        if (self.frame_count() as u64).saturating_mul(u64::from(self.substeps)) > MAX_STEPS {
            return Err(SimError::Invalid(format!("a simulation runs at most {MAX_STEPS} solver steps; lower the substeps or shorten the range")));
        }
        Ok(())
    }

    /// Frames simulated, the start one included.
    pub fn frame_count(&self) -> usize {
        let n = (self.end_frame - self.start_frame).floor() + 1.0;
        if n.is_finite() && n >= 1.0 { (n as usize).min(MAX_FRAMES + 1) } else { 0 }
    }

    /// Seconds per solver step.
    pub fn dt(&self) -> f32 {
        (1.0 / (self.fps * f64::from(self.substeps.max(1)))) as f32
    }

    /// The scene frame of sample `i`.
    pub fn frame_of(&self, i: usize) -> f64 {
        self.start_frame + i as f64
    }

    /// Replace unusable numbers with the defaults and clamp the rest into range.
    pub fn sanitized(mut self) -> SimTiming {
        let d = SimTiming::default();
        if !self.start_frame.is_finite() {
            self.start_frame = d.start_frame;
        }
        if !self.end_frame.is_finite() {
            self.end_frame = d.end_frame;
        }
        self.end_frame = self.end_frame.max(self.start_frame);
        if !self.fps.is_finite() || self.fps <= 0.0 {
            self.fps = d.fps;
        }
        self.fps = self.fps.min(1000.0);
        self.substeps = self.substeps.clamp(1, MAX_SUBSTEPS);
        if (self.end_frame - self.start_frame).floor() + 1.0 > MAX_FRAMES as f64 {
            self.end_frame = self.start_frame + (MAX_FRAMES - 1) as f64;
        }
        while (self.frame_count() as u64).saturating_mul(u64::from(self.substeps)) > MAX_STEPS && self.substeps > 1 {
            self.substeps -= 1;
        }
        self
    }
}

/// What a solver sees at each fixed step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StepContext {
    /// Seconds to advance.
    pub dt: f32,
    /// Which substep of the frame this is (0-based).
    pub substep: u32,
    /// Substeps in the frame.
    pub substeps: u32,
    /// The scene frame this step is heading to.
    pub frame: f64,
}

impl StepContext {
    /// How far through the frame this step ends: the last substep of a frame is 1. Use it to move
    /// something that is keyed per frame (a collider) smoothly across the substeps.
    pub fn alpha(&self) -> f32 {
        (self.substep + 1) as f32 / self.substeps.max(1) as f32
    }
}

/// A simulation the framework can run, step and cache.
pub trait Solver {
    /// Points written each frame; fixed for the whole run.
    fn point_count(&self) -> usize;
    /// Go back to the rest state at the start frame. All randomness restarts from `seed`.
    fn reset(&mut self, seed: u64);
    /// Called once per frame before its substeps (not for the start frame): move colliders and
    /// fields to where the scene has them at `frame`.
    fn begin_frame(&mut self, _frame: f64) {}
    /// Advance by one fixed step.
    fn step(&mut self, ctx: &StepContext);
    /// Write [`Solver::point_count`] positions, replacing what `out` holds.
    fn write_points(&self, out: &mut Vec<Vec3>);
    /// Whether points may be [`DEAD`] (NaN), as unborn and dead particles are. Otherwise a
    /// non-finite point means the simulation diverged.
    fn allows_dead(&self) -> bool {
        false
    }
}

/// Progress of a bake, reported after every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    /// Frames cached so far.
    pub done: usize,
    pub total: usize,
    /// Solver steps taken so far.
    pub steps: u64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 1.0 } else { self.done as f32 / self.total as f32 }
    }
}

/// Watches a bake: gets its progress and can stop it between frames.
pub trait Observer {
    fn progress(&mut self, _p: Progress) {}
    fn cancelled(&self) -> bool {
        false
    }
}

/// An observer that neither reports nor cancels.
pub struct Silent;
impl Observer for Silent {}

/// A flag another thread sets to stop a bake; clone it into the UI.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> CancelFlag {
        CancelFlag::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Observer for CancelFlag {
    fn cancelled(&self) -> bool {
        self.is_cancelled()
    }
}

/// Everything a bake needs besides the solver.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeConfig {
    pub timing: SimTiming,
    /// The solver is reset from this; the same seed gives the same cache.
    pub seed: u64,
    /// The cache may not hold more positions than this many bytes (at most [`MAX_CACHE_BYTES`]).
    pub max_cache_bytes: usize,
}

impl Default for BakeConfig {
    fn default() -> Self {
        BakeConfig { timing: SimTiming::default(), seed: 1, max_cache_bytes: DEFAULT_CACHE_BYTES }
    }
}

/// Run `solver` over the range and cache every frame.
///
/// The solver is reset from the seed; frame `start_frame` is its rest state, and each later frame
/// is `substeps` steps of `dt` after `begin_frame`. The cache is checked against its byte limit
/// before the first step, so a run that cannot fit fails at once. `observer` sees the progress
/// after each frame and may cancel between frames ([`SimError::Cancelled`]).
pub fn bake<S: Solver + ?Sized>(solver: &mut S, config: &BakeConfig, observer: &mut dyn Observer) -> Result<FrameCache, SimError> {
    let timing = config.timing;
    timing.validate()?;
    solver.reset(config.seed);
    let points = solver.point_count();
    if points > MAX_POINTS {
        return Err(SimError::Invalid(format!("{points} points is more than the {MAX_POINTS} a simulation may have")));
    }
    let frames = timing.frame_count();
    let limit = config.max_cache_bytes.min(MAX_CACHE_BYTES);
    if frames.saturating_mul(points).saturating_mul(12) > limit {
        return Err(SimError::CacheFull { limit });
    }
    let mut cache = FrameCache::new(timing.fps, timing.start_frame, 1.0, points);
    let dt = timing.dt();
    let mut buf: Vec<Vec3> = Vec::with_capacity(points);
    let mut steps = 0u64;
    for i in 0..frames {
        if observer.cancelled() {
            return Err(SimError::Cancelled { done: i });
        }
        let frame = timing.frame_of(i);
        if i > 0 {
            solver.begin_frame(frame);
            for s in 0..timing.substeps {
                solver.step(&StepContext { dt, substep: s, substeps: timing.substeps, frame });
                steps += 1;
            }
        }
        solver.write_points(&mut buf);
        if buf.len() != points {
            return Err(SimError::Invalid(format!("the solver wrote {} points, not the {points} it promised", buf.len())));
        }
        let broken = if solver.allows_dead() {
            buf.iter().any(|p| p.x.is_infinite() || p.y.is_infinite() || p.z.is_infinite() || p.abs().max_element() > 1e18)
        } else {
            buf.iter().any(|p| !p.is_finite())
        };
        if broken {
            return Err(SimError::Diverged { frame });
        }
        cache.push(&buf, limit)?;
        observer.progress(Progress { done: i + 1, total: frames, steps });
    }
    Ok(cache)
}

#[cfg(test)]
mod tests;
