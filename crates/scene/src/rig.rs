//! Rigging in the scene: joints and skins.
//!
//! A **joint** is an object like any other (a transform in the tree, keyable, parented), drawn as
//! a bone to each child joint. A **skin** is a child of a mesh object that binds the mesh's
//! points to joints: it stores, per joint, the matrix that takes a point of the mesh at bind time
//! into the joint's space, and per point the joints it follows ([`modelcraft_anim::skin`]).
//! Skins run after the mesh's parametric deformers, in child order together with the other stack
//! deformers ([`apply_stack`]). Posing a joint changes the skinned mesh
//! without touching the mesh object, so the evaluator's cache key for a skinned mesh also covers
//! the world matrices of its joints ([`pose_key`]).

use glam::{Mat4, Vec3};
use modelcraft_anim::skin::{self, SkinWeights};
use modelcraft_mesh::PolyMesh;
use serde::{Deserialize, Serialize};

use crate::{ObjectId, ObjectKind, Scene};

/// The default display radius of a joint.
pub const JOINT_RADIUS: f32 = 0.08;

fn is_false(b: &bool) -> bool {
    !*b
}

/// The binding of a mesh to joints.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Skin {
    /// The joints, in the order the weights index them.
    pub joints: Vec<ObjectId>,
    /// Per joint: mesh space at bind time → joint space (`inverse(joint world) · mesh world`).
    pub inverse_bind: Vec<Mat4>,
    pub weights: SkinWeights,
    /// Blend the joints' motions as dual quaternions instead of matrices.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dual_quaternion: bool,
}

impl Skin {
    pub fn heap_bytes(&self) -> usize {
        self.joints.len() * 72 + self.weights.heap_bytes()
    }

    /// Repair a skin read from a file; `false` when it cannot be used.
    pub fn sanitize(&mut self) -> bool {
        if self.joints.len() > skin::MAX_JOINTS || self.joints.len() != self.inverse_bind.len() {
            return false;
        }
        for m in &mut self.inverse_bind {
            if !m.is_finite() {
                *m = Mat4::IDENTITY;
            }
        }
        self.weights.sanitize(self.joints.len())
    }
}

impl Scene {
    /// The skins bound to a mesh object: its visible `Skin` children, in order.
    pub fn skins_of(&self, mesh: ObjectId) -> Vec<(ObjectId, std::sync::Arc<Skin>)> {
        self.objects()
            .iter()
            .filter(|o| o.parent == Some(mesh) && o.visible)
            .filter_map(|o| match &o.kind {
                ObjectKind::Skin { skin } => Some((o.id, skin.clone())),
                _ => None,
            })
            .collect()
    }

    /// The bones of a joint: a segment from it to each child joint, in world space; a joint
    /// without child joints has one zero-length bone at its own position.
    pub fn bones_of(&self, joint: ObjectId) -> Vec<(Vec3, Vec3)> {
        let head = self.world_matrix(joint).transform_point3(Vec3::ZERO);
        let tails: Vec<Vec3> = self
            .objects()
            .iter()
            .filter(|o| o.parent == Some(joint) && matches!(o.kind, ObjectKind::Joint { .. }))
            .map(|o| self.world_matrix(o.id).transform_point3(Vec3::ZERO))
            .collect();
        if tails.is_empty() { vec![(head, head)] } else { tails.into_iter().map(|t| (head, t)).collect() }
    }
}

