//! Tessellation: curves to polylines.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::curve::{Piece, eval_piece, split_piece};
use crate::{CurveKind, MAX_DEPTH, MAX_POLYLINE_POINTS, MAX_STEPS, Segment, Spline, SplineError};

/// How many points a curve gets between its knots.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Tessellation {
    /// The same number of steps for every piece of curve between two knots.
    Fixed { steps: u32 },
    /// Steps about `length` long (in scene units), so long pieces get more points than short ones.
    Uniform { length: f32 },
    /// Subdivide until the polyline stays within `tolerance` (scene units) of the curve and no
    /// two neighbouring pieces turn by more than `angle_deg` degrees: straight stretches get few
    /// points, tight bends many.
    Adaptive { tolerance: f32, angle_deg: f32 },
}

impl Default for Tessellation {
    fn default() -> Self {
        Tessellation::Adaptive { tolerance: 0.01, angle_deg: 12.0 }
    }
}

impl Tessellation {
    /// The mode with every number made finite and put in range.
    pub fn sanitized(self) -> Tessellation {
        let fin = |v: f32, d: f32| if v.is_finite() { v } else { d };
        match self {
            Tessellation::Fixed { steps } => Tessellation::Fixed { steps: steps.clamp(1, MAX_STEPS) },
            Tessellation::Uniform { length } => Tessellation::Uniform { length: fin(length, 0.1).clamp(1e-4, 1e6) },
            Tessellation::Adaptive { tolerance, angle_deg } => {
                Tessellation::Adaptive { tolerance: fin(tolerance, 0.01).clamp(1e-6, 1e6), angle_deg: fin(angle_deg, 12.0).clamp(1.0, 90.0) }
            }
        }
    }

    /// The kinds, in menu order: `fixed`, `uniform`, `adaptive`.
    pub fn kind(&self) -> &'static str {
        match self {
            Tessellation::Fixed { .. } => "fixed",
            Tessellation::Uniform { .. } => "uniform",
            Tessellation::Adaptive { .. } => "adaptive",
        }
    }

    pub fn default_of(kind: &str) -> Option<Tessellation> {
        Some(match kind {
            "fixed" => Tessellation::Fixed { steps: 8 },
            "uniform" => Tessellation::Uniform { length: 0.1 },
            "adaptive" => Tessellation::default(),
            _ => return None,
        })
    }
}

/// The kinds of [`Tessellation`].
pub const MODES: &[&str] = &["fixed", "uniform", "adaptive"];

/// A tessellated segment. A closed polyline does not repeat its first point at the end.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Polyline {
    pub points: Vec<Vec3>,
    pub closed: bool,
}

impl Polyline {
    /// The number of edges (a closed polyline has as many as points).
    pub fn edge_count(&self) -> usize {
        match self.points.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }

    pub fn edge(&self, i: usize) -> Option<(Vec3, Vec3)> {
        let a = *self.points.get(i)?;
        let b = *self.points.get((i + 1) % self.points.len())?;
        Some((a, b))
    }

    /// Length along the edges.
    pub fn length(&self) -> f32 {
        (0..self.edge_count()).filter_map(|i| self.edge(i)).map(|(a, b)| a.distance(b)).sum()
    }
}

impl Spline {
    /// Every segment as a polyline. Fails (without building the points) when the result would
    /// pass [`MAX_POLYLINE_POINTS`], and on a spline that does not validate.
    pub fn tessellate(&self, mode: Tessellation) -> Result<Vec<Polyline>, SplineError> {
        self.validate()?;
        let mode = mode.sanitized();
        let mut budget = MAX_POLYLINE_POINTS;
        self.segments.iter().map(|s| s.tessellate_in(mode, &mut budget)).collect()
    }
}

impl Segment {
    /// This segment as a polyline.
    pub fn tessellate(&self, mode: Tessellation) -> Result<Polyline, SplineError> {
        let mut budget = MAX_POLYLINE_POINTS;
        self.tessellate_in(mode.sanitized(), &mut budget)
    }

    fn tessellate_in(&self, mode: Tessellation, budget: &mut usize) -> Result<Polyline, SplineError> {
        if self.knots.iter().any(|k| !k.is_finite()) {
            return Err(SplineError::Invalid("a point of the curve is not a finite number".into()));
        }
        if self.knots.len() == 1 {
            take(budget, 1)?;
            return Ok(Polyline { points: self.knots.iter().map(|k| k.pos).collect(), closed: false });
        }
        // The size is known before anything is built for the modes that do not look at the curve.
        let pieces = self.piece_count();
        if self.kind == CurveKind::Linear {
            take(budget, self.knots.len())?;
            return Ok(Polyline { points: self.knots.iter().map(|k| k.pos).collect(), closed: self.closed });
        }
        if let Tessellation::Fixed { steps } = mode {
            check_fits(*budget, pieces.saturating_mul(steps as usize))?;
        }
        let ps = self.pieces();
        let points = tessellate_pieces(&ps, self.closed, mode, budget)?;
        Ok(Polyline { points, closed: self.closed })
    }
}

