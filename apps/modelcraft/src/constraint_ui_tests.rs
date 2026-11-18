//! Keyboard and accessibility-tree tests of constraints, IK and formulas (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::scene::constraint::{ConstraintKind, IkSolver};

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn pos(h: &Harness<'static, ModelApp>, id: ObjectId) -> Vec3 {
    h.state().doc.scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

fn set(h: &mut Harness<'static, ModelApp>, field: &str, v: f64) {
    h.get_by_role_and_label(Role::SpinButton, field).set_value_accesskit(v);
    h.run();
}

/// Three joints up the Y axis by pressing J three times; returns them root first.
fn chain(h: &mut Harness<'static, ModelApp>) -> Vec<ObjectId> {
    h.state_mut().selection.clear();
    let mut ids = Vec::new();
    for _ in 0..3 {
        h.key_press(Key::J);
        h.run();
        ids.push(h.state().selection[0]);
    }
    ids
}

#[test]
fn ik_from_the_palette_follows_its_goal_and_switches_to_fk_and_back() {
    let mut h = harness();
    h.run();
    let j = chain(&mut h);
    // With the end joint selected, IK makes a goal there and selects it.
    palette(&mut h, "Add IK to Joint Chain");
    let goal = h.state().selection[0];
    assert_ne!(goal, j[2]);
    assert!(h.state().status.contains("IK added"), "{}", h.state().status);
    h.get_by_role_and_label(Role::TreeItem, "Joint.2 goal");
    assert_eq!(pos(&h, goal), Vec3::Y * 2.0);
    // Type the goal's position: the chain reaches it in the same step.
    set(&mut h, "Position X", 1.0);
    set(&mut h, "Position Y", 1.0);
    assert!((pos(&h, j[2]) - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-3, "{:?}", pos(&h, j[2]));
    assert_eq!(crate::rig_ui::bone_lines(h.state()).len(), 3 * 3 + 2 * 12 + 1, "a guide from the end joint to the goal");
    // The end joint's Properties list the constraint: a switch, a blend field, a solver, remove.
    h.state_mut().select_only(j[2]);
    h.run();
    let switch = h.get_by_role_and_label(Role::CheckBox, "IK (IK)");
    assert_eq!(switch.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    h.get_by_role_and_label(Role::Button, "Remove IK");
    h.get_by_label("2 bones reach Joint.2 goal");
    set(&mut h, "IK IK blend", 0.0);
    assert!((pos(&h, j[2]) - Vec3::Y * 2.0).length() < 1e-3, "blended out: the joints' own pose");
    set(&mut h, "IK IK blend", 1.0);
    // Match FK to IK keeps the pose and blends the IK out; Match IK to FK goes back.
    let before = pos(&h, j[1]);
    palette(&mut h, "Match FK to IK");
    let id = h.state().doc.scene.constraints.list()[0].id;
    assert_eq!(h.state().doc.scene.constraints.get(id).map(|c| c.weight), Some(0.0));
    assert!((pos(&h, j[1]) - before).length() < 1e-3);
    palette(&mut h, "Match IK to FK");
    assert_eq!(h.state().doc.scene.constraints.get(id).map(|c| c.weight), Some(1.0));
    assert!((pos(&h, j[1]) - before).length() < 1e-3);
    // The solver is a combo box named by its label; pick another by keyboard.
    let combo = h.get_by_role_and_label(Role::ComboBox, "Solver");
    combo.focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.get_by_label("Joint by joint").click();
    h.run();
    assert!(matches!(h.state().doc.scene.constraints.get(id).map(|c| &c.kind), Some(ConstraintKind::Ik { solver: IkSolver::Ccd, .. })));
    // Switch it off with Space on the checkbox, then undo.
    h.get_by_role_and_label(Role::CheckBox, "IK (IK)").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(h.state().doc.scene.constraints.get(id).map(|c| c.enabled), Some(false));
    assert!((pos(&h, j[2]) - before).length() > 0.1 || (pos(&h, j[1]) - before).length() < 1e-3);
    palette(&mut h, "Undo");
    assert_eq!(h.state().doc.scene.constraints.get(id).map(|c| c.enabled), Some(true));
    // Remove from the palette; without a constraint the action says so.
    palette(&mut h, "Remove Constraints");
    assert!(h.state().doc.scene.constraints.list().is_empty());
    palette(&mut h, "Remove Constraints");
    assert_eq!(h.state().status, "The selected objects have no constraints");
    palette(&mut h, "Match FK to IK");
    assert_eq!(h.state().status, "Select a joint of an IK chain first");
    assert_eq!(h.get_by_label("Select a joint of an IK chain first").accesskit_node().live(), egui::accesskit::Live::Polite);
}

#[test]
fn constraints_formulas_and_driven_keys_from_properties() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let leader = h.state().selection[0];
    palette(&mut h, "Add Sphere");
    let follower = h.state().selection[0];
    set(&mut h, "Position X", 3.0);
    // Nothing to follow: the action explains the selection order.
    palette(&mut h, "Constrain Position");
    assert_eq!(h.state().status, "Select what to follow, then the object to constrain");
    // Leader first, follower last.
    h.state_mut().selection = vec![leader, follower];
    palette(&mut h, "Constrain as Child");
    assert!(h.state().status.contains("Constrained to 1 objects"), "{}", h.state().status);
    assert_eq!(pos(&h, follower), Vec3::new(3.0, 0.0, 0.0), "the offset is kept");
    h.state_mut().select_only(leader);
    h.run();
    set(&mut h, "Rotation Y", 90.0);
    assert!((pos(&h, follower) - Vec3::new(0.0, 0.0, -3.0)).length() < 1e-3, "{:?}", pos(&h, follower));
    // Half weight from the follower's Properties.
    h.state_mut().select_only(follower);
    h.run();
    h.get_by_label("Constraints (1)");
    h.get_by_label("Follows Cube");
    set(&mut h, "Parent constraint weight", 0.5);
    assert!((pos(&h, follower) - Vec3::new(1.5, 0.0, -1.5)).length() < 0.4, "{:?}", pos(&h, follower));
    // A formula typed into the new-formula field and confirmed with the button.
    let field = h.get_by_role_and_label(Role::TextInput, "New formula");
    field.focus();
    h.run();
    h.event(egui::Event::Text("frame * 0.5 + 1".into()));
    h.run();
    h.get_by_role_and_label(Role::Button, "Set Formula").click();
    h.run();
    assert_eq!(h.state().doc.scene.constraints.list().len(), 2);
    h.get_by_role_and_label(Role::TextInput, "Formula for translate.x");
    // The formula drives translate.x; the parent constraint runs too, in list order.
    h.state_mut().doc.set_frame(4.0);
    h.run();
    // A bad formula is refused with a message and nothing changes.
    h.get_by_role_and_label(Role::TextInput, "New formula").focus();
    h.run();
    h.event(egui::Event::Text("1 +".into()));
    h.run();
    h.get_by_role_and_label(Role::Button, "Set Formula").click();
    h.run();
    assert!(h.state().status.contains("failed"), "{}", h.state().status);
    assert_eq!(h.state().doc.scene.constraints.list().len(), 2);
    // A driven key needs a driver: the button is disabled with one object selected.
    assert!(h.get_by_role_and_label(Role::Button, "Set Driven Key").accesskit_node().is_disabled());
    h.state_mut().selection = vec![leader, follower];
    h.run();
    h.get_by_role_and_label(Role::Button, "Set Driven Key").click();
    h.run();
    assert!(h.state().doc.scene.constraints.list().iter().any(|c| matches!(c.kind, ConstraintKind::Driven { .. })), "{}", h.state().status);
    // Bake: the constraints are switched off and the follower has keys.
    h.state_mut().select_only(follower);
    palette(&mut h, "Bake Constraints to Keys");
    assert!(h.state().doc.scene.anim.is_animated(follower));
    assert!(h.state().doc.scene.constraints.list().iter().all(|c| !c.enabled));
    let off = h.get_by_role_and_label(Role::CheckBox, "Parent constraint (As child)");
    assert_eq!(off.accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
}
