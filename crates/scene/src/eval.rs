//! The dependency graph evaluator: caches step outputs and final meshes per object, keyed by
//! revisions, so an edit re-runs only the steps at and after the change.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Mat4, Vec3};
use modelcraft_ops::{OpError, edit, subdivide};

use crate::{CloneSet, ObjectId, ObjectKind, PolyMesh, Scene, Source, Step, StepOp};

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
    /// Things that went less well than asked but did not stop a step: an unwrap that fell back to
    /// a plane or to another solver, a solve that did not converge, UVs left out of a relaxation.
    pub warnings: Vec<StepError>,
    /// For a clone generator: the clones. The mesh is then empty; draw `instances.source` once per
    /// clone instead.
    pub instances: Option<Arc<Instances>>,
}

/// The clones of one generator: one source mesh, drawn many times.
#[derive(Clone, Debug, Default)]
pub struct Instances {
    /// The evaluated mesh every clone shares (in the source object's own space).
    pub source: Arc<PolyMesh>,
    /// The source object (its material colours the clones).
    pub source_object: Option<ObjectId>,
    /// Each clone's matrix in the generator's space; it already includes the source object's own
    /// transform, so a clone's world matrix is `world(generator) * matrix`.
    pub clones: CloneSet,
}

impl Instances {
    /// Triangles drawn in all (source triangles times clones).
    pub fn triangle_count(&self) -> usize {
        self.source.triangle_count().saturating_mul(self.clones.len())
    }
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
    warnings: Vec<StepError>,
}

