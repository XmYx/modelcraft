//! The `spline` command: adding, editing, locking, undo, JSON and project round trips.

use glam::Vec3;
use modelcraft_scene::splines::SplineSource;
use modelcraft_scene::{ObjectKind, Spline, SplinePrimitive, Tessellation};
use modelcraft_spline::{CurveKind, Knot, Segment};

use crate::spline::SplineCommand as S;
use crate::{Command, Document};

fn add(d: &mut Document, kind: &str) -> modelcraft_scene::ObjectId {
    d.execute(Command::Spline(S::Add { kind: kind.into(), parent: None, name: None })).unwrap().created[0]
}

fn lines(d: &mut Document, id: modelcraft_scene::ObjectId) -> usize {
    d.eval.spline(&d.scene, id).map_or(0, |e| e.point_count())
}

#[test]
fn every_primitive_can_be_added_undone_and_redone() {
    let mut d = Document::new();
    for k in modelcraft_spline::primitives::KINDS {
        let id = add(&mut d, k);
        assert!(matches!(d.scene.get(id).unwrap().kind, ObjectKind::Spline { source: SplineSource::Primitive { .. }, .. }));
        assert!(lines(&mut d, id) >= 2, "{k}");
    }
    assert_eq!(d.scene.len(), 6);
    assert_eq!(d.scene.get(modelcraft_scene::ObjectId(1)).map(|o| o.name.as_str()), Some("Circle"));
    assert!(d.scene.objects().iter().any(|o| o.name == "N-Side"));
    let n = d.undo.len();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.len(), 5);
    d.execute(Command::Redo).unwrap();
    assert_eq!((d.scene.len(), d.undo.len()), (6, n));
    assert!(d.execute(Command::Spline(S::Add { kind: "spiral".into(), parent: None, name: None })).is_err());
    assert_eq!(d.scene.len(), 6);
}

#[test]
fn a_primitive_edits_through_set_primitive_and_follows_the_cache() {
    let mut d = Document::new();
    let id = add(&mut d, "circle");
    let before = d.eval.spline(&d.scene, id).unwrap();
    let again = d.eval.spline(&d.scene, id).unwrap();
    assert!(std::sync::Arc::ptr_eq(&before, &again), "cached while nothing changes");
    d.execute(Command::Spline(S::SetPrimitive { id, primitive: SplinePrimitive::Circle { radius_x: 5.0, radius_y: f32::NAN, plane: Default::default() } }))
        .unwrap();
    let after = d.eval.spline(&d.scene, id).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    let (_, hi) = after.spline.bounds().unwrap();
    assert!((hi.x - 5.0).abs() < 0.01 && (hi.y - 1.0).abs() < 0.01, "radius 5 by 1: the NaN became the default");
    // The tessellation mode is sanitized and changes the point count.
    let adaptive = lines(&mut d, id);
    d.execute(Command::Spline(S::SetTessellation { id, tess: Tessellation::Fixed { steps: u32::MAX } })).unwrap();
    assert_eq!(lines(&mut d, id), 4 * 256);
    d.execute(Command::Spline(S::SetTessellation { id, tess: Tessellation::Fixed { steps: 2 } })).unwrap();
    assert_eq!(lines(&mut d, id), 8);
    assert_ne!(adaptive, 8);
    // Wrong object kinds fail without leaving an undo step.
    let n = d.undo.len();
    let cube = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    assert!(d.execute(Command::Spline(S::SetPrimitive { id: cube, primitive: SplinePrimitive::default_of("star").unwrap() })).is_err());
    assert!(d.execute(Command::Spline(S::SetTessellation { id: cube, tess: Tessellation::default() })).is_err());
    assert!(d.execute(Command::Spline(S::MakeEditable { id: cube })).is_err());
    assert!(d.execute(Command::Spline(S::SetCurve { id: cube, spline: Spline::default() })).is_err());
    assert_eq!(d.undo.len(), n + 1, "only the cube was added");
}

