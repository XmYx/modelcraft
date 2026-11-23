//! Falloff fields: a weight between 0 and 1 for every point in space.
//!
//! A [`Field`] is a shape (sphere, box, linear gradient, noise, radial sweep, cylinder, cone,
//! capsule, torus, random cells or a formula) whose value is 1 where its effect is full and 0
//! where there is none, reshaped by a [`Remap`] curve. A [`FieldStack`] combines several fields,
//! each in its own frame, with a blend mode and an opacity. Deformers and clone modifiers read
//! the stack to decide how strongly they act at a point.
//!
//! Everything is total and bounded: parameters are sanitized (NaN and infinities become neutral
//! values), a curve has at most [`Curve::MAX_POINTS`] points, noise has at most
//! [`MAX_OCTAVES`] octaves, and every value that comes out is finite and inside `0..=1`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use glam::{Mat4, Vec3};
use modelcraft_anim::expr::Expr;
use serde::{Deserialize, Serialize};

/// Most octaves a noise field adds up.
pub const MAX_OCTAVES: u32 = 6;
/// Largest magnitude accepted for sizes and scales.
const MAX_LEN: f32 = 100_000.0;

fn finite(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v } else { fallback }
}

fn unit(v: f32) -> f32 {
    finite(v, 0.0).clamp(0.0, 1.0)
}

// ------------------------------------------------------------------ remap curve

/// A curve from 0..1 to 0..1 through control points, smooth and never overshooting between them
/// (monotone cubic interpolation), used to reshape a field's value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    /// `[x, y]` pairs with x increasing. Sanitized on use.
    pub points: Vec<[f32; 2]>,
}

/// The ready-made curves.
pub const CURVE_PRESETS: &[&str] = &["linear", "smooth", "ease_in", "ease_out", "steep"];

impl Default for Curve {
    fn default() -> Self {
        Curve::linear()
    }
}

impl Curve {
    pub const MAX_POINTS: usize = 16;

    pub fn linear() -> Curve {
        Curve { points: vec![[0.0, 0.0], [1.0, 1.0]] }
    }

    pub fn preset(name: &str) -> Option<Curve> {
        let pts: &[[f32; 2]] = match name {
            "linear" => return Some(Curve::linear()),
            "smooth" => &[[0.0, 0.0], [0.25, 0.1], [0.5, 0.5], [0.75, 0.9], [1.0, 1.0]],
            "ease_in" => &[[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]],
            "ease_out" => &[[0.0, 0.0], [0.5, 0.8], [1.0, 1.0]],
            "steep" => &[[0.0, 0.0], [0.35, 0.05], [0.65, 0.95], [1.0, 1.0]],
            _ => return None,
        };
        Some(Curve { points: pts.to_vec() })
    }

    /// The preset this curve equals, if any (for the Properties picker).
    pub fn preset_name(&self) -> Option<&'static str> {
        CURVE_PRESETS.iter().find(|n| Curve::preset(n).is_some_and(|c| c == *self)).copied()
    }

    /// Points in 0..1, x strictly increasing, at least two, at most [`Curve::MAX_POINTS`]; a curve
    /// that cannot be repaired becomes the straight line.
    pub fn sanitized(&self) -> Curve {
        let mut pts: Vec<[f32; 2]> =
            self.points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-4);
        pts.truncate(Curve::MAX_POINTS);
        if pts.len() < 2 { Curve::linear() } else { Curve { points: pts } }
    }

    /// The curve's value at `x` (clamped to 0..1); always in 0..=1. For many points, [`Curve::compile`]
    /// once and evaluate that.
    pub fn eval(&self, x: f32) -> f32 {
        self.compile().eval(x)
    }

    /// Precompute the tangents (so a million evaluations do not each rebuild them).
    pub fn compile(&self) -> CompiledCurve {
        let c = self.sanitized();
        let p = c.points;
        let n = p.len();
        // Secants and Fritsch-Carlson tangents keep the curve monotone between monotone points.
        let secant: Vec<f32> = p.windows(2).map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0])).collect();
        let mut m = vec![0.0f32; n];
        if let (Some(first), Some(last)) = (secant.first(), secant.last()) {
            m[0] = *first;
            m[n - 1] = *last;
        }
        for i in 1..n - 1 {
            let (a, b) = (secant[i - 1], secant[i]);
            m[i] = if a * b <= 0.0 { 0.0 } else { (a + b) * 0.5 };
        }
        for i in 0..n - 1 {
            let s = secant[i];
            if s == 0.0 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
            } else {
                let (a, b) = (m[i] / s, m[i + 1] / s);
                let h = a.hypot(b);
                if h > 3.0 {
                    m[i] = 3.0 * a / h * s;
                    m[i + 1] = 3.0 * b / h * s;
                }
            }
        }
        CompiledCurve { p, m }
    }
}

