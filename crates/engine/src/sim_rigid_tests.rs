//! Rigid bodies through commands: add, set, bake, play, undo, save, recover, stale caches,
//! cancelling, and a moving collider.

use glam::Vec3;
use modelcraft_scene::{ObjectId, ObjectKind, Transform};
use modelcraft_sim::rigid::{BodyRole, RigidSettings};
use modelcraft_sim::{CancelFlag, SimError, SimKind};

use crate::sim::SimCommand as S;
use crate::{AnimCommand as A, Command, Document};

/// A cube `size` metres across (the primitive is two metres at scale one).
fn cube_at(d: &mut Document, name: &str, at: Vec3, size: Vec3) -> ObjectId {
    let id = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some(name.into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform { id, transform: Transform { translation: at, scale: size * 0.5, ..Default::default() } }).unwrap();
    id
}

fn add(d: &mut Document, mesh: ObjectId, kind: &str) -> ObjectId {
    d.execute(Command::Sim(S::Add { mesh, kind: kind.into() })).unwrap().created[0]
}

fn set_kind(d: &mut Document, sim: ObjectId, f: impl FnOnce(&mut RigidSettings)) {
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!("not a simulation") };
    let SimKind::Rigid(mut r) = s.kind.clone() else { panic!("not rigid") };
    f(&mut r);
    d.execute(Command::Sim(S::SetKind { id: sim, kind: SimKind::Rigid(r) })).unwrap();
}

fn set_range(d: &mut Document, sim: ObjectId, end: f64) {
    d.execute(Command::Sim(S::Set {
        id: sim,
        start_frame: Some(1.0),
        end_frame: Some(end),
        fps: Some(24.0),
        substeps: Some(4),
        seed: None,
        max_cache_mib: None,
    }))
    .unwrap();
}

/// The lowest world-space y of an object's evaluated mesh at `frame`.
fn lowest(d: &mut Document, id: ObjectId, frame: f64) -> f32 {
    d.set_frame(frame);
    let m = d.scene.world_matrix(id);
    d.eval.evaluate(&d.scene, id).mesh.positions().iter().map(|p| m.transform_point3(*p).y).fold(f32::MAX, f32::min)
}

fn cache_of(d: &Document, sim: ObjectId) -> Option<std::sync::Arc<modelcraft_sim::FrameCache>> {
    match &d.scene.get(sim)?.kind {
        ObjectKind::Sim { sim } => sim.cache.clone(),
        _ => None,
    }
}

/// A unit cube dropped from three metres onto a ground at height zero.
fn dropped(d: &mut Document) -> (ObjectId, ObjectId) {
    let cube = cube_at(d, "Crate", Vec3::new(0.0, 3.0, 0.0), Vec3::ONE);
    let sim = add(d, cube, "rigid_active");
    set_kind(d, sim, |r| {
        r.ground = Some(0.0);
        r.bounciness = 0.0;
    });
    set_range(d, sim, 96.0);
    (cube, sim)
}

#[test]
fn a_baked_rigid_body_falls_lands_and_plays_back_on_the_timeline() {
    let mut d = Document::new();
    let (cube, sim) = dropped(&mut d);
    assert!(!d.sim_stale(sim), "nothing baked, nothing stale");
    let out = d.bake_sim(sim).unwrap();
    assert_eq!(out.warnings, vec!["1 object baked".to_string()]);
    let cache = cache_of(&d, sim).unwrap();
    assert_eq!((cache.frames(), cache.points()), (96, 8));
    assert!(!d.sim_stale(sim));
    // Frame 1 is the rest pose; the box then falls and rests on the ground.
    assert!((lowest(&mut d, cube, 1.0) - 2.5).abs() < 1e-4);
    let mid = lowest(&mut d, cube, 12.0);
    assert!(mid < 2.5 && mid > 0.0, "still falling at frame 12: {mid}");
    let end = lowest(&mut d, cube, 96.0);
    assert!(end.abs() < 0.03, "resting on the ground: {end}");
    assert!(lowest(&mut d, cube, 500.0).abs() < 0.03, "after the cache the last frame holds");
    // The object's own transform did not change; the motion is in the cached points.
    assert_eq!(d.scene.get(cube).unwrap().transform.translation, Vec3::new(0.0, 3.0, 0.0));
    // Undo removes the cache in one step, redo brings it back.
    d.execute(Command::Undo).unwrap();
    assert!(cache_of(&d, sim).is_none());
    assert!((lowest(&mut d, cube, 96.0) - 2.5).abs() < 1e-4, "without the cache the mesh is where it was");
    d.execute(Command::Redo).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().checksum(), cache.checksum());
    // Clearing it also gives the mesh back.
    d.execute(Command::Sim(S::ClearCache { id: sim })).unwrap();
    assert!(cache_of(&d, sim).is_none());
}