#[test]
fn making_a_primitive_editable_keeps_its_shape_and_set_curve_refuses_hostile_curves() {
    let mut d = Document::new();
    let id = add(&mut d, "star");
    let shape = d.eval.spline(&d.scene, id).unwrap().spline.clone();
    d.execute(Command::Spline(S::MakeEditable { id })).unwrap();
    assert!(matches!(d.scene.get(id).unwrap().kind, ObjectKind::Spline { source: SplineSource::Curve { .. }, .. }));
    assert_eq!(*d.eval.spline(&d.scene, id).unwrap().spline, *shape);
    assert!(d.execute(Command::Spline(S::SetPrimitive { id, primitive: SplinePrimitive::default_of("circle").unwrap() })).is_err(), "no longer a primitive");
    // A curve with a NaN, or with too many points, is refused and changes nothing.
    let n = d.undo.len();
    let nan = Spline::single(Segment::new(CurveKind::Linear, false, vec![Knot::new(Vec3::NAN), Knot::new(Vec3::X)]));
    assert!(d.execute(Command::Spline(S::SetCurve { id, spline: nan })).is_err());
    let many = Spline::single(Segment::polyline(&vec![Vec3::ZERO; modelcraft_spline::MAX_KNOTS + 1], false));
    assert!(d.execute(Command::Spline(S::SetCurve { id, spline: many })).is_err());
    assert_eq!(d.undo.len(), n);
    assert_eq!(*d.eval.spline(&d.scene, id).unwrap().spline, *shape);
    // A good one replaces it, as one undo step.
    let line = Spline::single(Segment::polyline(&[Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)], false));
    d.execute(Command::Spline(S::SetCurve { id, spline: line.clone() })).unwrap();
    assert_eq!(*d.eval.spline(&d.scene, id).unwrap().spline, line);
    d.execute(Command::Undo).unwrap();
    assert_eq!(*d.eval.spline(&d.scene, id).unwrap().spline, *shape);
}

#[test]
fn a_spline_with_a_bad_curve_reports_the_error_instead_of_points() {
    // A project file can hold what the command refuses: the evaluator must not crash on it.
    let mut d = Document::new();
    let id = add(&mut d, "empty");
    if let ObjectKind::Spline { source, .. } = &mut d.scene.edit(id).unwrap().kind {
        *source = SplineSource::Curve { spline: std::sync::Arc::new(Spline::single(Segment::polyline(&[Vec3::NAN, Vec3::X], false))) };
    }
    let e = d.eval.spline(&d.scene, id).unwrap();
    assert!(e.error.is_some() && e.lines.is_empty());
    assert!(d.eval.spline(&d.scene, modelcraft_scene::ObjectId(99)).is_none());
}

#[test]
fn locked_splines_refuse_changes() {
    let mut d = Document::new();
    let id = add(&mut d, "circle");
    d.execute(Command::SetLocked { ids: vec![id], locked: true }).unwrap();
    for c in [
        S::SetPrimitive { id, primitive: SplinePrimitive::default_of("star").unwrap() },
        S::SetTessellation { id, tess: Tessellation::default() },
        S::MakeEditable { id },
    ] {
        let e = d.execute(Command::Spline(c)).unwrap_err().to_string();
        assert!(e.contains("locked"), "{e}");
    }
    assert!(d.execute(Command::Spline(S::MakeEditable { id: modelcraft_scene::ObjectId(77) })).is_err());
}

