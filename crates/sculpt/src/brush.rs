//! Brushes. A brush moves the vertices within its radius, weighted by a falloff curve, an
//! optional stamp (a pattern that shapes the dab) and the mask. Symmetry repeats each dab
//! mirrored across the chosen object axes.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{SculptMesh, StrokeState};

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
        }
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

/// Mirror axes, in object space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Symmetry {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

impl Symmetry {
    /// The mirror sign vectors to apply (always starts with the identity).
    pub fn passes(&self) -> Vec<Vec3> {
        let mut out = vec![Vec3::ONE];
        for (on, axis) in [(self.x, Vec3::new(-1.0, 1.0, 1.0)), (self.y, Vec3::new(1.0, -1.0, 1.0)), (self.z, Vec3::new(1.0, 1.0, -1.0))] {
            if on {
                let more: Vec<Vec3> = out.iter().map(|s| *s * axis).collect();
                out.extend(more);
            }
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
    /// Live detail: split the triangles under the brush until their edges are no longer than
    /// this (object units). None: the triangles stay as they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_detail: Option<f32>,
}

fn yes() -> bool {
    true
}

impl BrushSettings {
    pub fn new(kind: BrushKind, radius: f32) -> Self {
        let strength = match kind {
            BrushKind::Smooth => 0.5,
            BrushKind::Grab => 1.0,
            BrushKind::Mask => 0.8,
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
            live_detail: None,
        }
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
    let radius = (b.radius * if b.pressure_radius { pressure.max(0.05) } else { 1.0 }).clamp(1e-6, 1e6);
    let strength = b.strength.clamp(0.0, 1.0) * if b.pressure_strength { pressure } else { 1.0 };
    let sign = if b.invert { -1.0 } else { 1.0 };
    let first = st.dabs.is_empty();
    let mut verts = Vec::new();
    let mut targets: Vec<(u32, Vec3)> = Vec::new();
    for (pass, mirror) in b.symmetry.passes().into_iter().enumerate() {
        let center = dab.center * mirror;
        let view = dab.view_dir * mirror;
        if let Some(edge) = b.live_detail
            && b.kind != crate::BrushKind::Mask
            && (b.kind != crate::BrushKind::Grab || first)
        {
            m.refine_sphere(st, center, radius, edge);
        }
        if b.kind == crate::BrushKind::Grab {
            if first {
                m.query_sphere(center, radius, &mut verts);
                let captured = verts
                    .iter()
                    .filter_map(|v| {
                        let p = m.positions.get(*v as usize)?;
                        let w = b.falloff.weight(p.distance(center) / radius) * (1.0 - m.mask.get(*v as usize).copied().unwrap_or(0.0));
                        (w > 0.0).then_some((*v, w))
                    })
                    .collect();
                st.grabbed.push(captured);
            }
            let delta = dab.grab_delta * mirror;
            targets.clear();
            for (v, w) in st.grabbed.get(pass).map(Vec::as_slice).unwrap_or_default() {
                if let Some(p) = m.positions.get(*v as usize) {
                    targets.push((*v, *p + delta * *w * strength.max(if b.pressure_strength { 0.0 } else { 1.0 })));
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
        let mut weights = Vec::with_capacity(verts.len());
        let mut area_n = Vec3::ZERO;
        let mut area_c = Vec3::ZERO;
        let mut wsum = 0.0;
        let (tu, tv) = tangent_basis(view);
        for v in &verts {
            let p = m.positions.get(*v as usize).copied().unwrap_or_default();
            let d = p - center;
            let mut w = b.falloff.weight(d.length() / radius);
            if b.stamp != Stamp::None {
                w *= b.stamp.sample(d.dot(tu) / radius, d.dot(tv) / radius);
            }
            if b.kind != crate::BrushKind::Mask {
                w *= 1.0 - m.mask.get(*v as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0);
            }
            let n = m.normals.get(*v as usize).copied().unwrap_or(Vec3::Y);
            area_n += n * w;
            area_c += p * w;
            wsum += w;
            weights.push(w);
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
            kind => {
                let plane_offset = match kind {
                    crate::BrushKind::Clay => n * step * 0.25 * sign,
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
