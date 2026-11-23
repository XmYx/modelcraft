//! Parametric spline primitives: circle, rectangle, star, n-sided polygon, arc and helix. Each is
//! a few numbers that build a [`Spline`] on demand, like the mesh primitives; the scene keeps the
//! numbers and rebuilds when they change. Every number is sanitized (a NaN or a negative size
//! gives a sensible shape, never an error) and counts are capped.

use std::f32::consts::{PI, TAU};

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{CurveKind, HandleMode, Knot, Segment, Spline};

/// Most corners of a star or polygon, most knots of a helix, most knots of an arc.
pub const MAX_CORNERS: u32 = 1_024;
pub const MAX_HELIX_KNOTS: u32 = 8_192;
/// Largest size accepted.
const MAX_LEN: f32 = 1.0e6;

/// The plane a flat primitive lies in, named by the axis it faces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    /// Spans X and Y, faces +Z.
    #[default]
    Xy,
    /// Spans Y and Z, faces +X.
    Yz,
    /// Spans Z and X, faces +Y (the ground when Y is up).
    Zx,
}

impl Plane {
    pub const ALL: [Plane; 3] = [Plane::Xy, Plane::Yz, Plane::Zx];

    /// The point at `(u, v)` in the plane and `w` above it: a right-handed frame, so a curve that
    /// turns counter-clockwise in `(u, v)` turns counter-clockwise seen from the facing axis.
    pub fn point(self, u: f32, v: f32, w: f32) -> Vec3 {
        match self {
            Plane::Xy => Vec3::new(u, v, w),
            Plane::Yz => Vec3::new(w, u, v),
            Plane::Zx => Vec3::new(v, w, u),
        }
    }
}

/// How an arc is closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcClose {
    /// An open curve.
    #[default]
    Open,
    /// Closed by the straight chord between its ends.
    Chord,
    /// Closed through the centre, like a slice of a pie.
    Pie,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SplinePrimitive {
    /// An ellipse (a circle when both radii are equal), closed, four Bezier knots.
    Circle { radius_x: f32, radius_y: f32, plane: Plane },
    /// A rectangle, closed; `corner` rounds the corners (up to half the shorter side).
    Rectangle { width: f32, height: f32, corner: f32, plane: Plane },
    /// A star with `points` tips, closed, straight edges.
    Star { points: u32, inner: f32, outer: f32, plane: Plane },
    /// A regular polygon with `sides` corners on a circle of `radius`, closed.
    NSide { sides: u32, radius: f32, plane: Plane },
    /// An arc of a circle from `start` to `end` degrees.
    Arc { radius: f32, start: f32, end: f32, close: ArcClose, plane: Plane },
    /// A helix (or a spiral, when the radii differ): `turns` turns rising `height` along the
    /// plane's facing axis, with `knots_per_turn` smooth points per turn.
    Helix { radius_start: f32, radius_end: f32, height: f32, turns: f32, knots_per_turn: u32, plane: Plane },
}

/// The primitive kinds, in menu order.
pub const KINDS: &[&str] = &["circle", "rectangle", "star", "n_side", "arc", "helix"];

fn size(v: f32, default: f32) -> f32 {
    if v.is_finite() { v.abs().min(MAX_LEN) } else { default }
}

fn angle(v: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(-36_000.0, 36_000.0) } else { default }
}

