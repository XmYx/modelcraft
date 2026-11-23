use std::f32::consts::{PI, TAU};

use glam::{Vec2, Vec3};
use modelcraft_spline::Polyline;
use proptest::prelude::*;

use super::*;
use crate::PolyMesh;
use crate::testutil::{closed, euler};

fn ring(r: f32, n: usize, z: f32) -> Polyline {
    Polyline { points: (0..n).map(|i| Vec3::new(r * (TAU * i as f32 / n as f32).cos(), r * (TAU * i as f32 / n as f32).sin(), z)).collect(), closed: true }
}

fn rect(x0: f32, x1: f32, y0: f32, y1: f32) -> Polyline {
    Polyline { points: vec![Vec3::new(x0, y0, 0.0), Vec3::new(x1, y0, 0.0), Vec3::new(x1, y1, 0.0), Vec3::new(x0, y1, 0.0)], closed: true }
}

fn reversed(p: &Polyline) -> Polyline {
    Polyline { points: p.points.iter().rev().copied().collect(), closed: p.closed }
}

/// Signed volume by the divergence theorem: positive when the faces point outwards.
fn volume(m: &PolyMesh) -> f32 {
    let (tris, _) = m.triangulate();
    tris.iter().map(|t| m.position(t[0]).dot(m.position(t[1]).cross(m.position(t[2]))) / 6.0).sum()
}

fn ngon_area(r: f32, n: usize) -> f32 {
    0.5 * n as f32 * r * r * (TAU / n as f32).sin()
}

fn assert_solid(m: &PolyMesh, volume_want: f32) {
    m.validate().unwrap();
    assert!(closed(m), "every edge is shared by two faces");
    let v = volume(m);
    assert!((v - volume_want).abs() <= volume_want.abs() * 1e-3 + 1e-4, "volume {v}, wanted {volume_want}");
}

// ---------------------------------------------------------------- caps

fn tri_area(pts: &[Vec2], t: [u32; 3]) -> f32 {
    let (a, b, c) = (pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]);
    (b - a).perp_dot(c - a) * 0.5
}

fn flat(contours: &[Vec<Vec3>]) -> (Vec<Vec2>, Vec<&[Vec3]>) {
    let pts = contours.iter().flatten().map(|p| Vec2::new(p.x, p.y)).collect();
    (pts, contours.iter().map(Vec::as_slice).collect())
}

#[test]
fn caps_fill_concave_shapes_and_holes_exactly() {
    // An L, a plain square, and a square with a hole (drawn the wrong way round).
    let l = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(3.0, 0.0, 0.0),
        Vec3::new(3.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(1.0, 3.0, 0.0),
        Vec3::new(0.0, 3.0, 0.0),
    ];
    let (pts, views) = flat(std::slice::from_ref(&l));
    let t = cap::fill(&views, Vec3::Z).unwrap();
    assert_eq!(t.len(), 4);
    assert!((t.iter().map(|t| tri_area(&pts, *t)).sum::<f32>() - 5.0).abs() < 1e-5);
    assert!(t.iter().all(|t| tri_area(&pts, *t) > 0.0), "counter-clockwise about +Z");
    // Seen from below, the same triangles are clockwise.
    let below = cap::fill(&views, -Vec3::Z).unwrap();
    assert!(below.iter().all(|t| tri_area(&pts, *t) < 0.0));
    let outer = rect(0.0, 4.0, 0.0, 4.0).points;
    let hole = reversed(&rect(1.0, 3.0, 1.0, 3.0)).points; // drawn clockwise
    let ring_ = vec![outer.clone(), hole.clone()];
    let (pts, views) = flat(&ring_);
    let t = cap::fill(&views, Vec3::Z).unwrap();
    assert!((t.iter().map(|t| tri_area(&pts, *t)).sum::<f32>() - 12.0).abs() < 1e-4, "16 minus the hole's 4");
    // The hole drawn counter-clockwise like the outline gives the same fill.
    let ring2 = vec![outer, rect(1.0, 3.0, 1.0, 3.0).points];
    let (pts2, views2) = flat(&ring2);
    let t2 = cap::fill(&views2, Vec3::Z).unwrap();
    assert!((t2.iter().map(|t| tri_area(&pts2, *t)).sum::<f32>() - 12.0).abs() < 1e-4);
    assert!(t2.iter().all(|t| tri_area(&pts2, *t) > -1e-6));
    // An island inside a hole is filled again; a hole off to the side of the outline is not a hole.
    let nested = vec![rect(0.0, 10.0, 0.0, 10.0).points, rect(1.0, 9.0, 1.0, 9.0).points, rect(3.0, 7.0, 3.0, 7.0).points];
    let (p3, v3) = flat(&nested);
    let t3 = cap::fill(&v3, Vec3::Z).unwrap();
    assert!((t3.iter().map(|t| tri_area(&p3, *t)).sum::<f32>() - (100.0 - 64.0 + 16.0)).abs() < 1e-3);
}

