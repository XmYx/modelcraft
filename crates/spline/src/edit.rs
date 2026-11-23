//! Point editing: the operations a pen tool and a point-edit mode need, as plain functions on a
//! segment. They keep the curve's rules (a mirrored handle stays the opposite of its partner) and
//! never panic on a bad index: they return an error instead.

use glam::Vec3;

use crate::curve::{Piece, split_piece};
use crate::{CurveKind, HandleMode, Knot, MAX_KNOTS, Segment, Side, SplineError};

impl Segment {
    fn knot_mut(&mut self, i: usize) -> Result<&mut Knot, SplineError> {
        self.knots.get_mut(i).ok_or(SplineError::NoPoint(i))
    }

    /// Move a knot (its handles move with it).
    pub fn move_knot(&mut self, i: usize, pos: Vec3) -> Result<(), SplineError> {
        if !pos.is_finite() {
            return Err(SplineError::Invalid("the new position is not a finite number".into()));
        }
        self.knot_mut(i)?.pos = pos;
        Ok(())
    }

    /// Change a knot's handle mode. Going from smooth (auto) to anything else keeps the handles
    /// the knot had, so the shape does not jump; going to mirrored makes the incoming handle the
    /// opposite of the outgoing one.
    pub fn set_mode(&mut self, i: usize, mode: HandleMode) -> Result<(), SplineError> {
        let handles = self.effective_handles().get(i).copied().ok_or(SplineError::NoPoint(i))?;
        let k = self.knot_mut(i)?;
        if k.mode == mode {
            return Ok(());
        }
        (k.tan_in, k.tan_out) = handles;
        k.mode = mode;
        if mode == HandleMode::Mirrored {
            k.tan_in = -k.tan_out;
        }
        Ok(())
    }

    /// Set one handle to `offset` (from the knot). The knot's mode decides what happens to the
    /// other: mirrored makes it the opposite, aligned turns it to stay in line (keeping its
    /// length), free leaves it, and smooth (auto) turns the knot aligned first.
    pub fn set_handle(&mut self, i: usize, side: Side, offset: Vec3) -> Result<(), SplineError> {
        if !offset.is_finite() {
            return Err(SplineError::Invalid("the handle is not a finite number".into()));
        }
        if self.knots.get(i).is_some_and(|k| k.mode == HandleMode::Auto) {
            self.set_mode(i, HandleMode::Aligned)?;
        }
        let k = self.knot_mut(i)?;
        let (mine, other) = match side {
            Side::In => (&mut k.tan_in, &mut k.tan_out),
            Side::Out => (&mut k.tan_out, &mut k.tan_in),
        };
        *mine = offset;
        match k.mode {
            HandleMode::Mirrored => *other = -offset,
            HandleMode::Aligned => {
                if let (Some(d), len) = (offset.try_normalize(), other.length())
                    && len > 0.0
                {
                    *other = -d * len;
                }
            }
            HandleMode::Free | HandleMode::Auto => {}
        }
        Ok(())
    }

    /// Open or close the curve.
    pub fn set_closed(&mut self, closed: bool) {
        self.closed = closed;
    }