#[test]
fn a_bake_is_repeatable_survives_save_and_is_rebuilt_by_crash_recovery() {
    let dir = std::env::temp_dir().join(format!("mc-sim-rigid-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("drop.modelcraft");
    let mut d = Document::new();
    let (_, sim) = dropped(&mut d);
    d.save_as(&p).unwrap();
    d.bake_sim(sim).unwrap();
    let first = cache_of(&d, sim).unwrap().checksum();
    d.bake_sim(sim).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().checksum(), first, "the same scene bakes to the same bits");
    // The journal holds the bake command, not its data: replaying it simulates again.
    drop(d);
    let mut r = Document::open(&p).unwrap();
    assert!(r.pending_recovery > 0);
    r.recover().unwrap();
    assert_eq!(cache_of(&r, sim).unwrap().checksum(), first, "recovery re-simulates to the same cache");
    // Saved, the cache is in the project file.
    r.save_as(&p).unwrap();
    let again = Document::open(&p).unwrap();
    assert_eq!(cache_of(&again, sim).unwrap().checksum(), first);
    assert!(!again.sim_stale(sim), "the baked-with key survives the file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn changing_anything_in_the_world_makes_the_caches_stale_until_the_next_bake() {
    let mut d = Document::new();
    let (_, sim) = dropped(&mut d);
    d.bake_sim(sim).unwrap();
    assert!(!d.sim_stale(sim));
    set_kind(&mut d, sim, |r| r.mass = 5.0);
    assert!(d.sim_stale(sim), "a body's own settings");
    d.bake_sim(sim).unwrap();
    assert!(!d.sim_stale(sim));
    let other = cube_at(&mut d, "Wall", Vec3::new(4.0, 1.0, 0.0), Vec3::ONE);
    add(&mut d, other, "rigid_passive");
    assert!(d.sim_stale(sim), "another body joined the world");
    d.bake_sim(sim).unwrap();
    assert!(!d.sim_stale(sim));
    d.execute(Command::Sim(S::Set { id: sim, start_frame: None, end_frame: Some(48.0), fps: None, substeps: None, seed: None, max_cache_mib: None })).unwrap();
    assert!(d.sim_stale(sim), "the timing");
    d.bake_sim(sim).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().frames(), 48);
}

#[test]
fn bodies_bake_together_and_a_moving_collider_carries_what_rests_on_it() {
    let mut d = Document::new();
    // A platform that rises a metre over 24 frames, and a crate resting on it.
    let platform = cube_at(&mut d, "Platform", Vec3::new(0.0, -0.25, 0.0), Vec3::new(4.0, 0.5, 4.0));
    let crate_ = cube_at(&mut d, "Crate", Vec3::new(0.0, 0.5, 0.0), Vec3::ONE);
    let p_sim = add(&mut d, platform, "rigid_passive");
    let c_sim = add(&mut d, crate_, "rigid_active");
    set_kind(&mut d, c_sim, |r| r.bounciness = 0.0);
    set_range(&mut d, c_sim, 60.0);
    for (frame, y) in [(1.0, -0.25), (24.0, 0.75), (60.0, 0.75)] {
        d.execute(Command::Anim(A::SetKey { id: platform, path: "translate.y".into(), frame: Some(frame), value: Some(y) })).unwrap();
    }
    // Baking bakes the world; only the active body gets a cache.
    d.bake_sim(c_sim).unwrap();
    assert!(cache_of(&d, p_sim).is_none(), "a collider has no cache");
    let cache = cache_of(&d, c_sim).unwrap();
    assert_eq!(cache.frames(), 60);
    assert!((lowest(&mut d, crate_, 1.0) - 0.0).abs() < 1e-4);
    let end = lowest(&mut d, crate_, 60.0);
    assert!((end - 1.0).abs() < 0.12, "the crate rose with the platform: {end}");
    // The animation of the platform is untouched.
    assert_eq!(d.scene.anim.curve(platform, "translate.y").unwrap().len(), 3);
}

#[test]
fn a_hinged_body_swings_from_its_anchor_through_the_command_surface() {
    use modelcraft_sim::rigid::{JointKind, JointSpec};
    let mut d = Document::new();
    let bob = cube_at(&mut d, "Bob", Vec3::new(2.0, 5.0, 0.0), Vec3::splat(0.4));
    let sim = add(&mut d, bob, "rigid_active");
    set_kind(&mut d, sim, |r| {
        r.can_sleep = false;
        r.joints.push(JointSpec { axis: [0.0, 0.0, 1.0], ..JointSpec::new(JointKind::Hinge, None, [0.0, 5.0, 0.0]) });
    });
    set_range(&mut d, sim, 72.0);
    d.bake_sim(sim).unwrap();
    let cache = cache_of(&d, sim).unwrap();
    for f in 0..cache.frames() {
        // The cache is in the mesh's own space: take it to the world through the object.
        let world = d.scene.world_matrix(bob);
        let c = cache.frame(f).unwrap().iter().map(|p| world.transform_point3(*p)).sum::<Vec3>() / 8.0;
        assert!(((c - Vec3::new(0.0, 5.0, 0.0)).length() - 2.0).abs() < 0.1, "frame {f}: {c:?}");
    }
    assert!(lowest(&mut d, bob, 72.0) < 4.0, "it swung down");
}

#[test]
fn a_bake_can_run_apart_from_the_document_be_cancelled_and_then_stored_as_one_step() {
    let mut d = Document::new();
    let (_, sim) = dropped(&mut d);
    // Cancelled: nothing is stored.
    let job = d.prepare_bake(sim).unwrap();
    assert_eq!(job.objects(), 1);
    let flag = CancelFlag::new();
    flag.cancel();
    let r = job.run(&mut flag.clone());
    assert!(matches!(r, Err(SimError::Cancelled { done: 0 })));
    assert!(cache_of(&d, sim).is_none());
    // Run on another thread, store through the same command a direct bake uses.
    let job = d.prepare_bake(sim).unwrap();
    let result = std::thread::spawn(move || job.run(&mut modelcraft_sim::Silent)).join().unwrap().unwrap();
    let undo_before = d.undo.len();
    d.store_bake(result).unwrap();
    assert_eq!(d.undo.len(), undo_before + 1, "one undo step");
    let apart = cache_of(&d, sim).unwrap().checksum();
    d.bake_sim(sim).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().checksum(), apart, "the same cache either way");
}

