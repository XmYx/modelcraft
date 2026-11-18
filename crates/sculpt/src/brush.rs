//! Brushes. A brush moves the vertices within its radius, weighted by a falloff curve, an
//! optional stamp (a pattern that shapes the dab) and the mask. Symmetry repeats each dab
//! mirrored across the chosen object axes.

use glam::{Mat3, Vec3};
use serde::{Deserialize, Serialize};

use crate::{SculptMesh, StrokeState};

pub use crate::stroke::StrokeMode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrushKind {
    /// Push the surface out along the area normal.
    Draw,
    /// Build up towards a plane above the surface, flattening as it goes.
    Clay,
    /// Relax towards the neighbours' average.
    Smooth,
    /// Push every vertex along its own normal.
    Inflate,
    /// Drag the captured region with the pointer.
    Grab,
    /// Pull towards the brush centre.
    Pinch,
    /// Press towards the average plane.
    Flatten,
    /// Cut a sharp groove (pinch plus an inward push).
    Crease,
    /// Paint the mask (masked vertices resist every other brush).
    Mask,
    /// Pull the surface along with the pointer, picking up new surface as it goes.
    Pull,
    /// Raise (or sink) the surface by one even step, however often the stroke passes.
    Plateau,
    /// Cut away what stands above the brush plane.
    Trim,
    /// Fill in what lies below the brush plane.
    Fill,
    /// Flatten and relax at once, for hard smooth planes.
    Polish,
    /// Pile material up fast, with a flat top.
    Buildup,
    /// Grab that follows the surface: only what is connected to the point under the pointer.
    SurfaceGrab,
    /// Cut a narrow, deep groove.
    Gouge,
    /// Slide the surface sideways along the stroke.
    Slide,
    /// Swell the surface into a round bulge.
    Bulge,
    /// Turn what the brush captured around the view direction as the pointer moves sideways.
    Rotate,
    /// Move the surface as if it were rubber: everything nearby follows, less with distance,
    /// and the volume is kept (the regularized Kelvinlet of de Goes and James, 2017).
    ElasticMove,
    /// Plane away what stands proud of the surface, a little deeper with every pass.
    Scrape,
}

pub const KINDS: &[BrushKind] = &[
    BrushKind::Draw,
    BrushKind::Clay,
    BrushKind::Smooth,
    BrushKind::Inflate,
    BrushKind::Grab,
    BrushKind::Pinch,
    BrushKind::Flatten,
    BrushKind::Crease,
    BrushKind::Mask,
    BrushKind::Pull,
    BrushKind::Plateau,
    BrushKind::Trim,
    BrushKind::Fill,
    BrushKind::Polish,
    BrushKind::Buildup,
    BrushKind::SurfaceGrab,
    BrushKind::Gouge,
    BrushKind::Slide,
    BrushKind::Bulge,
    BrushKind::Rotate,
    BrushKind::ElasticMove,
    BrushKind::Scrape,
];

impl BrushKind {
    pub fn label(self) -> &'static str {
        match self {
            BrushKind::Draw => "Draw",
            BrushKind::Clay => "Clay",
            BrushKind::Smooth => "Smooth",
            BrushKind::Inflate => "Inflate",
            BrushKind::Grab => "Grab",
            BrushKind::Pinch => "Pinch",
            BrushKind::Flatten => "Flatten",
            BrushKind::Crease => "Crease",
            BrushKind::Mask => "Mask",
            BrushKind::Pull => "Pull",
            BrushKind::Plateau => "Plateau",
            BrushKind::Trim => "Trim",
            BrushKind::Fill => "Fill",
            BrushKind::Polish => "Polish",
            BrushKind::Buildup => "Buildup",
            BrushKind::SurfaceGrab => "Surface grab",
            BrushKind::Gouge => "Gouge",
            BrushKind::Slide => "Slide",
            BrushKind::Bulge => "Bulge",
            BrushKind::Rotate => "Rotate",
            BrushKind::ElasticMove => "Elastic move",
            BrushKind::Scrape => "Scrape",
        }
    }

    /// The brush moves what it captured when the stroke began, following the pointer's drag
    /// rather than dabbing along the surface.
    pub fn drags(self) -> bool {
        matches!(self, BrushKind::Grab | BrushKind::SurfaceGrab | BrushKind::Pull | BrushKind::Rotate | BrushKind::ElasticMove)
    }

    /// The brush centre travels with the pointer's drag (it does not stay where the stroke began).
    pub fn travels(self) -> bool {
        matches!(self, BrushKind::Pull | BrushKind::ElasticMove)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    #[default]
    Smooth,
    Sphere,
    Linear,
    Sharp,
    Constant,
    /// Full almost to the rim, then a quick drop.
    Root,
    /// A thin spike at the centre.
    Needle,
}

