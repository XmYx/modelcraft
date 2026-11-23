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
