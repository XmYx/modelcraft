//! Strand dynamics: guides that hang, swing and settle, baked per frame.
//!
//! Each guide is a chain of points: the root is pinned to the emitter and the rest move by Verlet
//! integration (Verlet 1967; Jakobsen, "Advanced Character Physics", GDC 2001) under gravity and
//! wind with damping. Segment lengths are held by iterating distance constraints from the root out
//! (the position-based approach of Müller et al., "Position Based Dynamics", 2007), a weak spring
//! pulls points back toward the groomed shape so a styled groom does not collapse, and points are
//! kept outside the emitter by a clearance from the nearest surface triangle.
//!
//! The solver implements [`modelcraft_sim::Solver`], so a bake is the simulation framework's:
//! fixed steps, seeded, cancellable, bounded, cached per frame ([`modelcraft_sim::FrameCache`]) and
//! scrubbed without running anything. Guides do not touch each other (strands are independent), so
//! every step runs on all cores and gives the same result as one thread.
//!
//! Everything is in the emitter's own space; gravity and wind are given in world space and the
//! caller turns them into that space (the emitter is taken to be still while baking).

use std::collections::HashMap;

use glam::Vec3;
use modelcraft_sim::{BakeConfig, FrameCache, Observer, SimTiming, Solver, StepContext, bake};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::guides::Guides;
use crate::surface::{Surface, SurfacePoint, closest_barycentric};
use crate::{HairError, in_range};

/// Most constraint iterations per step.
pub const MAX_ITERATIONS: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DynamicsSettings {
    /// Acceleration, in world units per second squared.
    pub gravity: Vec3,
    /// A constant extra acceleration (wind), same units.
    pub wind: Vec3,
    /// Fraction of its speed a point loses each second, 0 to 1.
    pub damping: f32,
    /// How strongly points are pulled back toward the groomed shape, 0 (limp) to 1 (rigid).
    pub stiffness: f32,
    /// Distance-constraint passes per step (more keeps lengths better).
    pub iterations: u32,
    /// Keep points outside the emitter.
    pub collide: bool,
    /// How far from the emitter's surface a point is kept, in the emitter's units.
    pub clearance: f32,
}

impl Default for DynamicsSettings {
    fn default() -> DynamicsSettings {
        DynamicsSettings {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            wind: Vec3::ZERO,
            damping: 0.4,
            stiffness: 0.15,
            iterations: 8,
            collide: true,
            clearance: 0.004,
        }
    }
}

impl DynamicsSettings {
    pub fn validate(&self) -> Result<(), HairError> {
        for (what, v) in [("gravity", self.gravity), ("wind", self.wind)] {
            if !v.is_finite() {
                return Err(HairError::NotFinite(what));
            }
            if v.length() > 1.0e4 {
                return Err(HairError::OutOfRange { what, min: 0.0, max: 1.0e4, found: f64::from(v.length()) });
            }
        }
        in_range("damping", self.damping, 0.0, 1.0)?;
        in_range("stiffness", self.stiffness, 0.0, 1.0)?;
        in_range("clearance", self.clearance, 0.0, 1.0e3)?;
        if self.iterations == 0 || self.iterations > MAX_ITERATIONS {
            return Err(HairError::OutOfRange { what: "iterations", min: 1.0, max: f64::from(MAX_ITERATIONS), found: f64::from(self.iterations) });
        }
        Ok(())
    }
}

/// Triangles in a uniform grid for "which triangles are near this point" queries.
struct Collider {
    tris: Vec<[Vec3; 3]>,
    normals: Vec<Vec3>,
    cell: f32,
    cells: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl Collider {
    fn new(surface: &Surface, clearance: f32) -> Collider {
        let tris: Vec<[Vec3; 3]> = surface.triangles().iter().map(|t| t.map(|i| surface.positions().get(i as usize).copied().unwrap_or(Vec3::ZERO))).collect();
        let normals: Vec<Vec3> = tris.iter().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).try_normalize().unwrap_or(Vec3::Y)).collect();
        let edge = tris.iter().map(|t| (t[1] - t[0]).length().max((t[2] - t[0]).length())).sum::<f32>() / tris.len().max(1) as f32;
        let cell = edge.max(clearance * 2.0).max(1e-4);
        let mut cells: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, t) in tris.iter().enumerate() {
            let (lo, hi) = (t[0].min(t[1]).min(t[2]) - Vec3::splat(clearance), t[0].max(t[1]).max(t[2]) + Vec3::splat(clearance));
            let (a, b) = (key(lo, cell), key(hi, cell));
            // A triangle larger than the cap would fill the table; those few cells are enough.
            let span = |a: i32, b: i32| a..=b.min(a.saturating_add(64));
            for x in span(a.0, b.0) {
                for y in span(a.1, b.1) {
                    for z in span(a.2, b.2) {
                        cells.entry((x, y, z)).or_default().push(i as u32);
                    }
                }
            }
        }
        Collider { tris, normals, cell, cells }
    }

    /// Push `p` out of the surface if it is closer to the nearest triangle than `clearance` or on its
    /// inner side.
    fn resolve(&self, p: Vec3, clearance: f32) -> Vec3 {
        let Some(ids) = self.cells.get(&key(p, self.cell)) else { return p };
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for &i in ids {
            let Some(t) = self.tris.get(i as usize) else { continue };
            let b = closest_barycentric(p, t[0], t[1], t[2]);
            let c = t[0] * b[0] + t[1] * b[1] + t[2] * b[2];
            let d = p.distance_squared(c);
            if best.as_ref().is_none_or(|x| d < x.0) {
                best = Some((d, c, self.normals.get(i as usize).copied().unwrap_or(Vec3::Y)));
            }
        }
        match best {
            Some((_, c, n)) if (p - c).dot(n) < clearance => c + n * clearance,
            _ => p,
        }
    }
}

