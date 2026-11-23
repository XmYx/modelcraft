//! Tests of the procedural stream's commands (primitives, generators, fields, deformers).

use super::*;
use modelcraft_ops::primitives::{KINDS, Primitive};

fn add(doc: &mut Document, kind: &str) -> modelcraft_scene::ObjectId {
    doc.execute(Command::AddPrimitive { kind: kind.into(), name: None, parent: None }).unwrap().created[0]
}

#[test]
fn every_primitive_kind_adds_evaluates_edits_and_undoes() {
    let mut d = Document::new();
    for kind in KINDS {
        let id = add(&mut d, kind);
        let mesh = d.eval.evaluate(&d.scene, id);
        assert!(mesh.errors.is_empty(), "{kind}: {:?}", mesh.errors);
        assert!(mesh.mesh.face_count() > 0, "{kind} is empty");
        let Some(modelcraft_scene::ObjectKind::Mesh { source: modelcraft_scene::Source::Primitive { primitive }, .. }) =
            d.scene.get(id).map(|o| o.kind.clone())
        else {
            panic!("{kind} is not a primitive")
        };
        assert_eq!(primitive.kind(), *kind);
    }
    assert!(d.execute(Command::AddPrimitive { kind: "teapot".into(), name: None, parent: None }).is_err());
    // The saved JSON names every kind and reads back.
    for kind in KINDS {
        let p = Primitive::default_of(kind).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Primitive>(&json).unwrap(), p, "{json}");
    }
    // Undo takes the last one away again.
    let n = d.scene.len();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.len(), n - 1);
}

#[test]
fn changing_a_new_primitive_through_set_primitive_survives_hostile_numbers() {
    let mut d = Document::new();
    let id = add(&mut d, "landscape");
    let before = d.undo.len();
    d.execute(Command::SetPrimitive {
        id,
        primitive: Primitive::Landscape { width: f32::NAN, depth: -3.0, height: f32::INFINITY, segments: u32::MAX, seed: 7, scale: f32::NAN, octaves: 99 },
    })
    .unwrap();
    assert_eq!(d.undo.len(), before + 1);
    let e = d.eval.evaluate(&d.scene, id);
    e.mesh.validate().unwrap();
    assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, id).mesh.face_count(), 64 * 64);
}
// ---------------------------------------------------------------- generators

use modelcraft_scene::generator::{self, Generator};
use modelcraft_scene::ObjectKind;

fn cmd(d: &mut Document, json: &str) -> Result<crate::Outcome, EngineError> {
    d.execute(serde_json::from_str::<Command>(json).unwrap())
}

#[test]
fn a_generator_with_no_inputs_makes_its_own_and_every_kind_is_documented() {
    let mut d = Document::new();
    for kind in generator::KINDS {
        let out = cmd(&mut d, &format!(r#"{{"cmd":"procedural","op":"add_generator","kind":"{kind}"}}"#)).unwrap();
        let id = out.created[0];
        assert_eq!(d.scene.get(id).map(|o| o.kind.type_name()).map(str::to_lowercase), Some(kind.to_string()));
        let e = d.eval.evaluate(&d.scene, id);
        assert!(e.errors.is_empty(), "{kind}: {:?}", e.errors);
        assert!(e.mesh.face_count() > 0, "{kind} made nothing");
    }
    for (op, _) in crate::procedural::COMMANDS {
        let e = serde_json::from_str::<crate::ProceduralCommand>(&format!(r#"{{"op":"{op}"}}"#)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!e.contains("unknown variant"), "{op}: {e}");
    }
}

#[test]
fn connect_joins_every_input_and_follows_them_live_with_undo() {
    let mut d = Document::new();
    let a = add(&mut d, "cube");
    let b = add(&mut d, "sphere");
    let out = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "connect".into(), inputs: vec![a, b] })).unwrap();
    let g = out.created[0];
    assert_eq!(d.scene.get(a).unwrap().parent, Some(g));
    assert!(d.scene.is_cage_input(a) && d.scene.is_cage_input(b), "both are shown through the generator");
    let faces = |d: &mut Document| d.eval.evaluate(&d.scene, g).mesh.face_count();
    let both = faces(&mut d);
    assert!(both > 6 + 100, "a cube and a sphere, joined: {both}");
    // Moving an input moves the joined mesh; changing a primitive re-meshes it.
    d.execute(Command::SetPrimitive { id: b, primitive: Primitive::Sphere { radius: 1.0, segments: 8, rings: 4 } }).unwrap();
    assert!(faces(&mut d) < both);
    d.execute(Command::Undo).unwrap();
    assert_eq!(faces(&mut d), both);
}

#[test]
fn thicken_of_an_open_sheet_is_closed_and_reduce_keeps_a_share() {
    let mut d = Document::new();
    let sheet = add(&mut d, "plane");
    let t = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "thicken".into(), inputs: vec![sheet] })).unwrap().created[0];
    let m = d.eval.evaluate(&d.scene, t).mesh;
    assert!((0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e)), "a closed solid");
    // Generators chain: reduce the thickened sheet.
    let r = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "reduce".into(), inputs: vec![t] })).unwrap().created[0];
    let before = m.face_count();
    d.execute(Command::Procedural(crate::ProceduralCommand::SetGenerator { id: r, generator: Generator::Reduce { ratio: 0.5 } })).unwrap();
    let after = d.eval.evaluate(&d.scene, r).mesh.face_count();
    assert!(after <= before, "{after} vs {before}");
}

