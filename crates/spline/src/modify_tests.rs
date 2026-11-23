use std::f32::consts::PI;

use glam::Vec3;
use proptest::prelude::*;

use crate::modify::Cut;
use crate::{CurveKind, HandleMode, Knot, Segment, Spline, SplineEdit, SplineError, Tessellation};

fn square(side: f32) -> Segment {
    Segment::polyline(&[Vec3::ZERO, Vec3::new(side, 0.0, 0.0), Vec3::new(side, side, 0.0), Vec3::new(0.0, side, 0.0)], true)
}

fn area(pts: &[Vec3]) -> f32 {
    (0..pts.len()).map(|i| pts[i].x * pts[(i + 1) % pts.len()].y - pts[(i + 1) % pts.len()].x * pts[i].y).sum::<f32>() * 0.5
}

fn flat(s: &Segment) -> Vec<Vec3> {
    s.tessellate(Tessellation::Adaptive { tolerance: 0.0005, angle_deg: 3.0 }).unwrap().points
}

#[test]
fn a_chamfer_cuts_each_chosen_corner_with_a_straight_edge() {
    let mut s = square(4.0);
    let r = s.cut_corners(&[0, 1, 2, 3], Cut::Chamfer(1.0)).unwrap();
    assert_eq!((r.cut, r.skipped.len()), (4, 0));
    assert_eq!(s.knots.len(), 8);
    assert!((area(&flat(&s)) - (16.0 - 4.0 * 0.5)).abs() < 1e-4, "each corner loses a right triangle of legs 1");
    // Only some corners.
    let mut one = square(4.0);
    one.cut_corners(&[2], Cut::Chamfer(1.0)).unwrap();
    assert_eq!(one.knots.len(), 5);
    assert!((area(&flat(&one)) - 15.5).abs() < 1e-4);
    // The cut is limited to half the edge, so neighbouring cuts meet and never cross.
    let mut big = square(4.0);
    big.cut_corners(&[0, 1, 2, 3], Cut::Chamfer(100.0)).unwrap();
    assert!((area(&flat(&big)) - 8.0).abs() < 1e-4, "a diamond: half of the square");
}

#[test]
fn a_fillet_is_a_circular_arc_tangent_to_both_edges() {
    let mut s = square(4.0);
    let r = s.cut_corners(&[0, 1, 2, 3], Cut::Fillet(1.0)).unwrap();
    assert_eq!(r.cut, 4);
    assert_eq!(s.kind, CurveKind::Bezier);
    // Each rounded corner loses (4 - pi) r^2 / 1 of area.
    assert!((area(&flat(&s)) - (16.0 - (4.0 - PI))).abs() < 1e-3, "{}", area(&flat(&s)));
    // The arcs are on circles of radius 1 about (1,1), (3,1), (3,3), (1,3); the straight edges are not.
    let pts = flat(&s);
    for (c, corner) in [
        (Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 0.0)),
        (Vec3::new(3.0, 1.0, 0.0), Vec3::new(4.0, 0.0, 0.0)),
        (Vec3::new(3.0, 3.0, 0.0), Vec3::new(4.0, 4.0, 0.0)),
        (Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 4.0, 0.0)),
    ] {
        let near: Vec<f32> = pts.iter().filter(|p| p.distance(c) < 1.2 && (**p - c).dot(corner - c) > 0.0).map(|p| p.distance(c)).collect();
        assert!(near.len() > 4 && near.iter().all(|d| (d - 1.0).abs() < 0.005), "{c:?}: {near:?}");
        // The corner point itself is gone: nothing comes nearer to it than the arc does.
        assert!(pts.iter().all(|p| p.distance(corner) > 0.4));
    }
    // The tangent does not jump at the arc's ends (a smooth join to the straight edge).
    let pieces = s.pieces();
    for w in pieces.windows(2) {
        let end = (w[0][3] - w[0][2]).try_normalize();
        let start = (w[1][1] - w[1][0]).try_normalize();
        if let (Some(a), Some(b)) = (end, start) {
            assert!(a.dot(b) > 0.999, "{a:?} {b:?}");
        }
    }
    // Other angles: an acute corner of a triangle, radius larger than the edges allow.
    let mut tri = Segment::polyline(&[Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0)], true);
    let r = tri.cut_corners(&[2], Cut::Fillet(50.0)).unwrap();
    assert_eq!(r.cut, 1);
    assert!(area(&flat(&tri)) > 0.0 && area(&flat(&tri)) < 2.0);
}

