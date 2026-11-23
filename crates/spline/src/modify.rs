//! Shape-changing operations on splines: reverse, close, cut corners (chamfer and fillet) and turn
//! curves into polygons.
//!
//! Corners are cut between straight edges: a corner point whose two neighbouring pieces of curve
//! are straight lines (a polyline, or a Bezier curve whose handles at the corner and its
//! neighbours are zero). The cut distance is limited to half of the shorter edge, so cuts at
//! neighbouring corners never overlap. A point that is not such a corner is left alone and
//! reported.

use glam::Vec3;

use crate::{CurveKind, HandleMode, Knot, KnotRef, MAX_KNOTS, Segment, Spline, SplineError, Tessellation};

/// How a corner is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cut {
    /// A straight cut `d` along each edge from the corner.
    Chamfer(f32),
    /// A circular arc of radius `r` tangent to both edges.
    Fillet(f32),
}

/// The result of cutting corners: which points were cut, and which were skipped and why.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CutReport {
    pub cut: usize,
    pub skipped: Vec<(KnotRef, &'static str)>,
}

impl Segment {
    /// Reverse the direction of the curve (the same curve, run the other way). A closed curve
    /// keeps its first point first.
    pub fn reverse(&mut self) {
        // The handles each point really has (a mirrored or smooth point's stored ones may differ).
        let handles = self.effective_handles();
        for (k, (hin, hout)) in self.knots.iter_mut().zip(handles) {
            k.tan_in = hout;
            k.tan_out = hin;
        }
        if self.closed && self.knots.len() > 1 {
            self.knots[1..].reverse();
        } else {
            self.knots.reverse();
        }
    }

    /// Close the curve. When its last point lies on its first they are one point, so the last is
    /// merged into the first (keeping the handle it arrived with).
    pub fn close_curve(&mut self) {
        if !self.closed
            && self.knots.len() >= 3
            && let (Some(first), Some(last)) = (self.knots.first().copied(), self.knots.last().copied())
            && first.pos.distance_squared(last.pos) < 1e-10
        {
            let h = self.effective_handles();
            let (arriving, leaving) = (h.last().map_or(Vec3::ZERO, |h| h.0), h.first().map_or(Vec3::ZERO, |h| h.1));
            self.knots.pop();
            if let Some(f) = self.knots.first_mut() {
                // The joined point keeps the handle it arrives with and the one it leaves with.
                f.tan_in = arriving;
                f.tan_out = leaving;
                f.mode = HandleMode::Free;
            }
        }
        self.closed = true;
    }

    /// Whether the piece of curve from knot `i` to `j` (neighbours) is a straight line.
    fn straight_between(&self, i: usize, j: usize) -> bool {
        match self.kind {
            CurveKind::Linear => true,
            CurveKind::BSpline => false,
            CurveKind::Bezier => {
                let h = self.effective_handles();
                let (a, b) = (self.knots[i].pos, self.knots[j].pos);
                let on_line = |p: Vec3| (p - a).cross(b - a).length() <= 1e-5 * (b - a).length().max(1e-6);
                on_line(a + h[i].1) && on_line(b + h[j].0)
            }
        }
    }

