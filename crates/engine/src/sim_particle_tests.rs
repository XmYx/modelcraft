//! Particles through commands: a fountain on a plane, shown as instances, colliding with the
//! scene, saved and exported.

use glam::Vec3;
use modelcraft_scene::{ObjectId, ObjectKind, Transform};
use modelcraft_sim::particles::{EmitterKind, ParticleSettings};
use modelcraft_sim::{SimKind, is_live};

use crate::sim::SimCommand as S;
use crate::{Command, Document};

fn plane(d: &mut Document, at: Vec3) -> ObjectId {
    let id = d.execute(Command::AddPrimitive { kind: "plane".into(), name: Some("Emitter".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform { id, transform: Transform { translation: at, ..Default::default() } }).unwrap();
    id
}

fn add(d: &mut Document, parent: ObjectId, f: impl FnOnce(&mut ParticleSettings)) -> ObjectId {
    let sim = d.execute(Command::Sim(S::Add { mesh: parent, kind: "particles".into() })).unwrap().created[0];
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!() };
    let SimKind::Particles(mut p) = s.kind.clone() else { panic!("not particles") };
    f(&mut p);
    d.execute(Command::Sim(S::SetKind { id: sim, kind: SimKind::Particles(p) })).unwrap();
    d.execute(Command::Sim(S::Set {
        id: sim,
        start_frame: Some(1.0),
        end_frame: Some(48.0),
        fps: Some(24.0),
        substeps: Some(4),
        seed: Some(5),
        max_cache_mib: None,
    }))
    .unwrap();
    sim
}

fn cache_of(d: &Document, sim: ObjectId) -> Option<std::sync::Arc<modelcraft_sim::FrameCache>> {
    match &d.scene.get(sim)?.kind {
        ObjectKind::Sim { sim } => sim.cache.clone(),
        _ => None,
    }
}

fn alive_at(c: &modelcraft_sim::FrameCache, f: usize) -> usize {
    c.frame(f).unwrap().iter().filter(|p| is_live(**p)).count()
}

#[test]
fn a_fountain_is_baked_shown_as_instances_and_follows_the_timeline() {
    let mut d = Document::new();
    let emitter = plane(&mut d, Vec3::new(0.0, 1.0, 0.0));
    let sim = add(&mut d, emitter, |p| {
        p.rate = 100.0;
        p.speed = 5.0;
        p.spread = 10.0;
        p.lifetime = 3.0;
        p.size = 0.05;
        p.capacity = 600;
        p.collide = false;
    });
    d.bake_sim(sim).unwrap();
    let cache = cache_of(&d, sim).unwrap();
    assert_eq!((cache.frames(), cache.points()), (48, 600));
    assert!(!d.sim_stale(sim));
    // Born on the plane (y = 1, within its 4 × 4 metres) and flying up.
    let first: Vec<Vec3> = cache.frame(2).unwrap().iter().copied().filter(|p| is_live(*p)).collect();
    assert!(first.len() > 5 && first.iter().all(|p| p.x.abs() <= 2.01 && p.z.abs() <= 2.01 && p.y > 0.99), "{first:?}");
    assert!(cache.frame(12).unwrap().iter().filter(|p| is_live(**p)).map(|p| p.y).fold(f32::MIN, f32::max) > 2.5, "they rise");
    // The system object evaluates to instances: one per live slot, at the cached world positions.
    for f in [3.0, 12.0, 30.0] {
        d.set_frame(f);
        let e = d.eval.evaluate(&d.scene, sim);
        let inst = e.instances.expect("instances");
        assert_eq!(inst.clones.instances.len(), alive_at(&cache, f as usize - 1), "frame {f}");
        let world = d.scene.world_matrix(sim);
        let c = &inst.clones.instances[0];
        let p = (world * c.matrix).transform_point3(Vec3::ZERO);
        assert!((p - cache.frame(f as usize - 1).unwrap()[c.index as usize]).length() < 1e-4);
    }
    assert!(d.triangle_count() > 0);
    // Moving the system's own object does not move the particles (the cache is in world space).
    d.execute(Command::SetTransform { id: sim, transform: Transform { translation: Vec3::new(7.0, 0.0, 0.0), ..Default::default() } }).unwrap();
    d.set_frame(12.0);
    let inst = d.eval.evaluate(&d.scene, sim).instances.unwrap();
    let c = &inst.clones.instances[0];
    let p = (d.scene.world_matrix(sim) * c.matrix).transform_point3(Vec3::ZERO);
    assert!((p - cache.frame(11).unwrap()[c.index as usize]).length() < 1e-3, "still where the cache has it");
    // Exported with the scene: a node per particle, sharing the source mesh.
    let items = d.export_items();
    assert!(items.len() > alive_at(&cache, 11), "{} items", items.len());
    // Undo.
    d.execute(Command::Undo).unwrap();
    assert!(cache_of(&d, sim).is_some(), "undoing the move keeps the bake");
    d.execute(Command::Undo).unwrap();
    assert!(cache_of(&d, sim).is_none(), "undoing the bake removes it");
}

#[test]
fn particles_land_on_a_floor_in_the_scene_and_stay_above_it() {
    let mut d = Document::new();
    let emitter = plane(&mut d, Vec3::new(0.0, 3.0, 0.0));
    let floor = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Floor".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform {
        id: floor,
        transform: Transform { translation: Vec3::new(0.0, -1.0, 0.0), scale: Vec3::new(20.0, 1.0, 20.0), ..Default::default() },
    })
    .unwrap();
    let sim = add(&mut d, emitter, |p| {
        p.rate = 80.0;
        p.speed = 1.0;
        p.lifetime = 5.0;
        p.bounciness = 0.3;
        p.capacity = 800;
        p.drag = 0.0;
    });
    d.execute(Command::Sim(S::Set { id: sim, start_frame: None, end_frame: Some(72.0), fps: None, substeps: None, seed: None, max_cache_mib: None })).unwrap();
    d.bake_sim(sim).unwrap();
    let c = cache_of(&d, sim).unwrap();
    let lowest = c.positions().iter().filter(|p| is_live(**p)).map(|p| p.y).fold(f32::MAX, f32::min);
    assert!(lowest > -1e-3, "no particle went below the floor top (y = 0): {lowest}");
    assert!(c.frame(71).unwrap().iter().filter(|p| is_live(**p)).any(|p| p.y < 0.05), "some have landed");
}