#[test]
fn corners_that_cannot_be_cut_are_skipped_with_a_reason() {
    let mut open = Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)], false);
    let r = open.cut_corners(&[0, 1, 2], Cut::Chamfer(0.2)).unwrap();
    assert_eq!((r.cut, r.skipped.len()), (1, 2), "only the middle point of an open curve is a corner");
    // In line: nothing to cut.
    let mut line = Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::X * 2.0], false);
    let r = line.cut_corners(&[1], Cut::Fillet(0.2)).unwrap();
    assert_eq!(r.cut, 0);
    assert_eq!(line.knots.len(), 3);
    // Next to a curved edge.
    let mut curve = Segment::new(
        CurveKind::Bezier,
        false,
        vec![
            Knot::new(Vec3::ZERO),
            Knot { pos: Vec3::X, tan_in: Vec3::new(-0.3, 0.5, 0.0), tan_out: Vec3::ZERO, mode: HandleMode::Free },
            Knot::new(Vec3::new(1.0, 1.0, 0.0)),
        ],
    );
    assert_eq!(curve.cut_corners(&[1], Cut::Chamfer(0.1)).unwrap().cut, 0);
    // A Bezier corner with zero handles is a corner.
    let mut bez = Segment::new(CurveKind::Bezier, true, square(2.0).knots);
    assert_eq!(bez.cut_corners(&[0], Cut::Fillet(0.5)).unwrap().cut, 1);
    // Bad sizes and indexes.
    assert!(square(1.0).cut_corners(&[0], Cut::Chamfer(0.0)).is_err());
    assert!(square(1.0).cut_corners(&[0], Cut::Fillet(f32::NAN)).is_err());
    assert!(square(1.0).cut_corners(&[0], Cut::Fillet(-1.0)).is_err());
    assert_eq!(square(1.0).cut_corners(&[99], Cut::Chamfer(0.1)).unwrap().cut, 0);
    // Two points on the same spot and a two-point curve.
    let mut zero = Segment::polyline(&[Vec3::ZERO, Vec3::ZERO, Vec3::X], false);
    assert_eq!(zero.cut_corners(&[1], Cut::Chamfer(0.1)).unwrap().cut, 0);
    let mut two = Segment::polyline(&[Vec3::ZERO, Vec3::X], true);
    assert_eq!(two.cut_corners(&[0], Cut::Chamfer(0.1)).unwrap().cut, 0);
}

#[test]
fn reversing_runs_the_same_curve_the_other_way() {
    for kind in [CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline] {
        for closed in [false, true] {
            let knots: Vec<Knot> = (0..5)
                .map(|i| Knot {
                    pos: Vec3::new(i as f32, (i as f32 * 1.3).sin(), (i * i) as f32 * 0.1),
                    tan_in: Vec3::new(-0.3, 0.2, 0.0),
                    tan_out: Vec3::new(0.4, -0.1, 0.1),
                    mode: HandleMode::Free,
                })
                .collect();
            let original = Segment::new(kind, closed, knots);
            let mut rev = original.clone();
            rev.reverse();
            // The same points on the curve, in the opposite order.
            let a = original.tessellate(Tessellation::Fixed { steps: 16 }).unwrap();
            let b = rev.tessellate(Tessellation::Fixed { steps: 16 }).unwrap();
            assert_eq!(a.points.len(), b.points.len());
            for p in &a.points {
                let d = b.points.iter().map(|q| q.distance(*p)).fold(f32::MAX, f32::min);
                assert!(d < 1e-4, "{kind:?} closed {closed}: {d}");
            }
            if !closed {
                assert!(a.points[0].distance(*b.points.last().unwrap()) < 1e-5);
            } else {
                assert_eq!(rev.knots[0].pos, original.knots[0].pos, "a closed curve keeps its first point first");
            }
            // Twice is the same as never.
            rev.reverse();
            assert_eq!(rev, original, "{kind:?} closed {closed}");
        }
    }
    // Mirrored and smooth handles stay valid.
    let mut m = Segment::new(
        CurveKind::Bezier,
        false,
        vec![Knot::smooth(Vec3::ZERO, Vec3::Y), Knot { mode: HandleMode::Auto, ..Knot::new(Vec3::X) }, Knot::smooth(Vec3::new(2.0, 0.0, 0.0), Vec3::Y)],
    );
    let before = m.tessellate(Tessellation::Fixed { steps: 8 }).unwrap();
    m.reverse();
    let after = m.tessellate(Tessellation::Fixed { steps: 8 }).unwrap();
    assert!(before.points.iter().all(|p| after.points.iter().any(|q| q.distance(*p) < 1e-4)));
}

