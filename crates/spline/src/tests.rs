use glam::Vec3;
use proptest::prelude::*;

use crate::curve::{eval_piece, split_piece};
use crate::{CurveKind, HandleMode, Knot, MAX_KNOTS, MAX_SEGMENTS, PathBuilder, Segment, Side, Spline, SplineError, Tessellation};

const KAPPA: f32 = 0.552_284_8;

fn circle(r: f32) -> Segment {
    let k = |p: Vec3, t: Vec3| Knot { pos: p, tan_in: -t, tan_out: t, mode: HandleMode::Mirrored };
    Segment::new(
        CurveKind::Bezier,
        true,
        vec![
            k(Vec3::new(r, 0.0, 0.0), Vec3::new(0.0, r * KAPPA, 0.0)),
            k(Vec3::new(0.0, r, 0.0), Vec3::new(-r * KAPPA, 0.0, 0.0)),
            k(Vec3::new(-r, 0.0, 0.0), Vec3::new(0.0, -r * KAPPA, 0.0)),
            k(Vec3::new(0.0, -r, 0.0), Vec3::new(r * KAPPA, 0.0, 0.0)),
        ],
    )
}

fn dist_to_polyline(p: Vec3, pts: &[Vec3], closed: bool) -> f32 {
    let n = pts.len();
    let edges = if closed { n } else { n.saturating_sub(1) };
    (0..edges)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            let ab = b - a;
            let t = if ab.length_squared() < 1e-20 { 0.0 } else { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) };
            p.distance(a + ab * t)
        })
        .fold(f32::MAX, f32::min)
}

fn knot_strategy() -> impl Strategy<Value = Knot> {
    let v = || (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0).prop_map(|(x, y, z)| Vec3::new(x, y, z));
    (v(), v(), v(), 0usize..4).prop_map(|(pos, a, b, m)| Knot {
        pos,
        tan_in: a * 0.3,
        tan_out: b * 0.3,
        mode: [HandleMode::Free, HandleMode::Aligned, HandleMode::Mirrored, HandleMode::Auto][m],
    })
}

fn segment_strategy() -> impl Strategy<Value = Segment> {
    (prop::collection::vec(knot_strategy(), 2..8), any::<bool>(), 0usize..3)
        .prop_map(|(knots, closed, k)| Segment::new([CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline][k], closed, knots))
}

#[test]
fn a_bezier_circle_stays_on_the_circle() {
    let s = Spline::single(circle(2.0));
    let pl = s.tessellate(Tessellation::Fixed { steps: 16 }).unwrap();
    assert_eq!(pl.len(), 1);
    assert!(pl[0].closed);
    assert_eq!(pl[0].points.len(), 64, "four pieces of sixteen steps; the closing point is not repeated");
    for p in &pl[0].points {
        assert!((p.length() - 2.0).abs() < 0.003, "{p:?} is off the circle");
    }
    // The length is the circumference.
    assert!((s.length() - std::f32::consts::TAU * 2.0).abs() < 0.01);
}

#[test]
fn the_three_modes_count_points_differently() {
    let s = Spline::single(circle(1.0));
    let fixed = s.tessellate(Tessellation::Fixed { steps: 5 }).unwrap()[0].points.len();
    assert_eq!(fixed, 20);
    let coarse = s.tessellate(Tessellation::Uniform { length: 1.0 }).unwrap()[0].points.len();
    let fine = s.tessellate(Tessellation::Uniform { length: 0.1 }).unwrap()[0].points.len();
    assert!(coarse < fine, "{coarse} < {fine}");
    // Uniform steps are about the asked length (a unit circle is 6.28 long).
    assert!((55..=80).contains(&fine), "{fine}");
    let loose = s.tessellate(Tessellation::Adaptive { tolerance: 0.1, angle_deg: 45.0 }).unwrap()[0].points.len();
    let tight = s.tessellate(Tessellation::Adaptive { tolerance: 0.0005, angle_deg: 5.0 }).unwrap()[0].points.len();
    assert!(loose < tight, "{loose} < {tight}");
}

