//! The data model and exact evaluation.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{MAX_KNOTS, MAX_SEGMENTS, SplineError};

/// One cubic Bezier piece: start, first handle point, second handle point, end.
pub type Piece = [Vec3; 4];

/// How the knots of a segment make a curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveKind {
    /// Straight lines from knot to knot.
    Linear,
    /// Cubic Bezier: the curve passes through every knot and leaves it along its handles.
    #[default]
    Bezier,
    /// Uniform cubic B-spline: the knots are control points the curve is pulled toward. An open
    /// B-spline starts and ends on its first and last knot; handles are not used.
    BSpline,
}

/// What editing one handle of a Bezier knot does to the other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandleMode {
    /// Independent handles (a corner when they point apart).
    #[default]
    Free,
    /// The other handle keeps its length and turns to stay in a straight line with the edited one.
    Aligned,
    /// The other handle is the exact opposite.
    Mirrored,
    /// The handles are worked out from the neighbouring knots; the stored ones are not used.
    Auto,
}

/// The two handles of a knot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    In,
    Out,
}

/// A point of a segment. The handles are offsets from `pos` (so moving the knot moves them too).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Knot {
    pub pos: Vec3,
    /// The handle toward the previous knot.
    #[serde(default)]
    pub tan_in: Vec3,
    /// The handle toward the next knot.
    #[serde(default)]
    pub tan_out: Vec3,
    #[serde(default)]
    pub mode: HandleMode,
}

impl Knot {
    /// A corner knot (no handles).
    pub fn new(pos: Vec3) -> Knot {
        Knot { pos, ..Knot::default() }
    }
    /// A smooth knot with the given outgoing handle and the opposite incoming one.
    pub fn smooth(pos: Vec3, out: Vec3) -> Knot {
        Knot { pos, tan_in: -out, tan_out: out, mode: HandleMode::Mirrored }
    }
    pub fn is_finite(&self) -> bool {
        self.pos.is_finite() && self.tan_in.is_finite() && self.tan_out.is_finite()
    }
}

/// One curve: a list of knots, open or closed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    #[serde(default)]
    pub kind: CurveKind,
    #[serde(default)]
    pub closed: bool,
    pub knots: Vec<Knot>,
}

impl Segment {
    pub fn new(kind: CurveKind, closed: bool, knots: Vec<Knot>) -> Segment {
        Segment { kind, closed, knots }
    }

    /// A polyline through `points`.
    pub fn polyline(points: &[Vec3], closed: bool) -> Segment {
        Segment { kind: CurveKind::Linear, closed, knots: points.iter().map(|p| Knot::new(*p)).collect() }
    }

    /// How many Bezier pieces the curve is made of (a closed segment has one more than an open
    /// one; fewer than two knots make none).
    pub fn piece_count(&self) -> usize {
        let n = self.knots.len();
        if n < 2 {
            0
        } else if self.closed {
            n
        } else {
            n - 1
        }
    }

    /// The handles every knot has when its mode is resolved: `(in, out)` offsets. A smooth
    /// (auto) knot gets them from its neighbours, a mirrored knot's `in` is the opposite of its
    /// `out`; the others are as stored.
    pub fn effective_handles(&self) -> Vec<(Vec3, Vec3)> {
        let n = self.knots.len();
        let at = |i: usize| self.knots.get(i).map_or(Vec3::ZERO, |k| k.pos);
        self.knots
            .iter()
            .enumerate()
            .map(|(i, k)| match k.mode {
                HandleMode::Free | HandleMode::Aligned => (k.tan_in, k.tan_out),
                HandleMode::Mirrored => (-k.tan_out, k.tan_out),
                HandleMode::Auto => {
                    let prev = if i > 0 {
                        Some(at(i - 1))
                    } else if self.closed && n > 2 {
                        Some(at(n - 1))
                    } else {
                        None
                    };
                    let next = if i + 1 < n {
                        Some(at(i + 1))
                    } else if self.closed && n > 2 {
                        Some(at(0))
                    } else {
                        None
                    };
                    match (prev, next) {
                        (Some(p), Some(q)) => {
                            let d = (q - p) / 6.0;
                            (-d, d)
                        }
                        (None, Some(q)) => {
                            let d = (q - k.pos) / 3.0;
                            (-d, d)
                        }
                        (Some(p), None) => {
                            let d = (k.pos - p) / 3.0;
                            (-d, d)
                        }
                        (None, None) => (Vec3::ZERO, Vec3::ZERO),
                    }
                }
            })
            .collect()
    }

    /// The curve as cubic Bezier pieces, whatever its kind (exact for all three).
    pub fn pieces(&self) -> Vec<Piece> {
        let n = self.knots.len();
        let count = self.piece_count();
        let pos = |i: usize| self.knots.get(i % n.max(1)).map_or(Vec3::ZERO, |k| k.pos);
        let mut out = Vec::with_capacity(count);
        if count == 0 {
            return out;
        }
        match self.kind {
            CurveKind::Linear => {
                for i in 0..count {
                    let (a, b) = (pos(i), pos(i + 1));
                    out.push([a, a + (b - a) / 3.0, a + (b - a) * (2.0 / 3.0), b]);
                }
            }
            CurveKind::Bezier => {
                let h = self.effective_handles();
                for i in 0..count {
                    let j = (i + 1) % n;
                    let (a, b) = (pos(i), pos(j));
                    let out_h = h.get(i).map_or(Vec3::ZERO, |h| h.1);
                    let in_h = h.get(j).map_or(Vec3::ZERO, |h| h.0);
                    out.push([a, a + out_h, b + in_h, b]);
                }
            }
            CurveKind::BSpline => {
                // The control points, with a mirrored phantom point at each end of an open curve so
                // that it starts and ends on its first and last knot.
                let ctrl: Vec<Vec3> = if self.closed {
                    (0..n + 3).map(|i| pos((i + n - 1) % n)).collect()
                } else {
                    let first = pos(0);
                    let last = pos(n - 1);
                    let mut c = Vec::with_capacity(n + 2);
                    c.push(first * 2.0 - pos(1));
                    c.extend((0..n).map(pos));
                    c.push(last * 2.0 - pos(n - 2));
                    c
                };
                for w in ctrl.windows(4).take(count) {
                    if let [a, b, c, d] = *w {
                        out.push([(a + b * 4.0 + c) / 6.0, (b * 2.0 + c) / 3.0, (b + c * 2.0) / 3.0, (b + c * 4.0 + d) / 6.0]);
                    }
                }
            }
        }
        out
    }

