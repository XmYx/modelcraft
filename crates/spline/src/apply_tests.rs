use glam::Vec3;
use proptest::prelude::*;

use crate::apply::{KnotMove, KnotRef};
use crate::{CurveKind, HandleMode, Knot, MAX_KNOTS, MAX_SEGMENTS, Segment, Side, Spline, SplineEdit, SplineError, Tessellation};

fn k(segment: usize, knot: usize) -> KnotRef {
    KnotRef { segment, knot }
}

fn line() -> Spline {
    Spline::single(Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)], false))
}

#[test]
fn append_grows_a_segment_and_starts_a_new_one_past_the_end() {
    let mut s = Spline::default();
    s.apply(&SplineEdit::Append { segment: 0, knot: Knot::new(Vec3::ZERO), kind: CurveKind::Bezier }).unwrap();
    s.apply(&SplineEdit::Append { segment: 0, knot: Knot::new(Vec3::X), kind: CurveKind::Bezier }).unwrap();
    assert_eq!((s.segments.len(), s.segments[0].knots.len(), s.segments[0].kind), (1, 2, CurveKind::Bezier));
    s.apply(&SplineEdit::Append { segment: 1, knot: Knot::new(Vec3::Y), kind: CurveKind::BSpline }).unwrap();
    assert_eq!((s.segments.len(), s.segments[1].kind), (2, CurveKind::BSpline));
    // Past the end by more than one is an error.
    assert!(s.apply(&SplineEdit::Append { segment: 5, knot: Knot::new(Vec3::Y), kind: CurveKind::Bezier }).is_err());
    assert_eq!(s.segments.len(), 2);
}

#[test]
fn moves_handles_modes_kinds_and_closing() {
    let mut s = line();
    s.apply(&SplineEdit::MoveKnots { moves: vec![KnotMove { at: k(0, 1), pos: Vec3::new(2.0, 0.0, 0.0) }] }).unwrap();
    assert_eq!(s.segments[0].knots[1].pos, Vec3::new(2.0, 0.0, 0.0));
    s.apply(&SplineEdit::Translate { knots: vec![k(0, 0), k(0, 2)], by: Vec3::new(0.0, 0.0, 3.0) }).unwrap();
    assert_eq!((s.segments[0].knots[0].pos.z, s.segments[0].knots[1].pos.z, s.segments[0].knots[2].pos.z), (3.0, 0.0, 3.0));
    s.apply(&SplineEdit::SetKind { segment: 0, kind: CurveKind::Bezier }).unwrap();
    s.apply(&SplineEdit::SetMode { at: k(0, 1), mode: HandleMode::Mirrored }).unwrap();
    s.apply(&SplineEdit::SetHandle { at: k(0, 1), side: Side::Out, offset: Vec3::new(0.0, 1.0, 0.0) }).unwrap();
    assert_eq!(s.segments[0].knots[1].tan_in, Vec3::new(0.0, -1.0, 0.0));
    s.apply(&SplineEdit::SetClosed { segment: 0, closed: true }).unwrap();
    assert!(s.segments[0].closed);
    assert_eq!(s.segments[0].piece_count(), 3);
}

#[test]
fn insert_keeps_the_shape_and_remove_drops_empty_segments() {
    let mut s = Spline::new(vec![
        Segment::new(
            CurveKind::Bezier,
            false,
            vec![Knot::smooth(Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)), Knot::smooth(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.0))],
        ),
        Segment::polyline(&[Vec3::Z], false),
    ]);
    let before = s.tessellate(Tessellation::Fixed { steps: 64 }).unwrap();
    s.apply(&SplineEdit::Insert { segment: 0, u: 0.5 }).unwrap();
    assert_eq!(s.segments[0].knots.len(), 3);
    let after = s.tessellate(Tessellation::Fixed { steps: 64 }).unwrap();
    // The middle point sits on the old curve.
    let mid = s.segments[0].knots[1].pos;
    assert!(before[0].points.iter().any(|p| p.distance(mid) < 0.05));
    assert!(after[0].length() > 0.0);
    // Removing duplicates and reversed order are fine; the single-point segment goes with its point.
    s.apply(&SplineEdit::Remove { knots: vec![k(1, 0), k(0, 2), k(0, 2), k(0, 0)] }).unwrap();
    assert_eq!((s.segments.len(), s.segments[0].knots.len()), (1, 1));
    s.apply(&SplineEdit::Remove { knots: vec![k(0, 0)] }).unwrap();
    assert!(s.segments.is_empty());
    s.apply(&SplineEdit::AddSegment { segment: Segment::polyline(&[Vec3::ZERO, Vec3::X], false) }).unwrap();
    s.apply(&SplineEdit::RemoveSegment { segment: 0 }).unwrap();
    assert!(s.segments.is_empty());
}