impl Falloff {
    /// Weight at normalised distance `x` (0 centre .. 1 rim).
    pub fn weight(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Falloff::Smooth => {
                let t = 1.0 - x;
                t * t * (3.0 - 2.0 * t)
            }
            Falloff::Sphere => (1.0 - x * x).max(0.0).sqrt(),
            Falloff::Linear => 1.0 - x,
            Falloff::Sharp => (1.0 - x).powi(3),
            Falloff::Constant => 1.0,
            Falloff::Root => (1.0 - x).max(0.0).sqrt(),
            Falloff::Needle => (1.0 - x).powi(6),
        }
    }
}

/// A procedural pattern that shapes each dab (sampled on the plane across the brush).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Stamp {
    #[default]
    None,
    Noise {
        scale: f32,
    },
    Rings {
        count: f32,
    },
    Square,
    /// A picture from the built-in library, turned by `turn` radians.
    Image {
        alpha: crate::alpha::Alpha,
        #[serde(default)]
        turn: f32,
    },
}

impl Stamp {
    fn sample(&self, u: f32, v: f32) -> f32 {
        match *self {
            Stamp::None => 1.0,
            Stamp::Noise { scale } => {
                let s = scale.clamp(0.1, 64.0);
                let h = |x: f32, y: f32| ((x * 12.9898 + y * 78.233).sin() * 43758.547).fract().abs();
                let (x, y) = (u * s, v * s);
                let (ix, iy) = (x.floor(), y.floor());
                let (fx, fy) = (x - ix, y - iy);
                let a = h(ix, iy) + (h(ix + 1.0, iy) - h(ix, iy)) * fx;
                let b = h(ix, iy + 1.0) + (h(ix + 1.0, iy + 1.0) - h(ix, iy + 1.0)) * fx;
                a + (b - a) * fy
            }
            Stamp::Rings { count } => 0.5 + 0.5 * ((u * u + v * v).sqrt() * count.clamp(1.0, 32.0) * std::f32::consts::TAU).cos(),
            Stamp::Image { alpha, turn } => {
                let (s, c) = if turn.is_finite() { turn.sin_cos() } else { (0.0, 1.0) };
                alpha.sample(u * c + v * s, v * c - u * s)
            }
            Stamp::Square => {
                if u.abs().max(v.abs()) < 0.7 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

/// Mirror axes and radial copies, in object space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symmetry {
    pub x: bool,
    pub y: bool,
    pub z: bool,
    /// Radial symmetry: this many copies of every dab, turned evenly around `radial_axis`
    /// (0 or 1: off).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub radial: u8,
    /// The axis radial copies turn around: 0 X, 1 Y, 2 Z.
    #[serde(default = "y_axis", skip_serializing_if = "is_y_axis")]
    pub radial_axis: u8,
}

impl Default for Symmetry {
    fn default() -> Self {
        Symmetry { x: false, y: false, z: false, radial: 0, radial_axis: 1 }
    }
}

/// Most radial copies of a dab.
pub const MAX_RADIAL: u8 = 64;

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}
fn y_axis() -> u8 {
    1
}
fn is_y_axis(v: &u8) -> bool {
    *v == 1
}

impl Symmetry {
    /// The transforms to repeat a dab with (always starts with the identity): every mirror
    /// combination, for every radial copy.
    pub fn passes(&self) -> Vec<Mat3> {
        let mut mirrors = vec![Vec3::ONE];
        for (on, axis) in [(self.x, Vec3::new(-1.0, 1.0, 1.0)), (self.y, Vec3::new(1.0, -1.0, 1.0)), (self.z, Vec3::new(1.0, 1.0, -1.0))] {
            if on {
                let more: Vec<Vec3> = mirrors.iter().map(|s| *s * axis).collect();
                mirrors.extend(more);
            }
        }
        let copies = self.radial.clamp(1, MAX_RADIAL);
        let axis = [Vec3::X, Vec3::Y, Vec3::Z].get(self.radial_axis as usize).copied().unwrap_or(Vec3::Y);
        let mut out = Vec::with_capacity(mirrors.len() * copies as usize);
        for k in 0..copies {
            let turn = if k == 0 { Mat3::IDENTITY } else { Mat3::from_axis_angle(axis, std::f32::consts::TAU * f32::from(k) / f32::from(copies)) };
            out.extend(mirrors.iter().map(|m| turn * Mat3::from_diagonal(*m)));
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushSettings {
    pub kind: BrushKind,
    /// Radius in object units.
    pub radius: f32,
    /// 0..1.
    pub strength: f32,
    #[serde(default)]
    pub falloff: Falloff,
    #[serde(default)]
    pub stamp: Stamp,
    /// Reverse the direction (dig instead of build, unmask instead of mask).
    #[serde(default)]
    pub invert: bool,
    #[serde(default)]
    pub symmetry: Symmetry,
    #[serde(default = "yes")]
    pub pressure_strength: bool,
    #[serde(default)]
    pub pressure_radius: bool,
    /// How much of the radius, from the centre, keeps full weight before the falloff starts (0..1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hardness: f32,
    /// How pointer movement becomes dabs.
    #[serde(default, skip_serializing_if = "is_default_stroke")]
    pub stroke: crate::stroke::StrokeMode,
    /// Distance between dabs, in brush radii.
    #[serde(default = "default_spacing")]
    pub spacing: f32,
    /// Live detail: split the triangles under the brush until their edges are no longer than
    /// this (object units). None: the triangles stay as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_detail: Option<f32>,
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}
fn is_default_stroke(m: &crate::stroke::StrokeMode) -> bool {
    *m == crate::stroke::StrokeMode::Spaced
}
fn default_spacing() -> f32 {
    0.2
}

impl BrushSettings {
    pub fn new(kind: BrushKind, radius: f32) -> Self {
        let strength = match kind {
            BrushKind::Smooth => 0.5,
            BrushKind::Grab | BrushKind::SurfaceGrab | BrushKind::Pull | BrushKind::Slide | BrushKind::Rotate | BrushKind::ElasticMove => 1.0,
            BrushKind::Mask => 0.8,
            BrushKind::Polish | BrushKind::Trim | BrushKind::Fill | BrushKind::Scrape => 0.6,
            _ => 0.4,
        };
        BrushSettings {
            kind,
            radius,
            strength,
            falloff: Falloff::Smooth,
            stamp: Stamp::None,
            invert: false,
            symmetry: Symmetry::default(),
            pressure_strength: true,
            pressure_radius: false,
            hardness: 0.0,
            stroke: crate::stroke::StrokeMode::Spaced,
            spacing: default_spacing(),
            live_detail: None,
        }
    }

    /// Weight at normalised distance `x` (0 centre .. 1 rim): the falloff, starting where the
    /// hard core ends.
    pub fn weight(&self, x: f32) -> f32 {
        let h = if self.hardness.is_finite() { self.hardness.clamp(0.0, 1.0) } else { 0.0 };
        if x <= h {
            return 1.0;
        }
        if h >= 1.0 { 0.0 } else { self.falloff.weight((x - h) / (1.0 - h)) }
    }
}

/// One application of the brush (object space).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Dab {
    pub center: Vec3,
    /// Direction from the eye into the scene.
    pub view_dir: Vec3,
    /// Pen pressure 0..1 (1 for a mouse).
    pub pressure: f32,
    /// Grab brush: pointer movement since the previous dab.
    #[serde(default)]
    pub grab_delta: Vec3,
    /// Multiplies the brush radius for this dab (spray and drag strokes).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale: f32,
}

/// A recorded stroke (replayable).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub brush: BrushSettings,
    pub dabs: Vec<Dab>,
}

/// Most dabs a single stroke may hold (about 20 minutes of drawing at 60 dabs per second).
pub const MAX_DABS: usize = 100_000;

pub(crate) fn apply(m: &mut SculptMesh, st: &mut StrokeState, dab: &Dab) {
    if st.dabs.len() >= MAX_DABS {
        return;
    }
    let b = st.brush.clone();
    let pressure = dab.pressure.clamp(0.0, 1.0);
    let scale = if dab.scale.is_finite() { dab.scale.clamp(0.01, 100.0) } else { 1.0 };
    let radius = (b.radius * scale * if b.pressure_radius { pressure.max(0.05) } else { 1.0 }).clamp(1e-6, 1e6);
    let strength = b.strength.clamp(0.0, 1.0) * if b.pressure_strength { pressure } else { 1.0 };
    let sign = if b.invert { -1.0 } else { 1.0 };
    let first = st.dabs.is_empty();
    let mut verts = Vec::new();
    let mut targets: Vec<(u32, Vec3)> = Vec::new();
    for (pass, mirror) in b.symmetry.passes().into_iter().enumerate() {
        let center = mirror * dab.center;
        let view = mirror * dab.view_dir;
        if let Some(edge) = b.live_detail
            && b.kind != crate::BrushKind::Mask
            && (!matches!(b.kind, BrushKind::Grab | BrushKind::SurfaceGrab | BrushKind::Rotate) || first)
        {
            m.refine_sphere(st, center, radius, edge);
        }
        if b.kind == BrushKind::ElasticMove {
            elastic_move(m, st, center, mirror * dab.grab_delta, radius, strength.max(if b.pressure_strength { 0.0 } else { 1.0 }));
            continue;
        }
        if matches!(b.kind, BrushKind::Grab | BrushKind::SurfaceGrab | BrushKind::Rotate) {
            if first {
                let captured = if b.kind == BrushKind::SurfaceGrab {
                    surface_capture(m, &b, center, radius)
                } else {
                    m.query_sphere(center, radius, &mut verts);
                    verts
                        .iter()
                        .filter_map(|v| {
                            let p = m.positions.get(*v as usize)?;
                            let w = b.weight(p.distance(center) / radius) * (1.0 - m.mask.get(*v as usize).copied().unwrap_or(0.0));
                            (w > 0.0).then_some((*v, w))
                        })
                        .collect()
                };
                st.grabbed.push(captured);
            }
            let delta = mirror * dab.grab_delta;
            let amount = strength.max(if b.pressure_strength { 0.0 } else { 1.0 });
            targets.clear();
            // Rotate: sideways movement of the pointer (across the view) is the angle, one
            // radian per brush radius; a mirrored pass turns the other way by itself.
            let axis = view.normalize_or(Vec3::Y);
            let angle = delta.dot(tangent_basis(view).0) / radius * sign * mirror.determinant().signum();
            for (v, w) in st.grabbed.get(pass).map(Vec::as_slice).unwrap_or_default() {
                if let Some(p) = m.positions.get(*v as usize) {
                    let q = if b.kind == BrushKind::Rotate {
                        center + glam::Quat::from_axis_angle(axis, angle * *w * amount) * (*p - center)
                    } else {
                        *p + delta * *w * amount
                    };
                    targets.push((*v, q));
                }
            }
            commit(m, st, &targets);
            continue;
        }
        m.query_sphere(center, radius, &mut verts);
        if verts.is_empty() {
            continue;
        }
        // Weights and the area normal.
        let (tu, tv) = tangent_basis(view);
        let weigh = |v: &u32| {
            let p = m.positions.get(*v as usize).copied().unwrap_or_default();
            let d = p - center;
            let mut w = b.weight(d.length() / radius);
            if b.stamp != Stamp::None {
                w *= b.stamp.sample(d.dot(tu) / radius, d.dot(tv) / radius);
            }
            if b.kind != crate::BrushKind::Mask {
                w *= 1.0 - m.mask.get(*v as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
            w
        };
        let weights: Vec<f32> = verts.iter().map(weigh).collect();
        let mut area_n = Vec3::ZERO;
        let mut area_c = Vec3::ZERO;
        let mut wsum = 0.0;
        for (v, w) in verts.iter().zip(&weights) {
            let p = m.positions.get(*v as usize).copied().unwrap_or_default();
            let n = m.normals.get(*v as usize).copied().unwrap_or(Vec3::Y);
            area_n += n * *w;
            area_c += p * *w;
            wsum += *w;
        }
        let mut n = area_n.normalize_or_zero();
        if n == Vec3::ZERO {
            n = -view.normalize_or(Vec3::Y);
        }
        let c = if wsum > 0.0 { area_c / wsum } else { center };
        targets.clear();
        let step = radius * strength;
        match b.kind {
            crate::BrushKind::Mask => {
                let mut changed = Vec::new();
                for (v, w) in verts.iter().zip(&weights) {
                    if let Some(mk) = m.mask.get(*v as usize).copied() {
                        record(st, m, *v);
                        let nm = (mk + sign * strength * w).clamp(0.0, 1.0);
                        if let Some(x) = m.mask.get_mut(*v as usize) {
                            *x = nm;
                        }
                        changed.push(*v);
                    }
                }
                for v in changed {
                    m.mark_dirty(v);
                }
                continue;
            }
            crate::BrushKind::Smooth => {
                for (v, w) in verts.iter().zip(&weights) {
                    let p = m.positions.get(*v as usize).copied().unwrap_or_default();
                    let ns = m.neighbors(*v);
                    if ns.is_empty() {
                        continue;
                    }
                    let avg = ns.iter().map(|k| m.positions.get(*k as usize).copied().unwrap_or(p)).sum::<Vec3>() / ns.len() as f32;
                    targets.push((*v, p.lerp(avg, (strength * w).clamp(0.0, 1.0))));
                }
            }
            BrushKind::Polish => {
                for (v, w) in verts.iter().zip(&weights) {
                    let p = m.positions.get(*v as usize).copied().unwrap_or_default();
                    let flat = p - n * (p - c).dot(n) * w * strength * 0.5;
                    let ns = m.neighbors(*v);
                    let avg = if ns.is_empty() {
                        flat
                    } else {
                        ns.iter().map(|k| m.positions.get(*k as usize).copied().unwrap_or(p)).sum::<Vec3>() / ns.len() as f32
                    };
                    targets.push((*v, flat.lerp(avg, (0.35 * strength * w).clamp(0.0, 1.0))));
                }
            }
            BrushKind::Plateau => {
                // Height above where the stroke found each point, capped at one step.
                let height = n * step * 0.5 * sign;
                for (v, w) in verts.iter().zip(&weights) {
                    let p = m.positions.get(*v as usize).copied().unwrap_or_default();
                    let from = st.original.get(v).map(|o| o.0).unwrap_or(p);
                    let reach = st.reach.entry(*v).or_insert(0.0);
                    if *w > *reach {
                        *reach = *w;
                    }
                    targets.push((*v, from + height * *reach));
                }
            }
            kind => {
                let delta = mirror * dab.grab_delta;
                let slide = delta - n * delta.dot(n);
                let bulge_from = center - n * radius * 0.5;
                let plane_offset = match kind {
                    crate::BrushKind::Clay => n * step * 0.25 * sign,
                    BrushKind::Buildup => n * step * 0.45 * sign,
                    // Scrape sets its plane a little into the surface at the pointer.
                    BrushKind::Scrape => (center - c).dot(n) * n - n * step * 0.08 * sign,
                    _ => Vec3::ZERO,
                };
                let plane_p = c + plane_offset;
                for (v, w) in verts.iter().zip(&weights) {
                    let p = m.positions.get(*v as usize).copied().unwrap_or_default();
                    let q = match kind {
                        crate::BrushKind::Draw => p + n * step * 0.1 * w * sign,
                        crate::BrushKind::Inflate => p + m.normals.get(*v as usize).copied().unwrap_or(n) * step * 0.08 * w * sign,
                        crate::BrushKind::Clay => {
                            let d = (p - plane_p).dot(n) * sign;
                            if d < 0.0 { p - n * d * sign * w * 0.5 } else { p }
                        }
                        crate::BrushKind::Flatten => p - n * (p - c).dot(n) * w * strength * 0.5,
                        BrushKind::Buildup => {
                            let d = (p - plane_p).dot(n) * sign;
                            if d < 0.0 { p - n * d * sign * w * 0.9 } else { p }
                        }
                        BrushKind::Trim | BrushKind::Fill => {
                            // Trim lowers what is above the plane; Fill raises what is below; inverting swaps them.
                            let above = (kind == BrushKind::Trim) != b.invert;
                            let d = (p - c).dot(n);
                            if (d > 0.0) == above { p - n * d * w * strength } else { p }
                        }
                        BrushKind::Scrape => {
                            // Lower what is above the plane (raise what is below when inverted).
                            let d = (p - plane_p).dot(n) * sign;
                            if d > 0.0 { p - n * d * sign * w * strength } else { p }
                        }
                        BrushKind::Pull => p + delta * w * strength.max(if b.pressure_strength { 0.0 } else { 1.0 }),
                        BrushKind::Slide => p + slide * w * strength,
                        BrushKind::Gouge => {
                            let mut to_c = center - p;
                            to_c -= n * to_c.dot(n);
                            p + to_c * w * strength * 0.12 * sign - n * step * 0.14 * w * w * sign
                        }
                        BrushKind::Bulge => p + (p - bulge_from).normalize_or(n) * step * 0.1 * w * sign,
                        crate::BrushKind::Pinch | crate::BrushKind::Crease => {
                            let mut to_c = center - p;
                            to_c -= n * to_c.dot(n);
                            let mut q = p + to_c * w * strength * 0.3 * sign;
                            if kind == crate::BrushKind::Crease {
                                q -= n * step * 0.05 * w * sign;
                            }
                            q
                        }
                        _ => p,
                    };
                    targets.push((*v, q));
                }
            }
        }
        commit(m, st, &targets);
    }
}

/// How far the elastic move reaches, in brush radii (its pull fades to nothing there).
const ELASTIC_REACH: f32 = 3.0;
/// Poisson's ratio of the rubber: close to one half, which keeps the volume.
const ELASTIC_POISSON: f32 = 0.45;

/// One step of the elastic move: a force at `center` that moves the centre itself by `delta`.
/// The displacement of a point at offset `r` is the regularized Kelvinlet
/// `((a − b)/rε + a ε²/(2 rε³)) f + (b/rε³) r (r·f)` with `rε = √(|r|² + ε²)`, here with the
/// brush radius as ε and scaled so that the centre moves by exactly `delta`.
fn elastic_move(m: &mut SculptMesh, st: &mut StrokeState, center: Vec3, delta: Vec3, radius: f32, amount: f32) {
    if delta == Vec3::ZERO || !delta.is_finite() {
        return;
    }
    let mut verts = Vec::new();
    let reach = radius * ELASTIC_REACH;
    m.query_sphere(center, reach, &mut verts);
    let (ka, kb) = (1.0f32, 1.0 / (4.0 * (1.0 - ELASTIC_POISSON)));
    let eps = radius;
    let at_centre = (1.5 * ka - kb) / eps;
    let mut targets = Vec::with_capacity(verts.len());
    for v in &verts {
        let Some(p) = m.positions.get(*v as usize).copied() else { continue };
        let r = p - center;
        let re = (r.length_squared() + eps * eps).sqrt();
        let u = (((ka - kb) / re + ka * eps * eps / (2.0 * re * re * re)) * delta + (kb / (re * re * re)) * r * r.dot(delta)) / at_centre;
        // Fade to nothing at the reach, so the stroke has an edge.
        let fade = Falloff::Smooth.weight(r.length() / reach);
        let w = fade * amount * (1.0 - m.mask.get(*v as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0));
        targets.push((*v, p + u * w));
    }
    commit(m, st, &targets);
}

/// What a surface grab moves: the points reachable from the one nearest the brush centre by
/// walking along edges for at most `radius`, weighted by that walking distance.
fn surface_capture(m: &mut SculptMesh, b: &BrushSettings, center: Vec3, radius: f32) -> Vec<(u32, f32)> {
    let mut near = Vec::new();
    m.query_sphere(center, radius, &mut near);
    let start = near.iter().copied().min_by(|a, c| {
        let d = |v: &u32| m.positions.get(*v as usize).map(|p| p.distance_squared(center)).unwrap_or(f32::MAX);
        d(a).total_cmp(&d(c)).then(a.cmp(c))
    });
    let Some(start) = start else { return Vec::new() };
    let first = m.positions.get(start as usize).map(|p| p.distance(center)).unwrap_or(0.0);
    let mut dist: std::collections::HashMap<u32, f32> = std::collections::HashMap::new();
    dist.insert(start, first);
    let mut queue = std::collections::VecDeque::from([start]);
    while let Some(v) = queue.pop_front() {
        let (Some(dv), Some(pv)) = (dist.get(&v).copied(), m.positions.get(v as usize).copied()) else { continue };
        for w in m.neighbors(v) {
            let Some(pw) = m.positions.get(*w as usize) else { continue };
            let d = dv + pv.distance(*pw);
            if d <= radius && dist.get(w).is_none_or(|old| d < *old - 1e-7) {
                dist.insert(*w, d);
                queue.push_back(*w);
            }
        }
    }
    let mut out: Vec<(u32, f32)> = dist
        .into_iter()
        .filter_map(|(v, d)| {
            let w = b.weight(d / radius) * (1.0 - m.mask.get(v as usize).copied().unwrap_or(0.0));
            (w > 0.0).then_some((v, w))
        })
        .collect();
    out.sort_unstable_by_key(|e| e.0);
    out
}

fn tangent_basis(view: Vec3) -> (Vec3, Vec3) {
    let n = view.normalize_or(Vec3::Z);
    let a = if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let u = n.cross(a).normalize_or(Vec3::X);
    (u, n.cross(u))
}

fn record(st: &mut StrokeState, m: &SculptMesh, v: u32) {
    st.original.entry(v).or_insert_with(|| (m.positions.get(v as usize).copied().unwrap_or_default(), m.mask.get(v as usize).copied().unwrap_or(0.0)));
}

fn commit(m: &mut SculptMesh, st: &mut StrokeState, targets: &[(u32, Vec3)]) {
    let mut moved = Vec::with_capacity(targets.len());
    for (v, q) in targets {
        if !q.is_finite() {
            continue;
        }
        record(st, m, *v);
        if let Some(p) = m.positions.get_mut(*v as usize) {
            let d = p.distance(*q);
            if d > 0.0 {
                // Drift: how far a vertex is from where the BVH last saw it (the stroke start).
                let orig = st.original.get(v).map(|o| o.0).unwrap_or(*p);
                m.drift = m.drift.max(orig.distance(*q));
                *p = *q;
                moved.push(*v);
            }
        }
    }
    m.update_normals(&moved);
}