/// Evaluator with caches; one per document. See the module docs.
#[derive(Default)]
pub struct Evaluator {
    steps: HashMap<ObjectId, StepCache>,
    finals: HashMap<ObjectId, (u64, Evaluated)>,
    /// Evaluated spline objects ([`crate::splines`]).
    pub(crate) splines: crate::splines::SplineCache,
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
        self.splines.clear();
    }

    /// Drop caches of objects that no longer exist.
    pub fn retain(&mut self, scene: &Scene) {
        self.steps.retain(|id, _| scene.get(*id).is_some());
        self.finals.retain(|id, _| scene.get(*id).is_some());
        self.splines.retain(scene);
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
            cache.warnings.retain(|e| e.step < keep);
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
            let out = if step.enabled && step.op.is_marker() {
                // Nothing to compute: share the previous output instead of copying it.
                prev
            } else if step.enabled {
                self.runs += 1;
                let mut warned = Vec::new();
                let result = run_step_warn(&prev, &step.op, &mut warned);
                cache.warnings.extend(warned.into_iter().map(|message| StepError { step: i, message }));
                match result {
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
        Evaluated {
            mesh,
            errors: cache.errors.iter().filter(|e| e.step < want || e.step == usize::MAX).cloned().collect(),
            warnings: cache.warnings.iter().filter(|e| e.step < want).cloned().collect(),
            instances: None,
        }
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
        let key = generator_key(scene, id) ^ crate::rig::pose_key(scene, id) ^ crate::sim::sim_key(scene, id);
        if let Some((k, e)) = self.finals.get(&id)
            && *k == key
        {
            return e.clone();
        }
        let result = match &obj.kind {
            ObjectKind::Mesh { .. } => {
                let mut e = self.steps_output(scene, id, usize::MAX);
                // The shrink-wrap targets of the deformer stack, evaluated first.
                let mut targets = crate::rig::Targets::new();
                for t in crate::rig::stack_targets(scene, id) {
                    targets.insert(t, self.evaluate_depth(scene, t, depth + 1).mesh);
                }
                let (skinned, problems) = crate::rig::apply_stack(scene, id, &e.mesh, &targets);
                if let Some(m) = skinned {
                    e.mesh = Arc::new(m);
                }
                e.errors.extend(problems.into_iter().map(|message| StepError { step: usize::MAX, message }));
                let (simulated, problems) = crate::sim::apply_sims(scene, id, &e.mesh);
                if let Some(m) = simulated {
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
                        let local = scene.get(c).map(|o| o.local_matrix()).unwrap_or(Mat4::IDENTITY);
                        let mut m = (*inner.mesh).clone();
                        m.map_positions(|_, p| local.transform_point3(p));
                        self.runs += 1;
                        match subdivide::catmull_clark(&m, *levels) {
                            Ok(s) => Evaluated { mesh: Arc::new(s), errors: inner.errors, warnings: inner.warnings, instances: None },
                            Err(e) => {
                                let mut errors = inner.errors;
                                errors.push(StepError { step: usize::MAX, message: e.to_string() });
                                Evaluated { mesh: Arc::new(m), errors, warnings: inner.warnings, instances: None }
                            }
                        }
                    }
                }
            }
            ObjectKind::Array { array } => self.generate(scene, id, depth, |m| array.build(m)),
            ObjectKind::Symmetry { axis, weld } => self.generate(scene, id, depth, |m| edit::mirror(m, *axis as usize, *weld)),
            ObjectKind::Cloner { cloner, target } => {
                let mut errors = Vec::new();
                let mut out = Evaluated::default();
                match scene.generator_input(id) {
                    None => errors.push(StepError { step: usize::MAX, message: "the clone generator needs a source object inside it".into() }),
                    Some(c) => {
                        let inner = self.evaluate_depth(scene, c, depth + 1);
                        errors.extend(inner.errors);
                        let local = scene.get(c).map(|o| o.local_matrix()).unwrap_or(Mat4::IDENTITY);
                        // The surface and vertices layouts sit on the target, seen from this generator.
                        let target_mesh = match target.filter(|t| *t != id && cloner.layout.needs_target()) {
                            Some(t) if scene.get(t).is_some() => {
                                let m = self.evaluate_depth(scene, t, depth + 1).mesh;
                                Some((m, target_frame(scene, id, t)))
                            }
                            _ => None,
                        };
                        self.runs += 1;
                        match cloner.generate_in(target_mesh.as_ref().map(|(m, _)| m.as_ref()), target_mesh.as_ref().map_or(Mat4::IDENTITY, |(_, f)| *f)) {
                            Ok(mut set) => {
                                errors.extend(
                                    apply_modifiers(scene, id, cloner.seed, &mut set).into_iter().map(|message| StepError { step: usize::MAX, message }),
                                );
                                for i in &mut set.instances {
                                    i.matrix *= local;
                                }
                                if set.truncated {
                                    errors.push(StepError {
                                        step: usize::MAX,
                                        message: format!("more than {} clones asked for; the rest are left out", crate::MAX_CLONES),
                                    });
                                }
                                out.instances = Some(Arc::new(Instances { source: inner.mesh, source_object: Some(c), clones: set }));
                            }
                            Err(e) => errors.push(StepError { step: usize::MAX, message: e.to_string() }),
                        }
                    }
                }
                out.errors = errors;
                out
            }
            // The master's result, shared: the same mesh allocation, so the GPU upload is shared too.
            ObjectKind::Instance { of } if *of != id => self.evaluate_depth(scene, *of, depth + 1),
            ObjectKind::Sim { sim } => {
                // A particle system is a generator: its pool, as instances of its source object
                // (a mesh inside it) or of a small sphere. Other simulations drive their mesh.
                let mut out = Evaluated::default();
                if let (modelcraft_sim::SimKind::Particles(p), Some(cache)) = (&sim.kind, &sim.cache) {
                    let (source, source_object) = match scene.generator_input(id) {
                        Some(c) => (self.evaluate_depth(scene, c, depth + 1).mesh, Some(c)),
                        None => (crate::sim::particle_sphere(), None),
                    };
                    let inst = crate::sim::particle_instances(scene, id, p, cache, source, source_object);
                    if inst.clones.truncated {
                        out.errors.push(StepError {
                            step: usize::MAX,
                            message: format!("more than {} particles are alive; the rest are not drawn", crate::MAX_CLONES),
                        });
                    }
                    out.instances = Some(Arc::new(inst));
                }
                out
            }
            ObjectKind::Instance { .. }
            | ObjectKind::Group
            | ObjectKind::Deformer { .. }
            | ObjectKind::Field { .. }
            | ObjectKind::CloneModifier { .. }
            | ObjectKind::Joint { .. }
            | ObjectKind::Skin { .. }
            | ObjectKind::PointDeformer { .. }
            | ObjectKind::Spline { .. }
            | ObjectKind::Hair { .. } => Evaluated::default(),
        };
        self.finals.insert(id, (key, result.clone()));
        result
    }
}

