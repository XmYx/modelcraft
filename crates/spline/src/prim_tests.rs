use std::f32::consts::{PI, TAU};

use glam::Vec3;
use proptest::prelude::*;

use crate::primitives::{KINDS, MAX_CORNERS};
use crate::{ArcClose, CurveKind, Plane, SplinePrimitive, Tessellation};

fn points(p: &SplinePrimitive) -> Vec<Vec3> {
    p.build().tessellate(Tessellation::Adaptive { tolerance: 0.001, angle_deg: 5.0 }).unwrap().into_iter().flat_map(|l| l.points).collect()
}

fn area_xy(pts: &[Vec3]) -> f32 {
    (0..pts.len()).map(|i| pts[i].x * pts[(i + 1) % pts.len()].y - pts[(i + 1) % pts.len()].x * pts[i].y).sum::<f32>() * 0.5
}

#[test]
fn every_kind_has_a_default_that_builds_a_valid_spline() {
    for k in KINDS {
        let p = SplinePrimitive::default_of(k).unwrap();
        assert_eq!(p.kind(), *k);
        let s = p.build();
        assert!(s.validate().is_ok(), "{k}");
        assert!(!points(&p).is_empty(), "{k}");
    }
    assert!(SplinePrimitive::default_of("spiral").is_none());
}

#[test]
fn a_circle_is_round_and_closed_and_an_ellipse_has_two_radii() {
    let c = SplinePrimitive::Circle { radius_x: 2.0, radius_y: 2.0, plane: Plane::Xy };
    assert!(c.build().segments[0].closed);
    for p in points(&c) {
        assert!((p.truncate().length() - 2.0).abs() < 0.002 && p.z == 0.0);
    }
    assert!((area_xy(&points(&c)) - PI * 4.0).abs() < 0.05, "counter-clockwise, area pi r squared");
    let e = SplinePrimitive::Circle { radius_x: 3.0, radius_y: 1.0, plane: Plane::Xy };
    let (lo, hi) = e.build().bounds().unwrap();
    assert!((hi.x - 3.0).abs() < 0.01 && (hi.y - 1.0).abs() < 0.01 && lo.x < -2.99, "{lo:?} {hi:?}");
}

#[test]
fn a_rectangle_has_its_size_and_rounded_corners_cut_the_corner() {
    let r = SplinePrimitive::Rectangle { width: 4.0, height: 2.0, corner: 0.0, plane: Plane::Xy };
    assert_eq!(r.build().segments[0].kind, CurveKind::Linear);
    assert!((area_xy(&points(&r)) - 8.0).abs() < 1e-4);
    let rr = SplinePrimitive::Rectangle { width: 4.0, height: 2.0, corner: 0.5, plane: Plane::Xy };
    let pts = points(&rr);
    assert!(pts.iter().all(|p| p.x.abs() <= 2.0 + 1e-4 && p.y.abs() <= 1.0 + 1e-4));
    assert!(pts.iter().all(|p| p.distance(Vec3::new(2.0, 1.0, 0.0)) > 0.2), "the corner point is cut off");
    // Area of a rounded rectangle: w*h - (4 - pi) r^2.
    assert!((area_xy(&pts) - (8.0 - (4.0 - PI) * 0.25)).abs() < 0.01);
    // A corner larger than half the short side is limited to it (a stadium).
    let big = SplinePrimitive::Rectangle { width: 4.0, height: 2.0, corner: 50.0, plane: Plane::Xy };
    assert!(points(&big).iter().all(|p| p.y.abs() <= 1.0 + 1e-3));
}

