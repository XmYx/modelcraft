//! Simulations in the scene.
//!
//! A **simulation object** is a child of the mesh it drives ([`modelcraft_sim::SimObject`]). Its
//! cache holds the mesh's points at each frame, in the mesh's own space; the evaluator shows the
//! cached points for the scene's current frame, so playing the timeline never runs a solver.
//! Like a skin, a simulation runs after the mesh's history and deformers, in child order. The
//! mesh's cache key covers the current frame while a simulation with a cache is on
//! ([`sim_key`]), since nothing in the tree changes between frames.

use std::sync::Arc;

use glam::Vec3;
use modelcraft_mesh::PolyMesh;
use modelcraft_sim::SimObject;

use crate::{ObjectId, ObjectKind, Scene};

impl Scene {
    /// The simulations of a mesh object that have a cache: its visible `Sim` children, in order.
    pub fn sims_of(&self, mesh: ObjectId) -> Vec<(ObjectId, Arc<SimObject>)> {
        self.objects()
            .iter()
            .filter(|o| o.parent == Some(mesh) && o.visible)
            .filter_map(|o| match &o.kind {
                ObjectKind::Sim { sim } if sim.cache.is_some() => Some((o.id, sim.clone())),
                _ => None,
            })
            .collect()
    }

    /// Every simulation object under (or at) `id`'s children, with or without a cache.
    pub fn sim_children(&self, mesh: ObjectId) -> Vec<(ObjectId, Arc<SimObject>)> {
        self.objects()
            .iter()
            .filter(|o| o.parent == Some(mesh))
            .filter_map(|o| match &o.kind {
                ObjectKind::Sim { sim } => Some((o.id, sim.clone())),
                _ => None,
            })
            .collect()
    }
}

/// A number that changes with the current frame while the mesh has a visible simulation with a
/// cache; zero otherwise.
pub fn sim_key(scene: &Scene, mesh: ObjectId) -> u64 {
    if scene.sims_of(mesh).is_empty() {
        return 0;
    }
    (scene.anim.frame.to_bits().wrapping_mul(0x0100_0000_01b3) ^ 0x6a09_e667_f3bc_c908) | 1
}

/// Show the cached points of a mesh object's simulations at the current frame. Returns the
/// changed mesh (`None` when nothing applies) and messages for caches that could not be shown.
pub fn apply_sims(scene: &Scene, mesh_id: ObjectId, mesh: &PolyMesh) -> (Option<PolyMesh>, Vec<String>) {
    let sims = scene.sims_of(mesh_id);
    if sims.is_empty() {
        return (None, Vec::new());
    }
    let mut errors = Vec::new();
    let mut points: Vec<Vec3> = Vec::new();
    let mut shown = false;
    for (_, s) in sims {
        let Some(cache) = &s.cache else { continue };
        if cache.points() != mesh.vertex_count() {
            errors.push(format!("the simulation cache has {} points but the mesh has {}: bake it again", cache.points(), mesh.vertex_count()));
            continue;
        }
        if cache.sample(scene.anim.frame, &mut points) {
            shown = true;
        }
    }
    if !shown {
        return (None, errors);
    }
    let mut out = mesh.clone();
    // A dead (NaN) point cannot be a mesh vertex: keep it where the mesh has it.
    for (i, p) in points.iter_mut().enumerate() {
        if !p.is_finite() {
            *p = mesh.positions().get(i).copied().unwrap_or(Vec3::ZERO);
        }
    }
    match out.set_positions(&points) {
        Ok(()) => (Some(out), errors),
        Err(e) => {
            errors.push(e.to_string());
            (None, errors)
        }
    }
}