impl Evaluator {
    /// A generator that turns its input mesh (in the generator's space) into another mesh. When
    /// the operation fails the input is shown unchanged with the reason.
    fn generate(&mut self, scene: &Scene, id: ObjectId, depth: usize, op: impl FnOnce(&PolyMesh) -> Result<PolyMesh, OpError>) -> Evaluated {
        let Some(c) = scene.generator_input(id) else {
            return Evaluated { errors: vec![StepError { step: usize::MAX, message: "needs a mesh inside it".into() }], ..Evaluated::default() };
        };
        let inner = self.evaluate_depth(scene, c, depth + 1);
        let local = scene.get(c).map(|o| o.transform.matrix()).unwrap_or(Mat4::IDENTITY);
        let mut m = (*inner.mesh).clone();
        m.map_positions(|_, p| local.transform_point3(p));
        // A mirroring transform (a negative scale) turns the faces inside out; put them back.
        if local.determinant() < 0.0 {
            let all: Vec<u32> = (0..m.face_count() as u32).collect();
            if let Ok(flipped) = edit::flip_faces(&m, &all) {
                m = flipped;
            }
        }
        self.runs += 1;
        let mut errors = inner.errors;
        match op(&m) {
            Ok(r) => Evaluated { mesh: Arc::new(r), errors, ..Evaluated::default() },
            Err(e) => {
                errors.push(StepError { step: usize::MAX, message: e.to_string() });
                Evaluated { mesh: Arc::new(m), errors, ..Evaluated::default() }
            }
        }
    }
}

/// The matrix that takes a target object's space into the generator's space.
fn target_frame(scene: &Scene, generator: ObjectId, target: ObjectId) -> Mat4 {
    let to_gen = scene.world_matrix(generator).inverse() * scene.world_matrix(target);
    if to_gen.is_finite() { to_gen } else { Mat4::IDENTITY }
}

/// The cache key of a final mesh. A clone generator also depends on its target and on where the
/// target sits relative to it, which the subtree revision does not cover.
fn generator_key(scene: &Scene, id: ObjectId) -> u64 {
    let mut key = scene.deep_rev(id);
    // A shrink-wrap also depends on its target object and on where it sits relative to the mesh.
    let is_mesh = matches!(scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }));
    for t in if is_mesh { crate::rig::stack_targets(scene, id) } else { Vec::new() } {
        let mut h = key ^ scene.deep_rev(t).rotate_left(29);
        for f in target_frame(scene, id, t).to_cols_array() {
            h = (h ^ u64::from(f.to_bits())).wrapping_mul(0x0100_0000_01b3);
        }
        key = h;
    }
    if matches!(scene.get(id).map(|o| &o.kind), Some(ObjectKind::Cloner { .. })) {
        // A clone modifier also depends on its target object, and on the time when it is animated.
        let mut h = key;
        let mut mix = |bits: u64| h = (h ^ bits).wrapping_mul(0x0100_0000_01b3);
        for m in scene.clone_modifiers(id) {
            let Some(ObjectKind::CloneModifier { modifier, target }) = scene.get(m).map(|o| &o.kind) else { continue };
            if modifier.is_time_dependent() {
                mix(scene.anim.frame.to_bits());
                mix(scene.anim.time.fps.to_bits());
            }
            if let Some(t) = target.filter(|t| scene.get(*t).is_some()) {
                mix(scene.deep_rev(t).rotate_left(21));
                for f in target_frame(scene, id, t).to_cols_array() {
                    mix(u64::from(f.to_bits()));
                }
            }
        }
        key = h;
    }
    if let Some(ObjectKind::Cloner { target: Some(t), .. }) = scene.get(id).map(|o| &o.kind)
        && scene.get(*t).is_some()
    {
        let mut h = key ^ scene.deep_rev(*t).rotate_left(21);
        for f in target_frame(scene, id, *t).to_cols_array() {
            h = (h ^ u64::from(f.to_bits())).wrapping_mul(0x0100_0000_01b3);
        }
        key = h;
    }
    // An instance depends on its master (and on the master's master, when it is an instance too).
    let mut c = id;
    for _ in 0..crate::MAX_DEPTH {
        let Some(ObjectKind::Instance { of }) = scene.get(c).map(|o| &o.kind) else { break };
        if *of == id || scene.get(*of).is_none() {
            break;
        }
        key = key.wrapping_mul(0x0100_0000_01b3) ^ scene.deep_rev(*of).rotate_left(13);
        c = *of;
    }
    key
}

