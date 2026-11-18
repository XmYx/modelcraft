//! Render hairs interpolated from guides.
//!
//! A render hair has its own root on the surface. Its shape is a weighted blend of the offsets of the
//! nearest guides from their roots, each offset turned by the rotation that carries the guide's root
//! normal onto the hair's root normal, so a groom stays coherent over a curved surface. A hair whose
//! root is a guide's root reproduces that guide exactly.

use glam::{Quat, Vec3};
use rayon::prelude::*;

use crate::guides::{Amount, Guides, polyline_length};
use crate::kdtree::{KdTree, MAX_K};
use crate::rng::{self, noise1, stream};
use crate::surface::{Sampler, SurfaceMaps, map_at};
use crate::{HairError, Limits, Surface, SurfacePoint, filled, in_range};

/// How a hair picks the guides it blends.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Blend {
    /// The three nearest guides, weighted by the barycentric coordinates of the hair's root in the
    /// triangle they form (clamped to the triangle; inverse distance when the three are collinear).
    Barycentric,
    /// The nearest `k` guides (1 to [`MAX_K`]) weighted by inverse squared distance.
    Nearest { k: usize },
}

/// Pull hairs toward the guide nearest their root, strongest at the tip, so they gather into locks.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clump {
    /// 0 leaves hairs alone, 1 puts every tip on its guide's tip.
    pub amount: f32,
    /// How late the pull starts along the strand: weight is `t^tip_bias` (0 pulls evenly, 1 is linear).
    pub tip_bias: f32,
    /// Each hair's amount varies by up to this fraction of itself.
    pub jitter: f32,
}

/// Seeded wobble along the strand: smooth noise in the three axes, zero at the root.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Frizz {
    /// Peak offset as a fraction of the strand length.
    pub amount: f32,
    /// Noise cells along the strand.
    pub frequency: f32,
}

/// Strand width: `root` at the root to `tip` at the tip along `t^exponent`, with each strand's own
/// width varying by up to `jitter` (a fraction of itself).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Width {
    pub root: f32,
    pub tip: f32,
    pub exponent: f32,
    pub jitter: f32,
}

impl Default for Width {
    fn default() -> Width {
        Width { root: 0.0004, tip: 0.0001, exponent: 1.0, jitter: 0.0 }
    }
}

/// What [`interpolate`] makes.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InterpSpec {
    pub amount: Amount,
    pub seed: u64,
    pub blend: Blend,
    /// Points per render hair; `None` keeps the guides' count. A different count resamples the guides
    /// with a Catmull-Rom spline.
    pub points_per_strand: Option<usize>,
    /// Each hair's length varies by up to this fraction, uniformly.
    pub length_jitter: f32,
    pub clump: Clump,
    pub frizz: Frizz,
    pub width: Width,
}

impl Default for InterpSpec {
    fn default() -> InterpSpec {
        InterpSpec {
            amount: Amount::Count(10_000),
            seed: 7,
            blend: Blend::Nearest { k: 3 },
            points_per_strand: None,
            length_jitter: 0.0,
            clump: Clump { amount: 0.0, tip_bias: 1.0, jitter: 0.0 },
            frizz: Frizz { amount: 0.0, frequency: 3.0 },
            width: Width::default(),
        }
    }
}

impl InterpSpec {
    fn validate(&self) -> Result<(), HairError> {
        in_range("length jitter", self.length_jitter, 0.0, 1.0)?;
        in_range("clump amount", self.clump.amount, 0.0, 1.0)?;
        in_range("clump tip bias", self.clump.tip_bias, 0.0, 16.0)?;
        in_range("clump jitter", self.clump.jitter, 0.0, 1.0)?;
        in_range("frizz amount", self.frizz.amount, 0.0, 10.0)?;
        in_range("frizz frequency", self.frizz.frequency, 0.0, 1000.0)?;
        in_range("root width", self.width.root, 0.0, crate::MAX_LENGTH)?;
        in_range("tip width", self.width.tip, 0.0, crate::MAX_LENGTH)?;
        in_range("width exponent", self.width.exponent, 0.01, 16.0)?;
        in_range("width jitter", self.width.jitter, 0.0, 1.0)?;
        Ok(())
    }
}

