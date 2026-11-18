//! Rigging commands: joints and skins. One command, `rig`, with the operation in `op`:
//! `{"cmd":"rig","op":"add_joint","parent":3,"position":[0,1,0]}`.

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use modelcraft_anim::skeleton;
use modelcraft_anim::skin::{self, MAX_INFLUENCES, MAX_JOINTS, SkinWeights};
use modelcraft_scene::rig::{JOINT_RADIUS, Skin};
use modelcraft_scene::{ObjectId, ObjectKind, SceneError, Transform};
use serde::{Deserialize, Serialize};

use modelcraft_scene::constraint::IkSolver;

use crate::command::Outcome;
use crate::document::{Document, EngineError};

/// How automatic weights are found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindMethod {
    /// Heat spreading from each bone over the surface: weights follow the mesh.
    #[default]
    Heat,
    /// Straight-line distance to the bones: fast, ignores the mesh's shape.
    Distance,
}

/// A change to the weights of a bound mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "weights", rename_all = "snake_case")]
pub enum WeightOp {
    /// Make every point's weights sum to one.
    Normalize,
    /// Drop weights below `threshold`.
    Prune { threshold: f32 },
    /// Keep at most `max` joints per point.
    Limit { max: usize },
    /// Copy weights from the positive side of a plane (normal `axis`: x, y or z) to the negative
    /// side; joints are paired by replacing `find` with `replace` in their names.
    Mirror {
        #[serde(default = "axis_x")]
        axis: String,
        #[serde(default)]
        find: String,
        #[serde(default)]
        replace: String,
        #[serde(default = "small")]
        tolerance: f32,
    },
    /// Set the weight of `joint` on listed points.
    Set { points: Vec<u32>, joint: ObjectId, weight: f32 },
    /// One dab of a weight brush: points within `radius` of `center` (in the mesh's own space,
    /// before skinning) move their weight for `joint` towards 1 (`strength` > 0) or 0
    /// (`strength` < 0), most at the centre.
    Paint { joint: ObjectId, center: [f32; 3], radius: f32, strength: f32 },
}