    /// The position and the (unnormalised) tangent at parameter `u`, where each piece spans one
    /// unit: `0..=piece_count`. Outside that range `u` is clamped. A segment with one knot is
    /// that point; with none, the origin.
    pub fn sample(&self, u: f32) -> (Vec3, Vec3) {
        sample_pieces(&self.pieces(), self.knots.first().map_or(Vec3::ZERO, |k| k.pos), u)
    }

    /// The length of the curve, measured on a fine polyline (32 steps per piece).
    pub fn length(&self) -> f32 {
        let mut total = 0.0;
        for p in self.pieces() {
            let mut prev = p[0];
            for k in 1..=32 {
                let q = eval_piece(&p, k as f32 / 32.0);
                total += prev.distance(q);
                prev = q;
            }
        }
        total
    }
}

/// Position at `t` (0..=1) of one piece.
pub fn eval_piece(p: &Piece, t: f32) -> Vec3 {
    let s = 1.0 - t;
    p[0] * (s * s * s) + p[1] * (3.0 * s * s * t) + p[2] * (3.0 * s * t * t) + p[3] * (t * t * t)
}

/// Derivative with respect to `t` of one piece.
pub fn derivative_piece(p: &Piece, t: f32) -> Vec3 {
    let s = 1.0 - t;
    (p[1] - p[0]) * (3.0 * s * s) + (p[2] - p[1]) * (6.0 * s * t) + (p[3] - p[2]) * (3.0 * t * t)
}

/// De Casteljau split of a piece at `t`.
pub fn split_piece(p: &Piece, t: f32) -> (Piece, Piece) {
    let a = p[0].lerp(p[1], t);
    let b = p[1].lerp(p[2], t);
    let c = p[2].lerp(p[3], t);
    let d = a.lerp(b, t);
    let e = b.lerp(c, t);
    let m = d.lerp(e, t);
    ([p[0], a, d, m], [m, e, c, p[3]])
}

pub(crate) fn sample_pieces(pieces: &[Piece], fallback: Vec3, u: f32) -> (Vec3, Vec3) {
    let Some(last) = pieces.len().checked_sub(1) else { return (fallback, Vec3::ZERO) };
    let u = if u.is_finite() { u.clamp(0.0, pieces.len() as f32) } else { 0.0 };
    let i = (u.floor() as usize).min(last);
    let t = u - i as f32;
    match pieces.get(i) {
        Some(p) => (eval_piece(p, t), derivative_piece(p, t)),
        None => (fallback, Vec3::ZERO),
    }
}

/// A spline: any number of segments (curves that need not touch), as one object.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spline {
    pub segments: Vec<Segment>,
}

impl Spline {
    pub fn new(segments: Vec<Segment>) -> Spline {
        Spline { segments }
    }

    /// A spline of one segment.
    pub fn single(segment: Segment) -> Spline {
        Spline { segments: vec![segment] }
    }

    pub fn knot_count(&self) -> usize {
        self.segments.iter().map(|s| s.knots.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.knot_count() == 0
    }

    /// Check the limits and that every number is finite. Everything else in this crate may assume
    /// a validated spline, but none of it panics on one that is not.
    pub fn validate(&self) -> Result<(), SplineError> {
        if self.segments.len() > MAX_SEGMENTS {
            return Err(SplineError::TooManySegments(self.segments.len()));
        }
        let mut total = 0usize;
        for s in &self.segments {
            total = total.saturating_add(s.knots.len());
            if total > MAX_KNOTS {
                return Err(SplineError::TooManyKnots(total));
            }
        }
        for (si, s) in self.segments.iter().enumerate() {
            if let Some(ki) = s.knots.iter().position(|k| !k.is_finite()) {
                return Err(SplineError::NonFinite { segment: si, knot: ki });
            }
        }
        Ok(())
    }

    /// The box around every knot and handle point (a box the curve cannot leave); `None` for an
    /// empty spline.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.segments.iter().flat_map(|s| s.pieces().into_iter().flatten().chain(s.knots.iter().map(|k| k.pos)));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Total length of all segments.
    pub fn length(&self) -> f32 {
        self.segments.iter().map(Segment::length).sum()
    }

    /// Apply a function to every point (knots; handles are offsets, so `f` should be linear for
    /// the shape to follow; use [`Spline::transformed`] for matrices).
    pub fn transformed(&self, m: glam::Mat4) -> Spline {
        let mut out = self.clone();
        for s in &mut out.segments {
            for k in &mut s.knots {
                k.pos = m.transform_point3(k.pos);
                k.tan_in = m.transform_vector3(k.tan_in);
                k.tan_out = m.transform_vector3(k.tan_out);
            }
        }
        out
    }
}