#[test]
fn only_a_listed_collider_stops_them_and_a_point_emitter_needs_no_mesh() {
    let mut d = Document::new();
    let group = d.execute(Command::AddGroup { name: Some("Nozzle".into()) }).unwrap().created[0];
    d.execute(Command::SetTransform { id: group, transform: Transform { translation: Vec3::new(0.0, 2.0, 0.0), ..Default::default() } }).unwrap();
    let floor = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Floor".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform {
        id: floor,
        transform: Transform { translation: Vec3::new(0.0, -1.0, 0.0), scale: Vec3::new(20.0, 1.0, 20.0), ..Default::default() },
    })
    .unwrap();
    let decoy = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Decoy".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetTransform { id: decoy, transform: Transform { translation: Vec3::new(50.0, 0.0, 0.0), ..Default::default() } }).unwrap();
    let sim = add(&mut d, group, |p| {
        p.rate = 50.0;
        p.speed = 0.0;
        p.lifetime = 5.0;
        p.collide_with = vec![decoy.0];
        p.drag = 0.0;
    });
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!() };
    let SimKind::Particles(p) = &s.kind else { panic!() };
    assert_eq!(p.emitter, EmitterKind::Point, "a group can only be a point emitter");
    d.execute(Command::Sim(S::Set { id: sim, start_frame: None, end_frame: Some(72.0), fps: None, substeps: None, seed: None, max_cache_mib: None })).unwrap();
    d.bake_sim(sim).unwrap();
    let c = cache_of(&d, sim).unwrap();
    let origin = c.frame(0).unwrap().iter().chain(c.frame(1).unwrap()).copied().find(|p| is_live(*p)).unwrap();
    assert!((origin - Vec3::new(0.0, 2.0, 0.0)).length() < 0.2, "born at the nozzle: {origin:?}");
    assert!(c.positions().iter().filter(|p| is_live(**p)).any(|p| p.y < -1.0), "the listed decoy is far away: they fall through the floor");
}

