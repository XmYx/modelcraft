//! The dependency graph evaluator: caches step outputs and final meshes per object, keyed by
//! revisions, so an edit re-runs only the steps at and after the change.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;
use modelcraft_ops::{OpError, edit, subdivide};

use crate::{ObjectId, ObjectKind, PolyMesh, Scene, Source, Step, StepOp};

/// A step that could not run (its selection no longer fits the mesh, a size limit…). The
/// evaluator skips it and goes on, so a stale step never breaks the object.
#[derive(Clone, Debug, PartialEq)]
pub struct StepError {
    pub step: usize,
    pub message: String,
}

/// The result of evaluating an object.
#[derive(Clone, Debug, Default)]
pub struct Evaluated {
    pub mesh: Arc<PolyMesh>,
    pub errors: Vec<StepError>,
}

#[derive(Default)]
struct StepCache {
    rev: u64,
    /// The source and steps the outputs were computed from.
    seen_source: Option<Source>,
    seen_steps: Vec<Step>,
    source: Option<Arc<PolyMesh>>,
    /// Output after each step (index i = after step i).
    outputs: Vec<Arc<PolyMesh>>,
    errors: Vec<StepError>,
}

/// Evaluator with caches; one per document. See the module docs.
#[derive(Default)]
pub struct Evaluator {
    steps: HashMap<ObjectId, StepCache>,
    finals: HashMap<ObjectId, (u64, Evaluated)>,
    /// Count of step evaluations performed (tests and stats).
    pub runs: u64,
}

impl Evaluator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.steps.clear();
        self.finals.clear();
    }

    /// Drop caches of objects that no longer exist.
    pub fn retain(&mut self, scene: &Scene) {
        self.steps.retain(|id, _| scene.get(*id).is_some());
        self.finals.retain(|id, _| scene.get(*id).is_some());
    }

    /// The mesh after the first `upto` steps of a mesh object (all of them for `usize::MAX`),
    /// before deformers. Empty for other kinds.
    pub fn steps_output(&mut self, scene: &Scene, id: ObjectId, upto: usize) -> Evaluated {
        let Some(obj) = scene.get(id) else { return Evaluated::default() };
        let ObjectKind::Mesh { source, steps } = &obj.kind else { return Evaluated::default() };
        let cache = self.steps.entry(id).or_default();
        if cache.rev != obj.rev {
            // Keep the outputs of the leading steps that did not change.
            cache.rev = obj.rev;
            let same_source = cache.seen_source.as_ref().is_some_and(|s| same_source(s, source));
            let keep = if same_source { cache.seen_steps.iter().zip(steps).take_while(|(a, b)| same_step(a, b)).count() } else { 0 };
            cache.outputs.truncate(keep);
            cache.errors.retain(|e| e.step < keep);
            if !same_source {
                cache.source = None;
                cache.seen_source = Some(source.clone());
            }
            cache.seen_steps = steps.clone();
        }
        let src = match &cache.source {
            Some(s) => s.clone(),
            None => {
                let s = match source {
                    Source::Mesh { mesh } => mesh.clone(),
                    Source::Primitive { primitive } => match primitive.build() {
                        Ok(m) => Arc::new(m),
                        Err(e) => {
                            cache.errors.push(StepError { step: usize::MAX, message: e.to_string() });
                            Arc::new(PolyMesh::default())
                        }
                    },
                };
                cache.source = Some(s.clone());
                s
            }
        };
        let want = upto.min(steps.len());
        while cache.outputs.len() < want {
            let i = cache.outputs.len();
            let prev = cache.outputs.last().cloned().unwrap_or_else(|| src.clone());
            let Some(step) = steps.get(i) else { break };
            let out = if step.enabled {
                self.runs += 1;
                match run_step(&prev, &step.op) {
                    Ok(m) => Arc::new(m),
                    Err(e) => {
                        cache.errors.push(StepError { step: i, message: e.to_string() });
                        prev
                    }
                }
            } else {
                prev
            };
            cache.outputs.push(out);
        }
        let mesh = if want == 0 { src } else { cache.outputs.get(want - 1).cloned().unwrap_or(src) };
        Evaluated { mesh, errors: cache.errors.iter().filter(|e| e.step < want || e.step == usize::MAX).cloned().collect() }
    }

    /// The final mesh of an object in its own space: steps, then deformer children; for a smooth
    /// cage, its first mesh child subdivided. Groups and deformers give an empty mesh.
    pub fn evaluate(&mut self, scene: &Scene, id: ObjectId) -> Evaluated {
        self.evaluate_depth(scene, id, 0)
    }

    fn evaluate_depth(&mut self, scene: &Scene, id: ObjectId, depth: usize) -> Evaluated {
        let Some(obj) = scene.get(id) else { return Evaluated::default() };
        if depth > crate::MAX_DEPTH {
            return Evaluated::default();
        }
        // A skinned mesh also depends on where its joints are.
        let key = scene.deep_rev(id) ^ crate::rig::pose_key(scene, id);
        if let Some((k, e)) = self.finals.get(&id)
            && *k == key
        {
            return e.clone();
        }
        let result = match &obj.kind {
            ObjectKind::Mesh { .. } => {
                let mut e = self.steps_output(scene, id, usize::MAX);
                let deformers: Vec<(Mat4, crate::Deformer)> = scene
                    .children(id)
                    .into_iter()
                    .filter_map(|c| scene.get(c))
                    .filter(|c| c.visible)
                    .filter_map(|c| match &c.kind {
                        ObjectKind::Deformer { deformer } => Some((c.transform.matrix().inverse(), *deformer)),
                        _ => None,
                    })
                    .collect();
                if !deformers.is_empty() {
                    let mut m = (*e.mesh).clone();
                    for (to_local, d) in deformers {
                        d.apply(&mut m, to_local);
                    }
                    e.mesh = Arc::new(m);
                }
                let (skinned, problems) = crate::rig::apply_skins(scene, id, &e.mesh);
                if let Some(m) = skinned {
                    e.mesh = Arc::new(m);
                }
                e.errors.extend(problems.into_iter().map(|message| StepError { step: usize::MAX, message }));
                e
            }
            ObjectKind::SmoothCage { levels } => {
                let child = scene.children(id).into_iter().find(|c| scene.get(*c).is_some_and(|o| matches!(o.kind, ObjectKind::Mesh { .. })));
                match child {
                    None => Evaluated::default(),
                    Some(c) => {
                        let inner = self.evaluate_depth(scene, c, depth + 1);
                        let local = scene.get(c).map(|o| o.transform.matrix()).unwrap_or(Mat4::IDENTITY);
                        let mut m = (*inner.mesh).clone();
                        m.map_positions(|_, p| local.transform_point3(p));
                        self.runs += 1;
                        match subdivide::catmull_clark(&m, *levels) {
                            Ok(s) => Evaluated { mesh: Arc::new(s), errors: inner.errors },
                            Err(e) => {
                                let mut errors = inner.errors;
                                errors.push(StepError { step: usize::MAX, message: e.to_string() });
                                Evaluated { mesh: Arc::new(m), errors }
                            }
                        }
                    }
                }
            }
            ObjectKind::Group | ObjectKind::Deformer { .. } | ObjectKind::Joint { .. } | ObjectKind::Skin { .. } => Evaluated::default(),
        };
        self.finals.insert(id, (key, result.clone()));
        result
    }
}

