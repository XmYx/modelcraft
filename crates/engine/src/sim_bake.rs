//! Baking simulations: reading a solver's input from the scene, running it, storing the caches.
//!
//! A bake has three parts so that the middle one can run on another thread with the scene
//! untouched: [`prepare`] reads the scene into a [`BakeJob`] (plain data), [`BakeJob::run`]
//! simulates and reports progress through an [`Observer`] that may cancel, and the `bake`
//! command stores the [`BakeResult`] as caches in one undo step. [`Document::bake_sim`] does all
//! three, and an app that wants a progress bar runs the middle part itself and hands the result
//! to [`Document::store_bake`].

use std::sync::Arc;

use glam::{Mat4, Vec3};
use modelcraft_scene::{Evaluator, ObjectId, ObjectKind};
use modelcraft_sim::rigid::{BodyDesc, BodyRole, RigidSettings, RigidWorld, WorldDesc};
use modelcraft_sim::{BakeConfig, FrameCache, Observer, SimError, SimKind, SimObject, SimTiming, bake};

use crate::command::{Command, Outcome};
use crate::document::{Document, EngineError};
use crate::sim::SimCommand;

type Work = Box<dyn FnOnce(&mut dyn Observer) -> Result<Vec<FrameCache>, SimError> + Send>;

/// Everything a bake needs, read from the scene. Holds no reference to it.
pub struct BakeJob {
    /// The simulation object the user baked.
    pub target: ObjectId,
    pub timing: SimTiming,
    pub seed: u64,
    /// The objects that get a cache, in the order `run` returns them.
    sims: Vec<ObjectId>,
    work: Work,
    /// Settings shared by every body of a rigid world: gravity and ground.
    world: Option<([f32; 3], Option<f32>)>,
}

/// A finished bake, waiting to be stored.
pub struct BakeResult {
    pub target: ObjectId,
    pub timing: SimTiming,
    pub seed: u64,
    sims: Vec<ObjectId>,
    caches: Vec<FrameCache>,
    world: Option<([f32; 3], Option<f32>)>,
}

impl BakeJob {
    /// How many objects this bake will cache.
    pub fn objects(&self) -> usize {
        self.sims.len()
    }