#[test]
fn bad_generator_commands_fail_cleanly() {
    let mut d = Document::new();
    let a = add(&mut d, "cube");
    let b = add(&mut d, "cube");
    let n = d.undo.len();
    for json in [
        r#"{"cmd":"procedural","op":"add_generator","kind":"knead"}"#.to_string(),
        format!(r#"{{"cmd":"procedural","op":"add_generator","kind":"thicken","inputs":[{},{}]}}"#, a.0, b.0),
        format!(r#"{{"cmd":"procedural","op":"add_generator","kind":"connect","inputs":[{},{}]}}"#, a.0, a.0),
        r#"{"cmd":"procedural","op":"add_generator","kind":"connect","inputs":[999]}"#.to_string(),
    ] {
        assert!(cmd(&mut d, &json).is_err(), "{json}");
    }
    assert!(d.execute(Command::Procedural(crate::ProceduralCommand::SetGenerator { id: a, generator: Generator::Reduce { ratio: 1.0 } })).is_err());
    assert_eq!(d.undo.len(), n, "nothing was recorded");
    // Hostile numbers are sanitised, and a locked generator refuses changes.
    let g = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "thicken".into(), inputs: vec![a] })).unwrap().created[0];
    d.execute(Command::Procedural(crate::ProceduralCommand::SetGenerator {
        id: g,
        generator: Generator::Thicken { amount: f32::NAN, direction: modelcraft_ops::thicken::Direction::Both },
    }))
    .unwrap();
    if let Some(ObjectKind::Generator { generator: Generator::Thicken { amount, .. } }) = d.scene.get(g).map(|o| o.kind.clone()) {
        assert!(amount.is_finite() && amount > 0.0);
    }
    d.execute(Command::SetLocked { ids: vec![g], locked: true }).unwrap();
    assert!(d.execute(Command::Procedural(crate::ProceduralCommand::SetGenerator { id: g, generator: Generator::Reduce { ratio: 1.0 } })).is_err());
}

#[test]
fn a_boolean_generator_is_live_closed_and_keeps_its_inputs_editable() {
    let mut d = Document::new();
    let a = add(&mut d, "cube");
    let b = add(&mut d, "sphere");
    d.execute(Command::SetTransform { id: b, transform: modelcraft_scene::Transform { translation: glam::Vec3::new(0.6, 0.2, 0.0), ..Default::default() } }).unwrap();
    let g = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "boolean".into(), inputs: vec![a, b] })).unwrap().created[0];
    let carved = d.eval.evaluate(&d.scene, g);
    assert!(carved.errors.is_empty(), "{:?}", carved.errors);
    assert!((0..carved.mesh.edge_count() as u32).all(|e| !carved.mesh.is_boundary_edge(e)), "closed");
    // The sphere moves away: the cube is whole again.
    d.execute(Command::SetTransform { id: b, transform: modelcraft_scene::Transform { translation: glam::Vec3::new(20.0, 0.0, 0.0), ..Default::default() } }).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, g).mesh.face_count(), 12 * 1, "a cube of two triangles a side, untouched");
    // Union keeps both; intersection of two apart is nothing.
    d.execute(Command::Procedural(crate::ProceduralCommand::SetGenerator { id: g, generator: Generator::Boolean { op: modelcraft_ops::boolean::BoolOp::Intersect } })).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, g).mesh.face_count(), 0);
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, g).mesh.face_count(), 12);
    // A boolean with a single input says what it needs and shows that input.
    let only = d.execute(Command::Procedural(crate::ProceduralCommand::AddGenerator { kind: "boolean".into(), inputs: vec![] })).unwrap().created[0];
    let lone = d.scene.children(only);
    d.execute(Command::Delete { ids: vec![lone[1]] }).unwrap();
    let e = d.eval.evaluate(&d.scene, only);
    assert!(e.errors.iter().any(|e| e.message.contains("two meshes")), "{:?}", e.errors);
    assert!(e.mesh.face_count() > 0);
}