#[test]
fn what_cannot_be_baked_is_refused_with_a_reason_and_leaves_the_scene_alone() {
    let mut d = Document::new();
    let group = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let wall = cube_at(&mut d, "Wall", Vec3::ZERO, Vec3::ONE);
    assert!(d.execute(Command::Sim(S::Add { mesh: group, kind: "rigid_active".into() })).is_err(), "not a mesh");
    assert!(d.execute(Command::Sim(S::Add { mesh: wall, kind: "magic".into() })).unwrap_err().to_string().contains("unknown simulation"));
    let sim = add(&mut d, wall, "rigid_passive");
    assert!(d.execute(Command::Sim(S::Add { mesh: wall, kind: "rigid_active".into() })).is_err(), "one simulation per mesh");
    let e = d.bake_sim(sim).unwrap_err().to_string();
    assert!(e.contains("no active rigid body"), "{e}");
    // Hidden bodies are left out of the world.
    let ball = cube_at(&mut d, "Ball", Vec3::new(0.0, 4.0, 0.0), Vec3::ONE);
    let b_sim = add(&mut d, ball, "rigid_active");
    d.execute(Command::SetVisible { id: ball, visible: false }).unwrap();
    assert!(d.bake_sim(b_sim).is_err());
    d.execute(Command::SetVisible { id: ball, visible: true }).unwrap();
    d.bake_sim(b_sim).unwrap();
    // Hostile settings are clamped, not trusted.
    set_kind(&mut d, b_sim, |r| {
        r.mass = f32::NAN;
        r.gravity = [0.0, f32::INFINITY, 0.0];
        r.role = BodyRole::Active;
    });
    let ObjectKind::Sim { sim: s } = &d.scene.get(b_sim).unwrap().kind else { panic!() };
    let SimKind::Rigid(r) = &s.kind else { panic!() };
    assert_eq!((r.mass, r.gravity), (1.0, [0.0; 3]));
    // A cache from a file takes no solver settings; a missing object is an error.
    assert!(d.execute(Command::Sim(S::Bake { id: ObjectId(999) })).is_err());
    assert!(d.execute(Command::Sim(S::Bake { id: ball })).is_err(), "a mesh is not a simulation");
    // A budget too small for the cache refuses before simulating.
    d.execute(Command::Sim(S::Set { id: b_sim, start_frame: None, end_frame: Some(5000.0), fps: None, substeps: None, seed: None, max_cache_mib: Some(1) }))
        .unwrap();
    let ok = d.bake_sim(b_sim);
    assert!(ok.is_ok(), "5000 frames of 8 points is 0.5 MiB: {ok:?}");
    d.execute(Command::Sim(S::Set {
        id: b_sim,
        start_frame: None,
        end_frame: Some(99_999.0),
        fps: None,
        substeps: Some(1),
        seed: None,
        max_cache_mib: Some(1),
    }))
    .unwrap();
    let e = d.bake_sim(b_sim).unwrap_err().to_string();
    assert!(e.contains("MiB"), "{e}");
}

#[test]
fn the_new_commands_round_trip_through_json() {
    let r = RigidSettings { mass: 2.5, ..RigidSettings::default() };
    for c in [
        Command::Sim(S::Add { mesh: ObjectId(2), kind: "rigid_active".into() }),
        Command::Sim(S::SetKind { id: ObjectId(3), kind: SimKind::Rigid(r) }),
        Command::Sim(S::Bake { id: ObjectId(3) }),
    ] {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c, "{j}");
    }
    let j = r#"{"cmd":"sim","op":"set_kind","id":3,"kind":{"type":"rigid","role":"passive","shape":"box","mass":4,"friction":0.3,"bounciness":0.1,"linear_damping":0,"angular_damping":0,"velocity":[0,0,0],"spin":[0,0,0],"can_sleep":true,"gravity":[0,-9.81,0],"ground":0,"joints":[]}}"#;
    assert!(serde_json::from_str::<Command>(j).is_ok());
}