#[test]
fn caps_with_many_holes_and_hostile_input() {
    // A grid of holes in a plate.
    let mut shapes = vec![rect(0.0, 10.0, 0.0, 10.0).points];
    for i in 0..4 {
        for j in 0..4 {
            let (x, y) = (1.0 + 2.2 * i as f32, 1.0 + 2.2 * j as f32);
            shapes.push(rect(x, x + 1.0, y, y + 1.0).points);
        }
    }
    let (pts, views) = flat(&shapes);
    let t = cap::fill(&views, Vec3::Z).unwrap();
    assert!((t.iter().map(|t| tri_area(&pts, *t)).sum::<f32>() - (100.0 - 16.0)).abs() < 1e-3);
    // Too many points or outlines is an error, not a stall.
    let big: Vec<Vec3> = (0..cap::MAX_CAP_POINTS + 1).map(|i| Vec3::new(i as f32, (i * i % 7) as f32, 0.0)).collect();
    assert!(cap::fill(&[&big], Vec3::Z).is_err());
    let tiny: Vec<Vec<Vec3>> = (0..cap::MAX_CAP_CONTOURS + 1).map(|i| rect(i as f32 * 2.0, i as f32 * 2.0 + 1.0, 0.0, 1.0).points).collect();
    assert!(cap::fill(&tiny.iter().map(Vec::as_slice).collect::<Vec<_>>(), Vec3::Z).is_err());
    // Nonsense numbers and a zero normal come back with something or an error, quickly.
    let nan = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(f32::NAN, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(1.0, 0.0, 0.0)];
    let _ = cap::fill(&[&nan], Vec3::Z);
    let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
    assert!(cap::fill(&[&line], Vec3::Z).unwrap().is_empty(), "no area, no triangles");
    let _ = cap::fill(&[&line], Vec3::ZERO);
}

proptest! {
    #[test]
    fn a_star_shaped_outline_fills_to_its_own_area(radii in prop::collection::vec(0.2f32..3.0, 3..40)) {
        let n = radii.len();
        let pts: Vec<Vec3> = radii.iter().enumerate().map(|(i, r)| { let a = TAU * i as f32 / n as f32; Vec3::new(r * a.cos(), r * a.sin(), 0.0) }).collect();
        let area: f32 = (0..n).map(|i| (pts[i].x * pts[(i + 1) % n].y - pts[(i + 1) % n].x * pts[i].y) * 0.5).sum();
        let (p2, views) = flat(std::slice::from_ref(&pts));
        let t = cap::fill(&views, Vec3::Z).unwrap();
        prop_assert!((t.iter().map(|t| tri_area(&p2, *t)).sum::<f32>() - area).abs() < 1e-3 * area.max(1.0));
        prop_assert!(t.iter().all(|t| tri_area(&p2, *t) > -1e-5));
        prop_assert_eq!(t.len(), n - 2);
    }
}

// ---------------------------------------------------------------- extrude

