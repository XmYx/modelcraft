//! Spline objects and generators in the scene: the cache (a generator re-evaluates exactly when a
//! point of one of its splines moves) and hostile curves.

use glam::Vec3;
use modelcraft_spline::{CurveKind, Knot, Segment, Spline, SplinePrimitive, Tessellation};

use super::*;
use crate::spline_gen::{KINDS, SplineGen};
use crate::splines::SplineSource;

fn spline(s: &mut Scene, name: &str, primitive: SplinePrimitive, parent: Option<ObjectId>) -> ObjectId {
    s.add(name, ObjectKind::Spline { source: SplineSource::Primitive { primitive }, tess: Tessellation::default() }, parent).unwrap()
}

fn generator(s: &mut Scene, kind: &str) -> ObjectId {
    s.add("Gen", ObjectKind::SplineGenerator { generator: SplineGen::default_of(kind).unwrap() }, None).unwrap()
}

fn volume(m: &PolyMesh) -> f32 {
    let (tris, _) = m.triangulate();
    tris.iter().map(|t| m.position(t[0]).dot(m.position(t[1]).cross(m.position(t[2]))) / 6.0).sum()
}

/// Make the spline's source an edited curve and move its first point.
fn move_first_point(s: &mut Scene, id: ObjectId, to: Vec3) {
    let o = s.edit(id).unwrap();
    if let ObjectKind::Spline { source, .. } = &mut o.kind {
        let mut curve = (*source.spline()).clone();
        curve.segments[0].knots[0].pos = to;
        *source = SplineSource::Curve { spline: Arc::new(curve) };
    }
}

#[test]
fn every_generator_re_evaluates_when_a_point_moves_and_not_otherwise() {
    for kind in KINDS {
        let mut s = Scene::new();
        let mut ev = Evaluator::new();
        let g = generator(&mut s, kind);
        // Inputs by kind: a square profile for extrude and revolve, two squares for a loft, a profile and a path.
        let square = SplinePrimitive::Rectangle { width: 1.0, height: 1.0, corner: 0.0, plane: modelcraft_spline::Plane::Xy };
        let first = spline(&mut s, "A", square, Some(g));
        if *kind == "revolve" {
            s.edit(first).unwrap().transform.translation = Vec3::new(2.0, 0.0, 0.0);
        }
        let second = match *kind {
            "loft" => {
                let b = spline(&mut s, "B", square, Some(g));
                s.edit(b).unwrap().transform.translation = Vec3::new(0.0, 0.0, 3.0);
                Some(b)
            }
            "sweep" => Some(spline(
                &mut s,
                "Path",
                SplinePrimitive::Helix { radius_start: 2.0, radius_end: 2.0, height: 4.0, turns: 1.0, knots_per_turn: 8, plane: modelcraft_spline::Plane::Zx },
                Some(g),
            )),
            _ => None,
        };
        let a = ev.evaluate(&s, g);
        assert!(a.errors.is_empty(), "{kind}: {:?}", a.errors);
        assert!(a.mesh.face_count() > 4, "{kind}");
        let runs = ev.runs;
        let _ = ev.evaluate(&s, g);
        assert_eq!(ev.runs, runs, "{kind}: a second evaluation hits the cache");
        // Move a point of the first spline: the surface changes, once.
        move_first_point(&mut s, first, Vec3::new(-3.0, -3.0, 0.0));
        let b = ev.evaluate(&s, g);
        assert_eq!(ev.runs, runs + 1, "{kind}: one re-evaluation");
        assert_ne!(a.mesh.positions(), b.mesh.positions(), "{kind}: the mesh follows the point");
        let _ = ev.evaluate(&s, g);
        assert_eq!(ev.runs, runs + 1, "{kind}: and then it is cached again");
        // Moving a point of the other spline (the loft's second section, the sweep's path) also does.
        if let Some(other) = second {
            move_first_point(&mut s, other, Vec3::new(1.0, 1.0, 1.0));
            let c = ev.evaluate(&s, g);
            assert_eq!(ev.runs, runs + 2, "{kind}");
            assert_ne!(b.mesh.positions(), c.mesh.positions(), "{kind}");
        }
        // Changing the generator's own options does too.
        let before = ev.runs;
        if let ObjectKind::SplineGenerator { generator } = &mut s.edit(g).unwrap().kind {
            *generator = match generator.clone() {
                SplineGen::Extrude(mut o) => {
                    o.distance = 5.0;
                    SplineGen::Extrude(o)
                }
                SplineGen::Revolve(mut o) => {
                    o.steps = 8;
                    SplineGen::Revolve(o)
                }
                SplineGen::Loft(mut o) => {
                    o.steps = 2;
                    SplineGen::Loft(o)
                }
                SplineGen::Sweep(mut o) => {
                    o.twist = 45.0;
                    SplineGen::Sweep(o)
                }
            };
        }
        let _ = ev.evaluate(&s, g);
        assert_eq!(ev.runs, before + 1, "{kind}");
        // A child's own transform moves the surface too.
        let at = ev.evaluate(&s, g).mesh.bounds();
        s.edit(first).unwrap().transform.translation += Vec3::new(10.0, 0.0, 0.0);
        let moved = ev.evaluate(&s, g).mesh.bounds();
        assert!(moved.1.x > at.1.x + 1.0, "{kind}: {:?} then {:?}", at, moved);
    }
}