#[test]
fn a_straight_stretch_gets_few_points_in_adaptive_mode_and_a_bend_many() {
    // A straight Bezier piece, then a hairpin.
    let mut seg = Segment::new(
        CurveKind::Bezier,
        false,
        vec![
            Knot { pos: Vec3::ZERO, tan_in: Vec3::ZERO, tan_out: Vec3::new(1.0, 0.0, 0.0), mode: HandleMode::Free },
            Knot { pos: Vec3::new(4.0, 0.0, 0.0), tan_in: Vec3::new(-1.0, 0.0, 0.0), tan_out: Vec3::new(2.0, 0.0, 0.0), mode: HandleMode::Free },
            Knot { pos: Vec3::new(4.0, 1.0, 0.0), tan_in: Vec3::new(2.0, 0.0, 0.0), tan_out: Vec3::ZERO, mode: HandleMode::Free },
        ],
    );
    let first = Spline::single(Segment::new(CurveKind::Bezier, false, seg.knots[..2].to_vec())).tessellate(Tessellation::default()).unwrap();
    assert_eq!(first[0].points.len(), 2, "a straight piece needs only its ends");
    let both = Spline::single(seg.clone()).tessellate(Tessellation::default()).unwrap();
    assert!(both[0].points.len() > 8);
    seg.kind = CurveKind::Linear;
    let lin = Spline::single(seg).tessellate(Tessellation::Fixed { steps: 50 }).unwrap();
    assert_eq!(lin[0].points.len(), 3, "a polyline is its knots whatever the mode");
}