#[test]
fn spline_commands_round_trip_as_json_and_the_registry_names_parse() {
    for c in [
        S::Add { kind: "helix".into(), parent: None, name: Some("Path".into()) },
        S::SetPrimitive { id: modelcraft_scene::ObjectId(2), primitive: SplinePrimitive::default_of("arc").unwrap() },
        S::SetTessellation { id: modelcraft_scene::ObjectId(2), tess: Tessellation::Uniform { length: 0.25 } },
        S::MakeEditable { id: modelcraft_scene::ObjectId(2) },
        S::SetCurve { id: modelcraft_scene::ObjectId(2), spline: SplinePrimitive::default_of("rectangle").unwrap().build() },
    ] {
        let cmd = Command::Spline(c);
        let j = serde_json::to_string(&cmd).unwrap();
        assert!(j.contains(r#""cmd":"spline""#), "{j}");
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), cmd, "{j}");
        assert!(!cmd.label().is_empty());
    }
    let e = serde_json::from_str::<Command>(r#"{"cmd":"spline","op":"wobble"}"#).unwrap_err().to_string();
    assert!(e.contains("unknown variant `wobble`"), "{e}");
    for (name, _) in crate::spline::COMMANDS {
        let e = serde_json::from_str::<S>(&format!(r#"{{"op":"{name}"}}"#)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!e.contains("unknown variant"), "{name}: {e}");
    }
    // The JSON a user would write.
    let j = r#"{"cmd":"spline","op":"add","kind":"circle"}"#;
    let mut d = Document::new();
    d.execute(serde_json::from_str(j).unwrap()).unwrap();
    assert_eq!(d.scene.len(), 1);
}

#[test]
fn splines_survive_saving_and_opening_a_project() {
    let dir = std::env::temp_dir().join(format!("mc-spline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("splines.modelcraft");
    let mut d = Document::new();
    let a = add(&mut d, "helix");
    let b = add(&mut d, "circle");
    d.execute(Command::Spline(S::MakeEditable { id: b })).unwrap();
    d.execute(Command::Spline(S::SetTessellation { id: a, tess: Tessellation::Uniform { length: 0.3 } })).unwrap();
    d.save_as(&path).unwrap();
    let mut e = Document::open(&path).unwrap();
    assert_eq!(e.scene.len(), 2);
    for id in [a, b] {
        assert_eq!(e.scene.get(id).unwrap().kind, d.scene.get(id).unwrap().kind);
        assert_eq!(lines(&mut e, id), lines(&mut d, id));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn curves_are_left_out_of_exports_and_the_scene_still_exports() {
    let dir = std::env::temp_dir().join(format!("mc-spline-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new();
    add(&mut d, "circle");
    d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap();
    assert_eq!(d.export_items().len(), 1, "the cube only");
    let p = dir.join("out.glb");
    d.export(&p).unwrap();
    assert!(p.metadata().unwrap().len() > 100);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn edit_commands_change_points_as_single_undo_steps_and_refuse_bad_ones() {
    use modelcraft_spline::{KnotMove, KnotRef, Side, SplineEdit};
    let mut d = Document::new();
    let id = add(&mut d, "empty");
    let at = |k| KnotRef { segment: 0, knot: k };
    for (i, p) in [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0)].into_iter().enumerate() {
        let before = d.undo.len();
        d.execute(Command::Spline(S::Edit { id, edit: SplineEdit::Append { segment: 0, knot: Knot::new(p), kind: CurveKind::Bezier } })).unwrap();
        assert_eq!((d.undo.len(), d.eval.spline(&d.scene, id).unwrap().spline.knot_count()), (before + 1, i + 1));
    }
    d.execute(Command::Spline(S::Edit { id, edit: SplineEdit::SetHandle { at: at(1), side: Side::Out, offset: Vec3::new(0.0, 0.5, 0.0) } })).unwrap();
    d.execute(Command::Spline(S::Edit { id, edit: SplineEdit::MoveKnots { moves: vec![KnotMove { at: at(2), pos: Vec3::new(5.0, 0.0, 0.0) }] } })).unwrap();
    let s = d.eval.spline(&d.scene, id).unwrap().spline.clone();
    assert_eq!((s.segments[0].knots[1].tan_out, s.segments[0].knots[2].pos), (Vec3::new(0.0, 0.5, 0.0), Vec3::new(5.0, 0.0, 0.0)));
    // A bad edit is an error and no undo step.
    let n = d.undo.len();
    for edit in [
        SplineEdit::Remove { knots: vec![at(9)] },
        SplineEdit::MoveKnots { moves: vec![KnotMove { at: at(0), pos: Vec3::NAN }] },
        SplineEdit::SetClosed { segment: 4, closed: true },
    ] {
        assert!(d.execute(Command::Spline(S::Edit { id, edit })).is_err());
    }
    assert_eq!(d.undo.len(), n);
    // On a primitive the first edit makes it editable; Undo brings the primitive back.
    let circle = add(&mut d, "circle");
    d.execute(Command::Spline(S::Edit { id: circle, edit: SplineEdit::Translate { knots: vec![at(0)], by: Vec3::new(1.0, 0.0, 0.0) } })).unwrap();
    assert!(matches!(d.scene.get(circle).unwrap().kind, ObjectKind::Spline { source: SplineSource::Curve { .. }, .. }));
    d.execute(Command::Undo).unwrap();
    assert!(matches!(d.scene.get(circle).unwrap().kind, ObjectKind::Spline { source: SplineSource::Primitive { .. }, .. }));
    // Locked, and on something that is not a spline.
    d.execute(Command::SetLocked { ids: vec![id], locked: true }).unwrap();
    let e = d.execute(Command::Spline(S::Edit { id, edit: SplineEdit::SetClosed { segment: 0, closed: true } })).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    let cube = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    assert!(d.execute(Command::Spline(S::Edit { id: cube, edit: SplineEdit::SetClosed { segment: 0, closed: true } })).is_err());
    // JSON as a user would write it.
    let mut d2 = Document::new();
    let c2 = add(&mut d2, "empty");
    let j = format!(r#"{{"cmd":"spline","op":"edit","id":{},"edit":{{"edit":"append","segment":0,"knot":{{"pos":[1,2,3]}}}}}}"#, c2.0);
    d2.execute(serde_json::from_str(&j).unwrap()).unwrap();
    assert_eq!(d2.eval.spline(&d2.scene, c2).unwrap().spline.segments[0].knots[0].pos, Vec3::new(1.0, 2.0, 3.0));
}

fn faces(d: &mut Document, id: modelcraft_scene::ObjectId) -> usize {
    d.eval.evaluate(&d.scene, id).mesh.face_count()
}

#[test]
fn every_generator_starts_with_example_splines_and_shows_a_surface() {
    use modelcraft_scene::SplineGen;
    for kind in modelcraft_scene::spline_gen::KINDS {
        let mut d = Document::new();
        let g = d.execute(Command::Spline(S::AddGenerator { kind: (*kind).into(), inputs: vec![] })).unwrap().created[0];
        assert!(matches!(d.scene.get(g).unwrap().kind, ObjectKind::SplineGenerator { .. }), "{kind}");
        assert!(!d.scene.spline_children(g).is_empty(), "{kind}");
        let e = d.eval.evaluate(&d.scene, g);
        assert!(e.errors.is_empty(), "{kind}: {:?}", e.errors);
        assert!(e.mesh.face_count() > 8, "{kind}");
        assert_eq!(d.triangle_count(), e.mesh.triangle_count(), "{kind}: the spline children draw no mesh");
        // One undo removes the generator and its splines together.
        d.execute(Command::Undo).unwrap();
        assert_eq!(d.scene.len(), 0, "{kind}");
        d.execute(Command::Redo).unwrap();
        assert_eq!(faces(&mut d, g), e.mesh.face_count(), "{kind}");
        let _ = SplineGen::default_of(kind);
    }
    assert!(Document::new().execute(Command::Spline(S::AddGenerator { kind: "twist".into(), inputs: vec![] })).is_err());
}

#[test]
fn a_generator_takes_the_listed_splines_inside_it_and_refuses_other_objects() {
    let mut d = Document::new();
    let a = add(&mut d, "circle");
    let b = add(&mut d, "rectangle");
    let g = d.execute(Command::Spline(S::AddGenerator { kind: "loft".into(), inputs: vec![a, b] })).unwrap().created[0];
    assert_eq!(d.scene.spline_children(g), vec![a, b]);
    assert_eq!(d.scene.get(a).unwrap().parent, Some(g));
    // Not splines, or the same spline twice: an error, and no trace.
    let n = d.undo.len();
    let cube = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    let n = n + 1;
    assert!(d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![cube] })).is_err());
    let c = add(&mut d, "star");
    let n = n + 1;
    assert!(d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![c, c] })).is_err());
    assert!(d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![modelcraft_scene::ObjectId(999)] })).is_err());
    assert_eq!(d.undo.len(), n);
    assert_eq!(d.scene.get(c).unwrap().parent, None);
}

#[test]
fn generator_options_are_sanitized_locked_and_checked() {
    use modelcraft_ops::spline_gen::{ExtrudeOptions, RevolveOptions};
    use modelcraft_scene::SplineGen;
    let mut d = Document::new();
    let g = d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![] })).unwrap().created[0];
    d.execute(Command::Spline(S::SetGenerator {
        id: g,
        generator: SplineGen::Extrude(ExtrudeOptions { distance: f32::NAN, steps: u32::MAX, ..Default::default() }),
    }))
    .unwrap();
    match &d.scene.get(g).unwrap().kind {
        ObjectKind::SplineGenerator { generator: SplineGen::Extrude(o) } => assert_eq!((o.distance, o.steps), (1.0, modelcraft_ops::spline_gen::MAX_STEPS)),
        other => panic!("{other:?}"),
    }
    // A different kind of generator replaces the options wholesale.
    d.execute(Command::Spline(S::SetGenerator { id: g, generator: SplineGen::Revolve(RevolveOptions::default()) })).unwrap();
    assert!(faces(&mut d, g) > 0);
    // Wrong objects fail; a locked generator refuses.
    let circle = add(&mut d, "circle");
    assert!(d.execute(Command::Spline(S::SetGenerator { id: circle, generator: SplineGen::Revolve(RevolveOptions::default()) })).is_err());
    d.execute(Command::SetLocked { ids: vec![g], locked: true }).unwrap();
    let e = d.execute(Command::Spline(S::SetGenerator { id: g, generator: SplineGen::Revolve(RevolveOptions::default()) })).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
}