/// A [`Curve`] with its tangents worked out.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledCurve {
    p: Vec<[f32; 2]>,
    m: Vec<f32>,
}

impl CompiledCurve {
    pub fn eval(&self, x: f32) -> f32 {
        let (p, m) = (&self.p, &self.m);
        let n = p.len();
        let x = unit(x);
        if n < 2 || x <= p[0][0] {
            return p.first().map_or(x, |q| unit(q[1]));
        }
        let i = p.partition_point(|q| q[0] <= x).clamp(1, n - 1) - 1;
        let (x0, y0, x1, y1) = (p[i][0], p[i][1], p[i + 1][0], p[i + 1][1]);
        let h = x1 - x0;
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let (t2, t3) = (t * t, t * t * t);
        let v = (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * m[i + 1];
        unit(v)
    }
}

/// Reshapes a field's value: the curve, then optional inversion, then a strength.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Remap {
    #[serde(default)]
    pub curve: Curve,
    #[serde(default)]
    pub invert: bool,
    /// Multiplies the result (0 switches the field off, 1 is full).
    #[serde(default = "one")]
    pub strength: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for Remap {
    fn default() -> Self {
        Remap { curve: Curve::linear(), invert: false, strength: 1.0 }
    }
}

impl Remap {
    pub fn apply(&self, v: f32) -> f32 {
        self.compile().apply(v)
    }

    pub fn compile(&self) -> CompiledRemap {
        CompiledRemap { curve: self.curve.compile(), invert: self.invert, strength: unit(self.strength) }
    }
}

/// A [`Remap`] ready for many evaluations.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledRemap {
    curve: CompiledCurve,
    invert: bool,
    strength: f32,
}

impl CompiledRemap {
    pub fn apply(&self, v: f32) -> f32 {
        let c = self.curve.eval(v);
        let c = if self.invert { 1.0 - c } else { c };
        unit(c * self.strength)
    }
}

// ------------------------------------------------------------------ shapes

/// What a field measures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// 1 inside `inner` (a fraction of the radius), falling to 0 at `radius`.
    Sphere { radius: f32, inner: f32 },
    /// A box of `size` centred on the origin; 1 inside the `inner` fraction, 0 at the faces.
    Box { size: Vec3, inner: f32 },
    /// A gradient along +Y: 1 at and below the origin plane, 0 at `length` above it.
    Linear { length: f32 },
    /// Smooth value noise (0..1) at `scale` features per unit, `octaves` layers deep, seeded.
    Noise { scale: f32, octaves: u32, seed: u32 },
    /// An angular sweep around the Y axis: 1 along +X, falling to 0 at half of `angle` degrees
    /// either way (`inner` is the fraction of that half-angle held at full strength).
    Radial { angle: f32, inner: f32 },
    /// A cylinder around Y of `radius` and `height`, full inside the `inner` fraction.
    Cylinder { radius: f32, height: f32, inner: f32 },
    /// A cone around Y with its base (radius `radius`) at the bottom and its tip at the top.
    Cone { radius: f32, height: f32, inner: f32 },
    /// A capsule along Y: a segment of `length` between two ends, thickened by `radius`.
    Capsule { radius: f32, length: f32, inner: f32 },
    /// A torus around Y: a ring of `radius` thickened by `tube`.
    Torus { radius: f32, tube: f32, inner: f32 },
    /// A random value per cell of a grid of `scale` cells per unit, seeded (blocky: every point
    /// of one cell shares a value).
    Random { scale: f32, seed: u32 },
    /// A formula over `x`, `y`, `z` (the point in the field's frame) and `r` (its distance from
    /// the origin), clamped to 0..1. It uses the expression language of the animation crate.
    Formula { expr: String },
}

