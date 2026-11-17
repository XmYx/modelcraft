//! Rigging in the app: the Rig menu and its actions (joints, mirroring, orienting, binding a
//! skin, weight tools) and the bones drawn in the viewport. Every action is a command with
//! typed parameters; joints are placed by number (Properties, Transform by Numbers) or with the
//! move gizmo after they are added.

use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{BindMethod, Command, RigCommand, WeightOp};

use crate::app::ModelApp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigAction {
    AddJoint,
    OrientJoints,
    MirrorJoints,
    BindSkin,
    UnbindSkin,
    ToggleDualQuaternion,
    NormalizeWeights,
    PruneWeights,
    MirrorWeights,
    AddIk,
    ConstrainPosition,
    ConstrainRotation,
    ConstrainScale,
    ConstrainAsChild,
    ConstrainLookAt,
    MatchFk,
    MatchIk,
    BakeConstraints,
    RemoveConstraints,
}

impl RigAction {
    pub const ALL: [RigAction; 19] = [
        RigAction::AddJoint,
        RigAction::OrientJoints,
        RigAction::MirrorJoints,
        RigAction::BindSkin,
        RigAction::UnbindSkin,
        RigAction::ToggleDualQuaternion,
        RigAction::NormalizeWeights,
        RigAction::PruneWeights,
        RigAction::MirrorWeights,
        RigAction::AddIk,
        RigAction::ConstrainPosition,
        RigAction::ConstrainRotation,
        RigAction::ConstrainScale,
        RigAction::ConstrainAsChild,
        RigAction::ConstrainLookAt,
        RigAction::MatchFk,
        RigAction::MatchIk,
        RigAction::BakeConstraints,
        RigAction::RemoveConstraints,
    ];

    pub fn label(self) -> String {
        match self {
            RigAction::AddJoint => tl!("Add Joint"),
            RigAction::OrientJoints => tl!("Orient Joints"),
            RigAction::MirrorJoints => tl!("Mirror Joints Across X"),
            RigAction::BindSkin => tl!("Bind Skin"),
            RigAction::UnbindSkin => tl!("Unbind Skin"),
            RigAction::ToggleDualQuaternion => tl!("Volume-Preserving Skin"),
            RigAction::NormalizeWeights => tl!("Normalize Skin Weights"),
            RigAction::PruneWeights => tl!("Prune Small Skin Weights"),
            RigAction::MirrorWeights => tl!("Mirror Skin Weights Across X"),
            RigAction::AddIk => tl!("Add IK to Joint Chain"),
            RigAction::ConstrainPosition => tl!("Constrain Position"),
            RigAction::ConstrainRotation => tl!("Constrain Rotation"),
            RigAction::ConstrainScale => tl!("Constrain Scale"),
            RigAction::ConstrainAsChild => tl!("Constrain as Child"),
            RigAction::ConstrainLookAt => tl!("Constrain to Look At"),
            RigAction::MatchFk => tl!("Match FK to IK"),
            RigAction::MatchIk => tl!("Match IK to FK"),
            RigAction::BakeConstraints => tl!("Bake Constraints to Keys"),
            RigAction::RemoveConstraints => tl!("Remove Constraints"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            RigAction::AddJoint => "rig.add_joint",
            RigAction::OrientJoints => "rig.orient_joints",
            RigAction::MirrorJoints => "rig.mirror_joints",
            RigAction::BindSkin => "rig.bind_skin",
            RigAction::UnbindSkin => "rig.unbind_skin",
            RigAction::ToggleDualQuaternion => "rig.dual_quaternion",
            RigAction::NormalizeWeights => "rig.normalize_weights",
            RigAction::PruneWeights => "rig.prune_weights",
            RigAction::MirrorWeights => "rig.mirror_weights",
            RigAction::AddIk => "rig.add_ik",
            RigAction::ConstrainPosition => "rig.constrain_position",
            RigAction::ConstrainRotation => "rig.constrain_rotation",
            RigAction::ConstrainScale => "rig.constrain_scale",
            RigAction::ConstrainAsChild => "rig.constrain_as_child",
            RigAction::ConstrainLookAt => "rig.constrain_look_at",
            RigAction::MatchFk => "rig.match_fk",
            RigAction::MatchIk => "rig.match_ik",
            RigAction::BakeConstraints => "rig.bake_constraints",
            RigAction::RemoveConstraints => "rig.remove_constraints",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            RigAction::AddJoint => Some("Model: J"),
            _ => None,
        }
    }
}

fn is_joint(app: &ModelApp, id: ObjectId) -> bool {
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Joint { .. }))
}

fn is_mesh(app: &ModelApp, id: ObjectId) -> bool {
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }))
}

/// The selected mesh a skin action works on.
fn target_mesh(app: &mut ModelApp) -> Option<ObjectId> {
    let mesh = app.selection.iter().copied().find(|id| is_mesh(app, *id));
    if mesh.is_none() {
        app.status = tl!("Select the skinned mesh first");
    }
    mesh
}

