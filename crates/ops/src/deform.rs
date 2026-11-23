//! Deformers: functions of position that bend, twist, taper, ripple, round, bulge, squash,
//! smooth, displace or shrink-wrap a mesh. A deformer works in its own frame (`to_local` /
//! `to_parent` matrices supplied by the scene) along its Y axis, over a box: the mesh's own
//! extent, or one the deformer carries ([`Limits`]).
//!
//! The pointwise deformers (bend to squash) are functions of one point. Smoothing and
//! displacement also need the mesh's connections and normals, and shrink-wrapping a target
//! surface, which a [`Context`] supplies. Falloff fields weigh them all through
//! [`Context::weight`].

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::PolyMesh;
use crate::bvh::TriBvh;

/// Most smoothing passes one deformer makes.
pub const MAX_SMOOTH_ITERATIONS: u32 = 100;
/// Most noise octaves a displacement adds up.
pub const MAX_OCTAVES: u32 = 6;

/// How far the deformation reaches along the deformer's box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitMode {
    /// Full effect inside the box, and the end value held beyond it: a bend carries on straight,
    /// a twist keeps its last angle. The way the deformers always worked.
    #[default]
    Limited,
    /// The effect carries on past the box in the same way: a twist keeps turning.
    Unlimited,
    /// Only points inside the box are touched; the others stay put.
    WithinBox,
}

pub const LIMIT_MODES: &[&str] = &["limited", "unlimited", "within_box"];

impl LimitMode {
    pub fn name(self) -> &'static str {
        match self {
            LimitMode::Limited => "limited",
            LimitMode::Unlimited => "unlimited",
            LimitMode::WithinBox => "within_box",
        }
    }
    pub fn from_name(n: &str) -> Option<LimitMode> {
        [LimitMode::Limited, LimitMode::Unlimited, LimitMode::WithinBox].into_iter().find(|m| m.name() == n)
    }
}

/// The box a deformer works over and how it treats what lies outside.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub mode: LimitMode,
    /// Use the box that just fits the mesh (in the deformer's frame) instead of `size`.
    #[serde(default = "yes")]
    pub fit_to_parent: bool,
    /// The box's full size, centred on the deformer's origin, when it does not fit the mesh.
    #[serde(default = "two")]
    pub size: Vec3,
}

fn yes() -> bool {
    true
}
fn two() -> Vec3 {
    Vec3::splat(2.0)
}

impl Default for Limits {
    fn default() -> Self {
        Limits { mode: LimitMode::Limited, fit_to_parent: true, size: two() }
    }
}

impl Limits {
    pub fn sanitized(self) -> Limits {
        let s = |v: f32| if v.is_finite() { v.abs().clamp(1e-4, 100_000.0) } else { 2.0 };
        Limits { size: Vec3::new(s(self.size.x), s(self.size.y), s(self.size.z)), ..self }
    }
}

/// What a displacement reads its height from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplaceSource {
    /// Seeded fractal noise.
    #[default]
    Noise,
    /// The falloff fields inside the deformer: their weight is the height, so a field shapes a
    /// bump rather than limiting where the deformer acts.
    Field,
}

/// How a shrink-wrap finds its place on the target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapMode {
    /// The nearest point of the target surface.
    #[default]
    Closest,
    /// Along the deformer's Y axis, to the nearest surface either way.
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Deformer {
    /// Bend around Z by `angle` degrees over the height.
    Bend { angle: f32 },
    /// Rotate around Y by up to `angle` degrees at the top.
    Twist { angle: f32 },
    /// Scale X and Z by `1 + amount` at the top.
    Taper { amount: f32 },
    /// Sine ripple along Y: `amplitude` in scene units, `frequency` in waves per unit.
    Ripple { amplitude: f32, frequency: f32 },
    /// Pull every point towards the mean radius (`amount` 0..1).
    Round { amount: f32 },
    /// Slide points sideways in proportion to their height above the bottom: `x` and `z` are how
    /// far the top moves per unit of height (the bottom stays put).
    Shear { x: f32, z: f32 },
    /// Swell the middle: points move away from the Y axis by up to `amount` of their distance,
    /// most at mid-height, not at all at either end.
    Bulge { amount: f32 },
    /// Squash (`amount` below 0) or stretch (above) along Y about the box centre, keeping the
    /// volume: the other two axes scale by `1 / sqrt(1 + amount)`.
    Squash { amount: f32 },
    /// Move every point towards the average of its neighbours, `iterations` times, by `strength`.
    Smooth { iterations: u32, strength: f32 },
    /// Smooth only what the deformers before this one did: the offset of each point from where it
    /// started is smoothed, so the mesh's own fine detail stays.
    DeltaSmooth { iterations: u32, strength: f32 },
    /// Push points along their normals by noise (or by the fields' weight) of up to `amplitude`.
    Displace { amplitude: f32, scale: f32, octaves: u32, seed: u32, source: DisplaceSource },
    /// Pull points onto the surface of the target object, keeping `offset` away from it.
    ShrinkWrap { mode: WrapMode, offset: f32 },
}