    /// Simulate. Safe to call on another thread.
    pub fn run(self, observer: &mut dyn Observer) -> Result<BakeResult, SimError> {
        let caches = (self.work)(observer)?;
        Ok(BakeResult { target: self.target, timing: self.timing, seed: self.seed, sims: self.sims, caches, world: self.world })
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

fn sim_error(e: SimError) -> EngineError {
    EngineError::Invalid(e.to_string())
}

/// Read a bake of the simulation object `target` from the scene.
pub fn prepare(doc: &Document, target: ObjectId) -> Result<BakeJob, EngineError> {
    let sim = match doc.scene.get(target).map(|o| &o.kind) {
        Some(ObjectKind::Sim { sim }) => sim.clone(),
        Some(_) => return invalid(format!("object {} is not a simulation", target.0)),
        None => return Err(modelcraft_scene::SceneError::NoObject(target).into()),
    };
    sim.timing.validate().map_err(sim_error)?;
    match &sim.kind {
        SimKind::Cache => invalid("this cache came from a file: there is nothing to bake"),
        SimKind::Rigid(settings) => prepare_rigid(doc, target, &sim, settings),
    }
}

/// The rigid simulations of the scene whose mesh is visible: (simulation object, mesh, settings).
fn rigid_sims(doc: &Document) -> Vec<(ObjectId, ObjectId, Arc<SimObject>)> {
    doc.scene
        .objects()
        .iter()
        .filter_map(|o| match (&o.kind, o.parent) {
            (ObjectKind::Sim { sim }, Some(mesh)) if matches!(sim.kind, SimKind::Rigid(_)) && doc.scene.visible(o.id) => Some((o.id, mesh, sim.clone())),
            _ => None,
        })
        .filter(|(_, mesh, _)| matches!(doc.scene.get(*mesh).map(|m| &m.kind), Some(ObjectKind::Mesh { .. })))
        .collect()
}

/// A hash of everything the bake of the rigid world depends on: every rigid body's settings, with
/// the timing and seed. A cache baked under another hash is stale.
pub fn rigid_world_key(doc: &Document) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (id, mesh, s) in rigid_sims(doc) {
        for x in [id.0, mesh.0, s.settings_key()] {
            h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h | 1
}

fn prepare_rigid(doc: &Document, target: ObjectId, sim: &SimObject, world_settings: &RigidSettings) -> Result<BakeJob, EngineError> {
    let timing = sim.timing;
    let sims = rigid_sims(doc);
    if !sims.iter().any(|(id, ..)| *id == target) {
        return invalid("the body is hidden or its mesh is not a mesh object");
    }
    // The scene at the start frame with every simulation switched off, so the bodies start from
    // the shapes of their own meshes.
    let mut scene = doc.scene.clone();
    let all_sims: Vec<ObjectId> = scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Sim { .. })).map(|o| o.id).collect();
    for id in all_sims {
        scene.edit(id)?.visible = false;
    }
    scene.set_frame(timing.start_frame);
    let mut ev = Evaluator::new();
    let frames = timing.frame_count();
    let mut bodies = Vec::with_capacity(sims.len());
    // Per active body: its mesh id, the matrix back to the mesh's own space, the object that caches it.
    let mut outputs: Vec<(u64, Mat4, ObjectId)> = Vec::new();
    for (sim_id, mesh_id, so) in &sims {
        let SimKind::Rigid(settings) = &so.kind else { continue };
        let mesh = ev.evaluate(&scene, *mesh_id).mesh;
        let m0 = scene.world_matrix(*mesh_id);
        let inv = m0.inverse();
        if mesh.vertex_count() == 0 || !m0.is_finite() || !inv.is_finite() {
            return invalid(format!("object {} has no points, or a transform that cannot be undone", mesh_id.0));
        }
        let points: Vec<Vec3> = mesh.positions().iter().map(|p| m0.transform_point3(*p)).collect();
        let triangles = mesh.triangulate().0;
        let motion = if settings.role == BodyRole::Passive && scene.moves(*mesh_id) {
            (0..frames).map(|i| scene.world_matrix_at(*mesh_id, timing.frame_of(i)) * inv).collect()
        } else {
            Vec::new()
        };
        let mut s = settings.clone();
        // The world's settings are the baked body's.
        s.gravity = world_settings.gravity;
        s.ground = world_settings.ground;
        s.sanitize();
        if s.role == BodyRole::Active {
            outputs.push((mesh_id.0, inv, *sim_id));
        }
        bodies.push(BodyDesc { id: mesh_id.0, points, triangles, settings: s, motion });
    }
    if outputs.is_empty() {
        return invalid("the scene has no active rigid body to simulate");
    }
    let gravity = Vec3::from_array(world_settings.gravity);
    let ground = world_settings.ground;
    let cfg = BakeConfig { timing, seed: sim.seed, max_cache_bytes: sim.cache_limit() };
    let desc = WorldDesc { gravity, ground, bodies };
    let sim_ids: Vec<ObjectId> = outputs.iter().map(|(_, _, s)| *s).collect();
    let work: Work = Box::new(move |observer| {
        let mut world = RigidWorld::new(desc)?;
        let ranges = world.outputs();
        let whole = bake(&mut world, &cfg, observer)?;
        // Split the world's points into one cache per body, back in each mesh's own space.
        let mut caches = Vec::with_capacity(outputs.len());
        for (mesh_id, inv, _) in &outputs {
            let range = ranges.iter().find(|(id, _)| id == mesh_id).map(|(_, r)| r.clone()).unwrap_or(0..0);
            let mut cache = FrameCache::new(whole.fps, whole.start, whole.step, range.len());
            let mut local: Vec<Vec3> = Vec::with_capacity(range.len());
            for f in 0..whole.frames() {
                let frame = whole.frame(f).and_then(|fr| fr.get(range.clone())).unwrap_or_default();
                local.clear();
                local.extend(frame.iter().map(|p| inv.transform_point3(*p)));
                cache.push(&local, usize::MAX)?;
            }
            caches.push(cache);
        }
        Ok(caches)
    });
    Ok(BakeJob { target, timing, seed: sim.seed, sims: sim_ids, work, world: Some((world_settings.gravity, ground)) })
}

/// Store a finished bake: every simulated object gets its cache, and the rigid world's shared
/// settings (timing, seed, gravity, ground) are copied to all of its bodies.
pub fn store(doc: &mut Document, r: BakeResult, out: &mut Outcome) -> Result<(), EngineError> {
    // Bodies of the same world share what the bake used.
    let rigid: Vec<ObjectId> = rigid_sims(doc).into_iter().map(|(id, ..)| id).collect();
    for id in &rigid {
        if let ObjectKind::Sim { sim } = &mut doc.scene.edit(*id)?.kind {
            let s = Arc::make_mut(sim);
            s.timing = r.timing;
            s.seed = r.seed;
            if let (SimKind::Rigid(k), Some((g, ground))) = (&mut s.kind, r.world) {
                k.gravity = g;
                k.ground = ground;
            }
        }
    }
    let key = rigid_world_key(doc);
    let n = r.sims.len();
    for (id, cache) in r.sims.into_iter().zip(r.caches) {
        match doc.scene.get(id).map(|o| &o.kind) {
            Some(ObjectKind::Sim { .. }) => {}
            _ => return invalid("a simulation was deleted while it was baking"),
        }
        if let ObjectKind::Sim { sim } = &mut doc.scene.edit(id)?.kind {
            let s = Arc::make_mut(sim);
            s.cache = Some(Arc::new(cache));
            s.baked_with = key;
        }
    }
    out.warnings.push(format!("{n} object{} baked", if n == 1 { "" } else { "s" }));
    Ok(())
}

impl Document {
    /// Bake the simulation object `id` (all of its world) now, without progress or cancelling,
    /// as one undo step.
    pub fn bake_sim(&mut self, id: ObjectId) -> Result<Outcome, EngineError> {
        self.execute(Command::Sim(SimCommand::Bake { id }))
    }

    /// Read a bake from the scene so it can run elsewhere ([`BakeJob::run`]).
    pub fn prepare_bake(&self, id: ObjectId) -> Result<BakeJob, EngineError> {
        prepare(self, id)
    }

    /// Store a bake that ran elsewhere, as the same undo step and journal entry `bake_sim` makes.
    pub fn store_bake(&mut self, result: BakeResult) -> Result<Outcome, EngineError> {
        let id = result.target;
        self.sim_ready = Some(result);
        let r = self.execute(Command::Sim(SimCommand::Bake { id }));
        self.sim_ready = None;
        r
    }

    /// The cache of a simulation no longer matches what it was baked from.
    pub fn sim_stale(&self, id: ObjectId) -> bool {
        let Some(ObjectKind::Sim { sim }) = self.scene.get(id).map(|o| &o.kind) else { return false };
        if sim.cache.is_none() {
            return false;
        }
        match sim.kind {
            SimKind::Cache => false,
            SimKind::Rigid(_) => sim.baked_with != rigid_world_key(self),
        }
    }
}