impl SplinePrimitive {
    /// The default primitive of a kind; `None` for an unknown kind.
    pub fn default_of(kind: &str) -> Option<SplinePrimitive> {
        Some(match kind {
            "circle" => SplinePrimitive::Circle { radius_x: 1.0, radius_y: 1.0, plane: Plane::Xy },
            "rectangle" => SplinePrimitive::Rectangle { width: 2.0, height: 1.5, corner: 0.0, plane: Plane::Xy },
            "star" => SplinePrimitive::Star { points: 5, inner: 0.45, outer: 1.0, plane: Plane::Xy },
            "n_side" => SplinePrimitive::NSide { sides: 6, radius: 1.0, plane: Plane::Xy },
            "arc" => SplinePrimitive::Arc { radius: 1.0, start: 0.0, end: 180.0, close: ArcClose::Open, plane: Plane::Xy },
            "helix" => SplinePrimitive::Helix { radius_start: 1.0, radius_end: 1.0, height: 2.0, turns: 3.0, knots_per_turn: 12, plane: Plane::Zx },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            SplinePrimitive::Circle { .. } => "circle",
            SplinePrimitive::Rectangle { .. } => "rectangle",
            SplinePrimitive::Star { .. } => "star",
            SplinePrimitive::NSide { .. } => "n_side",
            SplinePrimitive::Arc { .. } => "arc",
            SplinePrimitive::Helix { .. } => "helix",
        }
    }

    pub fn plane(&self) -> Plane {
        match self {
            SplinePrimitive::Circle { plane, .. }
            | SplinePrimitive::Rectangle { plane, .. }
            | SplinePrimitive::Star { plane, .. }
            | SplinePrimitive::NSide { plane, .. }
            | SplinePrimitive::Arc { plane, .. }
            | SplinePrimitive::Helix { plane, .. } => *plane,
        }
    }

    /// The primitive with every number finite and in range.
    pub fn sanitized(self) -> SplinePrimitive {
        match self {
            SplinePrimitive::Circle { radius_x, radius_y, plane } => {
                SplinePrimitive::Circle { radius_x: size(radius_x, 1.0), radius_y: size(radius_y, 1.0), plane }
            }
            SplinePrimitive::Rectangle { width, height, corner, plane } => {
                let (w, h) = (size(width, 2.0), size(height, 1.5));
                SplinePrimitive::Rectangle { width: w, height: h, corner: size(corner, 0.0).min(w.min(h) * 0.5), plane }
            }
            SplinePrimitive::Star { points, inner, outer, plane } => {
                SplinePrimitive::Star { points: points.clamp(2, MAX_CORNERS), inner: size(inner, 0.45), outer: size(outer, 1.0), plane }
            }
            SplinePrimitive::NSide { sides, radius, plane } => SplinePrimitive::NSide { sides: sides.clamp(3, MAX_CORNERS), radius: size(radius, 1.0), plane },
            SplinePrimitive::Arc { radius, start, end, close, plane } => {
                SplinePrimitive::Arc { radius: size(radius, 1.0), start: angle(start, 0.0), end: angle(end, 180.0), close, plane }
            }
            SplinePrimitive::Helix { radius_start, radius_end, height, turns, knots_per_turn, plane } => SplinePrimitive::Helix {
                radius_start: size(radius_start, 1.0),
                radius_end: size(radius_end, 1.0),
                height: if height.is_finite() { height.clamp(-MAX_LEN, MAX_LEN) } else { 2.0 },
                turns: if turns.is_finite() { turns.abs().min(512.0) } else { 3.0 },
                knots_per_turn: knots_per_turn.clamp(4, 64),
                plane,
            },
        }
    }

    /// The spline this primitive stands for (finite, within the crate's limits).
    pub fn build(&self) -> Spline {
        match self.sanitized() {
            SplinePrimitive::Circle { radius_x, radius_y, plane } => Spline::single(arc_segment(radius_x, radius_y, 0.0, TAU, true, plane, None)),
            SplinePrimitive::Rectangle { width, height, corner, plane } => Spline::single(rectangle(width, height, corner, plane)),
            SplinePrimitive::Star { points, inner, outer, plane } => {
                let n = points as usize * 2;
                let knots = (0..n)
                    .map(|i| {
                        let r = if i % 2 == 0 { outer } else { inner };
                        let a = PI * 0.5 + i as f32 * PI / points as f32;
                        Knot::new(plane.point(r * a.cos(), r * a.sin(), 0.0))
                    })
                    .collect();
                Spline::single(Segment::new(CurveKind::Linear, true, knots))
            }
            SplinePrimitive::NSide { sides, radius, plane } => {
                let knots = (0..sides)
                    .map(|i| {
                        let a = PI * 0.5 + i as f32 * TAU / sides as f32;
                        Knot::new(plane.point(radius * a.cos(), radius * a.sin(), 0.0))
                    })
                    .collect();
                Spline::single(Segment::new(CurveKind::Linear, true, knots))
            }
            SplinePrimitive::Arc { radius, start, end, close, plane } => {
                let (a0, a1) = (start.to_radians(), end.to_radians());
                // An arc is the shorter way round from start to end only when asked for that; the
                // sweep keeps its sign, so end below start runs clockwise.
                let sweep = (a1 - a0).clamp(-TAU, TAU);
                let centre = (close == ArcClose::Pie).then_some(plane.point(0.0, 0.0, 0.0));
                Spline::single(arc_segment(radius, radius, a0, sweep, close != ArcClose::Open, plane, centre))
            }
            SplinePrimitive::Helix { radius_start, radius_end, height, turns, knots_per_turn, plane } => {
                let steps = ((turns * knots_per_turn as f32).ceil() as u32).clamp(1, MAX_HELIX_KNOTS - 1);
                // Exact tangents at every knot (a Hermite curve in cubic form), so the curve stays
                // on the helix to a tiny fraction of its radius with a few knots per turn.
                let dt = 1.0 / steps as f32;
                let knots = (0..=steps)
                    .map(|i| {
                        let t = i as f32 / steps as f32;
                        let a = t * turns * TAU;
                        let r = radius_start + (radius_end - radius_start) * t;
                        let (da, dr) = (turns * TAU, radius_end - radius_start);
                        let d = plane.point(dr * a.cos() - r * a.sin() * da, dr * a.sin() + r * a.cos() * da, height);
                        Knot::smooth(plane.point(r * a.cos(), r * a.sin(), height * t), d * (dt / 3.0))
                    })
                    .collect();
                Spline::single(Segment::new(CurveKind::Bezier, false, knots))
            }
        }
    }
}

/// An elliptical arc as Bezier pieces of at most a quarter turn each. With `closed` and a full
/// turn it is a closed ellipse; with a `centre` it is a pie slice; otherwise closed means the
/// chord. Interior knots are smooth (mirrored handles).
fn arc_segment(rx: f32, ry: f32, start: f32, sweep: f32, closed: bool, plane: Plane, centre: Option<Vec3>) -> Segment {
    let full = (sweep.abs() - TAU).abs() < 1e-4;
    let pieces = ((sweep.abs() / (PI * 0.5)).ceil() as usize).clamp(1, 4);
    let step = sweep / pieces as f32;
    let h = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |a: f32| plane.point(rx * a.cos(), ry * a.sin(), 0.0);
    let tangent = |a: f32| plane.point(-rx * a.sin(), ry * a.cos(), 0.0);
    let count = if full { pieces } else { pieces + 1 };
    let mut knots: Vec<Knot> = (0..count)
        .map(|i| {
            let a = start + i as f32 * step;
            Knot::smooth(at(a), tangent(a) * h)
        })
        .collect();
    if closed && !full {
        // The closing line (chord, or through the centre) is straight: no handle on the ends.
        if let Some(k) = knots.first_mut() {
            k.mode = HandleMode::Free;
            k.tan_in = Vec3::ZERO;
        }
        if let Some(k) = knots.last_mut() {
            k.mode = HandleMode::Free;
            k.tan_out = Vec3::ZERO;
        }
    }
    if let Some(c) = centre {
        knots.push(Knot::new(c));
    }
    Segment::new(CurveKind::Bezier, closed || full, knots)
}

fn rectangle(width: f32, height: f32, corner: f32, plane: Plane) -> Segment {
    let (hw, hh) = (width * 0.5, height * 0.5);
    if corner <= 1e-6 {
        let knots = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().map(|(u, v)| Knot::new(plane.point(*u, *v, 0.0))).collect();
        return Segment::new(CurveKind::Linear, true, knots);
    }
    let r = corner;
    let k = 0.552_284_8 * r;
    // Counter-clockwise from the lower edge, two knots per corner: the end of one side and the
    // start of the next, joined by a quarter circle.
    let corners = [(hw, -hh, 0.0f32), (hw, hh, 1.0), (-hw, hh, 2.0), (-hw, -hh, 3.0)];
    let mut knots = Vec::with_capacity(8);
    for (cx, cy, q) in corners {
        // Directions along the side arriving at the corner and the side leaving it.
        let (din, dout) = match q as u32 {
            0 => (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            1 => (Vec3::new(0.0, 1.0, 0.0), Vec3::new(-1.0, 0.0, 0.0)),
            2 => (Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.0)),
            _ => (Vec3::new(0.0, -1.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
        };
        let corner_pt = Vec3::new(cx, cy, 0.0);
        let a = corner_pt - din * r;
        let b = corner_pt + dout * r;
        let to = |p: Vec3| plane.point(p.x, p.y, 0.0);
        let dir = |d: Vec3| plane.point(d.x, d.y, 0.0);
        knots.push(Knot { pos: to(a), tan_in: Vec3::ZERO, tan_out: dir(din) * k, mode: HandleMode::Free });
        knots.push(Knot { pos: to(b), tan_in: -dir(dout) * k, tan_out: Vec3::ZERO, mode: HandleMode::Free });
    }
    Segment::new(CurveKind::Bezier, true, knots)
}
