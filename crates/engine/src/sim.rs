//! Simulation commands. One command, `sim`, with the operation in `op`:
//! `{"cmd":"sim","op":"attach_cache","mesh":3,"path":"fall.mcpc"}`.
//!
//! A simulation is an object under the mesh it drives ([`modelcraft_scene::sim`]); its cache plays
//! back by the scene's frame, so the timeline scrubs it without running a solver.

use std::path::Path;
use std::sync::Arc;

use modelcraft_formats::FormatError;
use modelcraft_formats::pointcache::PointCache;
use modelcraft_scene::{ObjectId, ObjectKind, SceneError};
use modelcraft_sim::rigid::RigidSettings;
use modelcraft_sim::{SimKind, SimObject, SimTiming};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError, MAX_IMPORT_BYTES};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SimCommand {
    /// Play the points of a `.mcpc` file on a mesh. The file must hold one position per vertex of
    /// the mesh. `start_frame` is the scene frame of the file's first sample (the file's own
    /// start without it).
    AttachCache {
        mesh: ObjectId,
        path: String,
        #[serde(default)]
        start_frame: Option<f64>,
    },
    /// Change a simulation's start frame, end frame, frame rate, substeps, seed or cache budget.
    /// A baked cache stays, and is marked stale when what it was baked with changed.
    Set {
        id: ObjectId,
        #[serde(default)]
        start_frame: Option<f64>,
        #[serde(default)]
        end_frame: Option<f64>,
        #[serde(default)]
        fps: Option<f64>,
        #[serde(default)]
        substeps: Option<u32>,
        #[serde(default)]
        seed: Option<u64>,
        #[serde(default)]
        max_cache_mib: Option<u32>,
    },
    /// Throw the cache away; the mesh shows its own shape again until the next bake.
    ClearCache { id: ObjectId },
    /// Give a mesh a simulation. `kind`: `rigid_active` (falls and collides) or `rigid_passive`
    /// (a collider that may be animated). Its timing starts as the scene's time range.
    Add { mesh: ObjectId, kind: String },
    /// Replace a simulation's settings (its `kind` with the fields of that kind, e.g.
    /// `{"type":"rigid","role":"active","mass":2,…}`). A baked cache stays and is stale until the
    /// next bake.
    SetKind { id: ObjectId, kind: SimKind },
    /// Run the simulation from its start frame and cache every frame to its end frame. Rigid
    /// bodies share one world, so this bakes every rigid body of the scene, with the timing, seed,
    /// gravity and ground of the one named here. The same seed gives the same cache.
    Bake { id: ObjectId },
}

/// The `op` names of the `sim` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("attach_cache", "Play a .mcpc point cache on a mesh: mesh, path, start_frame? (the scene frame of the file's first sample)"),
    ("set", "Change a simulation: id, start_frame?, end_frame?, fps?, substeps? (1 to 64), seed?, max_cache_mib?"),
    ("clear_cache", "Throw a baked cache away: id"),
    ("add", "Give a mesh a simulation: mesh, kind (rigid_active, rigid_passive)"),
    (
        "set_kind",
        "Replace a simulation's settings: id, kind {type: rigid, role, shape, mass, friction, bounciness, velocity, spin, gravity, ground, joints, …}",
    ),
    ("bake", "Simulate from the start frame to the end frame and cache every frame (all rigid bodies together): id"),
];

