//! Guide curves: the few control hairs a groom is built from.

use glam::{Quat, Vec3};
use rayon::prelude::*;

use crate::rng::{self, stream};
use crate::surface::{Sampler, SurfaceMaps, map_at};
use crate::{HairError, Limits, MAX_LENGTH, Surface, SurfacePoint, filled, finite, in_range};

/// How many hairs to make.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Amount {
    /// Exactly this many.
    Count(usize),
    /// This many per square scene unit of (density weighted) surface.
    PerUnitArea(f32),
}

impl Amount {
    /// The count for a surface with `weighted_area`, checked against `cap`.
    pub(crate) fn resolve(&self, weighted_area: f64, what: &'static str, cap: usize) -> Result<usize, HairError> {
        let n = match *self {
            Amount::Count(n) => n,
            Amount::PerUnitArea(d) => {
                finite("density", d)?;
                if d < 0.0 {
                    return Err(HairError::OutOfRange { what: "density", min: 0.0, max: f64::from(f32::MAX), found: f64::from(d) });
                }
                let n = weighted_area * f64::from(d);
                if n > usize::MAX as f64 { usize::MAX } else { n.round() as usize }
            }
        };
        if n > cap { Err(HairError::TooMany { what, requested: n, max: cap }) } else { Ok(n) }
    }
}

/// What [`grow_guides`] makes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GrowSpec {
    pub amount: Amount,
    /// Strand length in scene units.
    pub length: f32,
    /// Each strand's length varies by up to this fraction (0 to 1), uniformly.
    pub length_jitter: f32,
    /// Control points per guide, root included (at least 2).
    pub points_per_strand: usize,
    pub seed: u64,
    /// Blend from the surface normal (0) to `lean_toward` (1) for the growth direction.
    pub lean: f32,
    pub lean_toward: Vec3,
    /// A bend toward this offset at the tip, as a multiple of the strand length (gravity droop is
    /// `(0, -0.5, 0)`). The strand keeps its length.
    pub bend: Vec3,
}

impl Default for GrowSpec {
    fn default() -> GrowSpec {
        GrowSpec {
            amount: Amount::Count(1000),
            length: 0.1,
            length_jitter: 0.0,
            points_per_strand: 8,
            seed: 1,
            lean: 0.0,
            lean_toward: Vec3::Y,
            bend: Vec3::ZERO,
        }
    }
}

impl GrowSpec {
    pub(crate) fn validate(&self, limits: &Limits) -> Result<(), HairError> {
        in_range("length", self.length, 0.0, MAX_LENGTH)?;
        in_range("length jitter", self.length_jitter, 0.0, 1.0)?;
        in_range("lean", self.lean, 0.0, 1.0)?;
        for (what, v) in [("lean direction", self.lean_toward), ("bend", self.bend)] {
            if !v.is_finite() {
                return Err(HairError::NotFinite(what));
            }
        }
        if self.bend.length() > 100.0 {
            return Err(HairError::OutOfRange { what: "bend", min: 0.0, max: 100.0, found: f64::from(self.bend.length()) });
        }
        let l = limits.clamped();
        if self.points_per_strand < 2 || self.points_per_strand > l.max_points_per_strand {
            return Err(HairError::OutOfRange {
                what: "points per strand",
                min: 2.0,
                max: l.max_points_per_strand as f64,
                found: self.points_per_strand as f64,
            });
        }
        Ok(())
    }
}

/// Guide curves: `len()` strands of `points_per_strand()` points each. Point 0 of a strand is its root.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Guides {
    roots: Vec<SurfacePoint>,
    points_per_strand: usize,
    points: Vec<Vec3>,
}

