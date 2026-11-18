//! Grooming brushes on guide curves.
//!
//! A [`Stroke`] is a brush, a symmetry and a list of [`Dab`]s; applying it moves guide points near
//! each dab by an amount that falls off with the distance from the dab to the guide's root, scaled
//! by the stroke strength, the dab pressure and an optional per-vertex mask. Roots never move. Every
//! brush except [`BrushKind::Cut`] and [`BrushKind::Length`] keeps each strand's segment lengths, so
//! combing bends a strand and does not stretch it.
//!
//! Strokes are plain data (and serialisable), which is what makes them commands: the same stroke
//! applied to the same guides always gives the same guides, whatever drew it.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::guides::Guides;
use crate::rng::{self, noise1};
use crate::surface::{Surface, SurfacePoint, map_at};
use crate::{HairError, MAX_LENGTH, in_range};

/// Most dabs one stroke may hold.
pub const MAX_DABS: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushKind {
    /// Drag strands along the stroke; tips follow most.
    Comb,
    /// Shorten strands to `cut_length`.
    Cut,
    /// Grow strands longer (shorter with `invert`).
    Length,
    /// Lift strands toward the surface normal (lay them down with `invert`).
    Puff,
    /// Even out kinks along a strand and the differences between neighbours.
    Smooth,
    /// Gather strands toward the one nearest the dab (spread them with `invert`).
    Clump,
    /// Add random wobble.
    Frizz,
}

impl BrushKind {
    pub const ALL: [BrushKind; 7] =
        [BrushKind::Comb, BrushKind::Cut, BrushKind::Length, BrushKind::Puff, BrushKind::Smooth, BrushKind::Clump, BrushKind::Frizz];

    /// The command-line and JSON name.
    pub fn id(self) -> &'static str {
        match self {
            BrushKind::Comb => "comb",
            BrushKind::Cut => "cut",
            BrushKind::Length => "length",
            BrushKind::Puff => "puff",
            BrushKind::Smooth => "smooth",
            BrushKind::Clump => "clump",
            BrushKind::Frizz => "frizz",
        }
    }

    pub fn from_id(id: &str) -> Option<BrushKind> {
        BrushKind::ALL.into_iter().find(|k| k.id() == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub kind: BrushKind,
    /// Reach, in scene units.
    pub radius: f32,
    /// 0 to 1.
    pub strength: f32,
    /// 0 is full strength to the edge, 1 a smooth fall to zero from the centre.
    pub falloff: f32,
    /// Opposite effect: shorter, flatter, spread apart.
    pub invert: bool,
    /// The length [`BrushKind::Cut`] cuts to.
    pub cut_length: f32,
    /// Noise cells along a strand for [`BrushKind::Frizz`].
    pub frequency: f32,
    pub seed: u64,
}

impl Brush {
    pub fn new(kind: BrushKind) -> Brush {
        Brush { kind, radius: 0.2, strength: 0.5, falloff: 0.5, invert: false, cut_length: 0.05, frequency: 3.0, seed: 1 }
    }

    pub fn validate(&self) -> Result<(), HairError> {
        in_range("brush radius", self.radius, 0.0, MAX_LENGTH)?;
        in_range("brush strength", self.strength, 0.0, 1.0)?;
        in_range("brush falloff", self.falloff, 0.0, 1.0)?;
        in_range("cut length", self.cut_length, 0.0, MAX_LENGTH)?;
        in_range("frizz frequency", self.frequency, 0.0, 1000.0)?;
        Ok(())
    }
}

/// Mirror planes through the origin of the emitter's space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symmetry {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Symmetry {
    pub fn any(&self) -> bool {
        self.x || self.y || self.z
    }

    /// The dab and its mirror images (up to eight), without repeats for a dab on a plane.
    pub fn images(&self, dab: &Dab) -> Vec<Dab> {
        let mut out = vec![*dab];
        for (on, axis) in [(self.x, 0), (self.y, 1), (self.z, 2)] {
            if !on {
                continue;
            }
            let mirrored: Vec<Dab> = out.iter().map(|d| d.mirrored(axis)).collect();
            for m in mirrored {
                if !out.iter().any(|d| d.center.distance_squared(m.center) < 1e-12 && d.delta.distance_squared(m.delta) < 1e-12) {
                    out.push(m);
                }
            }
        }
        out
    }
}