#[test]
fn a_generator_exports_its_mesh_and_chains_with_other_generators_and_saves() {
    let dir = std::env::temp_dir().join(format!("mc-spline-gen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new();
    let g = d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![] })).unwrap().created[0];
    let items = d.export_items();
    assert_eq!(items.len(), 1, "the generator, not its curve");
    assert_eq!(items[0].mesh.as_ref().map(|m| m.face_count()), Some(faces(&mut d, g)));
    for ext in ["glb", "obj", "stl"] {
        let p = dir.join(format!("out.{ext}"));
        d.export(&p).unwrap();
        assert!(p.metadata().unwrap().len() > 100, "{ext}");
    }
    // An array around it joins copies of the extruded mesh.
    let one = faces(&mut d, g);
    let arr = d.execute(Command::AddArray { kind: "linear".into(), input: Some(g) }).unwrap().created[0];
    assert_eq!(faces(&mut d, arr), one * 4);
    assert_eq!(d.export_items().len(), 1, "only the array is exported");
    // Save and open.
    let path = dir.join("gen.modelcraft");
    d.save_as(&path).unwrap();
    let mut e = Document::open(&path).unwrap();
    assert_eq!(faces(&mut e, arr), one * 4);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generator_commands_round_trip_as_json() {
    use modelcraft_scene::SplineGen;
    for c in [
        S::AddGenerator { kind: "sweep".into(), inputs: vec![modelcraft_scene::ObjectId(1), modelcraft_scene::ObjectId(2)] },
        S::SetGenerator { id: modelcraft_scene::ObjectId(3), generator: SplineGen::default_of("loft").unwrap() },
        S::SetGenerator { id: modelcraft_scene::ObjectId(3), generator: SplineGen::default_of("sweep").unwrap() },
    ] {
        let cmd = Command::Spline(c);
        let j = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), cmd, "{j}");
        assert!(!cmd.label().is_empty());
    }
    let j =
        r#"{"cmd":"spline","op":"set_generator","id":1,"generator":{"kind":"extrude","distance":2.5,"axis":"z","steps":3,"cap_start":true,"cap_end":false}}"#;
    assert!(serde_json::from_str::<Command>(j).is_ok());
}

