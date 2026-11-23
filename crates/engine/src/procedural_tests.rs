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