#[test]
fn a_failed_edit_changes_nothing() {
    let original = line();
    let bad = [
        SplineEdit::MoveKnots { moves: vec![KnotMove { at: k(0, 0), pos: Vec3::X }, KnotMove { at: k(0, 99), pos: Vec3::X }] },
        SplineEdit::MoveKnots { moves: vec![KnotMove { at: k(0, 0), pos: Vec3::NAN }] },
        SplineEdit::Translate { knots: vec![k(0, 0)], by: Vec3::INFINITY },
        SplineEdit::Translate { knots: vec![k(0, 0), k(3, 0)], by: Vec3::X },
        SplineEdit::SetHandle { at: k(0, 9), side: Side::In, offset: Vec3::X },
        SplineEdit::SetMode { at: k(7, 0), mode: HandleMode::Free },
        SplineEdit::SetKind { segment: 4, kind: CurveKind::Linear },
        SplineEdit::SetClosed { segment: 4, closed: true },
        SplineEdit::Insert { segment: 0, u: f32::NAN },
        SplineEdit::Insert { segment: 5, u: 0.5 },
        SplineEdit::Remove { knots: vec![k(0, 1), k(0, 50)] },
        SplineEdit::RemoveSegment { segment: 3 },
        SplineEdit::AddSegment { segment: Segment::polyline(&[Vec3::NAN, Vec3::X], false) },
        SplineEdit::Append { segment: 0, knot: Knot::new(Vec3::NAN), kind: CurveKind::Bezier },
    ];
    for e in bad {
        let mut s = original.clone();
        assert!(s.apply(&e).is_err(), "{e:?}");
        assert_eq!(s, original, "{e:?} left a trace");
    }
}

#[test]
fn edits_respect_the_limits() {
    let mut full = Spline::single(Segment::polyline(&vec![Vec3::ZERO; MAX_KNOTS], false));
    let e = full.apply(&SplineEdit::Append { segment: 0, knot: Knot::new(Vec3::X), kind: CurveKind::Linear }).unwrap_err();
    assert_eq!(e, SplineError::TooManyKnots(MAX_KNOTS + 1));
    assert!(full.apply(&SplineEdit::Insert { segment: 0, u: 0.5 }).is_err());
    assert_eq!(full.knot_count(), MAX_KNOTS);
    let mut many = Spline::new(vec![Segment::polyline(&[Vec3::ZERO, Vec3::X], false); MAX_SEGMENTS]);
    assert!(many.apply(&SplineEdit::AddSegment { segment: Segment::default() }).is_err());
    assert!(many.apply(&SplineEdit::Append { segment: MAX_SEGMENTS, knot: Knot::new(Vec3::X), kind: CurveKind::Linear }).is_err());
    assert_eq!(many.segments.len(), MAX_SEGMENTS);
}

#[test]
fn edits_round_trip_as_json() {
    for e in [
        SplineEdit::Append { segment: 1, knot: Knot::smooth(Vec3::X, Vec3::Y), kind: CurveKind::BSpline },
        SplineEdit::MoveKnots { moves: vec![KnotMove { at: k(0, 1), pos: Vec3::Z }] },
        SplineEdit::SetHandle { at: k(0, 1), side: Side::Out, offset: Vec3::Y },
        SplineEdit::Remove { knots: vec![k(0, 1)] },
        SplineEdit::SetClosed { segment: 0, closed: true },
    ] {
        let j = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<SplineEdit>(&j).unwrap(), e, "{j}");
    }
    let j = r#"{"edit":"append","segment":0,"knot":{"pos":[1,2,3]}}"#;
    assert!(matches!(serde_json::from_str::<SplineEdit>(j).unwrap(), SplineEdit::Append { kind: CurveKind::Bezier, .. }));
}

fn edit_strategy() -> impl Strategy<Value = SplineEdit> {
    let v = || (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0).prop_map(|(x, y, z)| Vec3::new(x, y, z));
    let r = || (0usize..4, 0usize..6).prop_map(|(segment, knot)| KnotRef { segment, knot });
    prop_oneof![
        (0usize..4, v(), 0usize..3).prop_map(|(segment, p, kind)| SplineEdit::Append {
            segment,
            knot: Knot::new(p),
            kind: [CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline][kind]
        }),
        (r(), v()).prop_map(|(at, pos)| SplineEdit::MoveKnots { moves: vec![KnotMove { at, pos }] }),
        (prop::collection::vec(r(), 0..4), v()).prop_map(|(knots, by)| SplineEdit::Translate { knots, by }),
        (r(), any::<bool>(), v()).prop_map(|(at, s, offset)| SplineEdit::SetHandle { at, side: if s { Side::In } else { Side::Out }, offset }),
        (r(), 0usize..4)
            .prop_map(|(at, m)| SplineEdit::SetMode { at, mode: [HandleMode::Free, HandleMode::Aligned, HandleMode::Mirrored, HandleMode::Auto][m] }),
        (0usize..4, 0usize..3)
            .prop_map(|(segment, kind)| SplineEdit::SetKind { segment, kind: [CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline][kind] }),
        (0usize..4, any::<bool>()).prop_map(|(segment, closed)| SplineEdit::SetClosed { segment, closed }),
        (0usize..4, -1.0f32..8.0).prop_map(|(segment, u)| SplineEdit::Insert { segment, u }),
        prop::collection::vec(r(), 0..4).prop_map(|knots| SplineEdit::Remove { knots }),
        (0usize..4).prop_map(|segment| SplineEdit::RemoveSegment { segment }),
    ]
}

proptest! {
    #[test]
    fn any_sequence_of_edits_keeps_the_spline_valid_and_failures_leave_no_trace(edits in prop::collection::vec(edit_strategy(), 1..40)) {
        let mut s = Spline::default();
        for e in &edits {
            let before = s.clone();
            match s.apply(e) {
                Ok(()) => {
                    prop_assert!(s.validate().is_ok());
                }
                Err(_) => prop_assert_eq!(&s, &before),
            }
            // Whatever the spline is, it tessellates or says why.
            let _ = s.tessellate(Tessellation::default());
        }
    }
}
