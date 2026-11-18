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

#[test]
fn a_smaller_amount_is_a_prefix_of_the_groom_and_hair_gets_a_colour() {
    let mut d = Document::new();
    let (head, hair) = grown(&mut d);
    assert_eq!(d.scene.get(hair).unwrap().material.base_color, [0.45, 0.30, 0.18, 1.0]);
    let mesh = d.eval.evaluate(&d.scene, head).mesh;
    let h = data(&d, hair);
    assert_eq!(h.strand_count(&mesh), 20_000);
    let all = h.strands(&mesh).unwrap();
    let some = h.strands_with_amount(&mesh, Amount::Count(1234)).unwrap();
    assert_eq!(some.strand_count(), 1234);
    for i in [0, 1, 600, 1233] {
        assert_eq!(some.strand(i), all.strand(i), "what the viewport draws at a lower percentage is the first strands of the full groom");
    }
}

fn quick_bake(d: &mut Document, hair: ObjectId) {
    let timing = modelcraft_sim::SimTiming { start_frame: 1.0, end_frame: 20.0, fps: 24.0, substeps: 4 };
    d.execute(Command::Hair(H::SetDynamics { id: hair, dynamics: None, timing: Some(timing), seed: Some(3) })).unwrap();
    let out = d.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    assert!(out.warnings[0].contains("20 frames"), "{:?}", out.warnings);
}

#[test]
fn a_bake_is_cached_played_by_frame_undone_and_dropped_when_the_groom_changes() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    let rest = data(&d, hair);
    quick_bake(&mut d, hair);
    let baked = data(&d, hair);
    let cache = baked.baked.clone().unwrap();
    assert_eq!((cache.frames(), cache.points()), (20, 1000 * 8));
    // Frame 1 is the groomed shape; later frames hang.
    let first = baked.at_frame(1.0);
    assert_eq!(first.guides.all_points(), rest.guides.all_points());
    let late = baked.at_frame(20.0);
    assert_ne!(late.guides.all_points(), rest.guides.all_points());
    assert_eq!(late.guides.roots(), rest.guides.roots());
    let mid = baked.at_frame(10.5);
    assert!(mid.guides.all_points().iter().all(|p| p.is_finite()));
    // Without a bake the hair is itself.
    assert!(matches!(rest.at_frame(10.0), std::borrow::Cow::Borrowed(_)));
    // Undo takes the bake back, redo restores it, the same bake twice gives the same cache.
    d.execute(Command::Undo).unwrap();
    assert!(data(&d, hair).baked.is_none());
    d.execute(Command::Redo).unwrap();
    assert_eq!(data(&d, hair).baked.as_ref().unwrap().checksum(), cache.checksum());
    d.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    assert_eq!(data(&d, hair).baked.as_ref().unwrap().checksum(), cache.checksum(), "baking again gives the same bytes");
    // Changing the guides or the settings makes the bake stale, so it goes.
    d.execute(Command::Hair(H::Stroke { id: hair, stroke: stroke_everywhere(BrushKind::Comb, Vec3::new(0.1, 0.0, 0.0)) })).unwrap();
    assert!(data(&d, hair).baked.is_none());
    d.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    d.execute(Command::Hair(H::SetDynamics {
        id: hair,
        dynamics: Some(modelcraft_hair::DynamicsSettings { wind: Vec3::new(5.0, 0.0, 0.0), ..Default::default() }),
        timing: None,
        seed: None,
    }))
    .unwrap();
    assert!(data(&d, hair).baked.is_none());
    d.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    d.execute(Command::Hair(H::ClearBake { id: hair })).unwrap();
    assert!(d.execute(Command::Hair(H::ClearBake { id: hair })).is_err(), "nothing left to clear");
}

#[test]
fn gravity_is_the_worlds_whatever_the_emitters_turn() {
    // A plane turned on its side grows hair sideways; world gravity still pulls it toward -y.
    let mut d = Document::new();
    let plane = d.execute(Command::AddPrimitive { kind: "plane".into(), name: None, parent: None }).unwrap().created[0];
    let turn = modelcraft_scene::Transform { rotation: glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..Default::default() };
    d.execute(Command::SetTransform { id: plane, transform: turn }).unwrap();
    let hair = d
        .execute(Command::Hair(H::Add {
            mesh: plane,
            grow: Some(GrowSpec { amount: Amount::Count(30), length: 0.5, ..GrowSpec::default() }),
            interp: None,
            name: None,
        }))
        .unwrap()
        .created[0];
    let still = modelcraft_hair::DynamicsSettings { stiffness: 0.0, collide: false, ..Default::default() };
    d.execute(Command::Hair(H::SetDynamics {
        id: hair,
        dynamics: Some(still),
        timing: Some(modelcraft_sim::SimTiming { start_frame: 1.0, end_frame: 40.0, fps: 24.0, substeps: 4 }),
        seed: None,
    }))
    .unwrap();
    d.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    let h = data(&d, hair);
    let world = d.scene.world_matrix(plane);
    let tip_y = |frame: f64| {
        let at = h.at_frame(frame);
        (0..at.guides.len()).map(|i| world.transform_point3(*at.guides.strand(i).last().unwrap()).y).sum::<f32>() / at.guides.len() as f32
    };
    assert!(tip_y(40.0) < tip_y(1.0) - 0.1, "world -y is down: {} to {}", tip_y(1.0), tip_y(40.0));
}