fn same_source(a: &Source, b: &Source) -> bool {
    match (a, b) {
        (Source::Mesh { mesh: x }, Source::Mesh { mesh: y }) => Arc::ptr_eq(x, y),
        (Source::Primitive { primitive: x }, Source::Primitive { primitive: y }) => x == y,
        _ => false,
    }
}

/// Cheap step comparison (sculpt layers by identity, not by content).
fn same_step(a: &Step, b: &Step) -> bool {
    match (&a.op, &b.op) {
        (StepOp::Sculpt { layer: x }, StepOp::Sculpt { layer: y }) => {
            a.id == b.id
                && a.enabled == b.enabled
                && x.level == y.level
                && x.topology.as_ref().map(Arc::as_ptr) == y.topology.as_ref().map(Arc::as_ptr)
                && Arc::ptr_eq(&x.positions, &y.positions)
                && Arc::ptr_eq(&x.mask, &y.mask)
        }
        _ => a == b,
    }
}

/// Run one history step.
pub fn run_step(m: &PolyMesh, op: &StepOp) -> Result<PolyMesh, OpError> {
    let (nv, ne, nf) = (m.vertex_count(), m.edge_count(), m.face_count());
    match op {
        StepOp::Extrude { faces, distance } => {
            edit::check_selection(faces, nf, "face")?;
            edit::extrude_faces(m, faces, *distance)
        }
        StepOp::Inset { faces, amount } => {
            edit::check_selection(faces, nf, "face")?;
            edit::inset_faces(m, faces, *amount)
        }
        StepOp::Bevel { edges, width } => {
            edit::check_selection(edges, ne, "edge")?;
            edit::bevel_edges(m, edges, *width)
        }
        StepOp::LoopCut { edge, cuts } => {
            edit::check_selection(&[*edge], ne, "edge")?;
            edit::loop_cut(m, *edge, *cuts)
        }
        StepOp::Merge { verts } => {
            edit::check_selection(verts, nv, "vertex")?;
            edit::merge_vertices(m, verts)
        }
        StepOp::Weld { distance } => edit::weld(m, *distance),
        StepOp::DeleteFaces { faces } => {
            edit::check_selection(faces, nf, "face")?;
            edit::delete_faces(m, faces)
        }
        StepOp::DeleteEdges { edges } => {
            edit::check_selection(edges, ne, "edge")?;
            edit::delete_edges(m, edges)
        }
        StepOp::DeleteVertices { verts } => {
            edit::check_selection(verts, nv, "vertex")?;
            edit::delete_vertices(m, verts)
        }
        StepOp::MoveVertices { verts, translate, rotate, scale } => {
            edit::check_selection(verts, nv, "vertex")?;
            edit::transform_vertices(m, verts, *translate, *rotate, *scale)
        }
        StepOp::Flip { faces } => edit::flip_faces(m, faces),
        StepOp::Triangulate => edit::triangulate(m),
        StepOp::Subdivide { levels } => subdivide::catmull_clark(m, *levels),
        StepOp::SetPatch { faces, patch } => edit::set_patch(m, faces, *patch),
        StepOp::Bridge { a, b } => {
            edit::check_selection(&[*a, *b], nf, "face")?;
            edit::bridge_faces(m, *a, *b)
        }
        StepOp::FillHole { edge } => {
            edit::check_selection(&[*edge], ne, "edge")?;
            edit::fill_hole(m, *edge)
        }
        StepOp::Mirror { axis, weld } => edit::mirror(m, *axis as usize, *weld),
        StepOp::Sculpt { layer } => {
            let base = layer.topology.as_deref().unwrap_or(m);
            let mut out = if layer.level == 0 { base.clone() } else { subdivide::catmull_clark(base, u32::from(layer.level))? };
            let nv = out.vertex_count();
            if layer.positions.len() != nv {
                return Err(OpError::Invalid(format!("the sculpt layer has {} points but the mesh now has {nv}", layer.positions.len())));
            }
            out.set_positions(&layer.positions)?;
            Ok(out)
        }
        StepOp::EvenRemesh { cell } => modelcraft_ops::remesh::even_remesh(m, *cell),
    }
}