fn axis_x() -> String {
    "x".into()
}
fn axis_y() -> String {
    "y".into()
}
fn axis_z() -> String {
    "z".into()
}
fn small() -> f32 {
    1e-3
}
fn four() -> usize {
    4
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RigCommand {
    /// Add a joint under `parent` (a joint or any object; top level without it) at a world
    /// `position` (one unit up its parent's Y axis by default).
    AddJoint {
        #[serde(default)]
        parent: Option<ObjectId>,
        #[serde(default)]
        position: Option<[f32; 3]>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Turn joints so that their `aim` axis points at their first child joint and their `up`
    /// axis points as near to world up as it can. Children stay where they are. With
    /// `children`, the joints below are oriented too.
    OrientJoints {
        ids: Vec<ObjectId>,
        #[serde(default = "axis_y")]
        aim: String,
        #[serde(default = "axis_z")]
        up: String,
        #[serde(default)]
        children: bool,
    },
    /// Copy a joint and the joints below it mirrored across the world plane with normal `axis`,
    /// renaming by replacing `find` with `replace` (for example `_L` → `_R`).
    MirrorJoints {
        id: ObjectId,
        #[serde(default = "axis_x")]
        axis: String,
        #[serde(default)]
        find: String,
        #[serde(default)]
        replace: String,
    },
    /// Bind a mesh to joints with automatic weights.
    BindSkin {
        mesh: ObjectId,
        joints: Vec<ObjectId>,
        #[serde(default)]
        method: BindMethod,
        #[serde(default = "four")]
        influences: usize,
        #[serde(default)]
        dual_quaternion: bool,
    },
    /// Remove the skin of a mesh.
    UnbindSkin {
        mesh: ObjectId,
    },
    /// Switch a skin between linear blending and dual quaternions.
    SetSkin {
        mesh: ObjectId,
        dual_quaternion: bool,
    },
    /// Change a bound mesh's weights.
    EditWeights {
        mesh: ObjectId,
        #[serde(flatten)]
        edit: WeightOp,
    },
    /// Constrain `target` to `sources`: `kind` is point, orient, scale, parent or aim. With
    /// `maintain_offset` (the default) the target stays where it is.
    AddConstraint {
        kind: String,
        target: ObjectId,
        sources: Vec<ObjectId>,
        /// One weight per source (all 1 without it).
        #[serde(default)]
        weights: Vec<f32>,
        #[serde(default = "yes")]
        maintain_offset: bool,
        /// Aim only: the axis that looks at the sources and the axis kept up.
        #[serde(default = "axis_y")]
        aim: String,
        #[serde(default = "axis_z")]
        up: String,
        /// Aim only: an object the up axis points towards (world up without it).
        #[serde(default)]
        up_object: Option<ObjectId>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Inverse kinematics: the joints above `end` (`bones` of them turn) reach for `goal`. A
    /// goal is created at the end joint without one. `solver`: auto, two_bone, fabrik, ccd.
    AddIk {
        end: ObjectId,
        #[serde(default)]
        goal: Option<ObjectId>,
        #[serde(default)]
        pole: Option<ObjectId>,
        #[serde(default = "two_bones")]
        bones: u32,
        #[serde(default)]
        solver: IkSolver,
        #[serde(default)]
        name: Option<String>,
    },
    /// Set a driven key: `path` of `target` follows `driver_path` of `driver`. The key pairs
    /// `driver_value` with `value`; without them, the two attributes' current values.
    SetDrivenKey {
        driver: ObjectId,
        driver_path: String,
        target: ObjectId,
        path: String,
        #[serde(default)]
        driver_value: Option<f64>,
        #[serde(default)]
        value: Option<f64>,
    },
    /// Drive `path` of `target` with a formula (an empty one removes it).
    SetExpression {
        target: ObjectId,
        path: String,
        expr: String,
    },
    /// Change a constraint: weight (the IK/FK blend of an IK constraint), on or off, name,
    /// source weights, solver settings, pole.
    SetConstraint {
        id: u32,
        #[serde(default)]
        weight: Option<f32>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        source_weights: Option<Vec<f32>>,
        #[serde(default)]
        solver: Option<IkSolver>,
        #[serde(default)]
        iterations: Option<u32>,
        /// An object id, or 0 for no pole.
        #[serde(default)]
        pole: Option<u64>,
    },
    RemoveConstraint {
        id: u32,
    },
    /// Switch an IK constraint without a jump: `to` "fk" copies the solved rotations into the
    /// joints and sets the blend to 0; "ik" moves the goal (and pole) onto the chain and sets it
    /// to 1.
    MatchIk {
        id: u32,
        to: String,
    },
    /// Turn what constraints do into keys: the transforms of `objects` over `start`..`end`
    /// (the playback range without them), one key per `step` frames; the constraints driving
    /// them are then switched off (`keep` leaves them on).
    BakeConstraints {
        objects: Vec<ObjectId>,
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        end: Option<f64>,
        #[serde(default = "one_frame")]
        step: f64,
        #[serde(default)]
        keep: bool,
    },
    /// Add a blend shape to `mesh` from the shape of `from` (a mesh with the same points). With
    /// `target`, the shape becomes an in-between of that target, shown when its weight is `at`.
    AddShape {
        mesh: ObjectId,
        from: ObjectId,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        target: Option<usize>,
        #[serde(default = "one_weight")]
        at: f32,
    },
    /// Change a blend shape: its weight (keyable as `target.<index>.weight`), its name.
    SetShape {
        mesh: ObjectId,
        target: usize,
        #[serde(default)]
        weight: Option<f32>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Remove a blend shape, or one of its in-betweens with `frame`.
    RemoveShape {
        mesh: ObjectId,
        target: usize,
        #[serde(default)]
        frame: Option<usize>,
    },
    /// Put a lattice of `divisions` cells around `mesh`; `linear` interpolates cell by cell.
    AddLattice {
        mesh: ObjectId,
        #[serde(default = "two_cells")]
        divisions: [u32; 3],
        #[serde(default)]
        linear: bool,
    },
    /// Move control points of a lattice by `translate` (in the lattice's space, whose box is one
    /// unit wide), or put them at `to`; `reset` puts every point back first.
    EditLattice {
        id: ObjectId,
        #[serde(default)]
        points: Vec<u32>,
        #[serde(default)]
        translate: Option<[f32; 3]>,
        #[serde(default)]
        to: Option<[f32; 3]>,
        #[serde(default)]
        reset: bool,
        #[serde(default)]
        linear: Option<bool>,
    },
    /// Make a cluster on `mesh`: `points` (with `weights`, all 1 without them) and, with
    /// `radius`, every point within it of the points' centre with a smooth falloff. The cluster
    /// is an object at that centre; moving, turning or scaling it moves the points.
    AddCluster {
        mesh: ObjectId,
        #[serde(default)]
        points: Vec<u32>,
        #[serde(default)]
        weights: Vec<f32>,
        #[serde(default)]
        radius: Option<f32>,
        #[serde(default)]
        center: Option<[f32; 3]>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Set the weight of points in a cluster (0 removes them).
    SetClusterWeights {
        id: ObjectId,
        points: Vec<u32>,
        weight: f32,
    },
    /// Move a deformer `delta` places in its mesh's deformer stack (negative: earlier).
    MoveInStack {
        id: ObjectId,
        delta: i32,
    },
}

fn one_weight() -> f32 {
    1.0
}
fn two_cells() -> [u32; 3] {
    [2, 2, 2]
}

fn yes() -> bool {
    true
}
fn two_bones() -> u32 {
    2
}
fn one_frame() -> f64 {
    1.0
}

/// The `op` names of the `rig` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("add_joint", "Add a joint: parent?, position? [x,y,z] in world space, name?"),
    ("orient_joints", "Aim joints at their first child joint: ids, aim (x, y, z; default y), up (default z), children?"),
    ("mirror_joints", "Copy a joint chain mirrored across a world plane: id, axis (default x), find?, replace?"),
    ("bind_skin", "Bind a mesh to joints with automatic weights: mesh, joints, method (heat, distance), influences (default 4), dual_quaternion?"),
    ("unbind_skin", "Remove a mesh's skin: mesh"),
    ("set_skin", "Linear blending or dual quaternions: mesh, dual_quaternion"),
    (
        "edit_weights",
        "Change weights: mesh, weights (normalize | prune + threshold | limit + max | mirror + axis, find, replace, tolerance | set + points, joint, weight | paint + joint, center, radius, strength)",
    ),
    (
        "add_constraint",
        "Constrain an object: kind (point, orient, scale, parent, aim), target, sources, weights?, maintain_offset (default true), aim (default y), up (default z), up_object?, name?",
    ),
    ("add_ik", "Inverse kinematics on a joint chain: end, goal? (created without it), pole?, bones (default 2), solver (auto, two_bone, fabrik, ccd), name?"),
    ("set_driven_key", "Make an attribute follow another through a curve: driver, driver_path, target, path, driver_value?, value?"),
    ("set_expression", "Drive an attribute with a formula (frame, time, self.<path>, @<id>.<path>; empty removes): target, path, expr"),
    ("set_constraint", "Change a constraint: id, weight? (IK/FK blend for IK), enabled?, name?, source_weights?, solver?, iterations?, pole? (0: none)"),
    ("remove_constraint", "Remove a constraint: id"),
    ("match_ik", "Switch an IK chain without a jump: id, to (fk, ik)"),
    ("bake_constraints", "Key the transforms constraints give and switch the constraints off: objects, start?, end?, step (default 1), keep?"),
    ("add_shape", "Add a blend shape from another mesh with the same points: mesh, from, name?, target? + at (an in-between of that target)"),
    ("set_shape", "Change a blend shape: mesh, target, weight?, name?"),
    ("remove_shape", "Remove a blend shape or one in-between: mesh, target, frame?"),
    ("add_lattice", "Put a lattice around a mesh: mesh, divisions [x,y,z] (1-8, default 2), linear?"),
    ("edit_lattice", "Move lattice control points: id, points, translate? or to? [x,y,z] in lattice space, reset?, linear?"),
    ("add_cluster", "Make a cluster of points that follows its own transform: mesh, points?, weights?, radius? (soft falloff), center?, name?"),
    ("set_cluster_weights", "Set the weight of points in a cluster: id, points, weight"),
    ("move_in_stack", "Reorder a deformer in its mesh's deformer stack: id, delta"),
];

impl RigCommand {
    pub fn label(&self) -> String {
        match self {
            RigCommand::AddJoint { .. } => "Add joint",
            RigCommand::OrientJoints { .. } => "Orient joints",
            RigCommand::MirrorJoints { .. } => "Mirror joints",
            RigCommand::BindSkin { .. } => "Bind skin",
            RigCommand::UnbindSkin { .. } => "Unbind skin",
            RigCommand::SetSkin { .. } => "Skin settings",
            RigCommand::EditWeights { edit, .. } => match edit {
                WeightOp::Normalize => "Normalize weights",
                WeightOp::Prune { .. } => "Prune weights",
                WeightOp::Limit { .. } => "Limit weights",
                WeightOp::Mirror { .. } => "Mirror weights",
                WeightOp::Set { .. } => "Set weights",
                WeightOp::Paint { .. } => "Paint weights",
            },
            RigCommand::AddConstraint { .. } => "Add constraint",
            RigCommand::AddIk { .. } => "Add IK",
            RigCommand::SetDrivenKey { .. } => "Set driven key",
            RigCommand::SetExpression { .. } => "Set expression",
            RigCommand::SetConstraint { .. } => "Constraint settings",
            RigCommand::RemoveConstraint { .. } => "Remove constraint",
            RigCommand::MatchIk { .. } => "Match IK and FK",
            RigCommand::BakeConstraints { .. } => "Bake constraints",
            RigCommand::AddShape { target: None, .. } => "Add blend shape",
            RigCommand::AddShape { .. } => "Add in-between shape",
            RigCommand::SetShape { .. } => "Blend shape",
            RigCommand::RemoveShape { .. } => "Remove blend shape",
            RigCommand::AddLattice { .. } => "Add lattice",
            RigCommand::EditLattice { .. } => "Edit lattice",
            RigCommand::AddCluster { .. } => "Add cluster",
            RigCommand::SetClusterWeights { .. } => "Cluster weights",
            RigCommand::MoveInStack { .. } => "Reorder deformers",
        }
        .into()
    }
}

pub(crate) fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

pub(crate) fn axis_index(name: &str) -> Result<usize, EngineError> {
    ["x", "y", "z"].iter().position(|a| *a == name).ok_or_else(|| EngineError::Invalid("an axis is x, y or z".into()))
}

fn is_joint(doc: &Document, id: ObjectId) -> bool {
    matches!(doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Joint { .. }))
}

pub(crate) fn transform_of(m: Mat4) -> Transform {
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    Transform { translation, rotation, scale }.sanitized()
}

/// The id of the mesh's skin object and its skin.
fn skin_of(doc: &Document, mesh: ObjectId) -> Result<(ObjectId, Arc<Skin>), EngineError> {
    if doc.scene.get(mesh).is_none() {
        return Err(SceneError::NoObject(mesh).into());
    }
    doc.scene
        .objects()
        .iter()
        .filter(|o| o.parent == Some(mesh))
        .find_map(|o| match &o.kind {
            ObjectKind::Skin { skin } => Some((o.id, skin.clone())),
            _ => None,
        })
        .ok_or_else(|| EngineError::Invalid(format!("object {} has no skin", mesh.0)))
}

pub fn apply(doc: &mut Document, cmd: &RigCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        RigCommand::AddConstraint { .. }
        | RigCommand::AddIk { .. }
        | RigCommand::SetDrivenKey { .. }
        | RigCommand::SetExpression { .. }
        | RigCommand::SetConstraint { .. }
        | RigCommand::RemoveConstraint { .. }
        | RigCommand::MatchIk { .. }
        | RigCommand::BakeConstraints { .. } => return crate::constraints::apply(doc, cmd, out),
        RigCommand::AddShape { .. }
        | RigCommand::SetShape { .. }
        | RigCommand::RemoveShape { .. }
        | RigCommand::AddLattice { .. }
        | RigCommand::EditLattice { .. }
        | RigCommand::AddCluster { .. }
        | RigCommand::SetClusterWeights { .. }
        | RigCommand::MoveInStack { .. } => return crate::deformers::apply(doc, cmd, out),
        RigCommand::AddJoint { parent, position, name } => {
            let parent_world = match parent {
                Some(p) => {
                    if doc.scene.get(*p).is_none() {
                        return Err(SceneError::NoObject(*p).into());
                    }
                    doc.scene.world_matrix(*p)
                }
                None => Mat4::IDENTITY,
            };
            let local = match position {
                Some(p) => {
                    let p = Vec3::from_array(*p);
                    if !p.is_finite() || !parent_world.inverse().is_finite() {
                        return invalid("a joint needs a finite position");
                    }
                    parent_world.inverse().transform_point3(p)
                }
                None if parent.is_some() => Vec3::Y,
                None => Vec3::ZERO,
            };
            let name = doc.scene.unique_name(name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Joint"));
            let id = doc.scene.add(name, ObjectKind::Joint { radius: JOINT_RADIUS }, *parent)?;
            doc.scene.edit(id)?.transform.translation = local;
            out.created.push(id);
        }
        RigCommand::OrientJoints { ids, aim, up, children } => {
            let (aim, up) = (axis_index(aim)?, axis_index(up)?);
            if aim == up {
                return invalid("the aim and up axes must differ");
            }
            if ids.is_empty() {
                return invalid("nothing to orient");
            }
            let mut todo: Vec<ObjectId> = Vec::new();
            for id in ids {
                if !is_joint(doc, *id) {
                    return invalid(format!("object {} is not a joint", id.0));
                }
                let list = if *children { doc.scene.subtree(*id) } else { vec![*id] };
                for j in list {
                    if is_joint(doc, j) && !todo.contains(&j) {
                        todo.push(j);
                    }
                }
            }
            let mut oriented = 0;
            // Parents first (subtree order), so a child sees its parent's new frame.
            for id in todo {
                let world = doc.scene.world_matrix(id);
                let here = world.transform_point3(Vec3::ZERO);
                let kids: Vec<(ObjectId, Mat4)> = doc.scene.children(id).into_iter().map(|c| (c, doc.scene.world_matrix(c))).collect();
                let target = kids.iter().find(|(c, _)| is_joint(doc, *c)).map(|(_, m)| m.transform_point3(Vec3::ZERO));
                let parent_world = doc.scene.get(id).and_then(|o| o.parent).map_or(Mat4::IDENTITY, |p| doc.scene.world_matrix(p));
                let (_, parent_rot, _) = parent_world.to_scale_rotation_translation();
                // An end joint has nothing to aim at: it lines up with its parent.
                let world_rot = match target.and_then(|t| skeleton::aim_rotation(t - here, Vec3::Y, aim, up)) {
                    Some(q) => q,
                    None if target.is_none() => parent_rot,
                    None => continue,
                };
                let local_rot = (parent_rot.inverse() * world_rot).normalize();
                if !local_rot.is_finite() {
                    continue;
                }
                doc.scene.edit(id)?.transform.rotation = local_rot;
                let new_world = doc.scene.world_matrix(id);
                let back = new_world.inverse();
                if !back.is_finite() {
                    continue;
                }
                for (c, old) in kids {
                    doc.scene.edit(c)?.transform = transform_of(back * old);
                }
                oriented += 1;
            }
            out.warnings.push(format!("{oriented} joints oriented"));
        }
        RigCommand::MirrorJoints { id, axis, find, replace } => {
            let axis = axis_index(axis)?;
            if !is_joint(doc, *id) {
                return invalid(format!("object {} is not a joint", id.0));
            }
            let joints: Vec<ObjectId> = doc.scene.subtree(*id).into_iter().filter(|j| is_joint(doc, *j)).collect();
            let mut map: std::collections::HashMap<ObjectId, ObjectId> = std::collections::HashMap::new();
            for j in joints {
                let Some(o) = doc.scene.get(j).cloned() else { continue };
                // A joint whose parent was not copied (the chain's root) stays under that parent.
                let parent = o.parent.map(|p| map.get(&p).copied().unwrap_or(p));
                let mirrored = skeleton::mirror_matrix(doc.scene.world_matrix(j), axis);
                let parent_world = parent.map_or(Mat4::IDENTITY, |p| doc.scene.world_matrix(p));
                let local = parent_world.inverse() * mirrored;
                if !local.is_finite() {
                    return invalid("a joint cannot be mirrored under a parent with zero scale");
                }
                let renamed = if !find.is_empty() && o.name.contains(find.as_str()) { o.name.replacen(find.as_str(), replace, 1) } else { o.name.clone() };
                let name = doc.scene.unique_name(&renamed);
                let new = doc.scene.add(name, o.kind.clone(), parent)?;
                doc.scene.edit(new)?.transform = transform_of(local);
                map.insert(j, new);
                out.created.push(new);
            }
        }
        RigCommand::BindSkin { mesh, joints, method, influences, dual_quaternion } => {
            if !matches!(doc.scene.get(*mesh).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })) {
                return invalid("only mesh objects can be bound to joints");
            }
            if skin_of(doc, *mesh).is_ok() {
                return invalid("the mesh is already bound: unbind it first");
            }
            if joints.is_empty() || joints.len() > MAX_JOINTS {
                return invalid(format!("a skin needs 1 to {MAX_JOINTS} joints"));
            }
            let mut seen = std::collections::HashSet::new();
            if let Some(bad) = joints.iter().find(|j| !is_joint(doc, **j) || !seen.insert(**j)) {
                return invalid(format!("object {} is not a joint, or is listed twice", bad.0));
            }
            let m = doc.eval.evaluate(&doc.scene, *mesh).mesh;
            let world = doc.scene.world_matrix(*mesh);
            if m.vertex_count() == 0 || !world.inverse().is_finite() {
                return invalid("the mesh has no points to bind");
            }
            let points: Vec<Vec3> = m.positions().iter().map(|p| world.transform_point3(*p)).collect();
            // One bone per (joint, child joint) pair, each owned by the joint at its head.
            let mut bones: Vec<(Vec3, Vec3)> = Vec::new();
            let mut owner: Vec<u16> = Vec::new();
            for (i, j) in joints.iter().enumerate() {
                for b in doc.scene.bones_of(*j) {
                    bones.push(b);
                    owner.push(i as u16);
                }
            }
            let per_bone = match method {
                BindMethod::Heat => {
                    let edges: Vec<(u32, u32)> = (0..m.edge_count() as u32).map(|e| m.edge_vertices(e)).collect();
                    skin::heat_weights(&points, &edges, &bones, MAX_INFLUENCES)
                }
                BindMethod::Distance => skin::distance_weights(&points, &bones, MAX_INFLUENCES),
            };
            let mut weights = SkinWeights::new(points.len(), (*influences).clamp(1, MAX_INFLUENCES));
            for v in 0..points.len() {
                let entries: Vec<(u16, f32)> = per_bone.of(v).filter_map(|(b, w)| owner.get(b as usize).map(|o| (*o, w))).collect();
                weights.set(v, &entries);
            }
            let inverse_bind =
                joints.iter().map(|j| doc.scene.world_matrix(*j).inverse() * world).map(|m| if m.is_finite() { m } else { Mat4::IDENTITY }).collect();
            let skin = Skin { joints: joints.clone(), inverse_bind, weights, dual_quaternion: *dual_quaternion };
            let name = doc.scene.unique_name("Skin");
            out.created.push(doc.scene.add(name, ObjectKind::Skin { skin: Arc::new(skin) }, Some(*mesh))?);
        }
        RigCommand::UnbindSkin { mesh } => {
            let (id, _) = skin_of(doc, *mesh)?;
            doc.scene.remove(id);
        }
        RigCommand::SetSkin { mesh, dual_quaternion } => {
            let (id, _) = skin_of(doc, *mesh)?;
            if let ObjectKind::Skin { skin } = &mut doc.scene.edit(id)?.kind {
                Arc::make_mut(skin).dual_quaternion = *dual_quaternion;
            }
        }
        RigCommand::EditWeights { mesh, edit } => {
            let (id, skin) = skin_of(doc, *mesh)?;
            let mut s = (*skin).clone();
            let joint_index = |j: ObjectId| {
                s.joints.iter().position(|x| *x == j).map(|i| i as u16).ok_or_else(|| EngineError::Invalid(format!("joint {} is not in the skin", j.0)))
            };
            // The mesh as it was bound: history and deformers, without the skin.
            let rest = |doc: &mut Document| -> Vec<Vec3> {
                let mut scene = doc.scene.clone();
                if let Ok(o) = scene.edit(id) {
                    o.visible = false;
                }
                doc.eval.evaluate(&scene, *mesh).mesh.positions().to_vec()
            };
            match edit {
                WeightOp::Normalize => s.weights.normalize(),
                WeightOp::Prune { threshold } => {
                    let n = s.weights.prune(*threshold);
                    out.warnings.push(format!("{n} weights dropped"));
                }
                WeightOp::Limit { max } => s.weights.limit(*max),
                WeightOp::Mirror { axis, find, replace, tolerance } => {
                    let axis = axis_index(axis)?;
                    // Pair joints by name: `find` ↔ `replace`, both ways; unpaired joints map to themselves.
                    let names: Vec<String> = s.joints.iter().map(|j| doc.scene.get(*j).map(|o| o.name.clone()).unwrap_or_default()).collect();
                    let partner = |name: &str| -> Option<usize> {
                        if find.is_empty() || replace.is_empty() {
                            return None;
                        }
                        let other = if name.contains(find.as_str()) {
                            name.replacen(find.as_str(), replace, 1)
                        } else if name.contains(replace.as_str()) {
                            name.replacen(replace.as_str(), find, 1)
                        } else {
                            return None;
                        };
                        names.iter().position(|n| *n == other)
                    };
                    let map: Vec<u16> = names.iter().enumerate().map(|(i, n)| partner(n).unwrap_or(i) as u16).collect();
                    let points = rest(doc);
                    let n = s.weights.mirror(&points, axis, &map, *tolerance);
                    out.warnings.push(format!("{n} points mirrored"));
                }
                WeightOp::Set { points, joint, weight } => {
                    let j = joint_index(*joint)?;
                    if !weight.is_finite() {
                        return invalid("a weight is a number from 0 to 1");
                    }
                    for v in points {
                        s.weights.set_weight(*v as usize, j, *weight);
                    }
                }
                WeightOp::Paint { joint, center, radius, strength } => {
                    let j = joint_index(*joint)?;
                    let c = Vec3::from_array(*center);
                    if !c.is_finite() || !radius.is_finite() || *radius <= 0.0 || !strength.is_finite() {
                        return invalid("a dab needs a finite centre, a positive radius and a finite strength");
                    }
                    let points = rest(doc);
                    let mut touched = 0;
                    for (v, p) in points.iter().enumerate().take(s.weights.points()) {
                        let d = p.distance(c) / radius;
                        if d >= 1.0 {
                            continue;
                        }
                        // Smooth falloff: full at the centre, none at the rim.
                        let f = 1.0 - d * d * (3.0 - 2.0 * d);
                        let now = s.weights.weight(v, j);
                        s.weights.set_weight(v, j, (now + strength.clamp(-1.0, 1.0) * f).clamp(0.0, 1.0));
                        touched += 1;
                    }
                    out.warnings.push(format!("{touched} points painted"));
                }
            }
            if let ObjectKind::Skin { skin } = &mut doc.scene.edit(id)?.kind {
                *skin = Arc::new(s);
            }
        }
    }
    Ok(())
}

/// World rotation of an object (for tools that aim things).
pub fn world_rotation(doc: &Document, id: ObjectId) -> Quat {
    doc.scene.world_matrix(id).to_scale_rotation_translation().1
}