/// Run the clone modifiers inside generator `id`, in order, on the freshly laid out clones (in the
/// generator's space). Returns what went wrong, for the status line.
fn apply_modifiers(scene: &Scene, id: ObjectId, seed: u32, set: &mut modelcraft_clone::CloneSet) -> Vec<String> {
    let mut problems = Vec::new();
    let seconds = (scene.anim.frame / scene.anim.time.fps.max(1e-6)) as f32;
    let to_generator = scene.world_matrix(id).inverse();
    for m in scene.clone_modifiers(id) {
        let Some(o) = scene.get(m) else { continue };
        let ObjectKind::CloneModifier { modifier, target } = &o.kind else { continue };
        let point = target.filter(|t| *t != id && scene.get(*t).is_some()).map(|t| to_generator.transform_point3(scene.world_matrix(t).w_axis.truncate()));
        if modifier.needs_target() && point.is_none() {
            problems.push(format!("{} needs a target object: choose one in the Properties panel", o.name));
            continue;
        }
        // The fields inside the modifier weigh each clone, seen in the generator's space.
        let mut stack = modelcraft_field::FieldStack::new();
        let to_modifier = o.transform.matrix().inverse();
        for c in scene.children(m) {
            if let Some(f) = scene.get(c)
                && f.visible
                && let ObjectKind::Field { field } = &f.kind
            {
                stack.push(f.transform.matrix().inverse() * to_modifier, field.clone());
            }
        }
        let weight = |p: Vec3| stack.weight(p);
        modifier.apply(set, &modelcraft_clone::Context { seed, time: seconds, target: point, weight: &weight });
    }
    problems
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
                && Arc::ptr_eq(&x.patches, &y.patches)
        }
        _ => a == b,
    }
}