#[test]
fn corner_cuts_reverse_and_polygon_conversion_are_single_undo_steps() {
    use modelcraft_spline::{KnotRef, SplineEdit};
    let mut d = Document::new();
    let r = add(&mut d, "rectangle");
    let at = |k| KnotRef { segment: 0, knot: k };
    let n = d.undo.len();
    d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::Chamfer { knots: vec![at(0), at(1), at(2), at(3)], distance: 0.25 } })).unwrap();
    assert_eq!((d.undo.len(), d.eval.spline(&d.scene, r).unwrap().spline.knot_count()), (n + 1, 8));
    d.execute(Command::Undo).unwrap();
    assert!(matches!(d.scene.get(r).unwrap().kind, ObjectKind::Spline { source: SplineSource::Primitive { .. }, .. }), "undo brings the primitive back");
    d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::Fillet { knots: vec![at(1)], radius: 0.3 } })).unwrap();
    let rounded = d.eval.spline(&d.scene, r).unwrap().spline.clone();
    assert_eq!(rounded.segments[0].kind, CurveKind::Bezier);
    d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::Reverse { segment: 0 } })).unwrap();
    assert_eq!(d.eval.spline(&d.scene, r).unwrap().spline.segments[0].knots[0].pos, rounded.segments[0].knots[0].pos);
    d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::ToPolygon { tess: Tessellation::Fixed { steps: 6 } } })).unwrap();
    assert_eq!(d.eval.spline(&d.scene, r).unwrap().spline.segments[0].kind, CurveKind::Linear);
    // Nothing to cut, or a bad size: an error and no undo step.
    let n = d.undo.len();
    assert!(d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::Fillet { knots: vec![at(99)], radius: 0.3 } })).is_err());
    assert!(d.execute(Command::Spline(S::Edit { id: r, edit: SplineEdit::Chamfer { knots: vec![at(1)], distance: f32::NAN } })).is_err());
    assert_eq!(d.undo.len(), n);
}

