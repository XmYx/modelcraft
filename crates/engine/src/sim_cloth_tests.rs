//! Cloth through commands: a sheet dropped on a box, pins by list and by sculpt mask, pins that
//! follow an animated object, choosing colliders, stale caches and the saved file.

use glam::Vec3;
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{ObjectId, ObjectKind, Transform};
use modelcraft_sim::SimKind;
use modelcraft_sim::cloth::ClothSettings;

use crate::sim::SimCommand as S;
use crate::{AnimCommand as A, Command, Document, MaskOp};

/// A square sheet `size` metres across with `segments` quads a side, centred at `at`.
fn sheet(d: &mut Document, at: Vec3, size: f32, segments: u32) -> ObjectId {
    let id = d.execute(Command::AddPrimitive { kind: "plane".into(), name: Some("Sheet".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id, primitive: Primitive::Plane { width: size, depth: size, segments } }).unwrap();
    d.execute(Command::SetTransform { id, transform: Transform { translation: at, ..Default::default() } }).unwrap();
    id
}

fn cube(d: &mut Document, name: &str, at: Vec3, size: f32) -> ObjectId {
    let id = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some(name.into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform { id, transform: Transform { translation: at, scale: Vec3::splat(size * 0.5), ..Default::default() } }).unwrap();
    id
}

fn add_cloth(d: &mut Document, mesh: ObjectId, end: f64, f: impl FnOnce(&mut ClothSettings)) -> ObjectId {
    let sim = d.execute(Command::Sim(S::Add { mesh, kind: "cloth".into() })).unwrap().created[0];
    set_cloth(d, sim, f);
    d.execute(Command::Sim(S::Set {
        id: sim,
        start_frame: Some(1.0),
        end_frame: Some(end),
        fps: Some(24.0),
        substeps: Some(8),
        seed: None,
        max_cache_mib: None,
    }))
    .unwrap();
    sim
}

fn set_cloth(d: &mut Document, sim: ObjectId, f: impl FnOnce(&mut ClothSettings)) {
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!("not a simulation") };
    let SimKind::Cloth(mut c) = s.kind.clone() else { panic!("not cloth") };
    f(&mut c);
    d.execute(Command::Sim(S::SetKind { id: sim, kind: SimKind::Cloth(c) })).unwrap();
}

fn cache_of(d: &Document, sim: ObjectId) -> Option<std::sync::Arc<modelcraft_sim::FrameCache>> {
    match &d.scene.get(sim)?.kind {
        ObjectKind::Sim { sim } => sim.cache.clone(),
        _ => None,
    }
}

/// The sheet's vertices in world space at `frame`.
fn world_points(d: &mut Document, mesh: ObjectId, frame: f64) -> Vec<Vec3> {
    d.set_frame(frame);
    let m = d.scene.world_matrix(mesh);
    d.eval.evaluate(&d.scene, mesh).mesh.positions().iter().map(|p| m.transform_point3(*p)).collect()
}

#[test]
fn a_sheet_dropped_on_a_box_lands_on_it_and_plays_back_on_the_timeline() {
    let mut d = Document::new();
    let sheet_id = sheet(&mut d, Vec3::new(0.0, 1.5, 0.0), 2.0, 16);
    let _block = cube(&mut d, "Block", Vec3::ZERO, 1.0);
    let sim = add_cloth(&mut d, sheet_id, 72.0, |_| {});
    assert!(!d.sim_stale(sim));
    d.bake_sim(sim).unwrap();
    let cache = cache_of(&d, sim).unwrap();
    assert_eq!((cache.frames(), cache.points()), (72, 17 * 17));
    assert!(!d.sim_stale(sim));
    // Frame 1 is the flat sheet; the last frame has it lying over the box.
    assert!(world_points(&mut d, sheet_id, 1.0).iter().all(|p| (p.y - 1.5).abs() < 1e-4));
    let end = world_points(&mut d, sheet_id, 72.0);
    // Never inside the box (half a metre each way) at any frame.
    for f in 1..=72 {
        let pts = world_points(&mut d, sheet_id, f64::from(f));
        assert!(pts.iter().all(|p| !(p.x.abs() < 0.49 && p.z.abs() < 0.49 && p.y.abs() < 0.49)), "a vertex is inside the box at frame {f}");
    }
    let middle = end[8 * 17 + 8];
    assert!(middle.y > 0.5 && middle.y < 0.58, "the middle of the sheet rests on top of the box: {middle:?}");
    assert!(end[0].y < 0.5, "the corners hang over the sides: {:?}", end[0]);
    // The object itself did not move; undo removes the cache, redo restores it.
    assert_eq!(d.scene.get(sheet_id).unwrap().transform.translation, Vec3::new(0.0, 1.5, 0.0));
    d.execute(Command::Undo).unwrap();
    assert!(cache_of(&d, sim).is_none());
    d.execute(Command::Redo).unwrap();
    let sum = cache_of(&d, sim).unwrap().checksum();
    // Repeatable, and stale once a setting changes.
    d.bake_sim(sim).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().checksum(), sum);
    set_cloth(&mut d, sim, |c| c.stretch = 0.5);
    assert!(d.sim_stale(sim));
    d.bake_sim(sim).unwrap();
    assert!(!d.sim_stale(sim));
}