fn key(p: Vec3, cell: f32) -> (i32, i32, i32) {
    let f = |v: f32| (v / cell).floor().clamp(-1.0e9, 1.0e9) as i32;
    (f(p.x), f(p.y), f(p.z))
}

/// The solver: a set of strands, their rest shape and the emitter.
pub struct StrandSolver {
    pps: usize,
    rest: Vec<Vec3>,
    pos: Vec<Vec3>,
    prev: Vec<Vec3>,
    seg: Vec<f32>,
    settings: DynamicsSettings,
    collider: Option<Collider>,
}

impl StrandSolver {
    /// A solver for `guides` hanging from `surface`. Gravity and wind in `settings` are taken to be
    /// in the emitter's space already.
    pub fn new(guides: &Guides, surface: &Surface, settings: &DynamicsSettings) -> Result<StrandSolver, HairError> {
        settings.validate()?;
        let pps = guides.points_per_strand();
        let rest = guides.all_points().to_vec();
        let seg: Vec<f32> = guides.all_points().chunks_exact(pps).flat_map(|s| s.windows(2).map(|w| w[0].distance(w[1])).collect::<Vec<_>>()).collect();
        let collider = (settings.collide && surface.triangle_count() > 0).then(|| Collider::new(surface, settings.clearance));
        Ok(StrandSolver { pps, pos: rest.clone(), prev: rest.clone(), rest, seg, settings: *settings, collider })
    }
}

impl Solver for StrandSolver {
    fn point_count(&self) -> usize {
        self.rest.len()
    }

    fn reset(&mut self, _seed: u64) {
        self.pos.clone_from(&self.rest);
        self.prev.clone_from(&self.rest);
    }

    fn step(&mut self, ctx: &StepContext) {
        let dt = ctx.dt;
        let accel = (self.settings.gravity + self.settings.wind) * (dt * dt);
        let keep = (1.0 - self.settings.damping).clamp(0.0, 1.0).powf(dt);
        let pull = 1.0 - (1.0 - self.settings.stiffness).clamp(0.0, 1.0).powf(dt * 60.0);
        let (pps, iterations, clearance) = (self.pps, self.settings.iterations, self.settings.clearance);
        let seg_per = pps - 1;
        let collider = self.collider.as_ref();
        self.pos.par_chunks_mut(pps).zip(self.prev.par_chunks_mut(pps)).zip(self.rest.par_chunks(pps)).zip(self.seg.par_chunks(seg_per)).for_each(
            |(((p, q), rest), seg)| {
                for j in 1..pps {
                    let v = (p[j] - q[j]) * keep;
                    q[j] = p[j];
                    p[j] = p[j] + v + accel + (rest[j] - p[j]) * pull;
                }
                for _ in 0..iterations {
                    // Segments from the root outward: the root stays, the other end of each moves.
                    for j in 1..pps {
                        let d = p[j] - p[j - 1];
                        let len = d.length();
                        if len > 1e-12 {
                            p[j] = p[j - 1] + d * (seg[j - 1] / len);
                        }
                    }
                    if let Some(c) = collider {
                        for point in p.iter_mut().skip(1) {
                            *point = c.resolve(*point, clearance);
                        }
                    }
                }
            },
        );
    }

    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        out.extend_from_slice(&self.pos);
    }
}

/// Bake the guides' motion over `timing`. `gravity` and `wind` in `settings` are in the emitter's
/// space.
pub fn bake_guides(
    guides: &Guides,
    surface: &Surface,
    settings: &DynamicsSettings,
    timing: SimTiming,
    seed: u64,
    max_cache_bytes: usize,
    observer: &mut dyn Observer,
) -> Result<FrameCache, HairError> {
    let mut solver = StrandSolver::new(guides, surface, settings)?;
    bake(&mut solver, &BakeConfig { timing, seed, max_cache_bytes }, observer).map_err(|e| HairError::Simulation(e.to_string()))
}

/// The guides as they are at one baked frame: the same roots, the points of the cache.
pub fn guides_from_points(rest: &Guides, points: &[Vec3], limits: &crate::Limits) -> Result<Guides, HairError> {
    let roots: Vec<SurfacePoint> = rest.roots().to_vec();
    Guides::from_parts(roots, rest.points_per_strand(), points.to_vec(), limits)
}