impl Guides {
    /// Guides from stored data, validated: equal counts, finite points, at least two per strand.
    pub fn from_parts(roots: Vec<SurfacePoint>, points_per_strand: usize, points: Vec<Vec3>, limits: &Limits) -> Result<Guides, HairError> {
        limits.check(roots.len(), points_per_strand, true)?;
        let expected = roots.len() * points_per_strand;
        if points.len() != expected {
            return Err(HairError::LengthMismatch { what: "guide points", expected, found: points.len() });
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(HairError::NotFinite("guide point"));
        }
        Ok(Guides { roots, points_per_strand, points })
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn points_per_strand(&self) -> usize {
        self.points_per_strand
    }

    pub fn roots(&self) -> &[SurfacePoint] {
        &self.roots
    }

    pub fn all_points(&self) -> &[Vec3] {
        &self.points
    }

    /// The points of strand `i` (empty past the end).
    pub fn strand(&self, i: usize) -> &[Vec3] {
        let n = self.points_per_strand;
        self.points.get(i * n..(i + 1) * n).unwrap_or_default()
    }

    /// Mutable points of strand `i`. Tools edit through this; the root point (index 0) is expected
    /// to stay where the root is.
    pub fn strand_mut(&mut self, i: usize) -> &mut [Vec3] {
        let n = self.points_per_strand;
        self.points.get_mut(i * n..(i + 1) * n).unwrap_or_default()
    }

    /// Polyline length of strand `i`.
    pub fn strand_length(&self, i: usize) -> f32 {
        polyline_length(self.strand(i))
    }

    /// Re-attach the guides to a deformed copy of their surface: each guide is carried by its
    /// root's motion and turned by how its root normal turned. `old` and `new` share topology.
    pub fn follow(&mut self, old: &Surface, new: &Surface) -> Result<(), HairError> {
        if old.triangle_count() != new.triangle_count() || old.vertex_count() != new.vertex_count() {
            return Err(HairError::LengthMismatch { what: "deformed surface triangles", expected: old.triangle_count(), found: new.triangle_count() });
        }
        if self.roots.iter().any(|r| !old.contains(r)) {
            return Err(HairError::BadRoot);
        }
        let n = self.points_per_strand;
        for (root, strand) in self.roots.iter().zip(self.points.chunks_mut(n)) {
            let (p0, n0) = old.eval(root);
            let (p1, n1) = new.eval(root);
            let q = Quat::from_rotation_arc(n0, n1);
            for p in strand.iter_mut() {
                *p = p1 + q * (*p - p0);
            }
        }
        Ok(())
    }

    /// Bytes of memory held.
    pub fn bytes(&self) -> usize {
        crate::estimate_bytes(self.len(), self.points_per_strand)
    }
}

pub(crate) fn polyline_length(p: &[Vec3]) -> f32 {
    p.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// Grow guides on `surface`: roots drawn by the density map, each strand `length` long, leaving the
/// surface along its normal (leaned and bent as the spec says). Deterministic in `spec.seed`; guide
/// `i` does not depend on the amount.
pub fn grow_guides(surface: &Surface, spec: &GrowSpec, maps: &SurfaceMaps, limits: &Limits) -> Result<Guides, HairError> {
    spec.validate(limits)?;
    maps.check(surface)?;
    let l = limits.clamped();
    let sampler = Sampler::new(surface, maps.density);
    let count = spec.amount.resolve(sampler.weighted_area(), "guides", l.max_guides)?;
    l.check(count, spec.points_per_strand, true)?;
    let n = spec.points_per_strand;
    let count = if sampler.weighted_area() > 0.0 { count } else { 0 };
    let mut roots = filled(count, SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] })?;
    let mut points = filled(count * n, Vec3::ZERO)?;
    roots.par_iter_mut().zip(points.par_chunks_mut(n)).enumerate().for_each(|(i, (root, strand))| {
        *root = sampler.sample(spec.seed, i as u64);
        let (p0, normal) = surface.eval(root);
        let scale = maps.length.map_or(1.0, |m| map_at(surface, m, root));
        let jitter = 1.0 + spec.length_jitter * (rng::rand(spec.seed, i as u64, stream::LENGTH) * 2.0 - 1.0);
        let length = spec.length * jitter * scale;
        shape_strand(strand, p0, normal, length, spec);
    });
    Ok(Guides { roots, points_per_strand: n, points })
}

/// Lay a strand out from `root` along the growth direction, then march it to its exact length.
fn shape_strand(out: &mut [Vec3], root: Vec3, normal: Vec3, length: f32, spec: &GrowSpec) {
    let n = out.len();
    let dir = if spec.lean > 0.0 {
        let lean = spec.lean_toward.try_normalize().unwrap_or(normal);
        (normal * (1.0 - spec.lean) + lean * spec.lean).try_normalize().unwrap_or(normal)
    } else {
        normal
    };
    let seg = length / (n.saturating_sub(1).max(1) as f32);
    let mut prev_target = root;
    let mut pos = root;
    for (j, slot) in out.iter_mut().enumerate() {
        if j == 0 {
            *slot = root;
            continue;
        }
        let t = j as f32 / (n - 1) as f32;
        let target = root + dir * (length * t) + spec.bend * (length * t * t);
        let step = (target - prev_target).try_normalize().unwrap_or(dir);
        pos += step * seg;
        prev_target = target;
        *slot = pos;
    }
}