/// The render hairs: `strand_count()` strands of `points_per_strand()` points, a root on the
/// surface and a width at each end per strand.
///
/// The strand list is in random order, so the first `n` strands of a groom are a uniform random
/// subset of it (see [`HairCurves::prefix_for_fraction`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RawCurves")]
pub struct HairCurves {
    points_per_strand: usize,
    points: Vec<Vec3>,
    roots: Vec<SurfacePoint>,
    root_width: Vec<f32>,
    tip_width: Vec<f32>,
    taper_exponent: f32,
}

/// The stored form of [`HairCurves`]: read from a file, then checked by [`HairCurves::from_parts`].
#[derive(serde::Deserialize)]
struct RawCurves {
    points_per_strand: usize,
    points: Vec<Vec3>,
    roots: Vec<SurfacePoint>,
    root_width: Vec<f32>,
    tip_width: Vec<f32>,
    taper_exponent: f32,
}

impl TryFrom<RawCurves> for HairCurves {
    type Error = HairError;
    fn try_from(r: RawCurves) -> Result<HairCurves, HairError> {
        HairCurves::from_parts(r.points_per_strand, r.points, r.roots, r.root_width, r.tip_width, r.taper_exponent, &Limits::default())
    }
}

impl HairCurves {
    /// Strands from stored data, validated.
    pub fn from_parts(
        points_per_strand: usize,
        points: Vec<Vec3>,
        roots: Vec<SurfacePoint>,
        root_width: Vec<f32>,
        tip_width: Vec<f32>,
        taper_exponent: f32,
        limits: &Limits,
    ) -> Result<HairCurves, HairError> {
        limits.check(roots.len(), points_per_strand, false)?;
        let n = roots.len();
        for (what, len, expected) in
            [("hair points", points.len(), n * points_per_strand), ("root widths", root_width.len(), n), ("tip widths", tip_width.len(), n)]
        {
            if len != expected {
                return Err(HairError::LengthMismatch { what, expected, found: len });
            }
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(HairError::NotFinite("hair point"));
        }
        if root_width.iter().chain(&tip_width).any(|w| !w.is_finite() || *w < 0.0) {
            return Err(HairError::NotFinite("strand width"));
        }
        in_range("width exponent", taper_exponent, 0.01, 16.0)?;
        Ok(HairCurves { points_per_strand, points, roots, root_width, tip_width, taper_exponent })
    }

    pub fn strand_count(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn points_per_strand(&self) -> usize {
        self.points_per_strand
    }

    pub fn total_points(&self) -> usize {
        self.points.len()
    }

    pub fn all_points(&self) -> &[Vec3] {
        &self.points
    }

    pub fn roots(&self) -> &[SurfacePoint] {
        &self.roots
    }

    pub fn taper_exponent(&self) -> f32 {
        self.taper_exponent
    }

    /// The points of strand `i` (empty past the end).
    pub fn strand(&self, i: usize) -> &[Vec3] {
        let n = self.points_per_strand;
        self.points.get(i * n..(i + 1) * n).unwrap_or_default()
    }

    pub fn strand_length(&self, i: usize) -> f32 {
        polyline_length(self.strand(i))
    }

    pub fn root_width(&self, i: usize) -> f32 {
        self.root_width.get(i).copied().unwrap_or(0.0)
    }

    pub fn tip_width(&self, i: usize) -> f32 {
        self.tip_width.get(i).copied().unwrap_or(0.0)
    }

    /// Width of strand `i` at `t` in `[0, 1]` along it (root to tip).
    pub fn width_at(&self, i: usize, t: f32) -> f32 {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        let (r, tip) = (self.root_width(i), self.tip_width(i));
        r + (tip - r) * t.powf(self.taper_exponent)
    }

    /// How many leading strands make up `fraction` (0 to 1) of the groom. Because strand order is
    /// random, that prefix is an unbiased thinner copy of the whole.
    pub fn prefix_for_fraction(&self, fraction: f32) -> usize {
        let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 1.0 };
        ((self.strand_count() as f64 * f64::from(f)).ceil() as usize).min(self.strand_count())
    }

    /// Axis-aligned bounds of every point, or `None` when empty.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let first = *self.points.first()?;
        Some(self.points.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
    }

    /// Bytes of memory held.
    pub fn bytes(&self) -> usize {
        crate::estimate_bytes(self.strand_count(), self.points_per_strand)
    }
}