#[test]
fn extruding_a_circle_makes_a_closed_capped_cylinder_whichever_way_it_was_drawn() {
    let c = ring(1.0, 64, 0.0);
    for curve in [c.clone(), reversed(&c)] {
        let m = extrude(&[curve], &ExtrudeOptions { distance: 2.0, ..Default::default() }).unwrap();
        assert_solid(&m, ngon_area(1.0, 64) * 2.0);
        assert_eq!(m.face_count(), 64 + 62 * 2, "sides and two caps of 62 triangles");
        let (lo, hi) = m.bounds();
        assert!((hi.z - 2.0).abs() < 1e-5 && lo.z.abs() < 1e-5, "extruded along the circle's normal");
        assert!(m.has_uvs());
    }
    // Steps divide the sides; no caps leaves it open.
    let m = extrude(std::slice::from_ref(&c), &ExtrudeOptions { distance: 1.0, steps: 3, cap_start: false, cap_end: false, ..Default::default() }).unwrap();
    assert_eq!(m.face_count(), 64 * 3);
    assert!(!closed(&m));
    let one = extrude(std::slice::from_ref(&c), &ExtrudeOptions { cap_start: false, ..Default::default() }).unwrap();
    assert_eq!(one.face_count(), 64 + 62);
    // Negative distance goes the other way and is still a solid.
    let neg = extrude(std::slice::from_ref(&c), &ExtrudeOptions { distance: -1.5, ..Default::default() }).unwrap();
    assert_solid(&neg, ngon_area(1.0, 64) * 1.5);
    let (lo, hi) = neg.bounds();
    assert!(lo.z < -1.49 && hi.z.abs() < 1e-5);
}

#[test]
fn the_extrusion_axis_is_the_plane_normal_or_a_chosen_axis() {
    // A circle standing in the XZ plane extrudes along Y by itself.
    let c = Polyline { points: ring(1.0, 32, 0.0).points.iter().map(|p| Vec3::new(p.x, 0.0, p.y)).collect(), closed: true };
    let m = extrude(std::slice::from_ref(&c), &ExtrudeOptions { distance: 2.0, ..Default::default() }).unwrap();
    m.validate().unwrap();
    assert!(closed(&m) && volume(&m).abs() > 1.0);
    let (lo, hi) = m.bounds();
    assert!((hi.y - lo.y - 2.0).abs() < 1e-4, "{lo:?} {hi:?}");
    // The same circle pushed along X instead is a tube across its own plane: still closed.
    let x = extrude(&[c], &ExtrudeOptions { distance: 3.0, axis: Axis::X, ..Default::default() }).unwrap();
    let (lo, hi) = x.bounds();
    assert!((hi.x - lo.x - 3.0 - 2.0).abs() < 1e-3 || (hi.x - lo.x) > 3.0, "{lo:?} {hi:?}");
}

#[test]
fn nested_outlines_extrude_with_a_hole_through() {
    let plate = extrude(&[rect(0.0, 4.0, 0.0, 4.0), rect(1.0, 3.0, 1.0, 3.0)], &ExtrudeOptions { distance: 0.5, ..Default::default() }).unwrap();
    assert_solid(&plate, 12.0 * 0.5);
    assert_eq!(euler(&plate), 0, "a plate with a hole is a torus");
    // Drawn the other way round, or in the other order: the same.
    let again =
        extrude(&[reversed(&rect(1.0, 3.0, 1.0, 3.0)), reversed(&rect(0.0, 4.0, 0.0, 4.0))], &ExtrudeOptions { distance: 0.5, ..Default::default() }).unwrap();
    assert_solid(&again, 6.0);
}

#[test]
fn an_open_curve_extrudes_to_a_ribbon_without_caps() {
    let c = Polyline { points: vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)], closed: false };
    let m = extrude(&[c], &ExtrudeOptions { distance: 1.0, steps: 2, axis: Axis::Z, ..Default::default() }).unwrap();
    assert_eq!(m.face_count(), 4);
    assert!(!closed(&m));
}