fn take(budget: &mut usize, n: usize) -> Result<(), SplineError> {
    if n > *budget {
        return Err(SplineError::TooLarge);
    }
    *budget -= n;
    Ok(())
}

/// Check that `n` points fit without taking them.
fn check_fits(budget: usize, n: usize) -> Result<(), SplineError> {
    if n > budget { Err(SplineError::TooLarge) } else { Ok(()) }
}

/// Tessellate a chain of Bezier pieces that join end to start. For a `closed` chain the last
/// point (the first again) is left out. `budget` is the number of points that may still be made
/// and is reduced by the points produced.
pub fn tessellate_pieces(pieces: &[Piece], closed: bool, mode: Tessellation, budget: &mut usize) -> Result<Vec<Vec3>, SplineError> {
    let mode = mode.sanitized();
    let mut out: Vec<Vec3> = Vec::new();
    for p in pieces {
        match mode {
            Tessellation::Fixed { steps } => fixed(p, steps, &mut out, budget)?,
            Tessellation::Uniform { length } => {
                let len = piece_length(p);
                let steps = ((len / length).ceil() as u32).clamp(1, MAX_STEPS);
                fixed(p, steps, &mut out, budget)?;
            }
            Tessellation::Adaptive { tolerance, angle_deg } => adaptive(p, tolerance, angle_deg, &mut out, budget)?,
        }
    }
    if !closed && let Some(last) = pieces.last() {
        take(budget, 1)?;
        out.push(last[3]);
    }
    Ok(out)
}

/// The start of each of `steps` equal parameter steps.
fn fixed(p: &Piece, steps: u32, out: &mut Vec<Vec3>, budget: &mut usize) -> Result<(), SplineError> {
    let steps = steps.clamp(1, MAX_STEPS);
    take(budget, steps as usize)?;
    out.push(p[0]);
    for k in 1..steps {
        out.push(eval_piece(p, k as f32 / steps as f32));
    }
    Ok(())
}

fn piece_length(p: &Piece) -> f32 {
    let mut prev = p[0];
    let mut total = 0.0;
    for k in 1..=8 {
        let q = eval_piece(p, k as f32 / 8.0);
        total += prev.distance(q);
        prev = q;
    }
    total
}

fn dist_to_line(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let l2 = ab.length_squared();
    if l2 < 1e-24 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The direction a piece leaves its start, skipping coincident handle points.
fn start_dir(p: &Piece) -> Vec3 {
    [p[1] - p[0], p[2] - p[0], p[3] - p[0]].into_iter().find(|v| v.length_squared() > 1e-18).unwrap_or(Vec3::ZERO)
}

fn end_dir(p: &Piece) -> Vec3 {
    [p[3] - p[2], p[3] - p[1], p[3] - p[0]].into_iter().find(|v| v.length_squared() > 1e-18).unwrap_or(Vec3::ZERO)
}

fn is_flat(p: &Piece, tolerance: f32, angle_deg: f32) -> bool {
    // The curve stays within three quarters of the control points' distance from the chord.
    let dev = dist_to_line(p[1], p[0], p[3]).max(dist_to_line(p[2], p[0], p[3]));
    if dev * 0.75 > tolerance {
        return false;
    }
    let (a, b) = (start_dir(p), end_dir(p));
    if a == Vec3::ZERO || b == Vec3::ZERO {
        return true;
    }
    a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos().to_degrees() <= angle_deg
}

fn adaptive(p: &Piece, tolerance: f32, angle_deg: f32, out: &mut Vec<Vec3>, budget: &mut usize) -> Result<(), SplineError> {
    // An explicit stack (left piece on top), so recursion depth is not stack depth.
    let mut stack: Vec<(Piece, u32)> = vec![(*p, 0)];
    while let Some((q, depth)) = stack.pop() {
        if depth >= MAX_DEPTH || is_flat(&q, tolerance, angle_deg) {
            take(budget, 1)?;
            out.push(q[0]);
        } else {
            let (l, r) = split_piece(&q, 0.5);
            stack.push((r, depth + 1));
            stack.push((l, depth + 1));
        }
    }
    Ok(())
}