#[test]
fn bad_dynamics_are_errors_and_leave_the_hair_alone() {
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    let before = data(&d, hair);
    for op in [
        H::SetDynamics { id: hair, dynamics: Some(modelcraft_hair::DynamicsSettings { damping: 3.0, ..Default::default() }), timing: None, seed: None },
        H::SetDynamics { id: hair, dynamics: Some(modelcraft_hair::DynamicsSettings { gravity: Vec3::NAN, ..Default::default() }), timing: None, seed: None },
        H::SetDynamics {
            id: hair,
            dynamics: None,
            timing: Some(modelcraft_sim::SimTiming { start_frame: 9.0, end_frame: 2.0, fps: 24.0, substeps: 4 }),
            seed: None,
        },
        H::SetDynamics {
            id: hair,
            dynamics: None,
            timing: Some(modelcraft_sim::SimTiming { start_frame: 1.0, end_frame: 1.0e9, fps: 24.0, substeps: 4 }),
            seed: None,
        },
        H::Bake { id: ObjectId(999) },
        H::ClearBake { id: hair },
    ] {
        assert!(d.execute(Command::Hair(op.clone())).is_err(), "{op:?}");
    }
    assert_eq!(*data(&d, hair), *before);
}

#[test]
fn hair_exports_as_gltf_lines_and_as_a_point_cache() {
    let dir = std::env::temp_dir().join(format!("mc-hair-{}-export", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new();
    let (head, hair) = grown(&mut d);
    d.execute(Command::SetTransform { id: head, transform: modelcraft_scene::Transform { translation: Vec3::new(10.0, 0.0, 0.0), ..Default::default() } })
        .unwrap();
    // glTF of the guides, in the emitter's space and in the world.
    let (local, world) = (dir.join("local.glb"), dir.join("world.glb"));
    let msg = d.export_hair(hair, &local, crate::HairExportFormat::Glb, crate::HairExportWhat::Guides, 0, false).unwrap();
    assert!(msg.contains("1000 guides of 8 points"), "{msg}");
    d.export_hair(hair, &world, crate::HairExportFormat::Glb, crate::HairExportWhat::Guides, 0, true).unwrap();
    let min_x = |p: &std::path::Path| {
        let b = std::fs::read(p).unwrap();
        let jl = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
        let j: serde_json::Value = serde_json::from_slice(&b[20..20 + jl]).unwrap();
        assert_eq!(j["meshes"][0]["primitives"][0]["mode"], 1);
        j["accessors"][0]["min"][0].as_f64().unwrap()
    };
    assert!(min_x(&world) > min_x(&local) + 8.0, "the world copy sits where the emitter does: {} against {}", min_x(&world), min_x(&local));
    // Render hairs, capped.
    let strands = dir.join("strands.glb");
    let msg = d.export_hair(hair, &strands, crate::HairExportFormat::Glb, crate::HairExportWhat::Strands, 500, false).unwrap();
    assert!(msg.contains("500 strands"), "{msg}");
    // A point cache: one frame without a bake, every baked frame with one.
    let still = dir.join("still.mcpc");
    d.export_hair(hair, &still, crate::HairExportFormat::Mcpc, crate::HairExportWhat::Guides, 0, false).unwrap();
    let pc = modelcraft_formats::pointcache::PointCache::from_bytes(&std::fs::read(&still).unwrap()).unwrap();
    assert_eq!((pc.frames(), pc.vertices), (1, 8000));
    quick_bake(&mut d, hair);
    let baked = dir.join("baked.mcpc");
    d.export_hair(hair, &baked, crate::HairExportFormat::Mcpc, crate::HairExportWhat::Guides, 0, true).unwrap();
    let pc = modelcraft_formats::pointcache::PointCache::from_bytes(&std::fs::read(&baked).unwrap()).unwrap();
    assert_eq!((pc.frames(), pc.vertices), (20, 8000));
    assert!(pc.frame(0).unwrap().iter().all(|p| p.x > 5.0), "world space includes the emitter's offset");
    // The cache plays on a mesh with the simulation framework: the file is the documented format.
    assert!(pc.to_frame_cache().is_ok());
    // A bad path is an error, not a crash.
    assert!(d.export_hair(hair, &dir.join("no/such/dir/x.glb"), crate::HairExportFormat::Glb, crate::HairExportWhat::Guides, 0, false).is_err());
    assert!(d.export_hair(head, &dir.join("x.glb"), crate::HairExportFormat::Glb, crate::HairExportWhat::Guides, 0, false).is_err(), "only hair exports");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_saved_project_keeps_the_dynamics_settings_and_bakes_again_on_request() {
    let dir = std::env::temp_dir().join(format!("mc-hair-{}-savebake", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bake.modelcraft");
    let mut d = Document::new();
    let (_, hair) = grown(&mut d);
    quick_bake(&mut d, hair);
    let checksum = data(&d, hair).baked.as_ref().unwrap().checksum();
    d.save_as(&path).unwrap();
    let mut back = Document::open(&path).unwrap();
    let h = data(&back, hair);
    assert_eq!((h.timing.end_frame, h.seed), (20.0, 3), "the settings are saved");
    assert!(h.baked.is_none(), "the cache is derived data and is not in the project");
    // The same settings and seed bake the same cache.
    back.execute(Command::Hair(H::Bake { id: hair })).unwrap();
    assert_eq!(data(&back, hair).baked.as_ref().unwrap().checksum(), checksum);
    std::fs::remove_dir_all(&dir).ok();
}