    /// Insert a knot at curve parameter `u` (one unit per piece, see [`Segment::sample`]) without
    /// changing the shape of a polyline or Bezier curve. A B-spline gets a control point on the
    /// control polygon instead (its curve moves a little). Returns the index of the new knot.
    pub fn insert_at(&mut self, u: f32) -> Result<usize, SplineError> {
        let count = self.piece_count();
        if count == 0 || !u.is_finite() {
            return Err(SplineError::Invalid("there is no curve to add a point to".into()));
        }
        if self.knots.len() >= MAX_KNOTS {
            return Err(SplineError::TooManyKnots(self.knots.len() + 1));
        }
        let u = u.clamp(0.0, count as f32);
        let piece = (u.floor() as usize).min(count - 1);
        let t = u - piece as f32;
        let n = self.knots.len();
        let (i, j) = (piece, (piece + 1) % n);
        match self.kind {
            CurveKind::Linear | CurveKind::BSpline => {
                let (a, b) = (self.knots[i].pos, self.knots[j].pos);
                self.knots.insert(piece + 1, Knot::new(a.lerp(b, t)));
            }
            CurveKind::Bezier => {
                // Give the neighbours explicit, independent handles first, so smooth and
                // mirrored knots keep their shape when the split changes one handle.
                for k in [i, j] {
                    if matches!(self.knots[k].mode, HandleMode::Auto | HandleMode::Mirrored) {
                        self.set_mode(k, HandleMode::Aligned)?;
                    }
                }
                let h = self.effective_handles();
                let (a, b) = (self.knots[i].pos, self.knots[j].pos);
                let p: Piece = [a, a + h[i].1, b + h[j].0, b];
                let (l, r) = split_piece(&p, t);
                self.knots[i].tan_out = l[1] - l[0];
                self.knots[j].tan_in = r[2] - r[3];
                let mid = Knot { pos: l[3], tan_in: l[2] - l[3], tan_out: r[1] - r[0], mode: HandleMode::Aligned };
                self.knots.insert(piece + 1, mid);
            }
        }
        Ok(piece + 1)
    }

    /// Remove a knot (the neighbours are joined; a Bezier curve keeps their handles).
    pub fn remove_knot(&mut self, i: usize) -> Result<(), SplineError> {
        if i >= self.knots.len() {
            return Err(SplineError::NoPoint(i));
        }
        self.knots.remove(i);
        Ok(())
    }

    /// Append a knot at the end of an open curve (what a pen tool does with each click).
    pub fn push_knot(&mut self, knot: Knot) -> Result<usize, SplineError> {
        if self.knots.len() >= MAX_KNOTS {
            return Err(SplineError::TooManyKnots(self.knots.len() + 1));
        }
        if !knot.is_finite() {
            return Err(SplineError::Invalid("the new point is not a finite number".into()));
        }
        self.knots.push(knot);
        Ok(self.knots.len() - 1)
    }

    /// The parameter `u` of the point on the curve nearest to `p`, with its distance (a
    /// 24-steps-per-piece search then a refinement); `None` for a curve with no points.
    pub fn nearest(&self, p: Vec3) -> Option<(f32, f32)> {
        let pieces = self.pieces();
        if pieces.is_empty() {
            let k = self.knots.first()?;
            return Some((0.0, k.pos.distance(p)));
        }
        let at = |u: f32| crate::curve::sample_pieces(&pieces, Vec3::ZERO, u).0;
        let steps = 24 * pieces.len();
        let pts: Vec<Vec3> = (0..=steps).map(|s| at(s as f32 / 24.0)).collect();
        let dist: Vec<f32> = pts.iter().map(|q| q.distance(p)).collect();
        let (mut best_u, mut best_d) = (0.0, f32::MAX);
        for (s, d) in dist.iter().enumerate() {
            if *d < best_d {
                (best_u, best_d) = (s as f32 / 24.0, *d);
            }
        }
        // The intervals between samples that could hold something nearer than the best sample,
        // nearest first (any point of an interval is within half its length of an end).
        let mut ivs: Vec<(f32, usize)> = (0..steps)
            .map(|s| {
                let half = pts[s].distance(pts[s + 1]) * 0.5;
                ((dist[s].min(dist[s + 1]) - half).max(0.0), s)
            })
            .collect();
        ivs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (lb, s) in ivs.into_iter().take(64) {
            if lb >= best_d {
                break;
            }
            let (mut lo, mut hi) = (s as f32 / 24.0, (s + 1) as f32 / 24.0);
            for _ in 0..24 {
                let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
                if at(m1).distance(p) < at(m2).distance(p) {
                    hi = m2;
                } else {
                    lo = m1;
                }
            }
            let u = (lo + hi) * 0.5;
            let d = at(u).distance(p);
            if d < best_d {
                (best_u, best_d) = (u, d);
            }
        }
        Some((best_u, best_d))
    }
}