#[test]
fn stars_and_polygons_have_the_right_corners() {
    let star = SplinePrimitive::Star { points: 5, inner: 0.4, outer: 1.0, plane: Plane::Xy }.build();
    assert_eq!(star.segments[0].knots.len(), 10);
    let lens: Vec<f32> = star.segments[0].knots.iter().map(|k| k.pos.length()).collect();
    assert!(lens.iter().step_by(2).all(|l| (l - 1.0).abs() < 1e-5) && lens.iter().skip(1).step_by(2).all(|l| (l - 0.4).abs() < 1e-5));
    let hex = SplinePrimitive::NSide { sides: 6, radius: 1.0, plane: Plane::Xy }.build();
    assert_eq!(hex.segments[0].knots.len(), 6);
    let pts: Vec<Vec3> = hex.segments[0].knots.iter().map(|k| k.pos).collect();
    let side = pts[0].distance(pts[1]);
    assert!((side - 1.0).abs() < 1e-5, "a hexagon's side is its radius");
    assert!((area_xy(&pts) - 3.0 * 3f32.sqrt() / 2.0).abs() < 1e-4);
}

#[test]
fn arcs_sweep_the_asked_angle_and_close_three_ways() {
    let arc = |close, start, end| SplinePrimitive::Arc { radius: 2.0, start, end, close, plane: Plane::Xy };
    let open = arc(ArcClose::Open, 0.0, 90.0).build();
    assert!(!open.segments[0].closed);
    let pts = points(&arc(ArcClose::Open, 0.0, 90.0));
    assert!(pts[0].distance(Vec3::new(2.0, 0.0, 0.0)) < 1e-4 && pts[pts.len() - 1].distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-4);
    // Clockwise when the end is below the start.
    let cw = points(&arc(ArcClose::Open, 90.0, 0.0));
    assert!(cw[0].distance(Vec3::new(0.0, 2.0, 0.0)) < 1e-4 && cw.iter().all(|p| p.x >= -1e-4 && p.y >= -1e-4));
    // A half circle closed by the chord has the half disc's area; the pie of a quarter adds the centre.
    assert!((area_xy(&points(&arc(ArcClose::Chord, 0.0, 180.0))) - PI * 4.0 / 2.0).abs() < 0.05);
    let pie = arc(ArcClose::Pie, 0.0, 90.0);
    assert!((area_xy(&points(&pie)) - PI * 4.0 / 4.0).abs() < 0.03);
    assert!(points(&pie).iter().any(|p| p.length() < 1e-5), "the centre is a knot");
    // A full sweep is a circle.
    assert!(arc(ArcClose::Open, 0.0, 360.0).build().segments[0].closed);
    // More than a full sweep is capped.
    assert!(points(&arc(ArcClose::Open, 0.0, 3600.0)).iter().all(|p| (p.length() - 2.0).abs() < 0.01));
}

#[test]
fn a_helix_climbs_by_its_height_and_turns_by_its_turns() {
    let h = SplinePrimitive::Helix { radius_start: 1.0, radius_end: 1.0, height: 6.0, turns: 3.0, knots_per_turn: 12, plane: Plane::Zx };
    let s = h.build();
    assert_eq!(s.segments[0].knots.len(), 37);
    assert!(!s.segments[0].closed);
    let pts = points(&h);
    assert!(pts.iter().all(|p| (Vec3::new(p.x, 0.0, p.z).length() - 1.0).abs() < 0.01), "on the cylinder");
    let (lo, hi) = (pts.iter().fold(Vec3::MAX, |a, p| a.min(*p)), pts.iter().fold(Vec3::MIN, |a, p| a.max(*p)));
    assert!((hi.y - 6.0).abs() < 1e-3 && lo.y.abs() < 1e-3);
    // Height rises with arc length: monotonic.
    assert!(pts.windows(2).all(|w| w[1].y >= w[0].y - 1e-6));
    // A spiral changes radius.
    let sp = points(&SplinePrimitive::Helix { radius_start: 0.5, radius_end: 2.5, height: 0.0, turns: 2.0, knots_per_turn: 16, plane: Plane::Xy });
    assert!((sp[0].truncate().length() - 0.5).abs() < 1e-3 && (sp[sp.len() - 1].truncate().length() - 2.5).abs() < 1e-3);
    // The total angle turned is turns * 2 pi.
    let turned: f32 =
        sp.windows(2).map(|w| (w[1].y.atan2(w[1].x) - w[0].y.atan2(w[0].x)).rem_euclid(TAU)).map(|d| if d > PI { d - TAU } else { d }).sum::<f32>().abs();
    assert!((turned - 4.0 * PI).abs() < 0.1, "{turned}");
}