/// Repair the simulation objects of a scene read from a file.
pub(crate) fn repair(scene: &mut Scene) {
    let ids: Vec<ObjectId> = scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Sim { .. })).map(|o| o.id).collect();
    for id in ids {
        let Ok(o) = scene.edit(id) else { continue };
        if let ObjectKind::Sim { sim } = &mut o.kind {
            Arc::make_mut(sim).sanitize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evaluator, Source};
    use modelcraft_ops::primitives::Primitive;
    use modelcraft_sim::{FrameCache, SimKind};

    fn plane() -> (Scene, ObjectId) {
        let mut s = Scene::new();
        let kind = ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::default_of("plane").unwrap() }, steps: vec![] };
        let id = s.add("Plane", kind, None).unwrap();
        (s, id)
    }

    /// A cache that lifts every point by `frame` units along Y.
    fn rising(rest: &[Vec3], frames: usize) -> FrameCache {
        let mut c = FrameCache::new(24.0, 1.0, 1.0, rest.len());
        for f in 0..frames {
            let pts: Vec<Vec3> = rest.iter().map(|p| *p + Vec3::Y * f as f32).collect();
            c.push(&pts, usize::MAX).unwrap();
        }
        c
    }

    fn add_sim(s: &mut Scene, mesh: ObjectId, cache: FrameCache) -> ObjectId {
        let mut sim = SimObject::new(SimKind::Cache);
        sim.cache = Some(Arc::new(cache));
        s.add("Cache", ObjectKind::Sim { sim: Arc::new(sim) }, Some(mesh)).unwrap()
    }

    #[test]
    fn the_timeline_plays_the_cache_and_only_then() {
        let (mut s, mesh) = plane();
        let mut ev = Evaluator::new();
        let rest = ev.evaluate(&s, mesh).mesh;
        assert_eq!(sim_key(&s, mesh), 0);
        let sim = add_sim(&mut s, mesh, rising(rest.positions(), 5));
        s.set_frame(1.0);
        let at1 = ev.evaluate(&s, mesh).mesh;
        assert!(at1.positions().iter().zip(rest.positions()).all(|(a, b)| (*a - *b).length() < 1e-6), "frame 1 is the rest state");
        s.set_frame(4.0);
        let at4 = ev.evaluate(&s, mesh).mesh;
        assert!(at4.positions().iter().zip(rest.positions()).all(|(a, b)| ((*a - *b) - Vec3::Y * 3.0).length() < 1e-5));
        s.set_frame(2.5);
        let mid = ev.evaluate(&s, mesh).mesh;
        assert!((mid.position(0) - rest.position(0) - Vec3::Y * 1.5).length() < 1e-5, "between frames it interpolates");
        s.set_frame(99.0);
        assert!((ev.evaluate(&s, mesh).mesh.position(0) - rest.position(0) - Vec3::Y * 4.0).length() < 1e-5, "after the cache the last frame holds");
        // The same frame is cached; a hidden simulation is off.
        let a = ev.evaluate(&s, mesh).mesh;
        assert!(Arc::ptr_eq(&a, &ev.evaluate(&s, mesh).mesh));
        s.edit(sim).unwrap().visible = false;
        assert!(ev.evaluate(&s, mesh).mesh.positions().iter().zip(rest.positions()).all(|(a, b)| (*a - *b).length() < 1e-6));
        assert_eq!(sim_key(&s, mesh), 0);
    }

    #[test]
    fn a_cache_of_another_size_is_skipped_and_says_so() {
        let (mut s, mesh) = plane();
        let mut ev = Evaluator::new();
        let rest = ev.evaluate(&s, mesh).mesh;
        add_sim(&mut s, mesh, rising(&rest.positions()[..2], 3));
        let e = ev.evaluate(&s, mesh);
        assert_eq!(e.errors.len(), 1);
        assert!(e.errors[0].message.contains("bake it again"));
        assert!(e.mesh.positions().iter().zip(rest.positions()).all(|(a, b)| (*a - *b).length() < 1e-6));
    }

    #[test]
    fn dead_points_keep_the_meshs_own_position() {
        let (mut s, mesh) = plane();
        let mut ev = Evaluator::new();
        let rest = ev.evaluate(&s, mesh).mesh;
        let mut c = FrameCache::new(24.0, 1.0, 1.0, rest.vertex_count());
        let mut f = rest.positions().to_vec();
        f[0] = modelcraft_sim::DEAD;
        c.push(&f, usize::MAX).unwrap();
        add_sim(&mut s, mesh, c);
        s.set_frame(1.0);
        let e = ev.evaluate(&s, mesh);
        assert!(e.errors.is_empty());
        assert_eq!(e.mesh.position(0), rest.position(0));
    }

    #[test]
    fn simulation_objects_repair_and_survive_the_json_round_trip_without_the_cache() {
        let (mut s, mesh) = plane();
        let rest = Evaluator::new().evaluate(&s, mesh).mesh;
        let id = add_sim(&mut s, mesh, rising(rest.positions(), 2));
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("data"), "no points in the project JSON");
        let back: Scene = serde_json::from_str(&json).unwrap();
        let back = back.repaired().unwrap();
        let ObjectKind::Sim { sim } = &back.get(id).unwrap().kind else { panic!() };
        assert!(sim.cache.is_none());
        let hostile = json.replace("\"substeps\":4", "\"substeps\":4000").replace("\"fps\":24.0", "\"fps\":-1.0");
        let back: Scene = serde_json::from_str(&hostile).unwrap();
        let back = back.repaired().unwrap();
        let ObjectKind::Sim { sim } = &back.get(id).unwrap().kind else { panic!() };
        assert!(sim.timing.validate().is_ok());
    }
}