impl SimCommand {
    pub fn label(&self) -> String {
        match self {
            SimCommand::AttachCache { .. } => "Attach cache",
            SimCommand::Set { .. } => "Simulation settings",
            SimCommand::ClearCache { .. } => "Clear cache",
            SimCommand::Add { .. } => "Add simulation",
            SimCommand::SetKind { .. } => "Simulation settings",
            SimCommand::Bake { .. } => "Bake simulation",
        }
        .into()
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

/// The simulation objects under a mesh.
fn sim_of(doc: &Document, mesh: ObjectId) -> Option<(ObjectId, Arc<SimObject>)> {
    doc.scene.sim_children(mesh).into_iter().next()
}

fn sim_object(doc: &Document, id: ObjectId) -> Result<Arc<SimObject>, EngineError> {
    match doc.scene.get(id).map(|o| &o.kind) {
        Some(ObjectKind::Sim { sim }) => Ok(sim.clone()),
        Some(_) => invalid(format!("object {} is not a simulation", id.0)),
        None => Err(SceneError::NoObject(id).into()),
    }
}

pub fn apply(doc: &mut Document, cmd: &SimCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        SimCommand::AttachCache { mesh, path, start_frame } => {
            if !matches!(doc.scene.get(*mesh).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })) {
                return invalid("a cache plays on a mesh object");
            }
            if let Some(s) = start_frame
                && !s.is_finite()
            {
                return invalid("the start frame must be a number");
            }
            if sim_of(doc, *mesh).is_some() {
                return invalid("the mesh already has a simulation: clear or delete it first");
            }
            let p = Path::new(path);
            let meta = std::fs::metadata(p).map_err(|e| FormatError::io(p, e))?;
            if meta.len() > MAX_IMPORT_BYTES {
                return invalid(format!("{path} is too large"));
            }
            let bytes = std::fs::read(p).map_err(|e| FormatError::io(p, e))?;
            let file = PointCache::from_bytes(&bytes)?;
            let mut cache = file.to_frame_cache()?;
            if let Some(s) = start_frame {
                cache.start = *s;
            }
            let verts = doc.eval.evaluate(&doc.scene, *mesh).mesh.vertex_count();
            if cache.points() != verts {
                return invalid(format!("{path} holds {} points per frame but the mesh has {verts} vertices", cache.points()));
            }
            if cache.is_empty() {
                return invalid(format!("{path} holds no frames"));
            }
            let mut sim = SimObject::new(SimKind::Cache);
            sim.timing.start_frame = cache.start;
            sim.timing.end_frame = cache.end_frame();
            sim.timing.fps = cache.fps;
            sim.cache = Some(Arc::new(cache));
            let name = doc.scene.unique_name("Cache");
            out.created.push(doc.scene.add(name, ObjectKind::Sim { sim: Arc::new(sim) }, Some(*mesh))?);
        }
        SimCommand::Set { id, start_frame, end_frame, fps, substeps, seed, max_cache_mib } => {
            let sim = sim_object(doc, *id)?;
            let mut s = (*sim).clone();
            if let Some(v) = start_frame {
                s.timing.start_frame = *v;
            }
            if let Some(v) = end_frame {
                s.timing.end_frame = *v;
            }
            if let Some(v) = fps {
                s.timing.fps = *v;
            }
            if let Some(v) = substeps {
                s.timing.substeps = *v;
            }
            if let Some(v) = seed {
                s.seed = *v;
            }
            if let Some(v) = max_cache_mib {
                s.max_cache_mib = *v;
            }
            s.timing.validate().map_err(|e| EngineError::Invalid(e.to_string()))?;
            if !(1..=(modelcraft_sim::MAX_CACHE_BYTES >> 20) as u32).contains(&s.max_cache_mib) {
                return invalid("the cache budget is 1 MiB up to 2 GiB");
            }
            if let ObjectKind::Sim { sim } = &mut doc.scene.edit(*id)?.kind {
                *sim = Arc::new(s);
            }
        }
        SimCommand::Add { mesh, kind } => {
            if !matches!(doc.scene.get(*mesh).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })) {
                return invalid("a simulation drives a mesh object");
            }
            if sim_of(doc, *mesh).is_some() {
                return invalid("the mesh already has a simulation: delete it first");
            }
            let (kind, label) = match kind.as_str() {
                "rigid_active" => (SimKind::Rigid(RigidSettings::default()), "Rigid body"),
                "rigid_passive" => (SimKind::Rigid(RigidSettings::passive()), "Collider"),
                other => return invalid(format!("unknown simulation {other:?} (rigid_active, rigid_passive)")),
            };
            let mut sim = SimObject::new(kind);
            let t = doc.scene.anim.time;
            sim.timing = SimTiming { start_frame: t.start, end_frame: t.end, fps: t.fps, substeps: sim.timing.substeps }.sanitized();
            let name = doc.scene.unique_name(label);
            out.created.push(doc.scene.add(name, ObjectKind::Sim { sim: Arc::new(sim) }, Some(*mesh))?);
        }
        SimCommand::SetKind { id, kind } => {
            let sim = sim_object(doc, *id)?;
            if sim.kind == SimKind::Cache || *kind == SimKind::Cache {
                return invalid("a cache from a file has no settings to change");
            }
            let mut s = (*sim).clone();
            s.kind = kind.clone();
            s.sanitize();
            if let ObjectKind::Sim { sim } = &mut doc.scene.edit(*id)?.kind {
                *sim = Arc::new(s);
            }
        }
        SimCommand::Bake { id } => {
            let result = match doc.sim_ready.take() {
                Some(r) if r.target == *id => r,
                _ => crate::sim_bake::prepare(doc, *id)?.run(&mut modelcraft_sim::Silent).map_err(|e| EngineError::Invalid(e.to_string()))?,
            };
            crate::sim_bake::store(doc, result, out)?;
        }
        SimCommand::ClearCache { id } => {
            let sim = sim_object(doc, *id)?;
            if sim.kind == SimKind::Cache {
                return invalid("this cache came from a file; delete the simulation to remove it");
            }
            if let ObjectKind::Sim { sim } = &mut doc.scene.edit(*id)?.kind {
                let s = Arc::make_mut(sim);
                s.cache = None;
                s.baked_with = 0;
            }
        }
    }
    Ok(())
}

impl Document {
    /// The cache of a simulation object, ready to be written as a `.mcpc` file.
    pub fn sim_point_cache(&self, id: ObjectId) -> Result<PointCache, EngineError> {
        let sim = sim_object(self, id)?;
        let cache = sim.cache.as_ref().ok_or_else(|| EngineError::Invalid("the simulation has no cache: bake it first".into()))?;
        Ok(PointCache::from_frame_cache(cache))
    }
}