#[test]
fn extrusions_refuse_nonsense_instead_of_building_it() {
    let c = ring(1.0, 8, 0.0);
    assert!(extrude(&[], &ExtrudeOptions::default()).is_err());
    assert!(extrude(&[Polyline { points: vec![Vec3::ZERO], closed: false }], &ExtrudeOptions::default()).is_err());
    assert!(extrude(&[Polyline { points: vec![Vec3::ZERO, Vec3::NAN, Vec3::X], closed: true }], &ExtrudeOptions::default()).is_err());
    assert!(extrude(std::slice::from_ref(&c), &ExtrudeOptions { distance: 0.0, ..Default::default() }).is_err());
    // NaN and huge numbers are put in range.
    let m = extrude(std::slice::from_ref(&c), &ExtrudeOptions { distance: f32::NAN, steps: u32::MAX, ..Default::default() }).unwrap();
    assert_eq!(m.face_count(), 8 * MAX_STEPS as usize + 6 * 2);
    // The face count is checked before anything is allocated.
    let fine = Polyline {
        points: (0..100_000).map(|i| Vec3::new((i as f32 * 0.001).cos(), (i as f32 * 0.001).sin(), 0.0) * (1.0 + i as f32 * 1e-6)).collect(),
        closed: true,
    };
    assert!(matches!(extrude(&[fine], &ExtrudeOptions { steps: MAX_STEPS, ..Default::default() }), Err(OpError::TooLarge(_))));
    // Too many input points.
    let many = Polyline { points: vec![Vec3::ZERO; MAX_INPUT_POINTS + 1], closed: false };
    assert!(extrude(&[many], &ExtrudeOptions::default()).is_err());
    // Repeated points are dropped, not turned into degenerate faces.
    let dup = Polyline { points: vec![Vec3::ZERO, Vec3::ZERO, Vec3::X, Vec3::X, Vec3::Y, Vec3::Y, Vec3::ZERO], closed: true };
    let m = extrude(&[dup], &ExtrudeOptions::default()).unwrap();
    assert_eq!(m.face_count(), 3 + 2);
}

// ---------------------------------------------------------------- revolve

#[test]
fn revolving_a_rectangle_makes_a_ring_with_a_hole() {
    let m = revolve(&[rect(1.0, 2.0, 0.0, 1.0)], &RevolveOptions::default()).unwrap();
    assert_solid(&m, (ngon_area(2.0, 32) - ngon_area(1.0, 32)) * 1.0);
    assert_eq!(euler(&m), 0, "a torus");
    assert_eq!(m.face_count(), 32 * 4);
    // Drawn the other way round: the same solid.
    let r = revolve(&[reversed(&rect(1.0, 2.0, 0.0, 1.0))], &RevolveOptions::default()).unwrap();
    assert_solid(&r, (ngon_area(2.0, 32) - ngon_area(1.0, 32)) * 1.0);
    // The other way round the axis, too.
    let neg = revolve(&[rect(1.0, 2.0, 0.0, 1.0)], &RevolveOptions { angle: -360.0, ..Default::default() }).unwrap();
    assert_solid(&neg, (ngon_area(2.0, 32) - ngon_area(1.0, 32)) * 1.0);
}

#[test]
fn points_on_the_axis_are_shared_so_a_cylinder_has_no_degenerate_faces() {
    let m = revolve(&[rect(0.0, 1.5, 0.0, 2.0)], &RevolveOptions { steps: 24, ..Default::default() }).unwrap();
    assert_solid(&m, ngon_area(1.5, 24) * 2.0);
    // Two triangles-with-the-axis per step (top and bottom) and two side faces.
    assert_eq!(m.face_count(), 24 * 3, "{}", m.face_count());
    assert_eq!(euler(&m), 2, "a sphere topologically");
    assert!((0..m.face_count() as u32).all(|f| m.face_len(f) >= 3));
}

#[test]
fn a_partial_turn_is_capped_and_a_cap_free_one_is_open() {
    let steps = 8;
    let m = revolve(&[rect(0.0, 2.0, 0.0, 1.0)], &RevolveOptions { angle: 90.0, steps, ..Default::default() }).unwrap();
    // A wedge: steps triangles of the disc, each with area r^2 sin(angle)/2, times the height.
    let want = steps as f32 * 0.5 * 4.0 * (PI / 2.0 / steps as f32).sin() * 1.0;
    assert_solid(&m, want);
    let open = revolve(&[rect(0.0, 2.0, 0.0, 1.0)], &RevolveOptions { angle: 90.0, steps, caps: false, ..Default::default() }).unwrap();
    assert!(!closed(&open));
    // The profile's own side: an open profile revolves to a surface (a cone).
    let cone = revolve(
        &[Polyline { points: vec![Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 0.0, 0.0)], closed: false }],
        &RevolveOptions { steps: 16, ..Default::default() },
    )
    .unwrap();
    assert_eq!(cone.face_count(), 16);
    cone.validate().unwrap();
    assert!(!closed(&cone));
}

