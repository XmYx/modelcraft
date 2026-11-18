use glam::Vec3;
use modelcraft_hair::{Amount, Brush, BrushKind, Dab, GrowSpec, Stroke, Symmetry};
use modelcraft_scene::{ObjectId, ObjectKind};

use crate::hair::{HairCommand as H, MapEdit, MapKind};
use crate::{Command, Document};

fn sphere(d: &mut Document) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Head".into()), parent: None }).unwrap().created[0]
}

fn grown(d: &mut Document) -> (ObjectId, ObjectId) {
    let head = sphere(d);
    let hair = d.execute(Command::Hair(H::Add { mesh: head, grow: None, interp: None, name: None })).unwrap().created[0];
    (head, hair)
}

fn data(d: &Document, hair: ObjectId) -> std::sync::Arc<modelcraft_scene::hair::HairData> {
    match &d.scene.get(hair).unwrap().kind {
        ObjectKind::Hair { hair } => hair.clone(),
        other => panic!("not hair: {other:?}"),
    }
}

fn stroke_everywhere(kind: BrushKind, delta: Vec3) -> Stroke {
    Stroke {
        brush: Brush { radius: 100.0, falloff: 0.0, strength: 1.0, ..Brush::new(kind) },
        symmetry: Symmetry::default(),
        dabs: vec![Dab { center: Vec3::ZERO, normal: Vec3::Y, delta, pressure: 1.0 }],
    }
}

#[test]
fn hair_grows_under_its_mesh_sized_to_it() {
    let mut d = Document::new();
    let (head, hair) = grown(&mut d);
    let o = d.scene.get(hair).unwrap();
    assert_eq!((o.parent, o.name.as_str()), (Some(head), "Hair"));
    assert_eq!(d.scene.hair_of(head), vec![hair]);
    let h = data(&d, hair);
    assert_eq!(h.guides.len(), 1000);
    // A tenth of the mesh's diagonal (a unit sphere's is 2·√3).
    assert!((h.guides.strand_length(0) - 0.35).abs() < 0.15, "{}", h.guides.strand_length(0));
    let strands = h.strands(&d.eval.evaluate(&d.scene, head).mesh).unwrap();
    assert_eq!(strands.strand_count(), 20_000);
    // The emitter's own mesh is unaffected by its hair.
    assert!(d.eval.evaluate(&d.scene, head).mesh.face_count() > 0);
}

#[test]
fn every_hair_operation_is_one_undo_and_redo() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    let before = data(&d, hair);
    let ops = [
        H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.2, 0.0, 0.0)) },
        H::Apply {
            id: hair,
            brush: Brush { strength: 1.0, ..Brush::new(BrushKind::Length) },
            symmetry: Symmetry::default(),
            center: None,
            direction: [0.0; 3],
        },
        H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Fill { value: 0.25 } },
        H::Map { id: hair, map: MapKind::Density, edit: MapEdit::Paint { center: [0.0, 1.0, 0.0], radius: 0.5, value: 0.0, strength: 1.0, falloff: 0.5 } },
        H::Regrow { id: hair, grow: Some(GrowSpec { amount: Amount::Count(300), length: 0.2, ..GrowSpec::default() }) },
    ];
    let mut states = vec![before];
    for op in ops {
        let steps = d.undo.len();
        d.execute(Command::Hair(op.clone())).unwrap();
        assert_eq!(d.undo.len(), steps + 1, "{op:?}");
        assert_ne!(*data(&d, hair), *states.last().unwrap().as_ref(), "{op:?} changed something");
        states.push(data(&d, hair));
    }
    for want in states.iter().rev().skip(1) {
        d.execute(Command::Undo).unwrap();
        assert_eq!(*data(&d, hair), **want);
    }
    for want in states.iter().skip(1) {
        d.execute(Command::Redo).unwrap();
        assert_eq!(*data(&d, hair), **want);
    }
}