/// Evaluate a uniform Catmull-Rom spline through `pts` at `t` in `[0, 1]`, ends clamped.
/// Knots are returned exactly.
fn sample_curve(pts: &[Vec3], t: f32) -> Vec3 {
    let n = pts.len();
    let at = |i: isize| pts.get(i.clamp(0, n as isize - 1) as usize).copied().unwrap_or(Vec3::ZERO);
    if n < 2 {
        return at(0);
    }
    let s = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let nearest = s.round();
    if (s - nearest).abs() < 1e-4 {
        return at(nearest as isize);
    }
    let i = (s.floor() as isize).min(n as isize - 2);
    let u = s - i as f32;
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let (u2, u3) = (u * u, u * u * u);
    0.5 * ((2.0 * p1) + (p2 - p0) * u + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u2 + (3.0 * p1 - p0 - 3.0 * p2 + p3) * u3)
}

/// Weights of up to [`MAX_K`] neighbours: `(guide index, weight)`, summing to 1.
#[derive(Clone, Copy)]
pub(crate) struct Weights {
    pub(crate) items: [(u32, f32); MAX_K],
    pub(crate) len: usize,
}

fn inverse_distance(found: &[(f32, u32)], scale2: f32) -> Weights {
    let mut w = Weights { items: [(0, 0.0); MAX_K], len: 0 };
    let eps = (scale2 * 1e-12).max(f32::MIN_POSITIVE);
    if let Some(&(d2, idx)) = found.first()
        && d2 <= eps
    {
        w.items[0] = (idx, 1.0);
        w.len = 1;
        return w;
    }
    let mut sum = 0.0;
    for (slot, &(d2, idx)) in w.items.iter_mut().zip(found) {
        let v = 1.0 / d2;
        *slot = (idx, v);
        sum += v;
        w.len += 1;
    }
    if sum > 0.0 && sum.is_finite() {
        for s in w.items.iter_mut().take(w.len) {
            s.1 /= sum;
        }
    } else if w.len > 0 {
        let e = 1.0 / w.len as f32;
        for s in w.items.iter_mut().take(w.len) {
            s.1 = e;
        }
    }
    w
}

