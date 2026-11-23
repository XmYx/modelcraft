//! A path builder in the move / line / curve / close style, for outlines that come as drawing
//! commands (glyphs from a font, SVG-like paths).

use glam::Vec3;

use crate::{CurveKind, HandleMode, Knot, MAX_KNOTS, MAX_SEGMENTS, Segment, Spline};

/// Builds a [`Spline`] of Bezier segments from drawing commands. Each `move_to` starts a new
/// segment; `close` ends it as a closed one. Commands past the spline's limits are ignored.
#[derive(Debug, Default)]
pub struct PathBuilder {
    done: Vec<Segment>,
    current: Option<Segment>,
    knots: usize,
}

impl PathBuilder {
    pub fn new() -> PathBuilder {
        PathBuilder::default()
    }

    fn flush(&mut self) {
        if let Some(s) = self.current.take()
            && s.knots.len() > 1
            && self.done.len() < MAX_SEGMENTS
        {
            self.done.push(s);
        }
    }

    fn push(&mut self, k: Knot) {
        if self.knots >= MAX_KNOTS || !k.is_finite() {
            return;
        }
        if let Some(s) = &mut self.current {
            s.knots.push(k);
            self.knots += 1;
        }
    }

    /// Start a new open segment at `p`.
    pub fn move_to(&mut self, p: Vec3) -> &mut Self {
        self.flush();
        if self.knots < MAX_KNOTS && p.is_finite() {
            self.current = Some(Segment { kind: CurveKind::Bezier, closed: false, knots: vec![Knot::new(p)] });
            self.knots += 1;
        }
        self
    }

    /// A straight line to `p`.
    pub fn line_to(&mut self, p: Vec3) -> &mut Self {
        self.push(Knot::new(p));
        self
    }

    /// A cubic curve to `p` with control points `c1` (leaving the current point) and `c2`.
    pub fn cubic_to(&mut self, c1: Vec3, c2: Vec3, p: Vec3) -> &mut Self {
        if let Some(last) = self.current.as_mut().and_then(|s| s.knots.last_mut()) {
            last.tan_out = c1 - last.pos;
        }
        self.push(Knot { pos: p, tan_in: c2 - p, tan_out: Vec3::ZERO, mode: HandleMode::Free });
        self
    }

    /// A quadratic curve to `p` with control point `c` (raised to a cubic).
    pub fn quad_to(&mut self, c: Vec3, p: Vec3) -> &mut Self {
        let from = self.current.as_ref().and_then(|s| s.knots.last()).map(|k| k.pos).unwrap_or(p);
        self.cubic_to(from + (c - from) * (2.0 / 3.0), p + (c - p) * (2.0 / 3.0), p)
    }

    /// Close the current segment. A last point on the first one is merged into it.
    pub fn close(&mut self) -> &mut Self {
        if let Some(s) = &mut self.current {
            if s.knots.len() > 1
                && let (Some(first), Some(last)) = (s.knots.first().copied(), s.knots.last().copied())
                && first.pos.distance_squared(last.pos) < 1e-12
            {
                s.knots.pop();
                self.knots = self.knots.saturating_sub(1);
                if let Some(f) = s.knots.first_mut() {
                    f.tan_in = last.tan_in;
                }
            }
            s.closed = true;
        }
        self.flush();
        self
    }

    /// The finished spline.
    pub fn finish(&mut self) -> Spline {
        self.flush();
        Spline { segments: std::mem::take(&mut self.done) }
    }
}