#[test]
fn the_cache_is_saved_with_the_project_and_comes_back_fresh() {
    let dir = std::env::temp_dir().join(format!("mc-sim-cloth-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("cloth.modelcraft");
    let mut d = Document::new();
    let sheet_id = sheet(&mut d, Vec3::new(0.0, 1.0, 0.0), 1.0, 8);
    let sim = add_cloth(&mut d, sheet_id, 24.0, |c| c.pinned = vec![0, 8]);
    d.bake_sim(sim).unwrap();
    let sum = cache_of(&d, sim).unwrap().checksum();
    d.save_as(&p).unwrap();
    let again = Document::open(&p).unwrap();
    assert_eq!(cache_of(&again, sim).unwrap().checksum(), sum);
    assert!(!again.sim_stale(sim));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pinned_vertices_hold_by_list_and_by_sculpt_mask_and_release_by_command() {
    let mut d = Document::new();
    let sheet_id = sheet(&mut d, Vec3::new(0.0, 1.0, 0.0), 1.0, 8);
    let sim = add_cloth(&mut d, sheet_id, 24.0, |c| c.collide = false);
    d.execute(Command::Sim(S::PinVertices { id: sim, vertices: vec![0, 1, 8, 8], pin: true })).unwrap();
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!() };
    let SimKind::Cloth(c) = &s.kind else { panic!() };
    assert_eq!(c.pinned, vec![0, 1, 8], "sorted and without repeats");
    d.bake_sim(sim).unwrap();
    let rest = world_points(&mut d, sheet_id, 1.0);
    let end = world_points(&mut d, sheet_id, 24.0);
    for i in [0usize, 1, 8] {
        assert!(end[i].distance(rest[i]) < 1e-4, "vertex {i} is pinned");
    }
    assert!(end[40].y < rest[40].y - 0.2, "the rest of the sheet fell");
    d.execute(Command::Sim(S::PinVertices { id: sim, vertices: vec![0, 1, 8], pin: false })).unwrap();
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!() };
    let SimKind::Cloth(c) = &s.kind else { panic!() };
    assert!(c.pinned.is_empty());
    // Pin by mask: the sculpt mask holds the vertices that stay.
    d.execute(Command::BeginSculpt { id: sheet_id }).unwrap();
    d.execute(Command::SculptMask { id: sheet_id, op: MaskOp::Fill }).unwrap();
    d.execute(Command::EndSculpt).unwrap();
    set_cloth(&mut d, sim, |c| c.pin_mask = true);
    d.bake_sim(sim).unwrap();
    let all = cache_of(&d, sim).unwrap();
    assert!(all.positions().windows(2).take(50).all(|w| w[0].is_finite() && w[1].is_finite()));
    let held = world_points(&mut d, sheet_id, 24.0);
    assert!(held.iter().zip(&rest).all(|(a, b)| a.distance(*b) < 1e-4), "a fully masked sheet does not move");
    // Not a cloth: no pins.
    let other = cube(&mut d, "Box", Vec3::new(5.0, 0.0, 0.0), 1.0);
    let rigid = d.execute(Command::Sim(S::Add { mesh: other, kind: "rigid_active".into() })).unwrap().created[0];
    assert!(d.execute(Command::Sim(S::PinVertices { id: rigid, vertices: vec![0], pin: true })).is_err());
}

#[test]
fn pins_can_follow_an_animated_object() {
    let mut d = Document::new();
    let sheet_id = sheet(&mut d, Vec3::new(0.0, 1.0, 0.0), 1.0, 8);
    let hand = d.execute(Command::AddGroup { name: Some("Hand".into()) }).unwrap().created[0];
    for (f, x) in [(1.0, 0.0), (24.0, 3.0)] {
        d.execute(Command::Anim(A::SetKey { id: hand, path: "translate.x".into(), frame: Some(f), value: Some(x) })).unwrap();
    }
    let sim = add_cloth(&mut d, sheet_id, 24.0, |c| {
        c.pinned = vec![0, 8];
        c.pin_follow = Some(hand.0);
        c.collide = false;
    });
    d.bake_sim(sim).unwrap();
    let rest = world_points(&mut d, sheet_id, 1.0);
    let end = world_points(&mut d, sheet_id, 24.0);
    for i in [0usize, 8] {
        assert!((end[i] - rest[i] - Vec3::new(3.0, 0.0, 0.0)).length() < 1e-3, "pin {i} went with the hand: {:?}", end[i]);
    }
}

#[test]
fn only_the_listed_colliders_are_collided_with() {
    let run = |listed: Option<bool>| {
        let mut d = Document::new();
        let sheet_id = sheet(&mut d, Vec3::new(0.0, 1.0, 0.0), 1.0, 8);
        let floor = cube(&mut d, "Floor", Vec3::new(0.0, -0.5, 0.0), 1.0);
        let decoy = cube(&mut d, "Decoy", Vec3::new(10.0, 0.0, 0.0), 1.0);
        let sim = add_cloth(&mut d, sheet_id, 48.0, |c| match listed {
            None => {}
            Some(true) => c.collide_with = vec![floor.0],
            Some(false) => c.collide_with = vec![decoy.0],
        });
        d.bake_sim(sim).unwrap();
        world_points(&mut d, sheet_id, 48.0).iter().map(|p| p.y).fold(f32::MAX, f32::min)
    };
    assert!(run(None) > -0.01, "every mesh is a collider by default");
    assert!(run(Some(true)) > -0.01, "the listed floor holds the sheet");
    assert!(run(Some(false)) < -1.0, "a listed object elsewhere leaves the floor out: the sheet falls through it");
}

#[test]
fn what_cannot_be_baked_is_refused_and_commands_round_trip() {
    let mut d = Document::new();
    let group = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    assert!(d.execute(Command::Sim(S::Add { mesh: group, kind: "cloth".into() })).is_err());
    let sheet_id = sheet(&mut d, Vec3::ZERO, 1.0, 4);
    let sim = add_cloth(&mut d, sheet_id, 24.0, |_| {});
    d.execute(Command::SetVisible { id: sheet_id, visible: false }).unwrap();
    d.execute(Command::SetVisible { id: sheet_id, visible: true }).unwrap();
    // A budget that cannot hold the cache refuses before simulating.
    d.execute(Command::Sim(S::Set { id: sim, start_frame: None, end_frame: Some(99_999.0), fps: None, substeps: Some(1), seed: None, max_cache_mib: Some(1) }))
        .unwrap();
    assert!(d.bake_sim(sim).unwrap_err().to_string().contains("MiB"));
    for c in [
        Command::Sim(S::Add { mesh: ObjectId(1), kind: "cloth".into() }),
        Command::Sim(S::PinVertices { id: ObjectId(2), vertices: vec![0, 4], pin: false }),
        Command::Sim(S::SetKind { id: ObjectId(2), kind: SimKind::Cloth(ClothSettings::default()) }),
    ] {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c, "{j}");
    }
    let j = r#"{"cmd":"sim","op":"pin_vertices","id":2,"vertices":[1,2]}"#;
    assert!(matches!(serde_json::from_str::<Command>(j), Ok(Command::Sim(S::PinVertices { pin: true, .. }))), "pin defaults to true");
}