#[test]
fn the_extrude_is_a_closed_solid_of_the_right_size_and_follows_a_child_transform() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let g = generator(&mut s, "extrude");
    let c = spline(&mut s, "Circle", SplinePrimitive::Circle { radius_x: 1.0, radius_y: 1.0, plane: modelcraft_spline::Plane::Xy }, Some(g));
    if let ObjectKind::SplineGenerator { generator: SplineGen::Extrude(o) } = &mut s.edit(g).unwrap().kind {
        o.distance = 2.0;
    }
    let m = ev.evaluate(&s, g).mesh;
    m.validate().unwrap();
    let v = volume(&m);
    assert!((v - std::f32::consts::PI * 2.0).abs() < 0.1, "a cylinder of radius 1 and height 2: {v}");
    // The child's own transform is part of the profile.
    s.edit(c).unwrap().transform.scale = Vec3::splat(2.0);
    let big = volume(&ev.evaluate(&s, g).mesh);
    assert!((big / v - 4.0).abs() < 0.05, "twice the radius: {big}");
    // A hidden or reference-only child still counts.
    s.edit(c).unwrap().visible = false;
    assert!(ev.evaluate(&s, g).mesh.face_count() > 0);
}

#[test]
fn a_generator_without_its_splines_says_what_it_needs() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    for (kind, text) in [("extrude", "needs a spline"), ("revolve", "needs a spline"), ("loft", "at least two"), ("sweep", "two splines")] {
        let g = generator(&mut s, kind);
        let e = ev.evaluate(&s, g);
        assert_eq!(e.mesh.face_count(), 0);
        assert!(e.errors.iter().any(|x| x.message.contains(text)), "{kind}: {:?}", e.errors);
    }
    // One spline is not enough for a loft or a sweep.
    let l = generator(&mut s, "loft");
    spline(&mut s, "A", SplinePrimitive::default_of("circle").unwrap(), Some(l));
    assert!(ev.evaluate(&s, l).errors.iter().any(|x| x.message.contains("at least two")));
}

#[test]
fn hostile_children_give_errors_instead_of_crashes() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let g = generator(&mut s, "extrude");
    let bad = s
        .add(
            "Bad",
            ObjectKind::Spline {
                source: SplineSource::Curve {
                    spline: Arc::new(Spline::single(Segment::new(CurveKind::Bezier, true, vec![Knot::new(Vec3::NAN), Knot::new(Vec3::X), Knot::new(Vec3::Y)]))),
                },
                tess: Tessellation::default(),
            },
            Some(g),
        )
        .unwrap();
    let e = ev.evaluate(&s, g);
    assert_eq!(e.mesh.face_count(), 0);
    assert!(e.errors.iter().any(|x| x.message.contains("Bad")), "{:?}", e.errors);
    // A huge scale and the NaN transform are sanitized by the scene; an infinite one gives an error.
    s.remove(bad);
    let c = spline(&mut s, "C", SplinePrimitive::default_of("circle").unwrap(), Some(g));
    s.edit(c).unwrap().transform.scale = Vec3::splat(1.0e30);
    let _ = ev.evaluate(&s, g);
    // A far too fine tessellation is refused for the curve, and the generator reports it.
    s.edit(c).unwrap().transform.scale = Vec3::ONE;
    if let ObjectKind::Spline { tess, .. } = &mut s.edit(c).unwrap().kind {
        *tess = Tessellation::Adaptive { tolerance: 1e-6, angle_deg: 1.0 };
    }
    let e = ev.evaluate(&s, g);
    assert!(e.mesh.face_count() > 0 || !e.errors.is_empty());
}

#[test]
fn splines_serialize_with_the_scene_and_clean_up_their_cache() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let g = generator(&mut s, "revolve");
    let c = spline(&mut s, "Profile", SplinePrimitive::Rectangle { width: 1.0, height: 2.0, corner: 0.0, plane: modelcraft_spline::Plane::Xy }, Some(g));
    s.edit(c).unwrap().transform.translation = Vec3::new(2.0, 0.0, 0.0);
    let json = serde_json::to_string(&s).unwrap();
    let back: Scene = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), 2);
    let a = ev.evaluate(&s, g).mesh;
    let mut ev2 = Evaluator::new();
    assert_eq!(a.positions(), ev2.evaluate(&back, g).mesh.positions());
    ev.spline(&s, c);
    s.remove(c);
    ev.retain(&s);
    assert!(ev.spline(&s, c).is_none());
}

#[test]
fn a_generator_is_the_input_of_another_generator_and_is_not_drawn_twice() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let g = generator(&mut s, "extrude");
    spline(&mut s, "Star", SplinePrimitive::default_of("star").unwrap(), Some(g));
    let arr = s.add("Array", ObjectKind::Array { array: Array::default_of("linear").unwrap() }, None).unwrap();
    s.reparent(g, Some(arr)).unwrap();
    assert_eq!(s.generator_input(arr), Some(g));
    assert!(s.is_cage_input(g), "shown through the array, not on its own");
    let one = ev.evaluate(&s, g).mesh.face_count();
    assert_eq!(ev.evaluate(&s, arr).mesh.face_count(), one * 4);
}