/// One stamp of the brush: where it touches the surface (in the emitter's space), the surface normal
/// there, and how far the pointer moved since the last dab (for combing).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dab {
    pub center: Vec3,
    pub normal: Vec3,
    pub delta: Vec3,
    pub pressure: f32,
}

impl Dab {
    pub fn new(center: Vec3) -> Dab {
        Dab { center, normal: Vec3::Y, delta: Vec3::ZERO, pressure: 1.0 }
    }

    fn mirrored(&self, axis: usize) -> Dab {
        let flip = |mut v: Vec3| {
            v[axis] = -v[axis];
            v
        };
        Dab { center: flip(self.center), normal: flip(self.normal), delta: flip(self.delta), pressure: self.pressure }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: Brush,
    #[serde(default)]
    pub symmetry: Symmetry,
    pub dabs: Vec<Dab>,
}

/// What a stroke did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrokeReport {
    /// Guides moved by at least one dab.
    pub guides_touched: usize,
    pub dabs_applied: usize,
}

/// Apply every dab of `stroke` to `guides`. `mask` is a per-vertex map (1 everywhere when absent): a
/// guide whose root sits where the mask is 0 is never touched.
pub fn apply_stroke(guides: &mut Guides, surface: &Surface, mask: Option<&[f32]>, stroke: &Stroke) -> Result<StrokeReport, HairError> {
    apply_dabs(guides, surface, mask, &stroke.brush, stroke.symmetry, &stroke.dabs, 0)
}

/// Apply `dabs` of a stroke that already placed `first` dabs. Applying a stroke in pieces gives the
/// same guides as applying it whole, which is what lets the app preview a stroke dab by dab and then
/// commit it as one command.
pub fn apply_dabs(
    guides: &mut Guides,
    surface: &Surface,
    mask: Option<&[f32]>,
    brush: &Brush,
    symmetry: Symmetry,
    dabs: &[Dab],
    first: usize,
) -> Result<StrokeReport, HairError> {
    brush.validate()?;
    if dabs.len().saturating_add(first) > MAX_DABS {
        return Err(HairError::TooMany { what: "dabs", requested: dabs.len().saturating_add(first), max: MAX_DABS });
    }
    if let Some(m) = mask
        && m.len() != surface.vertex_count()
    {
        return Err(HairError::LengthMismatch { what: "mask", expected: surface.vertex_count(), found: m.len() });
    }
    for d in dabs {
        if !(d.center.is_finite() && d.normal.is_finite() && d.delta.is_finite() && d.pressure.is_finite()) {
            return Err(HairError::NotFinite("dab"));
        }
    }
    if guides.roots().iter().any(|r| !surface.contains(r)) {
        return Err(HairError::BadRoot);
    }
    let mut touched = vec![false; guides.len()];
    let mut applied = 0;
    for (n, dab) in dabs.iter().enumerate() {
        for (m, image) in symmetry.images(dab).iter().enumerate() {
            apply_dab(guides, surface, mask, brush, image, ((first + n) as u64) << 3 | m as u64, &mut touched);
            applied += 1;
        }
    }
    Ok(StrokeReport { guides_touched: touched.iter().filter(|t| **t).count(), dabs_applied: applied })
}

/// Weight of a point `d` from the centre of a brush of radius `r`.
fn falloff(d: f32, r: f32, soft: f32) -> f32 {
    if r <= 0.0 || d >= r {
        return 0.0;
    }
    let x = d / r;
    let smooth = 1.0 - x * x * (3.0 - 2.0 * x);
    1.0 - soft * (1.0 - smooth)
}