#[test]
fn closing_joins_ends_that_are_on_each_other() {
    let mut s = Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::ZERO], false);
    s.close_curve();
    assert!(s.closed);
    assert_eq!(s.knots.len(), 3, "the last point is the first");
    let mut apart = Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)], false);
    apart.close_curve();
    assert!(apart.closed && apart.knots.len() == 3);
    // Idempotent.
    let again = apart.clone();
    apart.close_curve();
    assert_eq!(apart, again);
}

#[test]
fn to_polygon_flattens_curves_to_straight_edges_within_the_tolerance() {
    let circle = crate::SplinePrimitive::Circle { radius_x: 2.0, radius_y: 2.0, plane: Default::default() }.build();
    let poly = circle.to_polygon(Tessellation::Adaptive { tolerance: 0.001, angle_deg: 5.0 }).unwrap();
    assert!(poly.segments.iter().all(|s| s.kind == CurveKind::Linear && s.closed));
    assert!(poly.segments[0].knots.len() > 30);
    let a = area(&poly.segments[0].knots.iter().map(|k| k.pos).collect::<Vec<_>>());
    assert!((a - PI * 4.0).abs() < 0.02, "{a}");
    // Each segment is flattened, several of them too.
    let two = Spline::new(vec![square(1.0), circle.segments[0].clone()]);
    assert_eq!(two.to_polygon(Tessellation::Fixed { steps: 4 }).unwrap().segments.len(), 2);
    // Hostile curves are errors.
    let mut bad = circle.clone();
    bad.segments[0].knots[0].pos = Vec3::NAN;
    assert!(matches!(bad.to_polygon(Tessellation::default()), Err(SplineError::NonFinite { .. })));
}

#[test]
fn the_new_edits_work_through_apply_and_a_failed_one_changes_nothing() {
    let mut s = Spline::single(square(4.0));
    s.apply(&SplineEdit::Chamfer { knots: vec![crate::KnotRef { segment: 0, knot: 0 }, crate::KnotRef { segment: 0, knot: 2 }], distance: 1.0 }).unwrap();
    assert_eq!(s.segments[0].knots.len(), 6);
    s.apply(&SplineEdit::Reverse { segment: 0 }).unwrap();
    s.apply(&SplineEdit::ToPolygon { tess: Tessellation::Fixed { steps: 2 } }).unwrap();
    assert!(s.segments[0].kind == CurveKind::Linear);
    let mut f = Spline::single(square(4.0));
    f.apply(&SplineEdit::Fillet { knots: vec![crate::KnotRef { segment: 0, knot: 1 }], radius: 1.0 }).unwrap();
    assert_eq!(f.segments[0].kind, CurveKind::Bezier);
    // Nothing to cut is an error (and no change).
    let original = Spline::single(Segment::polyline(&[Vec3::ZERO, Vec3::X, Vec3::X * 2.0], false));
    let mut o = original.clone();
    assert!(o.apply(&SplineEdit::Fillet { knots: vec![crate::KnotRef { segment: 0, knot: 1 }], radius: 1.0 }).is_err());
    assert!(o.apply(&SplineEdit::Chamfer { knots: vec![crate::KnotRef { segment: 3, knot: 1 }], distance: 1.0 }).is_err());
    assert!(o.apply(&SplineEdit::Reverse { segment: 4 }).is_err());
    assert!(o.apply(&SplineEdit::Chamfer { knots: vec![crate::KnotRef { segment: 0, knot: 1 }], distance: f32::NAN }).is_err());
    assert_eq!(o, original);
}