/// The shape names, in menu order.
pub const SHAPES: &[&str] = &["sphere", "box", "linear", "noise", "radial", "cylinder", "cone", "capsule", "torus", "random", "formula"];

/// Longest formula kept (the expression language's own limit).
pub const MAX_FORMULA: usize = modelcraft_anim::expr::MAX_SOURCE;

impl Shape {
    pub fn default_of(kind: &str) -> Option<Shape> {
        Some(match kind {
            "sphere" => Shape::Sphere { radius: 1.0, inner: 0.0 },
            "box" => Shape::Box { size: Vec3::splat(2.0), inner: 0.0 },
            "linear" => Shape::Linear { length: 2.0 },
            "noise" => Shape::Noise { scale: 1.0, octaves: 2, seed: 1 },
            "radial" => Shape::Radial { angle: 180.0, inner: 0.0 },
            "cylinder" => Shape::Cylinder { radius: 1.0, height: 2.0, inner: 0.0 },
            "cone" => Shape::Cone { radius: 1.0, height: 2.0, inner: 0.0 },
            "capsule" => Shape::Capsule { radius: 0.5, length: 2.0, inner: 0.0 },
            "torus" => Shape::Torus { radius: 1.0, tube: 0.4, inner: 0.0 },
            "random" => Shape::Random { scale: 2.0, seed: 1 },
            "formula" => Shape::Formula { expr: "1 - r / 2".into() },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Sphere { .. } => "sphere",
            Shape::Box { .. } => "box",
            Shape::Linear { .. } => "linear",
            Shape::Noise { .. } => "noise",
            Shape::Radial { .. } => "radial",
            Shape::Cylinder { .. } => "cylinder",
            Shape::Cone { .. } => "cone",
            Shape::Capsule { .. } => "capsule",
            Shape::Torus { .. } => "torus",
            Shape::Random { .. } => "random",
            Shape::Formula { .. } => "formula",
        }
    }

    pub fn sanitized(&self) -> Shape {
        let len = |v: f32| finite(v, 1.0).clamp(1e-4, MAX_LEN);
        match *self {
            Shape::Sphere { radius, inner } => Shape::Sphere { radius: len(radius), inner: unit(inner).min(0.999) },
            Shape::Box { size, inner } => {
                Shape::Box { size: Vec3::new(len(size.x.abs()), len(size.y.abs()), len(size.z.abs())), inner: unit(inner).min(0.999) }
            }
            Shape::Linear { length } => Shape::Linear { length: len(length) },
            Shape::Noise { scale, octaves, seed } => {
                Shape::Noise { scale: finite(scale, 1.0).clamp(-MAX_LEN, MAX_LEN), octaves: octaves.clamp(1, MAX_OCTAVES), seed }
            }
            Shape::Radial { angle, inner } => Shape::Radial { angle: finite(angle, 180.0).abs().clamp(1.0, 360.0), inner: unit(inner).min(0.999) },
            Shape::Cylinder { radius, height, inner } => Shape::Cylinder { radius: len(radius), height: len(height), inner: unit(inner).min(0.999) },
            Shape::Cone { radius, height, inner } => Shape::Cone { radius: len(radius), height: len(height), inner: unit(inner).min(0.999) },
            Shape::Capsule { radius, length, inner } => {
                Shape::Capsule { radius: len(radius), length: finite(length, 1.0).abs().min(MAX_LEN), inner: unit(inner).min(0.999) }
            }
            Shape::Torus { radius, tube, inner } => Shape::Torus { radius: len(radius), tube: len(tube), inner: unit(inner).min(0.999) },
            Shape::Random { scale, seed } => Shape::Random { scale: finite(scale, 1.0).clamp(-MAX_LEN, MAX_LEN), seed },
            Shape::Formula { ref expr } => Shape::Formula { expr: expr.chars().take(MAX_FORMULA).collect() },
        }
    }

    /// The raw 0..1 value at `p`, in the field's own frame. (A formula is parsed on every call
    /// here; a [`FieldStack`] parses it once.)
    pub fn value(&self, p: Vec3) -> f32 {
        let s = self.sanitized();
        let expr = match &s {
            Shape::Formula { expr } => Expr::parse(expr).ok(),
            _ => None,
        };
        s.raw(p, expr.as_ref())
    }

    /// [`Shape::value`] for a shape that is already sanitized (the per-point path of a stack);
    /// `expr` is the parsed formula of a formula shape.
    fn raw(&self, p: Vec3, expr: Option<&Expr>) -> f32 {
        let p = if p.is_finite() { p } else { Vec3::ZERO };
        // 1 at or inside `inner`, 0 at 1, linear between (`d` is a fraction of the extent).
        let ramp = |d: f32, inner: f32| unit(1.0 - (d - inner) / (1.0 - inner));
        let (flat, y) = (p.x.hypot(p.z), p.y);
        match *self {
            Shape::Sphere { radius, inner } => ramp(p.length() / radius, inner),
            Shape::Box { size, inner } => {
                let q = (p.abs() / (size * 0.5)).max_element();
                ramp(q, inner)
            }
            Shape::Linear { length } => unit(1.0 - p.y / length),
            Shape::Noise { scale, octaves, seed } => fbm(p * scale, octaves, seed),
            Shape::Radial { angle, inner } => {
                // Angle from +X, turning towards -Z (the way the primitives' rings run).
                let a = (-p.z).atan2(p.x).abs();
                ramp(a / (angle.to_radians() * 0.5), inner)
            }
            Shape::Cylinder { radius, height, inner } => ramp((flat / radius).max(y.abs() / (height * 0.5)), inner),
            Shape::Cone { radius, height, inner } => {
                // 0 at the base, 1 at the tip; the allowed radius shrinks to nothing at the tip.
                let along = (y + height * 0.5) / height;
                let allowed = (radius * (1.0 - along)).max(radius * 1e-4);
                ramp((flat / allowed).max(y.abs() / (height * 0.5)), inner)
            }
            Shape::Capsule { radius, length, inner } => {
                let beyond = (y.abs() - length * 0.5).max(0.0);
                ramp(flat.hypot(beyond) / radius, inner)
            }
            Shape::Torus { radius, tube, inner } => ramp((flat - radius).hypot(y) / tube, inner),
            Shape::Random { scale, seed } => {
                let q = (p * scale).floor();
                lattice(q.x as i64, q.y as i64, q.z as i64, seed)
            }
            Shape::Formula { .. } => expr.map_or(0.0, |e| {
                let r = p.length();
                let v = e.eval(&|name: &str| match name {
                    "x" => Some(f64::from(p.x)),
                    "y" => Some(f64::from(p.y)),
                    "z" => Some(f64::from(p.z)),
                    "r" => Some(f64::from(r)),
                    "pi" => Some(std::f64::consts::PI),
                    "tau" => Some(std::f64::consts::TAU),
                    _ => None,
                });
                unit(v.map_or(0.0, |v| v as f32))
            }),
        }
    }
}

/// A hash of a lattice point to 0..1 (SplitMix-style mixing, identical on every platform).
fn lattice(x: i64, y: i64, z: i64, seed: u32) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9)
        ^ u64::from(seed).wrapping_mul(0x27D4_EB2F_1656_67C5);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 31;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn value_noise(p: Vec3, seed: u32) -> f32 {
    let f = p.floor();
    let t = p - f;
    let s = t * t * (Vec3::splat(3.0) - 2.0 * t);
    // `as i64` saturates, so a huge coordinate cannot wrap or panic.
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
        sum += value_noise(q, seed.wrapping_add(o.wrapping_mul(0x9E37_79B9))) * amp;
        norm += amp;
        amp *= 0.5;
        q *= 2.0;
    }
    unit(sum / norm)
}