#[test]
fn the_axis_can_be_x_or_z_and_zero_angles_are_refused() {
    // A profile in the XY plane about X.
    let m = revolve(&[rect(0.0, 2.0, 0.5, 1.0)], &RevolveOptions { axis: Axis::X, ..Default::default() }).unwrap();
    assert!(closed(&m));
    let (lo, hi) = m.bounds();
    assert!(hi.y > 0.9 && hi.z > 0.9 && lo.y < -0.9 && lo.z < -0.9 && (hi.x - 2.0).abs() < 1e-5, "{lo:?} {hi:?}");
    assert!(volume(&m) > 0.0);
    assert!(revolve(&[rect(1.0, 2.0, 0.0, 1.0)], &RevolveOptions { angle: 0.0, ..Default::default() }).is_err());
    assert!(revolve(&[], &RevolveOptions::default()).is_err());
    // Everything on the axis: nothing to build.
    let flat_ = Polyline { points: vec![Vec3::ZERO, Vec3::Y, Vec3::new(0.0, 2.0, 0.0)], closed: false };
    let r = revolve(&[flat_], &RevolveOptions::default());
    assert!(r.is_err() || r.unwrap().face_count() == 0);
    assert!(revolve(&[rect(1.0, 2.0, 0.0, 1.0)], &RevolveOptions { angle: f32::NAN, steps: 0, ..Default::default() }).is_ok());
}

// ---------------------------------------------------------------- loft

#[test]
fn a_loft_between_two_circles_is_a_frustum() {
    let (r0, r1, h, n) = (1.0f32, 2.0f32, 3.0f32, 32usize);
    let m = loft(&[ring(r0, n, 0.0), ring(r1, n, h)], &LoftOptions { steps: 1, ..Default::default() }).unwrap();
    let (a0, a1) = (ngon_area(r0, n), ngon_area(r1, n));
    assert_solid(&m, h / 3.0 * (a0 + a1 + (a0 * a1).sqrt()));
    // Sections drawn the other way, or in the other order, give the same solid.
    let r = loft(&[reversed(&ring(r1, n, h)), reversed(&ring(r0, n, 0.0))], &LoftOptions { steps: 1, ..Default::default() }).unwrap();
    assert_solid(&r, h / 3.0 * (a0 + a1 + (a0 * a1).sqrt()));
    // Rings between sections and no caps.
    let open = loft(&[ring(r0, n, 0.0), ring(r1, n, h)], &LoftOptions { steps: 4, caps: false, ..Default::default() }).unwrap();
    assert_eq!(open.face_count(), 4 * n);
    assert!(!closed(&open));
}

#[test]
fn sections_with_different_point_counts_and_start_points_still_line_up() {
    // A square to a circle: resampled to the same number of points.
    let m = loft(&[rect(-1.0, 1.0, -1.0, 1.0), ring(1.0, 16, 2.0)], &LoftOptions { steps: 2, ..Default::default() }).unwrap();
    m.validate().unwrap();
    assert!(closed(&m) && volume(&m) > 0.0);
    // Rotating where a section starts does not twist the loft.
    let a = ring(1.0, 24, 0.0);
    let mut b = ring(1.0, 24, 2.0);
    let plain = loft(&[a.clone(), b.clone()], &LoftOptions { steps: 1, ..Default::default() }).unwrap();
    b.points.rotate_left(7);
    let shifted = loft(&[a, b], &LoftOptions { steps: 1, ..Default::default() }).unwrap();
    assert!((volume(&plain) - volume(&shifted)).abs() < 1e-3, "{} vs {}", volume(&plain), volume(&shifted));
}

#[test]
fn a_smooth_loft_through_three_sections_bulges_where_the_middle_does() {
    let sections = [ring(1.0, 24, 0.0), ring(2.0, 24, 2.0), ring(1.0, 24, 4.0)];
    let smooth = loft(&sections, &LoftOptions { steps: 8, smooth: true, ..Default::default() }).unwrap();
    let straight = loft(&sections, &LoftOptions { steps: 8, smooth: false, ..Default::default() }).unwrap();
    for m in [&smooth, &straight] {
        m.validate().unwrap();
        assert!(closed(m) && volume(m) > 0.0);
    }
    // Both pass through the middle section; the smooth one is rounder there (wider between).
    let width_at = |m: &PolyMesh, z: f32| m.positions().iter().filter(|p| (p.z - z).abs() < 1e-4).map(|p| p.x).fold(f32::MIN, f32::max);
    assert!((width_at(&smooth, 2.0) - 2.0).abs() < 1e-3 && (width_at(&straight, 2.0) - 2.0).abs() < 1e-3);
    assert!(volume(&smooth) > volume(&straight) * 0.95);
}