#[test]
fn to_mesh_fills_closed_curves_with_holes_and_refuses_the_rest() {
    let mut d = Document::new();
    let a = add(&mut d, "rectangle");
    // A second, smaller rectangle inside the same spline as a second curve: a hole.
    let hole = modelcraft_spline::Segment::polyline(
        &[Vec3::new(-0.25, -0.25, 0.0), Vec3::new(0.25, -0.25, 0.0), Vec3::new(0.25, 0.25, 0.0), Vec3::new(-0.25, 0.25, 0.0)],
        true,
    );
    d.execute(Command::Spline(S::Edit { id: a, edit: modelcraft_spline::SplineEdit::AddSegment { segment: hole } })).unwrap();
    let m = d.execute(Command::Spline(S::ToMesh { id: a })).unwrap().created[0];
    let mesh = d.eval.evaluate(&d.scene, m).mesh;
    assert!(matches!(d.scene.get(m).unwrap().kind, ObjectKind::Mesh { .. }));
    mesh.validate().unwrap();
    let (tris, _) = mesh.triangulate();
    let area: f32 = tris.iter().map(|t| (mesh.position(t[1]) - mesh.position(t[0])).cross(mesh.position(t[2]) - mesh.position(t[0])).length() * 0.5).sum();
    assert!((area - (2.0 * 1.5 - 0.25)).abs() < 1e-4, "the rectangle minus the hole: {area}");
    // Open curves give no surface; other objects are refused; the lock check applies to the spline.
    let open = add(&mut d, "arc");
    assert!(d.execute(Command::Spline(S::ToMesh { id: open })).is_err());
    assert!(d.execute(Command::Spline(S::ToMesh { id: m })).is_err());
    d.execute(Command::SetLocked { ids: vec![a], locked: true }).unwrap();
    assert!(d.execute(Command::Spline(S::ToMesh { id: a })).is_err());
}

#[test]
fn to_mesh_of_a_generator_input_puts_the_mesh_beside_the_generator() {
    let mut d = Document::new();
    let g = d.execute(Command::Spline(S::AddGenerator { kind: "extrude".into(), inputs: vec![] })).unwrap().created[0];
    let star = d.scene.spline_children(g)[0];
    let m = d.execute(Command::Spline(S::ToMesh { id: star })).unwrap().created[0];
    assert_eq!(d.scene.get(m).unwrap().parent, None, "not inside the generator, where it would be taken for an input");
}