#[test]
fn planes_put_the_shape_where_they_say() {
    let at = |plane| SplinePrimitive::Circle { radius_x: 1.0, radius_y: 1.0, plane }.build().bounds().unwrap();
    let (_, hi) = at(Plane::Xy);
    assert!(hi.z.abs() < 1e-5);
    let (_, hi) = at(Plane::Yz);
    assert!(hi.x.abs() < 1e-5);
    let (_, hi) = at(Plane::Zx);
    assert!(hi.y.abs() < 1e-5);
    // A right-handed frame: u cross v is the facing axis.
    for (p, n) in [(Plane::Xy, Vec3::Z), (Plane::Yz, Vec3::X), (Plane::Zx, Vec3::Y)] {
        assert!(p.point(1.0, 0.0, 0.0).cross(p.point(0.0, 1.0, 0.0)).distance(n) < 1e-6);
        assert!(p.point(0.0, 0.0, 1.0).distance(n) < 1e-6);
    }
}

#[test]
fn hostile_numbers_give_a_shape_not_a_crash() {
    for k in KINDS {
        let p = match SplinePrimitive::default_of(k).unwrap() {
            SplinePrimitive::Circle { plane, .. } => SplinePrimitive::Circle { radius_x: f32::NAN, radius_y: f32::INFINITY, plane },
            SplinePrimitive::Rectangle { plane, .. } => SplinePrimitive::Rectangle { width: f32::NAN, height: -1e30, corner: f32::NAN, plane },
            SplinePrimitive::Star { plane, .. } => SplinePrimitive::Star { points: u32::MAX, inner: f32::NAN, outer: f32::NEG_INFINITY, plane },
            SplinePrimitive::NSide { plane, .. } => SplinePrimitive::NSide { sides: 0, radius: f32::NAN, plane },
            SplinePrimitive::Arc { plane, .. } => SplinePrimitive::Arc { radius: f32::NAN, start: f32::NAN, end: f32::INFINITY, close: ArcClose::Pie, plane },
            SplinePrimitive::Helix { plane, .. } => {
                SplinePrimitive::Helix { radius_start: f32::NAN, radius_end: 1e30, height: f32::NAN, turns: f32::INFINITY, knots_per_turn: u32::MAX, plane }
            }
        };
        let s = p.build();
        assert!(s.validate().is_ok(), "{k}");
        assert!(s.knot_count() <= 8_192 + 1, "{k}: {}", s.knot_count());
    }
    let huge = SplinePrimitive::Star { points: u32::MAX, inner: 1.0, outer: 2.0, plane: Plane::Xy }.build();
    assert_eq!(huge.knot_count(), 2 * MAX_CORNERS as usize);
}

proptest! {
    #[test]
    fn any_numbers_build_a_valid_spline(kind in 0usize..6, a in -1e3f32..1e3, b in -1e3f32..1e3, c in -1e3f32..1e3, n in 0u32..5000, plane in 0usize..3) {
        let plane = Plane::ALL[plane];
        let p = match kind {
            0 => SplinePrimitive::Circle { radius_x: a, radius_y: b, plane },
            1 => SplinePrimitive::Rectangle { width: a, height: b, corner: c, plane },
            2 => SplinePrimitive::Star { points: n, inner: a, outer: b, plane },
            3 => SplinePrimitive::NSide { sides: n, radius: a, plane },
            4 => SplinePrimitive::Arc { radius: a, start: b, end: c, close: ArcClose::Chord, plane },
            _ => SplinePrimitive::Helix { radius_start: a, radius_end: b, height: c, turns: (n % 20) as f32, knots_per_turn: n % 100, plane },
        };
        let s = p.build();
        prop_assert!(s.validate().is_ok());
        let json = serde_json::to_string(&p).unwrap();
        prop_assert_eq!(serde_json::from_str::<SplinePrimitive>(&json).unwrap(), p);
        let tess = s.tessellate(Tessellation::Fixed { steps: 4 });
        prop_assert!(tess.is_ok());
    }
}