#[test]
fn a_stroke_changes_the_guides_and_the_mask_protects_them() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    let g0 = data(&d, hair).guides.clone();
    d.execute(Command::Hair(H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Fill { value: 0.0 } })).unwrap();
    d.execute(Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.3, 0.0, 0.0)) })).unwrap();
    assert_eq!(data(&d, hair).guides, g0, "a zero mask protects every guide");
    d.execute(Command::Hair(H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Invert })).unwrap();
    d.execute(Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.3, 0.0, 0.0)) })).unwrap();
    assert_ne!(data(&d, hair).guides, g0);
    d.execute(Command::Hair(H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Clear })).unwrap();
    assert!(data(&d, hair).mask.is_none());
}

#[test]
fn a_density_map_steers_regrowth() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    // Hair only on the upper half of the head.
    d.execute(Command::Hair(H::Map { id: hair, map: MapKind::Density, edit: MapEdit::Fill { value: 0.0 } })).unwrap();
    d.execute(Command::Hair(H::Map {
        id: hair,
        map: MapKind::Density,
        edit: MapEdit::Paint { center: [0.0, 1.0, 0.0], radius: 1.1, value: 1.0, strength: 1.0, falloff: 0.0 },
    }))
    .unwrap();
    d.execute(Command::Hair(H::Regrow { id: hair, grow: None })).unwrap();
    let g = data(&d, hair).guides.clone();
    assert_eq!(g.len(), 1000);
    let high = (0..g.len()).filter(|i| g.strand(*i)[0].y > 0.0).count();
    assert!(high > 950, "{high} of 1000 grow on the painted half");
}

#[test]
fn numeric_apply_reaches_every_guide_and_a_point_reaches_some() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    let g0 = data(&d, hair).guides.clone();
    let cut = Brush { strength: 1.0, cut_length: 0.1, ..Brush::new(BrushKind::Cut) };
    d.execute(Command::Hair(H::Apply { id: hair, brush: cut, symmetry: Symmetry::default(), center: None, direction: [0.0; 3] })).unwrap();
    let g = data(&d, hair).guides.clone();
    for i in 0..g.len() {
        assert!(g.strand_length(i) <= 0.1 + 1e-3, "{}", g.strand_length(i));
    }
    d.execute(Command::Undo).unwrap();
    let small = Brush { radius: 0.3, falloff: 0.0, ..cut };
    d.execute(Command::Hair(H::Apply { id: hair, brush: small, symmetry: Symmetry::default(), center: Some([0.0, 1.0, 0.0]), direction: [0.0; 3] })).unwrap();
    let g = data(&d, hair).guides.clone();
    let changed = (0..g.len()).filter(|i| g.strand(*i) != g0.strand(*i)).count();
    assert!(changed > 0 && changed < g.len() / 2, "{changed} changed");
}

#[test]
fn bad_input_is_an_error_and_leaves_the_scene_alone() {
    let mut d = Document::new();
    let (head, hair) = grown(&mut d);
    let objects = d.scene.len();
    let nan = Brush { radius: f32::NAN, ..Brush::new(BrushKind::Comb) };
    let bad = [
        H::Add { mesh: ObjectId(999), grow: None, interp: None, name: None },
        H::Add { mesh: d.execute(Command::AddGroup { name: None }).unwrap().created[0], grow: None, interp: None, name: None },
        H::Add { mesh: head, grow: Some(GrowSpec { amount: Amount::Count(usize::MAX), ..GrowSpec::default() }), interp: None, name: None },
        H::Add { mesh: head, grow: Some(GrowSpec { length: f32::NAN, ..GrowSpec::default() }), interp: None, name: None },
        H::Remove { id: head },
        H::Remove { id: ObjectId(999) },
        H::Regrow { id: head, grow: None },
        H::Stroke { id: head, stroke: stroke_everywhere(BrushKind::Comb, Vec3::X) },
        H::Stroke { id: hair, stroke: Stroke { brush: nan, symmetry: Symmetry::default(), dabs: vec![Dab::new(Vec3::ZERO)] } },
        H::Apply { id: hair, brush: nan, symmetry: Symmetry::default(), center: None, direction: [0.0; 3] },
        H::Apply { id: hair, brush: Brush::new(BrushKind::Comb), symmetry: Symmetry::default(), center: None, direction: [f32::INFINITY, 0.0, 0.0] },
        H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Fill { value: 2.0 } },
        H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Paint { center: [f32::NAN; 3], radius: 1.0, value: 0.0, strength: 1.0, falloff: 0.5 } },
    ];
    let before = data(&d, hair);
    let steps = d.undo.len();
    for op in bad {
        assert!(d.execute(Command::Hair(op.clone())).is_err(), "{op:?}");
    }
    assert_eq!(d.scene.len(), objects + 1, "only the group the test added");
    assert_eq!(*data(&d, hair), *before);
    assert!(d.undo.len() <= steps + 1, "failed commands leave no undo steps (the group excepted)");
}