/// Barycentric coordinates of `p` projected onto triangle `abc`, as `Some` when it is not degenerate.
fn plane_barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<[f32; 3]> {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() <= 1e-12 * (d00 * d11).max(f32::MIN_POSITIVE) {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let r = [1.0 - v - w, v, w];
    r.iter().all(|x| x.is_finite()).then_some(r)
}

pub(crate) fn weights_for(blend: Blend, found: &[(f32, u32)], q: Vec3, guide_roots: &[Vec3]) -> Weights {
    let scale2 = found.last().map_or(0.0, |f| f.0);
    match blend {
        Blend::Nearest { .. } => inverse_distance(found, scale2),
        Blend::Barycentric => {
            if found.len() == 3 {
                let g = |i: usize| found.get(i).and_then(|f| guide_roots.get(f.1 as usize)).copied().unwrap_or(Vec3::ZERO);
                if let Some(b) = plane_barycentric(q, g(0), g(1), g(2)) {
                    let clamped = [b[0].max(0.0), b[1].max(0.0), b[2].max(0.0)];
                    let sum: f32 = clamped.iter().sum();
                    if sum > 1e-6 {
                        let mut w = Weights { items: [(0, 0.0); MAX_K], len: 3 };
                        for (slot, (f, c)) in w.items.iter_mut().zip(found.iter().zip(clamped)) {
                            *slot = (f.1, c / sum);
                        }
                        return w;
                    }
                }
            }
            inverse_distance(found, scale2)
        }
    }
}

/// Blend render hairs from `guides` on `surface`. Roots follow the density map; hair `i` is the same
/// whatever the amount. Runs on all cores and is deterministic.
pub fn interpolate(surface: &Surface, guides: &Guides, spec: &InterpSpec, maps: &SurfaceMaps, limits: &Limits) -> Result<HairCurves, HairError> {
    spec.validate()?;
    maps.check(surface)?;
    if guides.is_empty() {
        return Err(HairError::NoGuides);
    }
    if guides.roots().iter().any(|r| !surface.contains(r)) {
        return Err(HairError::BadRoot);
    }
    let l = limits.clamped();
    let pps = spec.points_per_strand.unwrap_or(guides.points_per_strand());
    let sampler = Sampler::new(surface, maps.density);
    let count = spec.amount.resolve(sampler.weighted_area(), "strands", l.max_strands)?;
    l.check(count, pps, false)?;
    let count = if sampler.weighted_area() > 0.0 { count } else { 0 };

    let (gn, gp) = (guides.points_per_strand(), guides.all_points());
    let guide_root: Vec<Vec3> = (0..guides.len()).map(|g| gp.get(g * gn).copied().unwrap_or(Vec3::ZERO)).collect();
    let guide_normal: Vec<Vec3> = guides.roots().iter().map(|r| surface.normal(r)).collect();
    let guide_len: Vec<f32> = (0..guides.len()).map(|g| guides.strand_length(g)).collect();
    let tree = KdTree::new(guide_root.clone());
    let k = match spec.blend {
        Blend::Barycentric => 3.min(guides.len()),
        Blend::Nearest { k } => k.clamp(1, MAX_K).min(guides.len()),
    };

    let mut roots = filled(count, SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] })?;
    let mut points = filled(count * pps, Vec3::ZERO)?;
    let mut root_width = filled(count, 0.0f32)?;
    let mut tip_width = filled(count, 0.0f32)?;
    let seed = spec.seed;

    roots.par_iter_mut().zip(points.par_chunks_mut(pps)).zip(root_width.par_iter_mut().zip(tip_width.par_iter_mut())).enumerate().for_each(
        |(i, ((root, strand), (rw, tw)))| {
            let idx = i as u64;
            *root = sampler.sample(seed, idx);
            let (pos, normal) = surface.eval(root);
            let found = tree.nearest(pos, k);
            let w = weights_for(spec.blend, found.as_slice(), pos, &guide_root);
            let len_scale =
                (1.0 + spec.length_jitter * (rng::rand(seed, idx, stream::LENGTH) * 2.0 - 1.0)) * maps.length.map_or(1.0, |m| map_at(surface, m, root));
            // Guides blended, each carried from its root normal to this hair's.
            let mut rot = [Quat::IDENTITY; MAX_K];
            let mut blended_len = 0.0;
            for (r, &(g, wt)) in rot.iter_mut().zip(w.items.iter().take(w.len)) {
                let gi = g as usize;
                *r = Quat::from_rotation_arc(guide_normal.get(gi).copied().unwrap_or(Vec3::Y), normal);
                blended_len += wt * guide_len.get(gi).copied().unwrap_or(0.0);
            }
            let clump_target = w.items.first().filter(|_| w.len > 0).map(|&(g, _)| g as usize);
            let clump_amount = (spec.clump.amount * (1.0 + spec.clump.jitter * (rng::rand(seed, idx, stream::CLUMP) * 2.0 - 1.0))).clamp(0.0, 1.0);
            let frizz_len = spec.frizz.amount * blended_len * len_scale;
            let phase = rng::rand(seed, idx, stream::FRIZZ_PHASE) * 1000.0;
            let denom = (pps - 1) as f32;
            for (j, out) in strand.iter_mut().enumerate() {
                let t = j as f32 / denom;
                let mut offset = Vec3::ZERO;
                for (r, &(g, wt)) in rot.iter().zip(w.items.iter().take(w.len)) {
                    let gi = g as usize;
                    let gs = gp.get(gi * gn..(gi + 1) * gn).unwrap_or_default();
                    let base = gs.first().copied().unwrap_or(Vec3::ZERO);
                    offset += *r * (sample_curve(gs, t) - base) * wt;
                }
                let mut p = pos + offset * len_scale;
                if clump_amount > 0.0
                    && let Some(gi) = clump_target
                {
                    let gs = gp.get(gi * gn..(gi + 1) * gn).unwrap_or_default();
                    let target = sample_curve(gs, t);
                    p = p.lerp(target, clump_amount * t.powf(spec.clump.tip_bias));
                }
                if frizz_len > 0.0 {
                    let x = t * spec.frizz.frequency + phase;
                    let ramp = t * t * (3.0 - 2.0 * t);
                    let d = Vec3::new(noise1(seed, stream::FRIZZ_AXIS, x), noise1(seed, stream::FRIZZ_AXIS + 1, x), noise1(seed, stream::FRIZZ_AXIS + 2, x));
                    p += d * (frizz_len * ramp);
                }
                *out = p;
            }
            let wj = 1.0 + spec.width.jitter * (rng::rand(seed, idx, stream::WIDTH) * 2.0 - 1.0);
            *rw = spec.width.root * wj;
            *tw = spec.width.tip * wj;
        },
    );
    Ok(HairCurves { points_per_strand: pps, points, roots, root_width, tip_width, taper_exponent: spec.width.exponent })
}