#[test]
fn lofts_check_their_sections() {
    assert!(loft(&[ring(1.0, 8, 0.0)], &LoftOptions::default()).is_err());
    let open = Polyline { points: vec![Vec3::ZERO, Vec3::X], closed: false };
    assert!(loft(&[ring(1.0, 8, 0.0), open.clone()], &LoftOptions::default()).is_err(), "closed and open do not mix");
    assert!(loft(&[ring(1.0, 8, 0.0), Polyline { points: vec![Vec3::NAN; 4], closed: true }], &LoftOptions::default()).is_err());
    // Open sections make a surface.
    let a = Polyline { points: vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0)], closed: false };
    let b = Polyline { points: a.points.iter().map(|p| *p + Vec3::new(0.0, 0.0, 3.0)).collect(), closed: false };
    let m = loft(&[a, b], &LoftOptions { steps: 2, ..Default::default() }).unwrap();
    assert_eq!(m.face_count(), 2 * 2);
    let many: Vec<Polyline> = (0..loft::MAX_SECTIONS + 1).map(|i| ring(1.0, 6, i as f32)).collect();
    assert!(loft(&many, &LoftOptions::default()).is_err());
    assert!(matches!(loft(&[ring(1.0, 4096, 0.0), ring(1.0, 4096, 1.0)], &LoftOptions { steps: MAX_STEPS, ..Default::default() }), Err(OpError::TooLarge(_))));
}

// ---------------------------------------------------------------- sweep

fn line_path(points: &[Vec3]) -> Polyline {
    Polyline { points: points.to_vec(), closed: false }
}

#[test]
fn sweeping_a_circle_along_a_line_is_a_cylinder_in_any_direction() {
    for dir in [Vec3::Z, Vec3::Y, Vec3::new(1.0, 2.0, 3.0).normalize(), -Vec3::Z] {
        let path = line_path(&[Vec3::ZERO, dir * 2.0, dir * 4.0]);
        let m = sweep(&[ring(0.5, 24, 0.0)], &path, &SweepOptions::default()).unwrap();
        assert_solid(&m, ngon_area(0.5, 24) * 4.0);
    }
}

#[test]
fn a_sweep_keeps_its_profile_size_along_a_helix() {
    // 4 turns of a helix of radius 3 rising 8, with 40 points per turn; a circle of radius 0.4.
    let (turns, per, radius, rise) = (4usize, 40usize, 3.0f32, 8.0f32);
    let n = turns * per + 1;
    let path = line_path(
        &(0..n)
            .map(|i| {
                let t = i as f32 / (n - 1) as f32;
                let a = t * turns as f32 * TAU;
                Vec3::new(radius * a.cos(), radius * a.sin(), rise * t)
            })
            .collect::<Vec<_>>(),
    );
    let profile = 36usize;
    // A circle at the origin (its origin rides the path): in the plane across the start direction.
    let t0 = (path.points[1] - path.points[0]).normalize();
    let u = t0.any_orthonormal_vector();
    let v = t0.cross(u);
    let pts: Vec<Vec3> = (0..profile)
        .map(|i| {
            let a = TAU * i as f32 / profile as f32;
            (u * a.cos() + v * a.sin()) * 0.4
        })
        .collect();
    let m = sweep(&[Polyline { points: pts, closed: true }], &path, &SweepOptions { caps: true, ..Default::default() }).unwrap();
    m.validate().unwrap();
    assert!(closed(&m) && volume(&m) > 0.0);
    // Every section is the same size: the opposite points of the profile are 0.8 apart at every step.
    let sections = path.points.len();
    let mut worst = 0.0f32;
    for s in 0..sections {
        for j in 0..profile / 2 {
            let (a, b) = (m.position((s * profile + j) as u32), m.position((s * profile + j + profile / 2) as u32));
            worst = worst.max((a.distance(b) - 0.8).abs());
        }
    }
    assert!(worst < 1e-4, "the profile changed size by {worst} along the helix");
    // Each section is perpendicular to the path: its points lie in the plane across the tangent.
    for s in 1..sections - 1 {
        let centre = (0..profile).map(|j| m.position((s * profile + j) as u32)).sum::<Vec3>() / profile as f32;
        assert!(centre.distance(path.points[s]) < 1e-4);
        let t = (path.points[s + 1] - path.points[s - 1]).normalize();
        let off = (0..profile).map(|j| (m.position((s * profile + j) as u32) - centre).dot(t).abs()).fold(0.0f32, f32::max);
        assert!(off < 0.02, "section {s} leans {off} out of its plane");
    }
    // The volume is the profile's area times the path's length (a thin tube).
    let len = path.points.windows(2).map(|w| w[0].distance(w[1])).sum::<f32>();
    let want = ngon_area(0.4, profile) * len;
    assert!((volume(&m) - want).abs() < want * 0.03, "{} vs {want}", volume(&m));
}