#[test]
fn removing_hair_or_its_mesh_takes_it_away_cleanly() {
    let mut d = Document::new();
    let (head, hair) = grown(&mut d);
    d.execute(Command::Hair(H::Remove { id: hair })).unwrap();
    assert!(d.scene.get(hair).is_none() && d.scene.hair_of(head).is_empty());
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.get(hair).is_some());
    d.execute(Command::Delete { ids: vec![head] }).unwrap();
    assert!(d.scene.get(hair).is_none(), "hair goes with its emitter");
}

#[test]
fn commands_are_json_and_the_scene_round_trips() {
    let cmd: Command = serde_json::from_str(r#"{"cmd":"hair","op":"add","mesh":1}"#).unwrap();
    assert_eq!(cmd, Command::Hair(H::Add { mesh: ObjectId(1), grow: None, interp: None, name: None }));
    let text = serde_json::to_string(&Command::Hair(H::Stroke { id: ObjectId(2), stroke: stroke_everywhere(BrushKind::Puff, Vec3::ZERO) })).unwrap();
    assert!(text.contains("\"cmd\":\"hair\"") && text.contains("\"op\":\"stroke\"") && text.contains("puff"), "{text}");
    assert_eq!(serde_json::from_str::<Command>(&text).unwrap().label(), "puff stroke");
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    d.execute(Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Frizz, Vec3::ZERO) })).unwrap();
    let json = serde_json::to_string(&d.scene).unwrap();
    let back: modelcraft_scene::Scene = serde_json::from_str(&json).unwrap();
    assert_eq!(*data_of(&back, hair), *data(&d, hair));
}

fn data_of(s: &modelcraft_scene::Scene, hair: ObjectId) -> std::sync::Arc<modelcraft_scene::hair::HairData> {
    match &s.get(hair).unwrap().kind {
        ObjectKind::Hair { hair } => hair.clone(),
        _ => panic!("not hair"),
    }
}

#[test]
fn replaying_the_journal_commands_gives_the_same_groom() {
    let mut a = Document::new();
    let (_, hair) = grown(&mut a);
    let cmds = [
        Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.1, 0.0, 0.0)) }),
        Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Frizz, Vec3::ZERO) }),
        Command::Hair(H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Fill { value: 0.5 } }),
        Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Smooth, Vec3::ZERO) }),
    ];
    for c in &cmds {
        a.execute(c.clone()).unwrap();
    }
    let mut b = Document::new();
    let (_, hair_b) = grown(&mut b);
    for c in &cmds {
        b.execute(c.clone()).unwrap();
    }
    assert_eq!(hair, hair_b);
    assert_eq!(*data(&a, hair), *data(&b, hair_b), "the same commands make the same hair");
}

#[test]
fn a_saved_project_brings_the_groom_back_with_its_undo_history() {
    let dir = std::env::temp_dir().join(format!("mc-hair-{}-save", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("groom.modelcraft");
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    d.execute(Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.1, 0.0, 0.2)) })).unwrap();
    d.execute(Command::Hair(H::Map { id: hair, map: MapKind::Mask, edit: MapEdit::Fill { value: 0.5 } })).unwrap();
    let want = data(&d, hair);
    d.save_as(&path).unwrap();
    let mut back = Document::open(&path).unwrap();
    assert_eq!(*data(&back, hair), *want, "the guides, the maps and the settings survive a save");
    // The undo history was rebuilt by replay and undoes the groom step by step.
    back.execute(Command::Undo).unwrap();
    assert!(data(&back, hair).mask.is_none());
    std::fs::remove_dir_all(&dir).ok();
}