// ------------------------------------------------------------------ fields and stacks

/// How a field joins the fields before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Multiply,
    Add,
    Subtract,
    Min,
    Max,
}

/// The blend names, in menu order.
pub const BLENDS: &[&str] = &["multiply", "add", "subtract", "min", "max"];

impl Blend {
    pub fn name(self) -> &'static str {
        match self {
            Blend::Multiply => "multiply",
            Blend::Add => "add",
            Blend::Subtract => "subtract",
            Blend::Min => "min",
            Blend::Max => "max",
        }
    }
    pub fn from_name(n: &str) -> Option<Blend> {
        BLENDS.iter().position(|b| *b == n).map(|i| [Blend::Multiply, Blend::Add, Blend::Subtract, Blend::Min, Blend::Max][i])
    }
    pub fn combine(self, a: f32, b: f32) -> f32 {
        unit(match self {
            Blend::Multiply => a * b,
            Blend::Add => a + b,
            Blend::Subtract => a - b,
            Blend::Min => a.min(b),
            Blend::Max => a.max(b),
        })
    }
}

/// One falloff field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(default)]
    pub remap: Remap,
    #[serde(default)]
    pub blend: Blend,
    /// How much of the blend this field contributes: 0 leaves the fields before it as they are,
    /// 1 applies it fully.
    #[serde(default = "one")]
    pub opacity: f32,
    /// A field that is off is left out of the stack.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Field {
    pub fn default_of(kind: &str) -> Option<Field> {
        Shape::default_of(kind).map(|shape| Field { shape, remap: Remap::default(), blend: Blend::Multiply, opacity: 1.0, enabled: true })
    }

    pub fn kind(&self) -> &'static str {
        self.shape.kind()
    }

    pub fn sanitized(&self) -> Field {
        Field {
            shape: self.shape.sanitized(),
            remap: Remap { curve: self.remap.curve.sanitized(), invert: self.remap.invert, strength: unit(self.remap.strength) },
            blend: self.blend,
            opacity: finite(self.opacity, 1.0).clamp(0.0, 1.0),
            enabled: self.enabled,
        }
    }

    /// The field's weight at `p` (in the field's own frame), 0..=1.
    pub fn weight(&self, p: Vec3) -> f32 {
        self.remap.apply(self.shape.value(p))
    }
}