/// A number that changes when a joint of one of the mesh's skins (or the mesh itself) moves;
/// zero for a mesh without skins.
pub fn pose_key(scene: &Scene, mesh: ObjectId) -> u64 {
    let skins = scene.skins_of(mesh);
    if skins.is_empty() {
        return 0;
    }
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut mix = |m: Mat4| {
        for f in m.to_cols_array() {
            h = (h ^ f.to_bits() as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    mix(scene.world_matrix(mesh));
    for (_, s) in &skins {
        for j in &s.joints {
            mix(scene.world_matrix(*j));
        }
    }
    h | 1
}

/// The meshes the shrink-wrap deformers of a mesh object pull onto, by object, already evaluated.
pub type Targets = std::collections::HashMap<ObjectId, std::sync::Arc<PolyMesh>>;

/// The objects the deformer stack of `mesh_id` reads besides its own mesh: the targets of its
/// shrink-wraps.
pub fn stack_targets(scene: &Scene, mesh_id: ObjectId) -> Vec<ObjectId> {
    let mut v: Vec<ObjectId> = scene
        .children(mesh_id)
        .into_iter()
        .filter_map(|c| scene.get(c))
        .filter(|o| o.visible)
        .filter_map(|o| match &o.kind {
            ObjectKind::Deformer { deformer, target: Some(t), .. } if deformer.needs_target() && *t != mesh_id && scene.get(*t).is_some() => Some(*t),
            _ => None,
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Run the deformer stack of a mesh object on its evaluated mesh: its visible parametric
/// deformers (bend, shrink-wrap…), blend-shape, lattice, cluster and skin children, in child
/// order ([`crate::point_deform`]). Returns the deformed mesh (`None` when nothing ran) and
/// messages for what could not run. `targets` holds the evaluated meshes of [`stack_targets`].
pub fn apply_stack(scene: &Scene, mesh_id: ObjectId, mesh: &PolyMesh, targets: &Targets) -> (Option<PolyMesh>, Vec<String>) {
    let stack: Vec<&crate::Object> = scene
        .objects()
        .iter()
        .filter(|o| {
            o.parent == Some(mesh_id) && o.visible && matches!(o.kind, ObjectKind::Skin { .. } | ObjectKind::PointDeformer { .. } | ObjectKind::Deformer { .. })
        })
        .collect();
    if stack.is_empty() {
        return (None, Vec::new());
    }
    let mut errors = Vec::new();
    let to_mesh = scene.world_matrix(mesh_id).inverse();
    let mut positions: Vec<Vec3> = mesh.positions().to_vec();
    let rest = positions.clone();
    let mut out = Vec::new();
    let mut changed = false;
    for o in stack {
        let s = match &o.kind {
            ObjectKind::Deformer { deformer, limits, target } => {
                let fields = scene.field_stack(o.id);
                let weight = |p: Vec3| fields.weight(p);
                let one = |_: Vec3| 1.0;
                let weigh: &dyn Fn(Vec3) -> f32 = if fields.is_empty() { &one } else { &weight };
                // The shrink-wrap target, in this mesh's space.
                let bvh = target.filter(|_| deformer.needs_target()).and_then(|t| {
                    let m = targets.get(&t)?;
                    Some(modelcraft_ops::bvh::TriBvh::from_mesh(m, to_mesh * scene.world_matrix(t)))
                });
                if deformer.needs_target() && bvh.is_none() {
                    errors.push(format!("{} needs a target object: choose one in the Properties panel", o.name));
                    continue;
                }
                // Smoothing and displacement read the mesh's connections; its points are `positions`.
                let cx = modelcraft_ops::deform::Context {
                    to_local: o.local_matrix().inverse(),
                    limits: *limits,
                    weight: weigh,
                    topology: Some(mesh),
                    rest: Some(&rest),
                    target: bvh.as_ref(),
                };
                deformer.apply_points(&mut positions, &cx);
                changed = true;
                continue;
            }
            ObjectKind::Skin { skin } => skin,
            ObjectKind::PointDeformer { deformer } => {
                errors.extend(deformer.apply(&mut positions, o.local_matrix()));
                changed = true;
                continue;
            }
            _ => continue,
        };
        if s.weights.points() != positions.len() {
            errors.push(format!("the skin binds {} points but the mesh now has {}: bind it again", s.weights.points(), positions.len()));
            continue;
        }
        if !to_mesh.is_finite() {
            continue;
        }
        // Mesh space at bind → joint → world now → mesh space now. A joint that no longer
        // exists holds its points where they were bound.
        let matrices: Vec<Mat4> = s
            .joints
            .iter()
            .zip(&s.inverse_bind)
            .map(|(j, ib)| if scene.get(*j).is_some() { to_mesh * scene.world_matrix(*j) * *ib } else { Mat4::IDENTITY })
            .map(|m| if m.is_finite() { m } else { Mat4::IDENTITY })
            .collect();
        if s.dual_quaternion {
            skin::dual_quaternion(&positions, &s.weights, &matrices, &mut out);
        } else {
            skin::linear_blend(&positions, &s.weights, &matrices, &mut out);
        }
        std::mem::swap(&mut positions, &mut out);
        changed = true;
    }
    if !changed {
        return (None, errors);
    }
    let mut deformed = mesh.clone();
    match deformed.set_positions(&positions) {
        Ok(()) => (Some(deformed), errors),
        Err(e) => {
            errors.push(e.to_string());
            (None, errors)
        }
    }
}

/// Repair the rig parts of a scene read from a file: unusable skins become empty ones.
pub(crate) fn repair(scene: &mut Scene) {
    let ids: Vec<ObjectId> = scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Skin { .. } | ObjectKind::Joint { .. })).map(|o| o.id).collect();
    for id in ids {
        let Ok(o) = scene.edit(id) else { continue };
        match &mut o.kind {
            ObjectKind::Skin { skin } => {
                let s = std::sync::Arc::make_mut(skin);
                if !s.sanitize() {
                    *s = Skin::default();
                }
            }
            ObjectKind::Joint { radius } => {
                *radius = if radius.is_finite() { radius.clamp(1e-4, 1e4) } else { JOINT_RADIUS };
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evaluator, Source};
    use glam::Quat;
    use modelcraft_ops::primitives::Primitive;
    use std::sync::Arc;

    /// A cylinder along Y (−1..1) with a joint chain: root at −1, elbow at 0, tip at 1.
    fn arm() -> (Scene, ObjectId, ObjectId, ObjectId) {
        let mut s = Scene::new();
        let kind = ObjectKind::Mesh {
            source: Source::Primitive { primitive: Primitive::Cylinder { radius: 0.3, height: 2.0, sides: 8, rings: 8, caps: false } },
            steps: vec![],
        };
        let mesh = s.add("Arm", kind, None).unwrap();
        let root = s.add("Root", ObjectKind::Joint { radius: JOINT_RADIUS }, None).unwrap();
        s.edit(root).unwrap().transform.translation = Vec3::new(0.0, -1.0, 0.0);
        let elbow = s.add("Elbow", ObjectKind::Joint { radius: JOINT_RADIUS }, Some(root)).unwrap();
        s.edit(elbow).unwrap().transform.translation = Vec3::new(0.0, 1.0, 0.0);
        let tip = s.add("Tip", ObjectKind::Joint { radius: JOINT_RADIUS }, Some(elbow)).unwrap();
        s.edit(tip).unwrap().transform.translation = Vec3::new(0.0, 1.0, 0.0);
        (s, mesh, root, elbow)
    }

    fn bind(s: &mut Scene, ev: &mut Evaluator, mesh: ObjectId, joints: &[ObjectId], dq: bool) -> ObjectId {
        let m = ev.evaluate(s, mesh).mesh;
        let world = s.world_matrix(mesh);
        let pts: Vec<Vec3> = m.positions().iter().map(|p| world.transform_point3(*p)).collect();
        let bones: Vec<(Vec3, Vec3)> = joints.iter().map(|j| s.bones_of(*j)[0]).collect();
        let weights = skin::distance_weights(&pts, &bones, 2);
        let inverse_bind = joints.iter().map(|j| s.world_matrix(*j).inverse() * world).collect();
        let skin = Skin { joints: joints.to_vec(), inverse_bind, weights, dual_quaternion: dq };
        s.add("Skin", ObjectKind::Skin { skin: Arc::new(skin) }, Some(mesh)).unwrap()
    }

    #[test]
    fn posing_a_joint_deforms_the_skinned_mesh_and_only_then() {
        let (mut s, mesh, root, elbow) = arm();
        let mut ev = Evaluator::new();
        let rest = ev.evaluate(&s, mesh).mesh;
        assert_eq!(s.bones_of(root), vec![(Vec3::new(0.0, -1.0, 0.0), Vec3::ZERO)]);
        assert_eq!(s.bones_of(elbow), vec![(Vec3::ZERO, Vec3::Y)]);
        let tip = s.children(elbow)[0];
        assert_eq!(s.bones_of(tip), vec![(Vec3::Y, Vec3::Y)], "a leaf joint has a point bone");
        assert_eq!(pose_key(&s, mesh), 0);
        bind(&mut s, &mut ev, mesh, &[root, elbow], false);
        let bound = ev.evaluate(&s, mesh);
        assert!(bound.errors.is_empty());
        assert!(rest.positions().iter().zip(bound.mesh.positions()).all(|(a, b)| (*a - *b).length() < 1e-5), "the bind pose changes nothing");
        // Bend the elbow: the top of the arm swings, the bottom stays.
        let runs = ev.runs;
        s.edit(elbow).unwrap().transform.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let bent = ev.evaluate(&s, mesh).mesh;
        assert_eq!(ev.runs, runs, "no history step ran again: only the skin");
        let top = rest.positions().iter().position(|p| p.y > 0.99).unwrap();
        let bottom = rest.positions().iter().position(|p| p.y < -0.99).unwrap();
        assert!((bent.position(bottom as u32) - rest.position(bottom as u32)).length() < 0.05, "the far end barely follows the elbow");
        assert!(bent.position(top as u32).x < -0.9, "the tip swung to −X: {:?}", bent.position(top as u32));
        assert!(Arc::ptr_eq(&ev.evaluate(&s, mesh).mesh, &bent), "the same pose is cached");
        // Moving the mesh and its joints together is a rigid move, not a deformation.
        let before = ev.evaluate(&s, mesh).mesh;
        s.edit(mesh).unwrap().transform.translation.x = 5.0;
        s.edit(root).unwrap().transform.translation.x = 5.0;
        let moved = ev.evaluate(&s, mesh).mesh;
        assert!(before.positions().iter().zip(moved.positions()).all(|(a, b)| (*a - *b).length() < 1e-4));
        // Deleting a joint leaves its points where they were bound; the mesh still evaluates.
        s.remove(elbow);
        assert!(ev.evaluate(&s, mesh).errors.is_empty());
    }

    #[test]
    fn dual_quaternion_skins_and_stale_skins() {
        let (mut s, mesh, root, elbow) = arm();
        let mut ev = Evaluator::new();
        let skin_id = bind(&mut s, &mut ev, mesh, &[root, elbow], true);
        s.edit(elbow).unwrap().transform.rotation = Quat::from_rotation_y(std::f32::consts::PI);
        let twisted = ev.evaluate(&s, mesh).mesh;
        let radius = twisted.positions().iter().map(|p| (p.x * p.x + p.z * p.z).sqrt()).fold(f32::MAX, f32::min);
        assert!(radius > 0.29, "a half twist keeps the radius: {radius}");
        if let ObjectKind::Skin { skin } = &mut s.edit(skin_id).unwrap().kind {
            Arc::make_mut(skin).dual_quaternion = false;
        }
        let collapsed = ev.evaluate(&s, mesh).mesh;
        let radius = collapsed.positions().iter().map(|p| (p.x * p.x + p.z * p.z).sqrt()).fold(f32::MAX, f32::min);
        assert!(radius < 0.2, "linear blending pinches: {radius}");
        // The mesh changes its point count: the skin is skipped and says so.
        if let ObjectKind::Mesh { source, .. } = &mut s.edit(mesh).unwrap().kind {
            *source = Source::Primitive { primitive: Primitive::Cylinder { radius: 0.3, height: 2.0, sides: 6, rings: 2, caps: false } };
        }
        let e = ev.evaluate(&s, mesh);
        assert_eq!(e.errors.len(), 1);
        assert!(e.errors[0].message.contains("bind it again"));
        // A hidden skin is off.
        s.edit(skin_id).unwrap().visible = false;
        assert!(ev.evaluate(&s, mesh).errors.is_empty());
    }

    #[test]
    fn hostile_rigs_are_repaired() {
        let json = r#"{"objects":[
            {"id":1,"name":"J","kind":{"type":"joint","radius":-4}},
            {"id":2,"name":"M","kind":{"type":"group"}},
            {"id":3,"name":"S","parent":2,"kind":{"type":"skin","skin":{"joints":[1,9],"inverse_bind":[[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]],
                "weights":{"influences":2,"joints":[0,1],"weights":[1,1]}}}},
            {"id":4,"name":"T","parent":2,"kind":{"type":"skin","skin":{"joints":[1],"inverse_bind":[[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]],
                "weights":{"influences":2,"joints":[0,5,0,0],"weights":[3,1,0,0]}}}}]}"#;
        let s = serde_json::from_str::<Scene>(json).unwrap().repaired().unwrap();
        assert!(matches!(s.get(ObjectId(1)).unwrap().kind, ObjectKind::Joint { radius } if radius > 0.0));
        let ObjectKind::Skin { skin } = &s.get(ObjectId(3)).unwrap().kind else { panic!() };
        assert_eq!(**skin, Skin::default(), "joints and matrices disagree: emptied");
        let ObjectKind::Skin { skin } = &s.get(ObjectId(4)).unwrap().kind else { panic!() };
        assert_eq!(skin.weights.of(0).collect::<Vec<_>>(), vec![(0, 1.0)], "the weight on a missing joint is dropped");
    }
}