pub fn act(app: &mut ModelApp, a: RigAction) {
    match a {
        RigAction::AddJoint => {
            // Under the selected joint (a chain grows by pressing the key again), else at the origin.
            let parent = app.active().filter(|id| is_joint(app, *id));
            if app.run(Command::Rig(RigCommand::AddJoint { parent, position: None, name: None })) {
                app.status = match parent {
                    Some(_) => tl!("Joint added one unit up its parent; move it with the gizmo or by numbers"),
                    None => tl!("Joint added at the origin; press again to add a child joint"),
                };
            }
        }
        RigAction::OrientJoints => {
            let ids: Vec<ObjectId> = app.selection.iter().copied().filter(|id| is_joint(app, *id)).collect();
            if ids.is_empty() {
                app.status = tl!("Select a joint to orient it and the joints below it");
                return;
            }
            app.run(Command::Rig(RigCommand::OrientJoints { ids, aim: "y".into(), up: "z".into(), children: true }));
        }
        RigAction::MirrorJoints => {
            let Some(id) = app.active().filter(|id| is_joint(app, *id)) else {
                app.status = tl!("Select the first joint of the chain to mirror");
                return;
            };
            // Left ↔ right by the usual suffixes.
            let name = app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
            let (find, replace) =
                [("_L", "_R"), ("_R", "_L"), ("Left", "Right"), ("Right", "Left")].into_iter().find(|(f, _)| name.contains(f)).unwrap_or(("", ""));
            app.run(Command::Rig(RigCommand::MirrorJoints { id, axis: "x".into(), find: find.into(), replace: replace.into() }));
        }
        RigAction::BindSkin => {
            let Some(mesh) = target_mesh(app) else { return };
            // The selected joints and every joint below them.
            let mut joints: Vec<ObjectId> = Vec::new();
            for id in app.selection.clone() {
                if is_joint(app, id) {
                    for j in app.doc.scene.subtree(id) {
                        if is_joint(app, j) && !joints.contains(&j) {
                            joints.push(j);
                        }
                    }
                }
            }
            if joints.is_empty() {
                app.status = tl!("Select the mesh and the first joint of its skeleton, then bind");
                return;
            }
            let n = joints.len();
            if app.run(Command::Rig(RigCommand::BindSkin { mesh, joints, method: BindMethod::Heat, influences: 4, dual_quaternion: false })) {
                app.selection = vec![mesh];
                app.status = crate::tlf!("Skin bound to {count} joints with automatic weights", count = n);
            }
        }
        RigAction::UnbindSkin => {
            if let Some(mesh) = target_mesh(app) {
                app.run(Command::Rig(RigCommand::UnbindSkin { mesh }));
            }
        }
        RigAction::ToggleDualQuaternion => {
            let Some(mesh) = target_mesh(app) else { return };
            let now = app.doc.scene.skins_of(mesh).first().map(|(_, s)| s.dual_quaternion);
            let Some(now) = now else {
                app.status = tl!("The mesh has no skin");
                return;
            };
            if app.run(Command::Rig(RigCommand::SetSkin { mesh, dual_quaternion: !now })) {
                app.status = if now { tl!("Skin blends linearly") } else { tl!("Skin keeps its volume when joints twist") };
            }
        }
        RigAction::NormalizeWeights | RigAction::PruneWeights | RigAction::MirrorWeights => {
            let Some(mesh) = target_mesh(app) else { return };
            let edit = match a {
                RigAction::NormalizeWeights => WeightOp::Normalize,
                RigAction::PruneWeights => WeightOp::Prune { threshold: 0.02 },
                _ => WeightOp::Mirror { axis: "x".into(), find: "_L".into(), replace: "_R".into(), tolerance: 1e-3 },
            };
            app.run(Command::Rig(RigCommand::EditWeights { mesh, edit }));
        }
        _ => crate::constraint_ui::act(app, a),
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Rig"), |ui| {
        for a in RigAction::ALL {
            if a == RigAction::AddIk {
                ui.separator();
            }
            let action = crate::actions::Action::Rig(a);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                act(app, a);
                ui.close();
            }
        }
    });
}

/// World-space line segments for the skeleton: a bone from every joint to each child joint and
/// a small cross at every joint; selected joints are brighter.
pub fn bone_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let scene = &app.doc.scene;
    let mut out = Vec::new();
    for o in scene.objects().iter().filter(|o| scene.visible(o.id)).take(20_000) {
        let ObjectKind::Joint { radius } = o.kind else { continue };
        let selected = app.selection.contains(&o.id);
        let color = if selected { [1.0, 0.65, 0.3, 1.0] } else { [0.45, 0.8, 0.95, 0.95] };
        let world = scene.world_matrix(o.id);
        let here = world.transform_point3(glam::Vec3::ZERO);
        for axis in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z] {
            let d = world.transform_vector3(axis).normalize_or_zero() * radius;
            out.push(((here - d).to_array(), (here + d).to_array(), color));
        }
        if let Some(p) = o.parent.filter(|p| is_joint(app, *p)) {
            let head = scene.world_matrix(p).transform_point3(glam::Vec3::ZERO);
            for (a, b) in modelcraft_engine::animation::skeleton::bone_shape(head, here, 0.08) {
                out.push((a.to_array(), b.to_array(), color));
            }
        }
    }
    out.extend(crate::constraint_ui::guide_lines(app));
    out
}