/// Several fields, each with the matrix that takes points into its own frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldStack {
    /// The frame is `None` for a field whose transform cannot be inverted (a scale of zero): it
    /// covers nothing, so it weighs 0 everywhere.
    layers: Vec<Layer>,
}

#[derive(Clone, Debug, PartialEq)]
struct Layer {
    frame: Option<Mat4>,
    field: Field,
    remap: CompiledRemap,
    /// The parsed formula of a formula field.
    expr: Option<Expr>,
}

/// Most fields one stack holds.
pub const MAX_LAYERS: usize = 32;

impl FieldStack {
    pub fn new() -> Self {
        FieldStack::default()
    }

    /// Add a field; `to_field` maps the stack's space into the field's frame. A matrix that is
    /// singular or not finite (the field was scaled to nothing) makes a field that covers nothing,
    /// so it weighs 0 and does not switch the others' falloff off. Fields past [`MAX_LAYERS`] are
    /// left out, and so is a field that is switched off.
    pub fn push(&mut self, to_field: Mat4, field: Field) {
        if self.layers.len() < MAX_LAYERS && field.enabled {
            let frame = (to_field.is_finite() && to_field.determinant().abs() > 1e-20).then_some(to_field);
            let field = field.sanitized();
            let remap = field.remap.compile();
            let expr = match &field.shape {
                Shape::Formula { expr } => Expr::parse(expr).ok(),
                _ => None,
            };
            self.layers.push(Layer { frame, field, remap, expr });
        }
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// The combined weight at `p`: the first field's value, then each next one blended in by its
    /// opacity. An empty stack weighs 1 everywhere (no falloff).
    pub fn weight(&self, p: Vec3) -> f32 {
        let mut acc: Option<f32> = None;
        for l in &self.layers {
            let v = l.frame.map_or(0.0, |m| l.remap.apply(l.field.shape.raw(m.transform_point3(p), l.expr.as_ref())));
            let op = l.field.opacity;
            // The first field has nothing to blend with: it fades in from "no falloff" (1).
            let from = acc.unwrap_or(1.0);
            let to = if acc.is_none() { v } else { l.field.blend.combine(from, v) };
            acc = Some(from + (to - from) * op);
        }
        acc.map_or(1.0, unit)
    }
}

#[cfg(test)]
mod tests;