#[test]
fn a_closed_path_sweeps_to_a_closed_ring_and_the_end_scale_and_twist_apply() {
    // A circle swept around a circle: a torus.
    let (big, small, n, p) = (3.0f32, 0.5f32, 48usize, 16usize);
    let path = Polyline {
        points: (0..n)
            .map(|i| {
                let a = TAU * i as f32 / n as f32;
                Vec3::new(big * a.cos(), big * a.sin(), 0.0)
            })
            .collect(),
        closed: true,
    };
    let t0 = (path.points[1] - path.points[n - 1]).normalize();
    let u = t0.any_orthonormal_vector();
    let v = t0.cross(u);
    let profile = Polyline {
        points: (0..p)
            .map(|i| {
                let a = TAU * i as f32 / p as f32;
                (u * a.cos() + v * a.sin()) * small
            })
            .collect(),
        closed: true,
    };
    let m = sweep(&[profile], &path, &SweepOptions::default()).unwrap();
    m.validate().unwrap();
    assert!(closed(&m));
    assert_eq!(euler(&m), 0);
    let want = ngon_area(small, p) * TAU * big;
    assert!((volume(&m) - want).abs() < want * 0.03, "{} vs {want}", volume(&m));
    // Along a line: the end scale makes a cone-like taper, twist turns a square.
    let straight = line_path(&[Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0), Vec3::new(0.0, 0.0, 4.0)]);
    let sq = rect(-1.0, 1.0, -1.0, 1.0);
    let tapered = sweep(std::slice::from_ref(&sq), &straight, &SweepOptions { end_scale: 0.5, ..Default::default() }).unwrap();
    let top: Vec<Vec3> = tapered.positions().iter().copied().filter(|q| (q.z - 4.0).abs() < 1e-4).collect();
    assert!(top.iter().all(|q| (q.x.abs() - 0.5).abs() < 1e-4 && (q.y.abs() - 0.5).abs() < 1e-4), "{top:?}");
    // A square turned 45 degrees by the top has its corners on the axes.
    let twisted = sweep(&[sq], &straight, &SweepOptions { twist: 45.0, ..Default::default() }).unwrap();
    twisted.validate().unwrap();
    assert!(closed(&twisted) && volume(&twisted) > 0.0);
    let top: Vec<Vec3> = twisted.positions().iter().copied().filter(|q| (q.z - 4.0).abs() < 1e-4).collect();
    assert!(!top.is_empty());
    assert!(top.iter().all(|q| (q.truncate().length() - 2f32.sqrt()).abs() < 1e-4 && (q.x.abs() < 1e-3 || q.y.abs() < 1e-3)), "{top:?}");
}