/// Run one history step, collecting in `warnings` what went less well than asked (see
/// [`Evaluated::warnings`]).
fn run_step_warn(m: &PolyMesh, op: &StepOp, warnings: &mut Vec<String>) -> Result<PolyMesh, OpError> {
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
        StepOp::Unwrap { seams, angle, pins, auto_cut, margin, method, from_patches } => {
            use modelcraft_uv::{Pin, SeamSet, UnwrapOptions};
            let mut set = SeamSet::from_vertex_pairs(seams);
            set.retain_existing(m);
            if let Some(a) = angle {
                set.mark_by_angle(m, *a);
            }
            if *from_patches {
                set.mark_patch_borders(m);
            }
            let pins = pins.iter().map(|p| Pin { corner: p.corner, uv: glam::Vec2::from(p.uv) }).collect();
            let opts = UnwrapOptions { pins, auto_cut: *auto_cut, layout: true, margin: margin.clamp(0.0, 0.1), method: *method };
            let (mesh, result) = modelcraft_uv::unwrap_mesh(m, &set, &opts).map_err(|e| OpError::Invalid(e.to_string()))?;
            warnings.extend(result.warnings());
            Ok(mesh)
        }
        StepOp::Seams { .. } => Ok(m.clone()),
        StepOp::TransformUvs { corners, translate, rotate_deg, scale, pivot } => {
            let t = modelcraft_uv::UvTransform {
                translate: glam::Vec2::from(*translate),
                rotate_deg: *rotate_deg,
                scale: glam::Vec2::from(*scale),
                pivot: glam::Vec2::from(*pivot),
            };
            modelcraft_uv::transform_corners(m, corners, &t).map_err(|e| OpError::Invalid(e.to_string()))
        }
        StepOp::StitchUvs { edges, align, scale } => {
            edit::check_selection(edges, ne, "edge")?;
            modelcraft_uv::stitch_islands(m, edges, &modelcraft_uv::StitchOptions { align: *align, scale: *scale }).map_err(|e| OpError::Invalid(e.to_string()))
        }
        StepOp::SetUvs { uvs } => {
            if uvs.len() != m.corner_count() {
                return Err(OpError::Invalid(format!("these UVs are for a mesh of {} corners, this one has {}", uvs.len(), m.corner_count())));
            }
            let mut out = m.clone();
            out.set_uvs(&uvs.iter().map(|u| glam::Vec2::from(*u)).collect::<Vec<_>>())?;
            Ok(out)
        }
        StepOp::RelaxUvs { corners, iterations, mode } => {
            let opts = modelcraft_uv::RelaxOptions { iterations: *iterations, mode: *mode };
            let (mesh, report) = modelcraft_uv::relax_corners(m, corners, &opts).map_err(|e| OpError::Invalid(e.to_string()))?;
            if report.skipped > 0 {
                warnings.push(format!("{} UV island(s) could not be relaxed and were left as they were", report.skipped));
            }
            Ok(mesh)
        }
        StepOp::StraightenUvs { corners, strength } => {
            modelcraft_uv::straighten_corners(m, corners, *strength).map(|(mesh, _)| mesh).map_err(|e| OpError::Invalid(e.to_string()))
        }
        StepOp::GroupPatches { grouping } => modelcraft_ops::patches::group(m, *grouping),
        StepOp::CleanUp { distance } => modelcraft_ops::patches::clean_up(m, *distance),
        StepOp::PackUvs { margin, rotation, density, tiles, texel_density } => {
            let uvs = if *tiles > 1 || texel_density.is_some() {
                let tiles = match texel_density {
                    Some(d) => modelcraft_uvpack::UdimTiles::Density { texel_density: *d, max_tiles: modelcraft_uvpack::MAX_TILES },
                    None => modelcraft_uvpack::UdimTiles::Count(*tiles),
                };
                let opts = modelcraft_uvpack::UdimOptions { margin: *margin, rotation: *rotation, density: *density, tiles };
                modelcraft_uvpack::pack_udim_mesh(m, &opts).map_err(|e| OpError::Invalid(e.to_string()))?.0
            } else {
                let opts = modelcraft_uvpack::PackOptions { margin: *margin, rotation: *rotation, density: *density };
                modelcraft_uvpack::pack_mesh(m, &opts).map_err(|e| OpError::Invalid(e.to_string()))?.0
            };
            let mut out = m.clone();
            out.set_uvs(&uvs)?;
            Ok(out)
        }
        StepOp::ProjectUvs { projection, fit, faces } => {
            edit::check_selection(faces, nf, "face")?;
            let opts = modelcraft_uvpack::ProjectOptions { projection: *projection, fit: *fit, faces: faces.clone() };
            modelcraft_uvpack::project_mesh(m, &opts).map_err(|e| OpError::Invalid(e.to_string()))
        }
        StepOp::Sculpt { layer } => {
            let base = layer.topology.as_deref().unwrap_or(m);
            let mut out = if layer.level == 0 { base.clone() } else { subdivide::catmull_clark(base, u32::from(layer.level))? };
            let nv = out.vertex_count();
            if layer.positions.len() != nv {
                return Err(OpError::Invalid(format!("the sculpt layer has {} points but the mesh now has {nv}", layer.positions.len())));
            }
            out.set_positions(&layer.positions)?;
            if layer.patches.len() == out.face_count() {
                out.set_patches(&layer.patches)?;
            }
            Ok(out)
        }
        StepOp::EvenRemesh { cell } => modelcraft_ops::remesh::even_remesh(m, *cell),
        StepOp::TrimPlane { point, normal, cap } => modelcraft_ops::trim::trim_plane(m, *point, *normal, *cap),
        StepOp::Decimate { ratio } => modelcraft_ops::decimate::decimate(m, *ratio),
    }
}
