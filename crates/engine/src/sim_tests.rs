//! Simulation caches through the command surface: bake with the framework, write a `.mcpc`,
//! attach it, play it on the timeline, undo, save and reopen.

use glam::Vec3;
use modelcraft_formats::pointcache::PointCache;
use modelcraft_scene::{ObjectId, ObjectKind};
use modelcraft_sim::{BakeConfig, Rng, Silent, SimTiming, Solver, StepContext};

use crate::sim::SimCommand as S;
use crate::{Command, Document};

/// The points of a mesh thrown up and left to fall under gravity, from a seeded random velocity.
struct Toss {
    rest: Vec<Vec3>,
    pos: Vec<Vec3>,
    vel: Vec<Vec3>,
}

impl Solver for Toss {
    fn point_count(&self) -> usize {
        self.rest.len()
    }
    fn reset(&mut self, seed: u64) {
        let mut r = Rng::new(seed);
        self.pos = self.rest.clone();
        self.vel = self.rest.iter().map(|_| Vec3::new(r.range(-0.5, 0.5), r.range(2.0, 4.0), r.range(-0.5, 0.5))).collect();
    }
    fn step(&mut self, ctx: &StepContext) {
        for (p, v) in self.pos.iter_mut().zip(&mut self.vel) {
            v.y -= 9.81 * ctx.dt;
            *p += *v * ctx.dt;
        }
    }
    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        out.extend_from_slice(&self.pos);
    }
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mc-sim-framework-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

fn plane(d: &mut Document) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "plane".into(), name: None, parent: None }).unwrap().created[0]
}

fn rest_points(d: &mut Document, id: ObjectId) -> Vec<Vec3> {
    d.eval.evaluate(&d.scene, id).mesh.positions().to_vec()
}

/// Bake `Toss` over frames 1..=24 and write the `.mcpc`.
fn baked_file(rest: Vec<Vec3>, name: &str) -> (std::path::PathBuf, modelcraft_sim::FrameCache) {
    let mut s = Toss { pos: vec![], vel: vec![], rest };
    let cfg = BakeConfig { timing: SimTiming { start_frame: 1.0, end_frame: 24.0, fps: 24.0, substeps: 4 }, seed: 9, ..BakeConfig::default() };
    let cache = modelcraft_sim::bake(&mut s, &cfg, &mut Silent).unwrap();
    let path = temp(name);
    std::fs::write(&path, PointCache::from_frame_cache(&cache).to_bytes().unwrap()).unwrap();
    (path, cache)
}

#[test]
fn a_baked_point_cache_plays_on_the_timeline_and_survives_undo_and_save() {
    let mut d = Document::new();
    let id = plane(&mut d);
    let rest = rest_points(&mut d, id);
    let (path, cache) = baked_file(rest.clone(), "toss.mcpc");
    let out = d.execute(Command::Sim(S::AttachCache { mesh: id, path: path.to_string_lossy().into(), start_frame: None })).unwrap();
    let sim = out.created[0];
    assert_eq!(d.scene.get(sim).unwrap().parent, Some(id));
    assert_eq!(d.scene.get(sim).unwrap().kind.type_name(), "Simulation");
    // Frame 1 is the rest state; later frames are the cached ones.
    d.set_frame(1.0);
    assert_eq!(rest_points(&mut d, id), rest);
    for f in [2.0, 13.0, 24.0] {
        d.set_frame(f);
        let got = rest_points(&mut d, id);
        assert_eq!(&got[..], cache.frame(f as usize - 1).unwrap(), "frame {f}");
    }
    d.set_frame(18.5);
    let mid = rest_points(&mut d, id);
    let (a, b) = (cache.frame(17).unwrap()[3], cache.frame(18).unwrap()[3]);
    assert!((mid[3] - (a + b) * 0.5).length() < 1e-5, "between frames it interpolates");
    // The sim is not an object of its own in the export, and undo removes it.
    assert!(d.export_items().iter().all(|i| i.name != "Cache"));
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.sim_children(id).is_empty());
    d.set_frame(24.0);
    assert_eq!(rest_points(&mut d, id), rest, "without the simulation the mesh is itself again");
    d.execute(Command::Redo).unwrap();
    assert_eq!(&rest_points(&mut d, id)[..], cache.frame(23).unwrap());
    // Save, reopen: the cache comes back from the project file.
    let project = temp("toss.modelcraft");
    d.save_as(&project).unwrap();
    let mut back = Document::open(&project).unwrap();
    back.set_frame(13.0);
    assert_eq!(&rest_points(&mut back, id)[..], cache.frame(12).unwrap());
    // The simulation's cache exports as a point cache again.
    let again = back.sim_point_cache(sim).unwrap();
    assert_eq!((again.frames(), again.vertices), (24, rest.len()));
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(project);
}

#[test]
fn the_start_frame_of_the_file_can_be_moved() {
    let mut d = Document::new();
    let id = plane(&mut d);
    let (path, cache) = baked_file(rest_points(&mut d, id), "shift.mcpc");
    let sim = d.execute(Command::Sim(S::AttachCache { mesh: id, path: path.to_string_lossy().into(), start_frame: Some(101.0) })).unwrap().created[0];
    d.set_frame(101.0);
    assert_eq!(&rest_points(&mut d, id)[..], cache.frame(0).unwrap());
    d.set_frame(112.0);
    assert_eq!(&rest_points(&mut d, id)[..], cache.frame(11).unwrap());
    d.set_frame(1.0);
    assert_eq!(&rest_points(&mut d, id)[..], cache.frame(0).unwrap(), "before the start the first frame holds");
    let ObjectKind::Sim { sim } = &d.scene.get(sim).unwrap().kind else { panic!() };
    assert_eq!((sim.timing.start_frame, sim.timing.end_frame), (101.0, 124.0));
    let _ = std::fs::remove_file(path);
}