pub const KINDS: &[&str] = &["bend", "twist", "taper", "ripple", "round", "shear", "bulge", "squash", "smooth", "delta_smooth", "displace", "shrink_wrap"];

impl Deformer {
    pub fn default_of(kind: &str) -> Option<Deformer> {
        Some(match kind {
            "bend" => Deformer::Bend { angle: 90.0 },
            "twist" => Deformer::Twist { angle: 90.0 },
            "taper" => Deformer::Taper { amount: -0.5 },
            "ripple" => Deformer::Ripple { amplitude: 0.1, frequency: 2.0 },
            "round" => Deformer::Round { amount: 1.0 },
            "shear" => Deformer::Shear { x: 0.5, z: 0.0 },
            "bulge" => Deformer::Bulge { amount: 0.5 },
            "squash" => Deformer::Squash { amount: 0.5 },
            "smooth" => Deformer::Smooth { iterations: 5, strength: 0.5 },
            "delta_smooth" => Deformer::DeltaSmooth { iterations: 5, strength: 0.5 },
            "displace" => Deformer::Displace { amplitude: 0.1, scale: 2.0, octaves: 3, seed: 1, source: DisplaceSource::Noise },
            "shrink_wrap" => Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Deformer::Bend { .. } => "bend",
            Deformer::Twist { .. } => "twist",
            Deformer::Taper { .. } => "taper",
            Deformer::Ripple { .. } => "ripple",
            Deformer::Round { .. } => "round",
            Deformer::Shear { .. } => "shear",
            Deformer::Bulge { .. } => "bulge",
            Deformer::Squash { .. } => "squash",
            Deformer::Smooth { .. } => "smooth",
            Deformer::DeltaSmooth { .. } => "delta_smooth",
            Deformer::Displace { .. } => "displace",
            Deformer::ShrinkWrap { .. } => "shrink_wrap",
        }
    }

    /// Whether the deformer needs a target object to do anything.
    pub fn needs_target(&self) -> bool {
        matches!(self, Deformer::ShrinkWrap { .. })
    }

    /// Deform `m` in place. `to_local` maps mesh space into the deformer's frame.
    pub fn apply(&self, m: &mut PolyMesh, to_local: Mat4) {
        self.apply_weighted(m, to_local, &|_| 1.0);
    }

    /// Deform `m` in place, acting at each point with the strength `weight` gives for it (0..1,
    /// asked in the deformer's frame, before the move): 0 leaves a point alone, 1 deforms it fully
    /// and values between blend. Falloff fields supply the weight.
    pub fn apply_weighted(&self, m: &mut PolyMesh, to_local: Mat4, weight: &dyn Fn(Vec3) -> f32) {
        let mut pts = m.positions().to_vec();
        let cx = Context { to_local, limits: Limits::default(), weight, topology: Some(m), rest: None, target: None };
        self.apply_points(&mut pts, &cx);
        let _ = m.set_positions(&pts);
    }

    /// Deform `pts` (positions of the mesh `cx.topology`, in mesh space) in place.
    pub fn apply_points(&self, pts: &mut [Vec3], cx: &Context) {
        if !cx.to_local.is_finite() || cx.to_local.determinant().abs() < 1e-12 || pts.is_empty() {
            return;
        }
        match *self {
            Deformer::Smooth { iterations, strength } => smooth(pts, cx, iterations, strength, false),
            Deformer::DeltaSmooth { iterations, strength } => smooth(pts, cx, iterations, strength, true),
            Deformer::Displace { amplitude, scale, octaves, seed, source } => displace(pts, cx, amplitude, scale, octaves, seed, source),
            Deformer::ShrinkWrap { mode, offset } => shrink_wrap(pts, cx, mode, offset),
            _ => self.pointwise(pts, cx),
        }
    }

    fn pointwise(&self, pts: &mut [Vec3], cx: &Context) {
        let back = cx.to_local.inverse();
        let local: Vec<Vec3> = pts.iter().map(|p| cx.to_local.transform_point3(*p)).collect();
        let limits = cx.limits.sanitized();
        let (lo, hi) = if limits.fit_to_parent {
            local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)))
        } else {
            (-limits.size * 0.5, limits.size * 0.5)
        };
        let h = (hi.y - lo.y).max(1e-6);
        let mean_r = local.iter().map(|p| p.length()).sum::<f32>() / local.len() as f32;
        let mid_y = (lo.y + hi.y) * 0.5;
        let f = |p: Vec3| -> Vec3 {
            let t_raw = (p.y - lo.y) / h;
            let t = if limits.mode == LimitMode::Unlimited { t_raw } else { t_raw.clamp(0.0, 1.0) };
            match *self {
                Deformer::Bend { angle } => {
                    let a = finite(angle).to_radians();
                    if a.abs() < 1e-5 {
                        return p;
                    }
                    let r = h / a;
                    let th = a * t;
                    let over = p.y - lo.y - h * t; // beyond the top continues straight
                    let x = r - (r - p.x) * th.cos() - over * th.sin();
                    let y = lo.y + (r - p.x) * th.sin() + over * th.cos();
                    Vec3::new(x, y, p.z)
                }
                Deformer::Twist { angle } => {
                    let th = finite(angle).to_radians() * t;
                    let (s, c) = th.sin_cos();
                    Vec3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c)
                }
                Deformer::Taper { amount } => {
                    let k = 1.0 + finite(amount) * t;
                    Vec3::new(p.x * k, p.y, p.z * k)
                }
                Deformer::Ripple { amplitude, frequency } => {
                    let w = (p.y * finite(frequency) * std::f32::consts::TAU).sin() * finite(amplitude);
                    let radial = Vec3::new(p.x, 0.0, p.z).normalize_or_zero();
                    p + radial * w
                }
                Deformer::Round { amount } => p.lerp(p.normalize_or_zero() * mean_r, finite(amount).clamp(0.0, 1.0)),
                Deformer::Shear { x, z } => {
                    let up = p.y - lo.y;
                    Vec3::new(p.x + finite(x) * up, p.y, p.z + finite(z) * up)
                }
                Deformer::Bulge { amount } => {
                    let swell = (4.0 * t * (1.0 - t)).max(0.0);
                    let k = (1.0 + finite(amount) * swell).max(0.0);
                    Vec3::new(p.x * k, p.y, p.z * k)
                }
                Deformer::Squash { amount } => {
                    let a = finite(amount).clamp(-0.95, 10.0);
                    let side = 1.0 / (1.0 + a).sqrt();
                    Vec3::new(p.x * side, mid_y + (p.y - mid_y) * (1.0 + a), p.z * side)
                }
                // The smoothing, displacement and wrapping deformers never reach here.
                _ => p,
            }
        };
        let inside = |p: Vec3| p.cmpge(lo).all() && p.cmple(hi).all();
        let moved: Vec<Vec3> = local
            .into_iter()
            .map(|p| {
                if limits.mode == LimitMode::WithinBox && !inside(p) {
                    return back.transform_point3(p);
                }
                let w = clean_weight((cx.weight)(p));
                let q = if w >= 1.0 {
                    f(p)
                } else if w <= 0.0 {
                    p
                } else {
                    p.lerp(f(p), w)
                };
                back.transform_point3(q)
            })
            .collect();
        for (p, q) in pts.iter_mut().zip(moved) {
            *p = q;
        }
    }
}