    /// Cut the corners at `at` (indices into this segment). Returns what was cut and what was not.
    pub fn cut_corners(&mut self, at: &[usize], cut: Cut) -> Result<CutReport, SplineError> {
        let amount = match cut {
            Cut::Chamfer(v) | Cut::Fillet(v) => v,
        };
        if !amount.is_finite() || amount <= 0.0 {
            return Err(SplineError::Invalid("the size of a cut must be a positive number".into()));
        }
        let n = self.knots.len();
        let mut report = CutReport::default();
        let mut todo: Vec<usize> = at.iter().copied().filter(|i| *i < n).collect();
        todo.sort_unstable();
        todo.dedup();
        // Work out every cut from the original curve, then apply them from the end, so that
        // indices stay valid.
        let mut replacements: Vec<(usize, Vec<Knot>)> = Vec::new();
        for i in todo {
            let (prev, next) = if self.closed {
                ((i + n - 1) % n, (i + 1) % n)
            } else if i > 0 && i + 1 < n {
                (i - 1, i + 1)
            } else {
                report.skipped.push((KnotRef { segment: 0, knot: i }, "an end point has no corner"));
                continue;
            };
            if n < 3 || prev == next {
                report.skipped.push((KnotRef { segment: 0, knot: i }, "too few points"));
                continue;
            }
            if !(self.straight_between(prev, i) && self.straight_between(i, next)) {
                report.skipped.push((KnotRef { segment: 0, knot: i }, "the edges next to it are curved"));
                continue;
            }
            let (c, p, q) = (self.knots[i].pos, self.knots[prev].pos, self.knots[next].pos);
            let (e1, e2) = (p - c, q - c);
            let (l1, l2) = (e1.length(), e2.length());
            if l1 < 1e-9 || l2 < 1e-9 {
                report.skipped.push((KnotRef { segment: 0, knot: i }, "an edge has no length"));
                continue;
            }
            let (u1, u2) = (e1 / l1, e2 / l2);
            let cos = u1.dot(u2).clamp(-1.0, 1.0);
            let theta = cos.acos();
            if theta > std::f32::consts::PI - 1e-3 || theta < 1e-3 {
                report.skipped.push((KnotRef { segment: 0, knot: i }, "the edges are in line"));
                continue;
            }
            // Never take more than half of an edge: the next corner may cut the other half.
            let room = l1.min(l2) * 0.5;
            let pair = match cut {
                Cut::Chamfer(d) => {
                    let d = d.min(room);
                    vec![Knot::new(c + u1 * d), Knot::new(c + u2 * d)]
                }
                Cut::Fillet(r) => {
                    let tan_half = (theta * 0.5).tan();
                    let t = (r / tan_half).min(room);
                    let r = t * tan_half;
                    let phi = std::f32::consts::PI - theta;
                    let h = 4.0 / 3.0 * (phi * 0.25).tan() * r;
                    vec![
                        Knot { pos: c + u1 * t, tan_in: Vec3::ZERO, tan_out: -u1 * h, mode: HandleMode::Free },
                        Knot { pos: c + u2 * t, tan_in: -u2 * h, tan_out: Vec3::ZERO, mode: HandleMode::Free },
                    ]
                }
            };
            replacements.push((i, pair));
            report.cut += 1;
        }
        if report.cut == 0 {
            return Ok(report);
        }
        // Refuse before building: each cut adds one point.
        if n + report.cut > MAX_KNOTS {
            return Err(SplineError::TooManyKnots(n + report.cut));
        }
        let fillet = matches!(cut, Cut::Fillet(_));
        // Put the new points in one pass over the old ones (replacements are in index order).
        let mut out: Vec<Knot> = Vec::with_capacity(n + report.cut);
        let mut from_cut: Vec<bool> = Vec::with_capacity(n + report.cut);
        let mut next = replacements.into_iter().peekable();
        for (i, k) in self.knots.iter().enumerate() {
            if next.peek().is_some_and(|(r, _)| *r == i)
                && let Some((_, pair)) = next.next()
            {
                for (j, p) in pair.into_iter().enumerate() {
                    // A cut clamped to half an edge at both ends meets the neighbour's cut: one point.
                    if j == 0
                        && from_cut.last() == Some(&true)
                        && let Some(prev) = out.last_mut()
                        && prev.pos.distance_squared(p.pos) < 1e-12
                    {
                        prev.tan_out = p.tan_out;
                        continue;
                    }
                    out.push(p);
                    from_cut.push(true);
                }
            } else {
                // Linear points keep any handles they have stored; a fillet turns the curve into a
                // Bezier one, so it must not bring them back.
                let mut k = *k;
                if fillet && self.kind == CurveKind::Linear {
                    (k.tan_in, k.tan_out, k.mode) = (Vec3::ZERO, Vec3::ZERO, HandleMode::Free);
                }
                out.push(k);
                from_cut.push(false);
            }
        }
        // Around the start of a closed curve the last cut may meet the first.
        if self.closed
            && out.len() > 3
            && from_cut.first() == Some(&true)
            && from_cut.last() == Some(&true)
            && let (Some(first), Some(last)) = (out.first().copied(), out.last().copied())
            && first.pos.distance_squared(last.pos) < 1e-12
        {
            out.pop();
            if let Some(f) = out.first_mut() {
                f.tan_in = last.tan_in;
            }
        }
        if fillet && self.kind == CurveKind::Linear {
            self.kind = CurveKind::Bezier;
        }
        self.knots = out;
        Ok(report)
    }

    /// The segment as a polyline: its tessellated points as corners.
    pub fn to_polygon(&self, mode: Tessellation) -> Result<Segment, SplineError> {
        let line = self.tessellate(mode)?;
        Ok(Segment::polyline(&line.points, line.closed))
    }
}

impl Spline {
    /// Every segment as a polyline: the curves flattened into straight edges, with the points
    /// the tessellation mode gives.
    pub fn to_polygon(&self, mode: Tessellation) -> Result<Spline, SplineError> {
        self.validate()?;
        let lines = self.tessellate(mode)?;
        Ok(Spline { segments: lines.iter().map(|l| Segment::polyline(&l.points, l.closed)).collect() })
    }
}

impl Spline {
    /// Cut the corners at `knots` (any segments). An error when none could be cut, naming why the
    /// first was skipped; the spline is changed only when something was cut.
    pub(crate) fn cut(&mut self, knots: &[KnotRef], cut: Cut) -> Result<usize, SplineError> {
        // Each cut adds at most one point: refuse before touching anything when that passes the cap.
        if self.knot_count().saturating_add(knots.len()) > MAX_KNOTS {
            return Err(SplineError::TooManyKnots(self.knot_count().saturating_add(knots.len())));
        }
        let mut sorted = knots.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut total = 0;
        let mut why: Option<&'static str> = None;
        let mut i = 0;
        while i < sorted.len() {
            let s = sorted[i].segment;
            let end = sorted[i..].iter().position(|k| k.segment != s).map_or(sorted.len(), |p| i + p);
            let at: Vec<usize> = sorted[i..end].iter().map(|k| k.knot).collect();
            let seg = self.segments.get_mut(s).ok_or_else(|| SplineError::Invalid(format!("there is no segment {s}")))?;
            let r = seg.cut_corners(&at, cut)?;
            total += r.cut;
            why = why.or(r.skipped.first().map(|x| x.1));
            i = end;
        }
        if total == 0 {
            return Err(SplineError::Invalid(format!("there is no corner to cut here ({})", why.unwrap_or("no point chosen"))));
        }
        Ok(total)
    }
}