#[test]
fn files_that_do_not_fit_are_refused_without_changing_the_scene() {
    let mut d = Document::new();
    let id = plane(&mut d);
    let group = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let n = rest_points(&mut d, id).len();
    let (wrong, _) = baked_file(vec![Vec3::ZERO; n - 1], "wrong.mcpc");
    let garbage = temp("garbage.mcpc");
    std::fs::write(&garbage, b"not a cache at all").unwrap();
    let (good, _) = baked_file(rest_points(&mut d, id), "good.mcpc");
    let objects = d.scene.len();
    let attach = |d: &mut Document, mesh, p: &std::path::Path, start| {
        d.execute(Command::Sim(S::AttachCache { mesh, path: p.to_string_lossy().into(), start_frame: start }))
    };
    let e = attach(&mut d, id, &wrong, None).unwrap_err().to_string();
    assert!(e.contains("points per frame but the mesh has"), "{e}");
    assert!(attach(&mut d, id, &garbage, None).is_err());
    assert!(attach(&mut d, id, &temp("missing.mcpc"), None).is_err());
    assert!(attach(&mut d, group, &good, None).unwrap_err().to_string().contains("mesh object"));
    assert!(attach(&mut d, id, &good, Some(f64::NAN)).is_err());
    assert_eq!(d.scene.len(), objects, "nothing was added");
    attach(&mut d, id, &good, None).unwrap();
    let e = attach(&mut d, id, &good, None).unwrap_err().to_string();
    assert!(e.contains("already has a simulation"), "{e}");
    for f in [wrong, garbage, good] {
        let _ = std::fs::remove_file(f);
    }
}

#[test]
fn settings_are_checked_and_a_file_cache_cannot_be_cleared() {
    let mut d = Document::new();
    let id = plane(&mut d);
    let (path, _) = baked_file(rest_points(&mut d, id), "settings.mcpc");
    let sim = d.execute(Command::Sim(S::AttachCache { mesh: id, path: path.to_string_lossy().into(), start_frame: None })).unwrap().created[0];
    let set = |d: &mut Document, c: S| d.execute(Command::Sim(c));
    // (start, end, fps, substeps, seed, budget)
    type Args = (Option<f64>, Option<f64>, Option<f64>, Option<u32>, Option<u64>, Option<u32>);
    let cmd = |a: Args| S::Set { id: sim, start_frame: a.0, end_frame: a.1, fps: a.2, substeps: a.3, seed: a.4, max_cache_mib: a.5 };
    set(&mut d, cmd((None, None, None, Some(8), Some(77), Some(64)))).unwrap();
    let ObjectKind::Sim { sim: s } = &d.scene.get(sim).unwrap().kind else { panic!() };
    assert_eq!((s.timing.substeps, s.seed, s.max_cache_mib), (8, 77, 64));
    assert!(s.cache.is_some(), "settings do not drop the cache");
    assert!(!d.sim_stale(sim), "a cache from a file is never stale");
    for bad in [
        (None, None, None, Some(0), None, None),
        (None, None, None, Some(1000), None, None),
        (None, None, Some(-1.0), None, None, None),
        (Some(f64::NAN), None, None, None, None, None),
        (Some(500.0), Some(5.0), None, None, None, None),
        (None, None, None, None, None, Some(0)),
        (None, None, None, None, None, Some(5_000_000)),
    ] {
        assert!(set(&mut d, cmd(bad)).is_err(), "{bad:?}");
    }
    assert!(set(&mut d, S::ClearCache { id: sim }).unwrap_err().to_string().contains("came from a file"));
    assert!(set(&mut d, S::ClearCache { id }).is_err(), "a mesh is not a simulation");
    assert!(set(&mut d, S::ClearCache { id: ObjectId(999) }).is_err());
    // Deleting the simulation object removes it.
    d.execute(Command::Delete { ids: vec![sim] }).unwrap();
    assert!(d.scene.sim_children(id).is_empty());
    let _ = std::fs::remove_file(path);
}

#[test]
fn commands_round_trip_through_json_and_the_list_names_each_op() {
    for c in [
        Command::Sim(S::AttachCache { mesh: ObjectId(3), path: "a.mcpc".into(), start_frame: Some(5.0) }),
        Command::Sim(S::Set { id: ObjectId(4), start_frame: None, end_frame: Some(80.0), fps: None, substeps: Some(6), seed: Some(3), max_cache_mib: None }),
        Command::Sim(S::ClearCache { id: ObjectId(4) }),
    ] {
        let j = serde_json::to_string(&c).unwrap();
        assert!(j.contains(r#""cmd":"sim""#), "{j}");
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c, "{j}");
        assert!(!c.label().is_empty());
    }
    let e = serde_json::from_str::<Command>(r#"{"cmd":"sim","op":"wobble"}"#).unwrap_err().to_string();
    assert!(e.contains("unknown variant `wobble`"), "{e}");
    for (name, _) in crate::sim::COMMANDS {
        let e = serde_json::from_str::<S>(&format!(r#"{{"op":"{name}"}}"#)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!e.contains("unknown variant"), "{name}: {e}");
    }
}

#[test]
fn the_same_seed_bakes_the_same_file_twice() {
    let mut d = Document::new();
    let id = plane(&mut d);
    let rest = rest_points(&mut d, id);
    let (a, ca) = baked_file(rest.clone(), "twin-a.mcpc");
    let (b, cb) = baked_file(rest, "twin-b.mcpc");
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "byte for byte");
    assert_eq!(ca.checksum(), cb.checksum());
    let _ = (std::fs::remove_file(a), std::fs::remove_file(b));
}