fn apply_dab(guides: &mut Guides, surface: &Surface, mask: Option<&[f32]>, b: &Brush, dab: &Dab, dab_id: u64, touched: &mut [bool]) {
    let n = guides.points_per_strand();
    if n < 2 {
        return;
    }
    // Who is in reach, with the weight each gets.
    let mut hits: Vec<(usize, f32)> = Vec::new();
    for i in 0..guides.len() {
        let root = guides.strand(i).first().copied().unwrap_or(Vec3::ZERO);
        let d = root.distance(dab.center);
        let mut w = falloff(d, b.radius, b.falloff) * b.strength * dab.pressure.clamp(0.0, 1.0);
        if let (Some(m), Some(r)) = (mask, guides.roots().get(i)) {
            w *= map_at(surface, m, r);
        }
        if w > 1e-6 {
            hits.push((i, w));
        }
    }
    if hits.is_empty() {
        return;
    }
    // The clump leader and the smoothing targets read the strands as they were before this dab.
    let before: Vec<Vec<Vec3>> = hits.iter().map(|(i, _)| guides.strand(*i).to_vec()).collect();
    let leader = hits.iter().zip(&before).min_by(|a, c| {
        let da = a.1.first().map_or(f32::MAX, |p| p.distance_squared(dab.center));
        let dc = c.1.first().map_or(f32::MAX, |p| p.distance_squared(dab.center));
        da.total_cmp(&dc)
    });
    let leader_pts: Option<Vec<Vec3>> = leader.map(|(_, pts)| pts.clone());
    let mean_offsets = if b.kind == BrushKind::Smooth { Some(mean_shape(&hits, &before, surface, guides.roots(), n)) } else { None };

    for (h, (i, w)) in hits.iter().enumerate() {
        let (gi, w) = (*i, *w);
        let Some(old) = before.get(h) else { continue };
        let root = old.first().copied().unwrap_or(Vec3::ZERO);
        let seg: Vec<f32> = old.windows(2).map(|p| p[0].distance(p[1])).collect();
        let len: f32 = seg.iter().sum();
        let mut pts = old.clone();
        let t = |j: usize| j as f32 / (n - 1) as f32;
        match b.kind {
            BrushKind::Comb => {
                for (j, p) in pts.iter_mut().enumerate().skip(1) {
                    *p += dab.delta * (w * t(j));
                }
                march(&mut pts, &seg);
            }
            BrushKind::Cut => {
                let target = len - (len - b.cut_length).max(0.0) * w.min(1.0);
                pts = resample(old, target);
            }
            BrushKind::Length => {
                let k = 1.0 + 0.25 * w * if b.invert { -1.0 } else { 1.0 };
                let k = k.clamp(0.05, 4.0);
                if len * k <= MAX_LENGTH {
                    for p in pts.iter_mut().skip(1) {
                        *p = root + (*p - root) * k;
                    }
                }
            }
            BrushKind::Puff => {
                let up = surface_normal(surface, guides.roots().get(gi), dab.normal);
                for (j, p) in pts.iter_mut().enumerate().skip(1) {
                    let o = *p - root;
                    let target = if b.invert { o - up * o.dot(up) } else { up * o.length() };
                    *p = root + o + (target - o) * (w * t(j).sqrt()).min(1.0);
                }
                march(&mut pts, &seg);
            }
            BrushKind::Smooth => {
                let a = (w * 0.5).min(0.5);
                let src = old.clone();
                for j in 1..n {
                    let target = if j + 1 < n {
                        (src[j - 1] + src[j + 1]) * 0.5
                    } else if n >= 3 {
                        src[j - 1] * 2.0 - src[j - 2]
                    } else {
                        src[j]
                    };
                    pts[j] = src[j] + (target - src[j]) * a;
                }
                if let Some(mean) = mean_offsets.as_ref().and_then(|m| m.get(h)) {
                    for (j, p) in pts.iter_mut().enumerate().skip(1) {
                        if let Some(m) = mean.get(j) {
                            *p += (root + *m - *p) * a * t(j);
                        }
                    }
                }
                march(&mut pts, &seg);
            }
            BrushKind::Clump => {
                if let Some(lead) = &leader_pts {
                    for (j, p) in pts.iter_mut().enumerate().skip(1) {
                        let target = lead.get(j).copied().unwrap_or(*p);
                        let k = (w * t(j)).min(1.0);
                        *p = if b.invert { *p + (*p - target) * (k * 0.5) } else { p.lerp(target, k) };
                    }
                    march(&mut pts, &seg);
                }
            }
            BrushKind::Frizz => {
                let seed = rng::hash(b.seed, gi as u64, dab_id);
                for (j, p) in pts.iter_mut().enumerate().skip(1) {
                    let x = t(j) * b.frequency;
                    let d = Vec3::new(noise1(seed, 1, x), noise1(seed, 2, x), noise1(seed, 3, x));
                    *p += d * (len * 0.5 * w * t(j));
                }
                march(&mut pts, &seg);
            }
        }
        if pts.iter().all(|p| p.is_finite())
            && let Some(dst) = guides.strand_mut(gi).get_mut(1..)
            && let Some(src) = pts.get(1..)
            && dst != src
        {
            dst.copy_from_slice(src);
            if let Some(t) = touched.get_mut(gi) {
                *t = true;
            }
        }
    }
}