proptest! {
    #[test]
    fn cutting_corners_keeps_a_valid_simple_shape(pts in prop::collection::vec((0.5f32..5.0, 0.0f32..1.0), 3..12), d in 0.01f32..3.0, fillet in any::<bool>()) {
        let n = pts.len();
        // A star-shaped polygon: points at increasing angles.
        let poly: Vec<Vec3> = pts.iter().enumerate().map(|(i, (r, _))| { let a = std::f32::consts::TAU * i as f32 / n as f32; Vec3::new(r * a.cos(), r * a.sin(), 0.0) }).collect();
        let mut s = Segment::polyline(&poly, true);
        let before = area(&poly);
        let all: Vec<usize> = (0..n).collect();
        let r = s.cut_corners(&all, if fillet { Cut::Fillet(d) } else { Cut::Chamfer(d) }).unwrap();
        prop_assert!(Spline::single(s.clone()).validate().is_ok());
        // Cuts that meet in the middle of an edge share a point, so the count is at most one more per cut.
        prop_assert!(s.knots.len() >= n && s.knots.len() <= n + r.cut);
        let after = area(&flat(&s));
        // Cutting corners off a convex-or-not star shape never adds area to a polygon whose corners are convex; for
        // reflex corners the cut adds area, so only check that the result is a finite, non-degenerate shape.
        prop_assert!(after.is_finite() && after.abs() > 1e-6 * before.abs().min(1.0));
    }

    #[test]
    fn reversing_twice_changes_nothing(pts in prop::collection::vec((-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0), 2..10), closed in any::<bool>(), kind in 0usize..3) {
        let knots: Vec<Knot> = pts.iter().map(|(x, y, z)| Knot::smooth(Vec3::new(*x, *y, *z), Vec3::new(0.3, 0.1, 0.0))).collect();
        let original = Segment::new([CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline][kind], closed, knots);
        let mut s = original.clone();
        s.reverse();
        s.reverse();
        prop_assert_eq!(s, original);
    }
}

#[test]
fn a_fillet_on_a_polyline_with_stored_handles_keeps_the_straight_edges_straight() {
    // A polyline whose points still carry handles (drawn with the pen, then switched to a polyline).
    let knots: Vec<Knot> = [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)]
        .iter()
        .map(|(x, y)| Knot { pos: Vec3::new(*x, *y, 0.0), tan_in: Vec3::new(-0.7, 0.3, 0.0), tan_out: Vec3::new(0.7, -0.3, 0.0), mode: HandleMode::Mirrored })
        .collect();
    let mut s = Segment::new(CurveKind::Linear, true, knots);
    assert_eq!(s.cut_corners(&[1], Cut::Fillet(1.0)).unwrap().cut, 1);
    assert_eq!(s.kind, CurveKind::Bezier);
    assert!(s.knots.iter().all(|k| k.mode == HandleMode::Free));
    // Area: the square minus one rounded corner, and no edge bows out.
    assert!((area(&flat(&s)) - (16.0 - (1.0 - PI / 4.0))).abs() < 2e-3, "{}", area(&flat(&s)));
    for p in flat(&s) {
        assert!(p.x >= -1e-4 && p.x <= 4.0 + 1e-4 && p.y >= -1e-4 && p.y <= 4.0 + 1e-4, "{p:?} left the square");
    }
}