#[test]
fn a_profile_with_a_hole_sweeps_to_a_tube_and_hostile_sweeps_are_errors() {
    let tube =
        sweep(&[ring(1.0, 24, 0.0), reversed(&ring(0.6, 24, 0.0))], &line_path(&[Vec3::ZERO, Vec3::new(0.0, 0.0, 3.0)]), &SweepOptions::default()).unwrap();
    assert_solid(&tube, (ngon_area(1.0, 24) - ngon_area(0.6, 24)) * 3.0);
    assert_eq!(euler(&tube), 0);
    let path = line_path(&[Vec3::ZERO, Vec3::Z]);
    assert!(sweep(&[], &path, &SweepOptions::default()).is_err());
    assert!(sweep(&[ring(1.0, 8, 0.0)], &line_path(&[Vec3::ZERO]), &SweepOptions::default()).is_err());
    assert!(sweep(&[ring(1.0, 8, 0.0)], &line_path(&[Vec3::ZERO, Vec3::ZERO]), &SweepOptions::default()).is_err());
    assert!(sweep(&[ring(1.0, 8, 0.0)], &line_path(&[Vec3::ZERO, Vec3::NAN]), &SweepOptions::default()).is_err());
    let r = sweep(&[ring(1.0, 8, 0.0)], &path, &SweepOptions { end_scale: f32::NAN, twist: f32::INFINITY, roll: f32::NAN, caps: true });
    assert!(r.is_ok());
    // A path that doubles back on itself (a 180 degree turn) still builds.
    let back = line_path(&[Vec3::ZERO, Vec3::Z, Vec3::ZERO + Vec3::Z * 0.5]);
    sweep(&[ring(0.2, 8, 0.0)], &back, &SweepOptions::default()).unwrap().validate().unwrap();
    // The size of the result is checked before it is built.
    let long = line_path(&(0..100_000).map(|i| Vec3::new(i as f32 * 0.01, (i as f32 * 0.01).sin(), 0.0)).collect::<Vec<_>>());
    assert!(matches!(sweep(&[ring(1.0, 200, 0.0)], &long, &SweepOptions::default()), Err(OpError::TooLarge(_))));
}

proptest! {
    #[test]
    fn no_generator_panics_or_builds_a_broken_mesh(
        radii in prop::collection::vec(0.1f32..4.0, 3..16),
        d in 0.1f32..5.0,
        angle in -360.0f32..360.0,
        steps in 1u32..9,
        wob in -1.0f32..1.0,
    ) {
        let n = radii.len();
        let profile = Polyline { points: radii.iter().enumerate().map(|(i, r)| { let a = TAU * i as f32 / n as f32; Vec3::new(r * a.cos() + 6.0, r * a.sin(), 0.0) }).collect(), closed: true };
        let open = Polyline { points: radii.iter().enumerate().map(|(i, r)| Vec3::new(r + 0.5, i as f32, 0.0)).collect(), closed: false };
        if let Ok(m) = extrude(std::slice::from_ref(&profile), &ExtrudeOptions { distance: d, steps, ..Default::default() }) {
            prop_assert!(m.validate().is_ok());
            prop_assert!(closed(&m));
            prop_assert!(volume(&m) > 0.0);
        }
        if let Ok(m) = revolve(std::slice::from_ref(&open), &RevolveOptions { angle, steps, ..Default::default() }) {
            prop_assert!(m.validate().is_ok());
        }
        if let Ok(m) = revolve(std::slice::from_ref(&profile), &RevolveOptions { angle, steps, ..Default::default() }) {
            prop_assert!(m.validate().is_ok());
            // A step that turns more than about 120 degrees folds the wedge over itself.
            if angle.abs() > 0.5 && angle.abs() / steps as f32 <= 120.0 {
                prop_assert!(volume(&m) > 0.0, "angle {angle}");
            }
        }
        let path = line_path(&[Vec3::new(0.0, 0.0, 0.0), Vec3::new(wob, 1.0, 0.0), Vec3::new(2.0 * wob, 2.0, 1.0), Vec3::new(wob, 3.0, 2.0)]);
        if let Ok(m) = sweep(std::slice::from_ref(&profile), &path, &SweepOptions { twist: angle, ..Default::default() }) {
            prop_assert!(m.validate().is_ok());
        }
        let other = Polyline { points: profile.points.iter().map(|p| *p * 1.5 + Vec3::new(0.0, 0.0, d)).collect(), closed: true };
        if let Ok(m) = loft(&[profile, other], &LoftOptions { steps, ..Default::default() }) {
            prop_assert!(m.validate().is_ok());
        }
    }
}