proptest! {
    #[test]
    fn adaptive_tessellation_stays_within_the_tolerance(seg in segment_strategy(), tol in 0.005f32..0.2) {
        prop_assume!(seg.kind != CurveKind::Linear);
        let mode = Tessellation::Adaptive { tolerance: tol, angle_deg: 90.0 };
        let pl = seg.tessellate(mode).unwrap();
        for piece in seg.pieces() {
            for k in 0..=40 {
                let p = eval_piece(&piece, k as f32 / 40.0);
                let d = dist_to_polyline(p, &pl.points, pl.closed);
                prop_assert!(d <= tol * 1.05 + 1e-4, "{d} > {tol}");
            }
        }
    }

    #[test]
    fn tessellation_is_finite_inside_the_control_box_and_starts_on_the_first_knot(seg in segment_strategy(), steps in 1u32..20) {
        let s = Spline::single(seg.clone());
        let pl = s.tessellate(Tessellation::Fixed { steps }).unwrap();
        let (lo, hi) = s.bounds().unwrap();
        for p in &pl[0].points {
            prop_assert!(p.is_finite());
            prop_assert!(p.cmpge(lo - Vec3::splat(1e-3)).all() && p.cmple(hi + Vec3::splat(1e-3)).all());
        }
        if seg.kind != CurveKind::BSpline {
            prop_assert!(pl[0].points[0].distance(seg.knots[0].pos) < 1e-5);
        }
    }

    #[test]
    fn an_open_bezier_or_b_spline_ends_on_its_end_knots(seg in segment_strategy()) {
        let mut seg = seg;
        seg.closed = false;
        let last = seg.knots[seg.knots.len() - 1].pos;
        let (a, _) = seg.sample(0.0);
        let (b, _) = seg.sample(seg.piece_count() as f32);
        prop_assert!(a.distance(seg.knots[0].pos) < 1e-4);
        prop_assert!(b.distance(last) < 1e-4);
    }

    #[test]
    fn pieces_join_end_to_start_and_a_closed_curve_returns_to_its_start(seg in segment_strategy()) {
        let pieces = seg.pieces();
        for w in pieces.windows(2) {
            prop_assert!(w[0][3].distance(w[1][0]) < 1e-4);
        }
        if seg.closed && !pieces.is_empty() {
            prop_assert!(pieces[pieces.len() - 1][3].distance(pieces[0][0]) < 1e-4);
        }
    }

    #[test]
    fn a_closed_b_spline_has_a_continuous_tangent_across_pieces(knots in prop::collection::vec(knot_strategy(), 3..8)) {
        let seg = Segment::new(CurveKind::BSpline, true, knots);
        let pieces = seg.pieces();
        for i in 0..pieces.len() {
            let a = &pieces[i];
            let b = &pieces[(i + 1) % pieces.len()];
            let end = (a[3] - a[2]) * 3.0;
            let start = (b[1] - b[0]) * 3.0;
            prop_assert!(end.distance(start) < 1e-3);
        }
    }

    #[test]
    fn inserting_a_point_keeps_the_shape(seg in segment_strategy(), frac in 0.0f32..1.0) {
        prop_assume!(seg.kind != CurveKind::BSpline);
        let mut after = seg.clone();
        let u = frac * seg.piece_count() as f32;
        let at = after.insert_at(u).unwrap();
        prop_assert_eq!(after.knots.len(), seg.knots.len() + 1);
        prop_assert!(after.knots[at].pos.distance(seg.sample(u).0) < 1e-3);
        // Every point of the old curve is on the new one.
        let dense = after.tessellate(Tessellation::Fixed { steps: 200 }).unwrap();
        for k in 0..=30 {
            let p = seg.sample(k as f32 / 30.0 * seg.piece_count() as f32).0;
            prop_assert!(dist_to_polyline(p, &dense.points, dense.closed) < 0.02);
        }
    }

    #[test]
    fn nearest_finds_a_point_on_the_curve(seg in segment_strategy(), frac in 0.0f32..1.0) {
        let u = frac * seg.piece_count() as f32;
        let p = seg.sample(u).0;
        let (_, d) = seg.nearest(p).unwrap();
        prop_assert!(d < 1e-2);
    }

    #[test]
    fn splines_survive_json(seg in segment_strategy()) {
        let s = Spline::new(vec![seg.clone(), seg]);
        let json = serde_json::to_string(&s).unwrap();
        let back: Spline = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(s, back);
    }

    #[test]
    fn split_pieces_trace_the_same_curve(a in -3.0f32..3.0, b in -3.0f32..3.0, t in 0.05f32..0.95) {
        let p = [Vec3::ZERO, Vec3::new(a, 1.0, 0.0), Vec3::new(b, -1.0, 2.0), Vec3::new(1.0, 1.0, 1.0)];
        let (l, r) = split_piece(&p, t);
        prop_assert!(eval_piece(&l, 0.5).distance(eval_piece(&p, t * 0.5)) < 1e-4);
        prop_assert!(eval_piece(&r, 0.5).distance(eval_piece(&p, t + (1.0 - t) * 0.5)) < 1e-4);
    }
}

#[test]
fn the_handle_modes_decide_what_the_other_handle_does() {
    let mk = |mode| {
        Segment::new(
            CurveKind::Bezier,
            false,
            vec![Knot::new(Vec3::ZERO), Knot { pos: Vec3::X, tan_in: Vec3::new(-0.5, 0.0, 0.0), tan_out: Vec3::new(0.25, 0.0, 0.0), mode }],
        )
    };
    let mut s = mk(HandleMode::Mirrored);
    s.set_handle(1, Side::In, Vec3::new(-0.3, 0.3, 0.0)).unwrap();
    assert_eq!(s.knots[1].tan_out, Vec3::new(0.3, -0.3, 0.0));
    let mut s = mk(HandleMode::Aligned);
    s.set_handle(1, Side::In, Vec3::new(0.0, -2.0, 0.0)).unwrap();
    assert!((s.knots[1].tan_out - Vec3::new(0.0, 0.25, 0.0)).length() < 1e-6, "it turned to stay in line and kept its length");
    let mut s = mk(HandleMode::Free);
    s.set_handle(1, Side::In, Vec3::new(0.0, -2.0, 0.0)).unwrap();
    assert_eq!(s.knots[1].tan_out, Vec3::new(0.25, 0.0, 0.0));
    // Editing a smooth knot's handle makes it aligned, with the handles it had.
    let mut s = Segment::new(
        CurveKind::Bezier,
        false,
        vec![Knot::new(Vec3::ZERO), Knot { mode: HandleMode::Auto, ..Knot::new(Vec3::X) }, Knot::new(Vec3::new(2.0, 1.0, 0.0))],
    );
    let before = s.effective_handles()[1];
    s.set_mode(1, HandleMode::Free).unwrap();
    assert_eq!((s.knots[1].tan_in, s.knots[1].tan_out), before);
    assert!(s.set_handle(9, Side::In, Vec3::X).is_err());
    assert!(s.set_handle(1, Side::In, Vec3::NAN).is_err());
}

