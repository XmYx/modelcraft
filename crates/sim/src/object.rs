//! The simulation as scene data: what a simulation object stores and how it checks it.
//!
//! A simulation object is a child of the mesh it drives. It holds the settings of one kind of
//! simulation, the timing and seed it runs with, and the [`FrameCache`] its last bake produced.
//! The cache is the result: playing a frame shows the cached points, so scrubbing the timeline
//! never runs a solver. The scene saves the cache next to the project (as a `.mcpc` blob), not
//! in the project JSON.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::rigid::RigidSettings;
use crate::{DEFAULT_CACHE_BYTES, FrameCache, MAX_CACHE_BYTES, SimTiming};

/// Which simulation an object runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SimKind {
    /// No solver: plays a point cache that came from a file (or an earlier bake) on the mesh.
    Cache,
    /// A rigid body: falls, collides and is constrained with the other bodies of the scene
    /// ([`crate::rigid`]). Baking one bakes every body.
    Rigid(RigidSettings),
}

impl SimKind {
    /// Stable id, also the English label key.
    pub fn name(&self) -> &'static str {
        match self {
            SimKind::Cache => "Cache",
            SimKind::Rigid(_) => "Rigid body",
        }
    }

    /// Whether baking this kind runs a solver (a cache from a file has nothing to run).
    pub fn is_solver(&self) -> bool {
        !matches!(self, SimKind::Cache)
    }
}

/// A simulation object's whole state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimObject {
    pub kind: SimKind,
    pub timing: SimTiming,
    /// The solver restarts from this at every bake; the same seed gives the same result.
    #[serde(default = "one")]
    pub seed: u64,
    /// The most MiB of positions a bake of this object may cache.
    #[serde(default = "default_cache_mib")]
    pub max_cache_mib: u32,
    /// What the cache was baked from, as a hash the engine computes (for a rigid body, every
    /// rigid body of the scene with the timing and seed; otherwise [`SimObject::settings_key`]).
    /// A cache whose hash no longer matches is stale. Zero for a cache from a file.
    #[serde(default)]
    pub baked_with: u64,
    /// The baked points. Saved as a blob, not in the project JSON.
    #[serde(skip)]
    pub cache: Option<Arc<FrameCache>>,
}

fn one() -> u64 {
    1
}
fn default_cache_mib() -> u32 {
    (DEFAULT_CACHE_BYTES >> 20) as u32
}

impl SimObject {
    pub fn new(kind: SimKind) -> SimObject {
        SimObject { kind, timing: SimTiming::default(), seed: 1, max_cache_mib: default_cache_mib(), baked_with: 0, cache: None }
    }

    /// A hash of everything that changes what a bake produces: the kind with its settings, the
    /// timing and the seed.
    pub fn settings_key(&self) -> u64 {
        let json = serde_json::to_string(&(&self.kind, &self.timing, self.seed)).unwrap_or_default();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in json.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h | 1
    }

    /// The cache byte budget in bytes.
    pub fn cache_limit(&self) -> usize {
        ((self.max_cache_mib as usize) << 20).clamp(1 << 20, MAX_CACHE_BYTES)
    }

    pub fn heap_bytes(&self) -> usize {
        256 + self.cache.as_ref().map_or(0, |c| c.bytes())
    }

    /// Repair settings read from a file.
    pub fn sanitize(&mut self) {
        self.timing = self.timing.sanitized();
        if let SimKind::Rigid(r) = &mut self.kind {
            r.sanitize();
        }
        self.max_cache_mib = self.max_cache_mib.clamp(1, (MAX_CACHE_BYTES >> 20) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_key_follows_the_timing_and_the_seed() {
        let a = SimObject::new(SimKind::Cache);
        let mut b = a.clone();
        assert_eq!(a.settings_key(), b.settings_key());
        b.seed = 2;
        assert_ne!(a.settings_key(), b.settings_key());
        let mut c = a.clone();
        c.timing.substeps += 1;
        assert_ne!(a.settings_key(), c.settings_key());
        assert_ne!(a.settings_key(), 0);
    }

    #[test]
    fn hostile_settings_are_repaired() {
        let json = r#"{"kind":{"type":"cache"},"timing":{"start_frame":5,"end_frame":1,"fps":-3,"substeps":999},"max_cache_mib":0}"#;
        let mut s: SimObject = serde_json::from_str(json).unwrap();
        s.sanitize();
        assert!(s.timing.validate().is_ok());
        assert_eq!(s.max_cache_mib, 1);
        assert_eq!(s.seed, 1);
        assert!(s.cache.is_none(), "the cache never comes from the project JSON");
    }
}