/// What a deformer needs besides the points.
pub struct Context<'a> {
    /// Maps mesh space into the deformer's frame.
    pub to_local: Mat4,
    pub limits: Limits,
    /// The strength at a point (asked in the deformer's frame): falloff fields supply it.
    pub weight: &'a dyn Fn(Vec3) -> f32,
    /// The mesh's connections and faces (its own positions are not read, `pts` are).
    pub topology: Option<&'a PolyMesh>,
    /// The points as they were before the deformers of the stack ran (delta smoothing).
    pub rest: Option<&'a [Vec3]>,
    /// The surface a shrink-wrap pulls onto, already in the deformed mesh's space.
    pub target: Option<&'a TriBvh>,
}

fn clean_weight(w: f32) -> f32 {
    if w.is_finite() { w.clamp(0.0, 1.0) } else { 1.0 }
}

/// The weight at each point of `pts`, asked in the deformer's frame, with the box masking
/// applied when the deformer only acts within its box.
fn weights(pts: &[Vec3], cx: &Context) -> Vec<f32> {
    let limits = cx.limits.sanitized();
    let local: Vec<Vec3> = pts.iter().map(|p| cx.to_local.transform_point3(*p)).collect();
    let (lo, hi) = if limits.fit_to_parent {
        local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)))
    } else {
        (-limits.size * 0.5, limits.size * 0.5)
    };
    local
        .iter()
        .map(|p| {
            if limits.mode == LimitMode::WithinBox && !(p.cmpge(lo).all() && p.cmple(hi).all()) {
                0.0
            } else {
                clean_weight((cx.weight)(*p))
            }
        })
        .collect()
}