#[test]
fn smooth_knots_make_a_smooth_curve_through_the_points() {
    let knots: Vec<Knot> = (0..6).map(|i| Knot { mode: HandleMode::Auto, ..Knot::new(Vec3::new(i as f32, (i as f32).sin(), 0.0)) }).collect();
    let s = Segment::new(CurveKind::Bezier, false, knots);
    let pieces = s.pieces();
    for w in pieces.windows(2) {
        let end = w[0][3] - w[0][2];
        let start = w[1][1] - w[1][0];
        assert!(end.normalize().dot(start.normalize()) > 0.999, "a kink at a smooth knot");
    }
}

#[test]
fn a_path_builder_makes_a_closed_outline() {
    let mut b = PathBuilder::new();
    b.move_to(Vec3::ZERO).line_to(Vec3::new(2.0, 0.0, 0.0)).quad_to(Vec3::new(3.0, 1.0, 0.0), Vec3::new(2.0, 2.0, 0.0)).cubic_to(
        Vec3::new(1.0, 3.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
        Vec3::new(0.0, 2.0, 0.0),
    );
    b.line_to(Vec3::ZERO).close();
    b.move_to(Vec3::new(5.0, 0.0, 0.0)).line_to(Vec3::new(6.0, 0.0, 0.0)).line_to(Vec3::new(6.0, 1.0, 0.0));
    let s = b.finish();
    assert_eq!(s.segments.len(), 2);
    assert!(s.segments[0].closed && !s.segments[1].closed);
    assert_eq!(s.segments[0].knots.len(), 4, "the last point on the first is merged");
    assert!(s.validate().is_ok());
    let pl = s.tessellate(Tessellation::default()).unwrap();
    assert!(pl[0].points.iter().all(|p| p.z == 0.0));
}

#[test]
fn hostile_splines_give_errors_not_crashes() {
    // Not a number.
    let mut s = Spline::single(circle(1.0));
    s.segments[0].knots[2].tan_out = Vec3::new(f32::NAN, 0.0, 0.0);
    assert_eq!(s.validate(), Err(SplineError::NonFinite { segment: 0, knot: 2 }));
    assert!(s.tessellate(Tessellation::default()).is_err());
    assert!(s.segments[0].tessellate(Tessellation::default()).is_err());
    let mut inf = Spline::single(circle(1.0));
    inf.segments[0].knots[0].pos = Vec3::splat(f32::INFINITY);
    assert!(inf.validate().is_err());
    // A million points.
    let many = Spline::single(Segment::polyline(&vec![Vec3::ZERO; 1_000_000], false));
    assert_eq!(many.validate(), Err(SplineError::TooManyKnots(1_000_000)));
    assert!(many.tessellate(Tessellation::default()).is_err());
    // Too many segments.
    let segs = Spline::new(vec![Segment::polyline(&[Vec3::ZERO, Vec3::X], false); MAX_SEGMENTS + 1]);
    assert!(matches!(segs.validate(), Err(SplineError::TooManySegments(_))));
    // Everything at one point (zero length): finite and small.
    let zero = Spline::single(Segment::new(CurveKind::Bezier, true, vec![Knot::new(Vec3::ONE); 5]));
    for mode in [Tessellation::default(), Tessellation::Fixed { steps: 4 }, Tessellation::Uniform { length: 0.01 }] {
        let pl = zero.tessellate(mode).unwrap();
        assert!(pl[0].points.iter().all(|p| *p == Vec3::ONE));
    }
    assert_eq!(zero.length(), 0.0);
    // Degenerate sizes: no knots, one knot.
    let empty = Spline::single(Segment::default());
    assert!(empty.tessellate(Tessellation::default()).unwrap()[0].points.is_empty());
    let one = Spline::single(Segment::new(CurveKind::BSpline, true, vec![Knot::new(Vec3::Y)]));
    assert_eq!(one.tessellate(Tessellation::default()).unwrap()[0].points, vec![Vec3::Y]);
    assert!(empty.bounds().is_none());
    // Absurd mode numbers are clamped.
    let c = Spline::single(circle(1.0));
    assert!(c.tessellate(Tessellation::Fixed { steps: u32::MAX }).is_ok());
    assert!(c.tessellate(Tessellation::Uniform { length: f32::NAN }).is_ok());
    assert!(c.tessellate(Tessellation::Uniform { length: 0.0 }).is_ok());
    assert!(c.tessellate(Tessellation::Adaptive { tolerance: 0.0, angle_deg: 0.0 }).is_ok());
    assert!(c.tessellate(Tessellation::Adaptive { tolerance: f32::NAN, angle_deg: f32::NAN }).is_ok());
    // Bad indexes.
    let mut seg = circle(1.0);
    assert!(seg.move_knot(40, Vec3::X).is_err());
    assert!(seg.move_knot(0, Vec3::NAN).is_err());
    assert!(seg.remove_knot(9).is_err());
    assert!(seg.insert_at(f32::NAN).is_err());
    assert!(Segment::default().insert_at(0.5).is_err());
    assert!(Segment::default().nearest(Vec3::ZERO).is_none());
    assert_eq!(Segment::default().sample(3.0), (Vec3::ZERO, Vec3::ZERO));
}

#[test]
fn tessellation_work_is_capped_by_the_output_not_only_the_input() {
    // The input is allowed (100,000 knots), the output of 256 steps each is not: it is refused
    // before any point is built.
    let ring: Vec<Vec3> = (0..MAX_KNOTS).map(|i| Vec3::new((i as f32).cos(), (i as f32).sin(), 0.0)).collect();
    let big = Spline::single(Segment::new(CurveKind::Bezier, true, ring.iter().map(|p| Knot::new(*p)).collect()));
    assert!(big.validate().is_ok());
    assert_eq!(big.tessellate(Tessellation::Fixed { steps: 256 }), Err(SplineError::TooLarge));
    // A tight adaptive tolerance on a wild curve hits the cap too and stops there.
    let wild = Spline::single(Segment::new(
        CurveKind::Bezier,
        true,
        ring.iter().map(|p| Knot { pos: *p * 50.0, tan_in: -*p * 40.0, tan_out: *p * 40.0, mode: HandleMode::Free }).collect(),
    ));
    let r = wild.tessellate(Tessellation::Adaptive { tolerance: 1e-6, angle_deg: 1.0 });
    assert_eq!(r, Err(SplineError::TooLarge));
    // The budget is shared by all segments: many small ones add up.
    let seg = Segment::new(CurveKind::Bezier, true, (0..5).map(|i| Knot::new(Vec3::new(i as f32, 0.0, 0.0))).collect());
    let many = Spline::new(vec![seg; MAX_SEGMENTS]);
    assert_eq!(many.tessellate(Tessellation::Fixed { steps: 256 }), Err(SplineError::TooLarge));
    let ok = many.tessellate(Tessellation::Fixed { steps: 100 }).unwrap();
    assert_eq!(ok.iter().map(|p| p.points.len()).sum::<usize>(), MAX_SEGMENTS * 500);
}