fn surface_normal(surface: &Surface, root: Option<&SurfacePoint>, fallback: Vec3) -> Vec3 {
    root.map_or(fallback, |r| surface.normal(r))
}

/// Re-march `pts` from its root keeping each segment's old length, along the new directions.
fn march(pts: &mut [Vec3], seg: &[f32]) {
    for j in 1..pts.len() {
        let prev = pts[j - 1];
        let dir = (pts[j] - prev).try_normalize().unwrap_or(Vec3::Y);
        pts[j] = prev + dir * seg.get(j - 1).copied().unwrap_or(0.0);
    }
}

/// The same curve cut or stretched to total length `target`, with the same number of points,
/// evenly spaced along the original path.
fn resample(pts: &[Vec3], target: f32) -> Vec<Vec3> {
    let n = pts.len();
    let mut cum = vec![0.0f32; n];
    for j in 1..n {
        cum[j] = cum[j - 1] + pts[j - 1].distance(pts[j]);
    }
    let total = cum.last().copied().unwrap_or(0.0);
    let target = target.clamp(0.0, total);
    let mut out = Vec::with_capacity(n);
    for j in 0..n {
        let s = if n > 1 { target * j as f32 / (n - 1) as f32 } else { 0.0 };
        let k = cum.partition_point(|c| *c < s).clamp(1, n - 1);
        let (c0, c1) = (cum[k - 1], cum[k]);
        let u = if c1 > c0 { (s - c0) / (c1 - c0) } else { 0.0 };
        out.push(pts[k - 1].lerp(pts[k], u.clamp(0.0, 1.0)));
    }
    out
}

/// For each hit, the weighted mean of the offsets (from the root) of every hit strand, each turned
/// from its own root normal onto this one's.
fn mean_shape(hits: &[(usize, f32)], before: &[Vec<Vec3>], surface: &Surface, roots: &[SurfacePoint], n: usize) -> Vec<Vec<Vec3>> {
    use glam::Quat;
    let normals: Vec<Vec3> = hits.iter().map(|(i, _)| roots.get(*i).map_or(Vec3::Y, |r| surface.normal(r))).collect();
    let sum_w: f32 = hits.iter().map(|h| h.1).sum();
    (0..hits.len())
        .map(|a| {
            let mut mean = vec![Vec3::ZERO; n];
            for (c, (_, w)) in hits.iter().enumerate() {
                let q = Quat::from_rotation_arc(normals.get(c).copied().unwrap_or(Vec3::Y), normals.get(a).copied().unwrap_or(Vec3::Y));
                let root = before[c].first().copied().unwrap_or(Vec3::ZERO);
                for (j, m) in mean.iter_mut().enumerate() {
                    *m += q * (before[c].get(j).copied().unwrap_or(root) - root) * (*w / sum_w.max(1e-9));
                }
            }
            mean
        })
        .collect()
}

// ---- per-vertex maps ----

/// Paint a per-vertex map (a mask or a density) with a round brush: values within `radius` of
/// `center` move toward `value` (0 to 1) by `strength` at the centre, less toward the edge.
pub fn paint_map(map: &mut [f32], surface: &Surface, center: Vec3, radius: f32, value: f32, strength: f32, soft: f32) -> Result<usize, HairError> {
    if map.len() != surface.vertex_count() {
        return Err(HairError::LengthMismatch { what: "map", expected: surface.vertex_count(), found: map.len() });
    }
    in_range("brush radius", radius, 0.0, MAX_LENGTH)?;
    in_range("map value", value, 0.0, 1.0)?;
    in_range("brush strength", strength, 0.0, 1.0)?;
    in_range("brush falloff", soft, 0.0, 1.0)?;
    if !center.is_finite() {
        return Err(HairError::NotFinite("brush center"));
    }
    let mut changed = 0;
    for (m, p) in map.iter_mut().zip(surface.positions()) {
        let w = falloff(p.distance(center), radius, soft) * strength;
        if w > 0.0 {
            let old = if m.is_finite() { *m } else { 0.0 };
            let new = (old + (value - old) * w).clamp(0.0, 1.0);
            if new != old {
                *m = new;
                changed += 1;
            }
        }
    }
    Ok(changed)
}