fn neighbours(m: &PolyMesh, n: usize) -> Vec<Vec<u32>> {
    (0..n as u32).map(|v| m.vertex_neighbors(v).collect()).collect()
}

fn smooth(pts: &mut [Vec3], cx: &Context, iterations: u32, strength: f32, delta: bool) {
    let Some(m) = cx.topology else { return };
    if m.vertex_count() != pts.len() {
        return;
    }
    let iterations = iterations.min(MAX_SMOOTH_ITERATIONS);
    let strength = finite(strength).clamp(0.0, 1.0);
    if iterations == 0 || strength == 0.0 {
        return;
    }
    let rest = match (delta, cx.rest) {
        (true, Some(r)) if r.len() == pts.len() => Some(r),
        (true, _) => return,
        _ => None,
    };
    let w = weights(pts, cx);
    let nb = neighbours(m, pts.len());
    // Smooth the points themselves, or their offsets from the rest points.
    let mut cur: Vec<Vec3> = match rest {
        Some(r) => pts.iter().zip(r).map(|(p, q)| *p - *q).collect(),
        None => pts.to_vec(),
    };
    let mut next = cur.clone();
    for _ in 0..iterations {
        for (i, n) in nb.iter().enumerate() {
            if n.is_empty() {
                continue;
            }
            let avg = n.iter().filter_map(|j| cur.get(*j as usize)).copied().sum::<Vec3>() / n.len() as f32;
            next[i] = cur[i] + (avg - cur[i]) * (strength * w[i]);
        }
        std::mem::swap(&mut cur, &mut next);
    }
    for (i, p) in pts.iter_mut().enumerate() {
        *p = match rest {
            Some(r) => r[i] + cur[i],
            None => cur[i],
        };
    }
}

/// 3D value noise in 0..1, seeded.
fn noise3(p: Vec3, seed: u32) -> f32 {
    fn lattice(x: i64, y: i64, z: i64, seed: u32) -> f32 {
        let mut h = (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9) ^ u64::from(seed).wrapping_mul(0x27D4_EB2F_1656_67C5);
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        (h >> 40) as f32 / (1u64 << 24) as f32
    }
    let f = p.floor();
    let t = p - f;
    let s = t * t * (Vec3::splat(3.0) - 2.0 * t);
    let (x, y, z) = (f.x as i64, f.y as i64, f.z as i64);
    let c = |dx: i64, dy: i64, dz: i64| lattice(x.wrapping_add(dx), y.wrapping_add(dy), z.wrapping_add(dz), seed);
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), s.x);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), s.x);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), s.x);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), s.x);
    lerp(lerp(x00, x10, s.y), lerp(x01, x11, s.y), s.z)
}

fn fbm(p: Vec3, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves.clamp(1, MAX_OCTAVES) {
        sum += noise3(q, seed.wrapping_add(o.wrapping_mul(0x9E37_79B9))) * amp;
        norm += amp;
        amp *= 0.5;
        q *= 2.0;
    }
    (sum / norm).clamp(0.0, 1.0)
}

fn displace(pts: &mut [Vec3], cx: &Context, amplitude: f32, scale: f32, octaves: u32, seed: u32, source: DisplaceSource) {
    let amplitude = finite(amplitude).clamp(-100_000.0, 100_000.0);
    let scale = if scale.is_finite() { scale.clamp(-100_000.0, 100_000.0) } else { 1.0 };
    // Normals of the points as they are now.
    let normals: Vec<Vec3> = match cx.topology {
        Some(m) if m.vertex_count() == pts.len() => {
            let mut tmp = m.clone();
            if tmp.set_positions(pts).is_ok() { tmp.vertex_normals() } else { radial(pts) }
        }
        _ => radial(pts),
    };
    let limits = cx.limits.sanitized();
    let local: Vec<Vec3> = pts.iter().map(|p| cx.to_local.transform_point3(*p)).collect();
    let (lo, hi) = if limits.fit_to_parent {
        local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)))
    } else {
        (-limits.size * 0.5, limits.size * 0.5)
    };
    for (i, p) in pts.iter_mut().enumerate() {
        let q = local[i];
        if limits.mode == LimitMode::WithinBox && !(q.cmpge(lo).all() && q.cmple(hi).all()) {
            continue;
        }
        let w = clean_weight((cx.weight)(q));
        let (height, mask) = match source {
            DisplaceSource::Noise => ((fbm(q * scale, octaves, seed) - 0.5) * 2.0, w),
            DisplaceSource::Field => (w, 1.0),
        };
        *p += normals[i] * (height * amplitude * mask);
    }
}

