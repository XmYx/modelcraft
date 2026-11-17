//! Keyboard and accessibility-tree tests of the rigging actions (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::scene::ObjectKind;

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

#[test]
fn build_a_chain_bind_a_mesh_and_pose_it_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cylinder");
    let mesh = h.state().selection[0];
    // J adds a joint; with a joint selected, J adds a child one unit up: a chain by one key.
    h.state_mut().selection.clear();
    for _ in 0..3 {
        h.key_press(Key::J);
        h.run();
    }
    let joints: Vec<_> = h.state().doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Joint { .. })).map(|o| (o.id, o.parent)).collect();
    assert_eq!(joints.len(), 3);
    assert_eq!((joints[1].1, joints[2].1), (Some(joints[0].0), Some(joints[1].0)), "each joint is the child of the one before");
    // The tree lists them as items; the viewport draws bones for them.
    for name in ["Joint", "Joint.1", "Joint.2"] {
        h.get_by_role_and_label(Role::TreeItem, name);
    }
    let bones = crate::rig_ui::bone_lines(h.state());
    assert_eq!(bones.len(), 3 * 3 + 2 * 12, "a cross per joint and a bone shape per parent–child pair");
    // Put the root at the bottom of the cylinder by typing, select mesh and root, bind.
    h.state_mut().select_only(joints[0].0);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Position Y").set_value_accesskit(-1.0);
    h.run();
    h.state_mut().selection = vec![mesh, joints[0].0];
    palette(&mut h, "Bind Skin");
    assert_eq!(h.state().doc.scene.skins_of(mesh).len(), 1);
    assert!(h.state().status.contains("3 joints"), "{}", h.state().status);
    h.get_by_role_and_label(Role::TreeItem, "Skin");
    // Rotate the middle joint by typing: the mesh bends.
    let scene = h.state().doc.scene.clone();
    let rest = h.state_mut().doc.eval.evaluate(&scene, mesh).mesh;
    h.state_mut().select_only(joints[1].0);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Rotation Z").set_value_accesskit(80.0);
    h.run();
    let scene = h.state().doc.scene.clone();
    let bent = h.state_mut().doc.eval.evaluate(&scene, mesh).mesh;
    let moved = rest.positions().iter().zip(bent.positions()).map(|(a, b)| (*a - *b).length()).fold(0.0, f32::max);
    assert!(moved > 0.5, "the top of the cylinder swung: {moved}");
    // The volume-preserving switch and the weight tools run from the palette on the mesh.
    h.state_mut().select_only(mesh);
    palette(&mut h, "Volume-Preserving Skin");
    assert!(h.state().doc.scene.skins_of(mesh)[0].1.dual_quaternion);
    palette(&mut h, "Prune Small Skin Weights");
    assert!(h.state().status.contains("weights dropped"), "{}", h.state().status);
    palette(&mut h, "Unbind Skin");
    assert!(h.state().doc.scene.skins_of(mesh).is_empty());
    // Undo brings the skin back.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.skins_of(mesh).len(), 1);
}

#[test]
fn mirror_and_orient_joints_from_the_palette() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Joint");
    let hip = h.state().selection[0];
    assert!(h.state_mut().run(modelcraft_engine::Command::Rename { id: hip, name: "Hip_L".into() }));
    h.get_by_role_and_label(Role::SpinButton, "Position X").set_value_accesskit(1.0);
    h.run();
    palette(&mut h, "Add Joint");
    let knee = h.state().selection[0];
    assert!(h.state_mut().run(modelcraft_engine::Command::Rename { id: knee, name: "Knee_L".into() }));
    h.get_by_role_and_label(Role::SpinButton, "Position X").set_value_accesskit(0.5);
    h.run();
    h.state_mut().select_only(hip);
    palette(&mut h, "Orient Joints");
    let aim = h.state().doc.scene.world_matrix(hip).transform_vector3(Vec3::Y).normalize();
    let to_knee = (h.state().doc.scene.world_matrix(knee).transform_point3(Vec3::ZERO) - Vec3::new(1.0, 0.0, 0.0)).normalize();
    assert!((aim - to_knee).length() < 1e-4, "the hip's Y axis points at the knee");
    h.state_mut().select_only(hip);
    palette(&mut h, "Mirror Joints Across X");
    let names: Vec<String> = h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect();
    assert_eq!(names, vec!["Hip_L", "Knee_L", "Hip_R", "Knee_R"]);
    let mirrored = h.state().doc.scene.objects()[2].id;
    assert_eq!(h.state().doc.scene.world_matrix(mirrored).transform_point3(Vec3::ZERO), Vec3::new(-1.0, 0.0, 0.0));
    // Without a selection the actions say what they need instead of failing silently.
    h.state_mut().selection.clear();
    palette(&mut h, "Bind Skin");
    assert_eq!(h.state().status, "Select the skinned mesh first");
    assert_eq!(h.get_by_label("Select the skinned mesh first").accesskit_node().live(), egui::accesskit::Live::Polite, "announced through the status line");
}