#[test]
fn a_mesh_inside_the_system_is_the_shape_of_every_particle() {
    let mut d = Document::new();
    let emitter = plane(&mut d, Vec3::ZERO);
    let sim = add(&mut d, emitter, |p| {
        p.burst = 20;
        p.rate = 0.0;
        p.collide = false;
    });
    let pebble = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Pebble".into()), parent: Some(sim) }).unwrap().created[0];
    d.bake_sim(sim).unwrap();
    d.set_frame(2.0);
    let inst = d.eval.evaluate(&d.scene, sim).instances.unwrap();
    assert_eq!(inst.source_object, Some(pebble));
    assert_eq!(inst.clones.instances.len(), 20);
    assert!(d.scene.is_cage_input(pebble), "the source is shown through the system, not on its own");
}

#[test]
fn a_bake_is_repeatable_saved_with_the_project_and_marked_stale_by_a_change() {
    let dir = std::env::temp_dir().join(format!("mc-sim-particles-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("fountain.modelcraft");
    let mut d = Document::new();
    let emitter = plane(&mut d, Vec3::new(0.0, 1.0, 0.0));
    let sim = add(&mut d, emitter, |p| p.collide = false);
    d.bake_sim(sim).unwrap();
    let sum = cache_of(&d, sim).unwrap().checksum();
    d.bake_sim(sim).unwrap();
    assert_eq!(cache_of(&d, sim).unwrap().checksum(), sum, "the same seed gives the same particles");
    d.execute(Command::Sim(S::Set { id: sim, start_frame: None, end_frame: None, fps: None, substeps: None, seed: Some(77), max_cache_mib: None })).unwrap();
    assert!(d.sim_stale(sim));
    d.bake_sim(sim).unwrap();
    assert_ne!(cache_of(&d, sim).unwrap().checksum(), sum, "another seed gives other particles");
    d.save_as(&p).unwrap();
    let again = Document::open(&p).unwrap();
    assert_eq!(cache_of(&again, sim).unwrap().checksum(), cache_of(&d, sim).unwrap().checksum());
    assert!(!again.sim_stale(sim));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_cannot_be_emitted_is_refused() {
    let mut d = Document::new();
    let group = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let sim = add(&mut d, group, |p| p.emitter = EmitterKind::Surface);
    let e = d.bake_sim(sim).unwrap_err().to_string();
    assert!(e.contains("needs a mesh"), "{e}");
    assert!(d.execute(Command::Sim(S::Add { mesh: group, kind: "cloth".into() })).is_err(), "cloth needs a mesh");
    assert!(d.execute(Command::Sim(S::Add { mesh: group, kind: "particles".into() })).is_err(), "one system per object");
    assert!(d.execute(Command::Sim(S::Add { mesh: ObjectId(99), kind: "particles".into() })).is_err());
    // A pool too big for the cache budget is refused before anything runs.
    let emitter = plane(&mut d, Vec3::ZERO);
    let big = add(&mut d, emitter, |p| p.capacity = 1_000_000);
    d.execute(Command::Sim(S::Set { id: big, start_frame: None, end_frame: Some(100.0), fps: None, substeps: None, seed: None, max_cache_mib: Some(8) }))
        .unwrap();
    assert!(d.bake_sim(big).unwrap_err().to_string().contains("MiB"));
}