/// Directions away from the centre, for a mesh whose faces are not known.
fn radial(pts: &[Vec3]) -> Vec<Vec3> {
    let c = pts.iter().copied().sum::<Vec3>() / pts.len().max(1) as f32;
    pts.iter().map(|p| (*p - c).normalize_or(Vec3::Y)).collect()
}

fn shrink_wrap(pts: &mut [Vec3], cx: &Context, mode: WrapMode, offset: f32) {
    let Some(target) = cx.target else { return };
    if target.is_empty() {
        return;
    }
    let offset = finite(offset).clamp(-100_000.0, 100_000.0);
    let w = weights(pts, cx);
    // The deformer's Y axis seen from the mesh.
    let up = (cx.to_local.inverse().transform_vector3(Vec3::Y)).normalize_or(Vec3::Y);
    for (i, p) in pts.iter_mut().enumerate() {
        if w[i] <= 0.0 {
            continue;
        }
        let hit = match mode {
            WrapMode::Closest => target.closest(*p).map(|h| (h.point, h.tri)),
            WrapMode::Project => {
                let far = 1e7;
                let a = target.ray(*p, up, far);
                let b = target.ray(*p, -up, far);
                let t = match (a, b) {
                    (Some(a), Some(b)) => Some(if a.0 <= b.0 { (a.0, a.1, 1.0) } else { (b.0, b.1, -1.0) }),
                    (Some(a), None) => Some((a.0, a.1, 1.0)),
                    (None, Some(b)) => Some((b.0, b.1, -1.0)),
                    (None, None) => None,
                };
                t.map(|(t, tri, sign)| (*p + up * (sign * t), tri))
            }
        };
        let Some((point, tri)) = hit else { continue };
        let n = target.normal(tri);
        let goal = point + n * offset;
        *p = p.lerp(goal, w[i]);
    }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1000.0, 1000.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;

    #[test]
    fn deformers_move_points_and_keep_topology() {
        let base = Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 12, rings: 8, caps: true }.build().unwrap();
        for k in KINDS {
            let mut m = base.clone();
            Deformer::default_of(k).unwrap().apply(&mut m, Mat4::IDENTITY);
            m.validate().unwrap();
            assert_eq!(m.face_count(), base.face_count());
            // Delta smoothing and shrink-wrapping need more than the mesh: see their own tests.
            if !matches!(*k, "delta_smooth" | "shrink_wrap") {
                assert!(m.positions() != base.positions(), "{k} did nothing");
            }
        }
    }

    #[test]
    fn weights_scale_how_far_each_point_moves() {
        let base = Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 12, rings: 8, caps: true }.build().unwrap();
        let d = Deformer::Twist { angle: 90.0 };
        let mut full = base.clone();
        d.apply(&mut full, Mat4::IDENTITY);
        let mut none = base.clone();
        d.apply_weighted(&mut none, Mat4::IDENTITY, &|_| 0.0);
        assert!(none == base, "weight 0 leaves the mesh alone");
        let mut one = base.clone();
        d.apply_weighted(&mut one, Mat4::IDENTITY, &|_| 1.0);
        assert_eq!(one, full);
        let mut half = base.clone();
        d.apply_weighted(&mut half, Mat4::IDENTITY, &|_| 0.5);
        for i in 0..base.vertex_count() as u32 {
            let mid = base.position(i).lerp(full.position(i), 0.5);
            assert!((half.position(i) - mid).length() < 1e-5);
        }
        // A weight that depends on position: only the upper half moves; bad weights count as 1.
        let mut top = base.clone();
        d.apply_weighted(&mut top, Mat4::IDENTITY, &|p| if p.y > 0.0 { 1.0 } else { 0.0 });
        for i in 0..base.vertex_count() as u32 {
            if base.position(i).y <= 0.0 {
                assert_eq!(top.position(i), base.position(i));
            }
        }
        let mut nan = base.clone();
        d.apply_weighted(&mut nan, Mat4::IDENTITY, &|_| f32::NAN);
        assert_eq!(nan, full);
        nan.validate().unwrap();
    }

    #[test]
    fn singular_frames_are_ignored() {
        let base = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        let mut m = base.clone();
        Deformer::Twist { angle: 45.0 }.apply(&mut m, Mat4::ZERO);
        assert_eq!(m, base);
    }

    use crate::bvh::TriBvh;
    use proptest::prelude::*;

    fn cylinder() -> PolyMesh {
        Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 12, rings: 8, caps: true }.build().unwrap()
    }

    fn run(d: Deformer, m: &PolyMesh, limits: Limits) -> Vec<Vec3> {
        let mut pts = m.positions().to_vec();
        let one = |_: Vec3| 1.0;
        d.apply_points(&mut pts, &Context { to_local: Mat4::IDENTITY, limits, weight: &one, topology: Some(m), rest: None, target: None });
        pts
    }

    #[test]
    fn bulge_swells_the_middle_by_its_formula_and_leaves_the_ends() {
        let m = cylinder();
        let out = run(Deformer::Bulge { amount: 0.5 }, &m, Limits::default());
        for (p, q) in m.positions().iter().zip(&out) {
            let t = (p.y + 2.0) / 4.0;
            let k = 1.0 + 0.5 * 4.0 * t * (1.0 - t);
            assert!((q.x - p.x * k).abs() < 1e-5 && (q.z - p.z * k).abs() < 1e-5 && q.y == p.y, "{p} -> {q}");
        }
        let mid = m.positions().iter().position(|p| p.y.abs() < 1e-6).unwrap();
        assert!((out[mid].x.hypot(out[mid].z) - 0.75).abs() < 1e-4, "half as much again at mid height");
    }

    #[test]
    fn squash_and_stretch_keep_the_volume() {
        let m = cylinder();
        for a in [-0.5f32, 0.5, 2.0] {
            let out = run(Deformer::Squash { amount: a }, &m, Limits::default());
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in &out {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let (h, w) = (hi.y - lo.y, hi.x - lo.x);
            assert!((h - 4.0 * (1.0 + a)).abs() < 1e-4, "height {h}");
            assert!((h * w * w - 4.0 * 1.0 * 1.0).abs() < 1e-3, "volume of the bounding box stays: {}", h * w * w);
        }
        // The centre of the box stays where it was.
        let out = run(Deformer::Squash { amount: 1.0 }, &m, Limits::default());
        let mean_y: f32 = out.iter().map(|p| p.y).sum::<f32>() / out.len() as f32;
        assert!(mean_y.abs() < 1e-5);
    }

    fn bumpy_plane() -> PolyMesh {
        let mut m = Primitive::Plane { width: 4.0, depth: 4.0, segments: 8 }.build().unwrap();
        m.map_positions(|i, p| if i % 7 == 3 { p + Vec3::Y * 0.5 } else { p });
        m
    }

    fn roughness(pts: &[Vec3]) -> f32 {
        let mean = pts.iter().map(|p| p.y).sum::<f32>() / pts.len() as f32;
        pts.iter().map(|p| (p.y - mean).powi(2)).sum::<f32>()
    }

    #[test]
    fn smoothing_flattens_bumps_more_with_more_passes_and_stays_put_without_any() {
        let m = bumpy_plane();
        let base = roughness(m.positions());
        let one = roughness(&run(Deformer::Smooth { iterations: 1, strength: 0.5 }, &m, Limits::default()));
        let ten = roughness(&run(Deformer::Smooth { iterations: 10, strength: 0.5 }, &m, Limits::default()));
        assert!(one < base && ten < one, "{base} > {one} > {ten}");
        assert!(run(Deformer::Smooth { iterations: 0, strength: 1.0 }, &m, Limits::default()) == m.positions());
        assert!(run(Deformer::Smooth { iterations: 5, strength: 0.0 }, &m, Limits::default()) == m.positions());
        // A huge pass count is capped, not looped.
        let out = run(Deformer::Smooth { iterations: u32::MAX, strength: 1.0 }, &m, Limits::default());
        assert!(out.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn delta_smoothing_smooths_the_change_and_keeps_the_rest() {
        let m = Primitive::Plane { width: 4.0, depth: 4.0, segments: 8 }.build().unwrap();
        // The stack so far lifted one point by 1.
        let rest = m.positions().to_vec();
        let mut lifted = rest.clone();
        lifted[40].y += 1.0;
        let one = |_: Vec3| 1.0;
        let cx = Context { to_local: Mat4::IDENTITY, limits: Limits::default(), weight: &one, topology: Some(&m), rest: Some(&rest), target: None };
        let mut out = lifted.clone();
        Deformer::DeltaSmooth { iterations: 3, strength: 0.5 }.apply_points(&mut out, &cx);
        assert!(out[40].y < 1.0 && out[40].y > 0.0, "the spike came down: {}", out[40].y);
        let spread: Vec<_> = m.vertex_neighbors(40).collect();
        assert!(spread.iter().all(|n| out[*n as usize].y > 0.0), "and its neighbours came up");
        // Points the deformers before did not move keep their rest place exactly (x and z too).
        assert!(out.iter().zip(&rest).all(|(o, r)| (o.x - r.x).abs() < 1e-6 && (o.z - r.z).abs() < 1e-6));
        // Without the rest points, or with the wrong number, nothing happens.
        let mut same = lifted.clone();
        Deformer::DeltaSmooth { iterations: 3, strength: 1.0 }.apply_points(&mut same, &Context { rest: None, ..cx });
        assert!(same == lifted);
        // No change before this deformer: the delta is zero, so nothing moves.
        let mut still = rest.clone();
        Deformer::DeltaSmooth { iterations: 3, strength: 1.0 }.apply_points(&mut still, &cx);
        assert!(still == rest);
    }

    #[test]
    fn displacement_moves_along_normals_by_at_most_the_amplitude_and_is_seeded() {
        let m = Primitive::Sphere { radius: 1.0, segments: 16, rings: 8 }.build().unwrap();
        let d = Deformer::Displace { amplitude: 0.2, scale: 2.0, octaves: 3, seed: 5, source: DisplaceSource::Noise };
        let out = run(d, &m, Limits::default());
        let normals = m.vertex_normals();
        let mut moved = 0;
        for ((p, q), n) in m.positions().iter().zip(&out).zip(&normals) {
            let delta = *q - *p;
            assert!(delta.length() <= 0.2 + 1e-5, "{}", delta.length());
            if delta.length() > 1e-4 {
                moved += 1;
                assert!(delta.normalize().dot(*n).abs() > 0.99, "along the normal");
            }
        }
        assert!(moved > m.vertex_count() / 2);
        assert!(run(d, &m, Limits::default()) == out, "the same seed gives the same result");
        let other = Deformer::Displace { amplitude: 0.2, scale: 2.0, octaves: 3, seed: 6, source: DisplaceSource::Noise };
        assert!(run(other, &m, Limits::default()) != out);
        // From the fields: the weight is the height, so a weight of 1 everywhere lifts every
        // point by exactly the amplitude.
        let field = Deformer::Displace { amplitude: 0.3, scale: 1.0, octaves: 1, seed: 1, source: DisplaceSource::Field };
        let lifted = run(field, &m, Limits::default());
        for ((p, q), n) in m.positions().iter().zip(&lifted).zip(&normals) {
            assert!((*q - (*p + *n * 0.3)).length() < 1e-5);
        }
    }

    #[test]
    fn shrink_wrapping_puts_points_on_the_target_closest_or_projected() {
        let target = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
        let bvh = TriBvh::from_mesh(&target, Mat4::IDENTITY);
        let ball = Primitive::Sphere { radius: 0.5, segments: 16, rings: 8 }.build().unwrap();
        let one = |_: Vec3| 1.0;
        let mut pts = ball.positions().to_vec();
        let cx = Context { to_local: Mat4::IDENTITY, limits: Limits::default(), weight: &one, topology: Some(&ball), rest: None, target: Some(&bvh) };
        Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 }.apply_points(&mut pts, &cx);
        assert!(pts.iter().all(|p| (p.abs().max_element() - 1.0).abs() < 1e-4), "every point is on the cube's surface");
        // An offset keeps them off it, outward.
        let mut off = ball.positions().to_vec();
        Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.25 }.apply_points(&mut off, &cx);
        assert!(off.iter().all(|p| (p.abs().max_element() - 1.25).abs() < 1e-4));
        // Projected along Y: the x and z of every point are kept.
        let mut proj = ball.positions().to_vec();
        Deformer::ShrinkWrap { mode: WrapMode::Project, offset: 0.0 }.apply_points(&mut proj, &cx);
        for (p, q) in ball.positions().iter().zip(&proj) {
            assert!((p.x - q.x).abs() < 1e-5 && (p.z - q.z).abs() < 1e-5 && (q.y.abs() - 1.0).abs() < 1e-4, "{p} -> {q}");
        }
        // Weight 0 leaves it; half weight goes half way; no target does nothing.
        let zero = |_: Vec3| 0.0;
        let mut still = ball.positions().to_vec();
        Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 }.apply_points(&mut still, &Context { weight: &zero, ..cx });
        assert!(still == ball.positions());
        let half = |_: Vec3| 0.5;
        let mut mid = ball.positions().to_vec();
        Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 }.apply_points(&mut mid, &Context { weight: &half, ..cx });
        assert!(mid.iter().zip(ball.positions().iter().zip(&pts)).all(|(m, (p, wrapped))| (*m - p.lerp(*wrapped, 0.5)).length() < 1e-5));
        let mut none = ball.positions().to_vec();
        Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 }.apply_points(&mut none, &Context { target: None, ..cx });
        assert!(none == ball.positions());
    }

    #[test]
    fn limit_modes_and_the_box_change_what_is_reached() {
        let m = cylinder();
        let twist = Deformer::Twist { angle: 90.0 };
        let angle_of = |p: Vec3| p.z.atan2(p.x);
        // Points above the box when it is smaller than the mesh.
        let small = Limits { mode: LimitMode::Limited, fit_to_parent: false, size: Vec3::new(2.0, 2.0, 2.0) };
        let top = m.positions().iter().position(|p| (p.y - 2.0).abs() < 1e-6 && p.x > 0.4).unwrap();
        let limited = run(twist, &m, small);
        let unlimited = run(twist, &m, Limits { mode: LimitMode::Unlimited, ..small });
        let within = run(twist, &m, Limits { mode: LimitMode::WithinBox, ..small });
        let turned = |out: &[Vec3]| (angle_of(out[top]) - angle_of(m.position(top as u32))).to_degrees().abs();
        assert!((turned(&limited) - 90.0).abs() < 1e-2, "held at the end value: {}", turned(&limited));
        assert!(turned(&unlimited) > 130.0, "keeps turning: {}", turned(&unlimited));
        assert!(turned(&within) < 1e-3, "outside the box, untouched");
        // A point inside the box moves in all three modes the same way.
        let inside = m.positions().iter().position(|p| p.y.abs() < 1e-6 && p.x > 0.4).unwrap();
        assert!((limited[inside] - unlimited[inside]).length() < 1e-5 && (limited[inside] - within[inside]).length() < 1e-5);
        assert!(limited[inside] != m.position(inside as u32));
        // Fitting to the mesh is the old behaviour.
        let old = run(twist, &m, Limits::default());
        let mut reference = m.clone();
        twist.apply(&mut reference, Mat4::IDENTITY);
        assert!(old == reference.positions());
    }

    #[test]
    fn limits_serialise_with_defaults_for_old_files() {
        let l: Limits = serde_json::from_str("{}").unwrap();
        assert_eq!(l, Limits::default());
        let json = serde_json::to_string(&Limits { mode: LimitMode::WithinBox, fit_to_parent: false, size: Vec3::ONE }).unwrap();
        assert!(json.contains("within_box"));
        for k in KINDS {
            let d = Deformer::default_of(k).unwrap();
            assert_eq!(serde_json::from_str::<Deformer>(&serde_json::to_string(&d).unwrap()).unwrap(), d);
            assert_eq!(d.kind(), *k);
        }
        assert!(Deformer::default_of("shrink_wrap").unwrap().needs_target());
    }

    fn any_f32() -> impl Strategy<Value = f32> {
        prop_oneof![proptest::num::f32::ANY, -10.0f32..10.0, Just(f32::NAN), Just(f32::INFINITY), Just(0.0)]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]
        /// Whatever the numbers, a deformer leaves every point finite and the topology alone.
        #[test]
        fn any_deformer_with_any_numbers_keeps_points_finite(
            kind in 0usize..12, a in any_f32(), b in any_f32(), n in any::<u32>(), seed in any::<u32>(),
            mode in 0usize..3, fit in any::<bool>(), size in any_f32(), w in any_f32(),
        ) {
            let m = Primitive::Sphere { radius: 1.0, segments: 8, rings: 6 }.build().unwrap();
            let d = match kind {
                0 => Deformer::Bend { angle: a },
                1 => Deformer::Twist { angle: a },
                2 => Deformer::Taper { amount: a },
                3 => Deformer::Ripple { amplitude: a, frequency: b },
                4 => Deformer::Round { amount: a },
                5 => Deformer::Shear { x: a, z: b },
                6 => Deformer::Bulge { amount: a },
                7 => Deformer::Squash { amount: a },
                8 => Deformer::Smooth { iterations: n % 200, strength: a },
                9 => Deformer::DeltaSmooth { iterations: n % 200, strength: a },
                10 => Deformer::Displace { amplitude: a, scale: b, octaves: n, seed, source: if n % 2 == 0 { DisplaceSource::Noise } else { DisplaceSource::Field } },
                _ => Deformer::ShrinkWrap { mode: if n % 2 == 0 { WrapMode::Closest } else { WrapMode::Project }, offset: a },
            };
            let limits = Limits { mode: [LimitMode::Limited, LimitMode::Unlimited, LimitMode::WithinBox][mode], fit_to_parent: fit, size: Vec3::splat(size) };
            let target = TriBvh::from_mesh(&Primitive::Cube { size: Vec3::splat(1.5), segments: 1 }.build().unwrap(), Mat4::IDENTITY);
            let weight = move |_: Vec3| w;
            let mut pts = m.positions().to_vec();
            let rest = pts.clone();
            d.apply_points(&mut pts, &Context { to_local: Mat4::IDENTITY, limits, weight: &weight, topology: Some(&m), rest: Some(&rest), target: Some(&target) });
            prop_assert!(pts.iter().all(|p| p.is_finite()), "{d:?}");
            let mut out = m.clone();
            prop_assert!(out.set_positions(&pts).is_ok());
            prop_assert!(out.validate().is_ok());
        }
    }
}
