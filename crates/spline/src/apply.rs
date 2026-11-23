//! Edits as data: every change a pen tool, a point editor or a command makes to a spline is a
//! [`SplineEdit`], applied by [`Spline::apply`]. An edit either happens in full or the spline is
//! left as it was (and a [`SplineError`] says why), so a bad index or a hostile number cannot leave
//! half a change behind.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::modify::Cut;
use crate::{CurveKind, HandleMode, Knot, MAX_KNOTS, MAX_SEGMENTS, Segment, Side, Spline, SplineError, Tessellation};

/// A knot of a spline: which segment, which point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct KnotRef {
    pub segment: usize,
    pub knot: usize,
}

/// A knot's new position.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnotMove {
    pub at: KnotRef,
    pub pos: Vec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum SplineEdit {
    /// Add a knot at the end of a segment; with `segment` equal to the number of segments, a new
    /// segment (of `kind`) starts with it.
    Append {
        segment: usize,
        knot: Knot,
        #[serde(default)]
        kind: CurveKind,
    },
    /// Move knots to new positions (their handles go with them).
    MoveKnots { moves: Vec<KnotMove> },
    /// Move knots by the same amount.
    Translate { knots: Vec<KnotRef>, by: Vec3 },
    /// Set one handle of a knot (the knot's handle mode decides what the other does).
    SetHandle { at: KnotRef, side: Side, offset: Vec3 },
    /// Change a knot's handle mode.
    SetMode { at: KnotRef, mode: HandleMode },
    /// Change how a segment's knots make a curve.
    SetKind { segment: usize, kind: CurveKind },
    /// Open or close a segment.
    SetClosed { segment: usize, closed: bool },
    /// Insert a knot at parameter `u` of a segment (one unit per piece), keeping the shape of a
    /// polyline or Bezier curve.
    Insert { segment: usize, u: f32 },
    /// Remove knots. A segment left with no knots goes too.
    Remove { knots: Vec<KnotRef> },
    /// Add a whole segment.
    AddSegment { segment: Segment },
    /// Remove a whole segment.
    RemoveSegment { segment: usize },
    /// Run a segment the other way.
    Reverse { segment: usize },
    /// Cut the corners at these points with a straight edge `distance` along each side.
    Chamfer { knots: Vec<KnotRef>, distance: f32 },
    /// Round the corners at these points with an arc of `radius`.
    Fillet { knots: Vec<KnotRef>, radius: f32 },
    /// Turn every segment into a polyline of the points the tessellation gives.
    ToPolygon { tess: Tessellation },
}

impl Spline {
    fn seg_mut(&mut self, i: usize) -> Result<&mut Segment, SplineError> {
        let n = self.segments.len();
        self.segments.get_mut(i).ok_or_else(|| SplineError::Invalid(format!("there is no segment {i} (the spline has {n})")))
    }

    /// Apply one edit. On an error nothing has changed.
    pub fn apply(&mut self, edit: &SplineEdit) -> Result<(), SplineError> {
        let mut work = self.clone();
        work.apply_in_place(edit)?;
        work.validate()?;
        *self = work;
        Ok(())
    }

    fn apply_in_place(&mut self, edit: &SplineEdit) -> Result<(), SplineError> {
        match edit {
            SplineEdit::Append { segment, knot, kind } => {
                if self.knot_count() >= MAX_KNOTS {
                    return Err(SplineError::TooManyKnots(self.knot_count() + 1));
                }
                if *segment == self.segments.len() {
                    if self.segments.len() >= MAX_SEGMENTS {
                        return Err(SplineError::TooManySegments(self.segments.len() + 1));
                    }
                    self.segments.push(Segment::new(*kind, false, Vec::new()));
                }
                self.seg_mut(*segment)?.push_knot(*knot)?;
            }
            SplineEdit::MoveKnots { moves } => {
                for m in moves {
                    self.seg_mut(m.at.segment)?.move_knot(m.at.knot, m.pos)?;
                }
            }
            SplineEdit::Translate { knots, by } => {
                if !by.is_finite() {
                    return Err(SplineError::Invalid("the move is not a finite number".into()));
                }
                for k in knots {
                    let s = self.seg_mut(k.segment)?;
                    let p = s.knots.get(k.knot).ok_or(SplineError::NoPoint(k.knot))?.pos;
                    s.move_knot(k.knot, p + *by)?;
                }
            }
            SplineEdit::SetHandle { at, side, offset } => self.seg_mut(at.segment)?.set_handle(at.knot, *side, *offset)?,
            SplineEdit::SetMode { at, mode } => self.seg_mut(at.segment)?.set_mode(at.knot, *mode)?,
            SplineEdit::SetKind { segment, kind } => {
                let s = self.seg_mut(*segment)?;
                // Handles that were smooth keep the shape when a polyline becomes a curve.
                s.kind = *kind;
            }
            SplineEdit::SetClosed { segment, closed } => {
                let s = self.seg_mut(*segment)?;
                if *closed { s.close_curve() } else { s.set_closed(false) }
            }
            SplineEdit::Insert { segment, u } => {
                if self.knot_count() >= MAX_KNOTS {
                    return Err(SplineError::TooManyKnots(self.knot_count() + 1));
                }
                self.seg_mut(*segment)?.insert_at(*u)?;
            }
            SplineEdit::Remove { knots } => {
                // From the highest index down, so that earlier removals do not move later ones.
                let mut sorted = knots.clone();
                sorted.sort_unstable();
                sorted.dedup();
                for k in sorted.iter().rev() {
                    self.seg_mut(k.segment)?.remove_knot(k.knot)?;
                }
                self.segments.retain(|s| !s.knots.is_empty());
            }
            SplineEdit::AddSegment { segment } => {
                if self.segments.len() >= MAX_SEGMENTS {
                    return Err(SplineError::TooManySegments(self.segments.len() + 1));
                }
                self.segments.push(segment.clone());
            }
            SplineEdit::Reverse { segment } => self.seg_mut(*segment)?.reverse(),
            SplineEdit::Chamfer { knots, distance } => {
                self.cut(knots, Cut::Chamfer(*distance))?;
            }
            SplineEdit::Fillet { knots, radius } => {
                self.cut(knots, Cut::Fillet(*radius))?;
            }
            SplineEdit::ToPolygon { tess } => *self = self.to_polygon(*tess)?,
            SplineEdit::RemoveSegment { segment } => {
                if *segment >= self.segments.len() {
                    return Err(SplineError::Invalid(format!("there is no segment {segment}")));
                }
                self.segments.remove(*segment);
            }
        }
        Ok(())
    }
}