#[test]
fn cutting_every_corner_of_a_big_polygon_is_one_pass_and_the_cap_is_checked_first() {
    use crate::{KnotRef, MAX_KNOTS};
    let n = 60_000;
    let pts: Vec<Vec3> = (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            // A zigzag, so that every point is a real corner.
            Vec3::new(a.cos(), a.sin(), 0.0) * if i % 2 == 0 { 100.0 } else { 99.0 }
        })
        .collect();
    let mut s = Spline::single(Segment::polyline(&pts, true));
    let all: Vec<KnotRef> = (0..n).map(|k| KnotRef { segment: 0, knot: k }).collect();
    // 120,000 points would pass the cap: refused before anything is built, and nothing changed.
    let before = s.clone();
    let e = s.apply(&SplineEdit::Chamfer { knots: all.clone(), distance: 0.001 }).unwrap_err();
    assert_eq!(e, SplineError::TooManyKnots(2 * n));
    assert_eq!(s, before);
    // Under the cap it is fine (every other point).
    let some: Vec<KnotRef> = all.iter().step_by(2).copied().collect();
    let t = std::time::Instant::now();
    s.apply(&SplineEdit::Chamfer { knots: some, distance: 0.001 }).unwrap();
    assert_eq!(s.knot_count(), n + n / 2, "a point is added for every cut");
    assert!(s.knot_count() <= MAX_KNOTS);
    assert!(t.elapsed().as_secs_f32() < 5.0, "one pass, not one shift per corner");
}

#[test]
fn a_cut_clamped_at_both_ends_leaves_no_coincident_points() {
    let mut s = square(4.0);
    s.cut_corners(&[0, 1, 2, 3], Cut::Chamfer(100.0)).unwrap();
    assert_eq!(s.knots.len(), 4, "the cuts meet at the edge middles: one point each");
    let mut f = square(4.0);
    f.cut_corners(&[0, 1, 2, 3], Cut::Fillet(100.0)).unwrap();
    for w in f.knots.windows(2) {
        assert!(w[0].pos.distance(w[1].pos) > 1e-4);
    }
}

#[test]
fn reversing_and_closing_use_the_handles_a_point_really_has() {
    // A mirrored point whose stored in-handle is not the opposite of its out-handle.
    let k = |x: f32, tin: Vec3, tout: Vec3, mode| Knot { pos: Vec3::new(x, 0.0, 0.0), tan_in: tin, tan_out: tout, mode };
    let s0 = Segment::new(
        CurveKind::Bezier,
        false,
        vec![
            k(0.0, Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), HandleMode::Free),
            k(2.0, Vec3::new(0.0, 5.0, 0.0), Vec3::new(0.0, -1.0, 0.0), HandleMode::Mirrored),
            k(4.0, Vec3::new(0.0, -1.0, 0.0), Vec3::ZERO, HandleMode::Free),
        ],
    );
    let mut r = s0.clone();
    r.reverse();
    let (a, b) = (s0.tessellate(Tessellation::Fixed { steps: 16 }).unwrap(), r.tessellate(Tessellation::Fixed { steps: 16 }).unwrap());
    assert!(a.points.iter().all(|p| b.points.iter().any(|q| q.distance(*p) < 1e-4)), "the same curve");
    // Closing an open curve whose last point is on its first keeps both handles of the joined point.
    let mut c = Segment::new(
        CurveKind::Bezier,
        false,
        vec![
            k(0.0, Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0), HandleMode::Mirrored),
            k(2.0, Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), HandleMode::Free),
            Knot { pos: Vec3::new(1.0, 2.0, 0.0), ..Knot::default() },
            Knot { pos: Vec3::ZERO, tan_in: Vec3::new(0.0, -2.0, 0.0), tan_out: Vec3::ZERO, mode: HandleMode::Free },
        ],
    );
    c.close_curve();
    assert!(c.closed && c.knots.len() == 3);
    assert_eq!(
        (c.knots[0].tan_in, c.knots[0].tan_out),
        (Vec3::new(0.0, -2.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        "arrives as the last point did, leaves as the first did"
    );
}
