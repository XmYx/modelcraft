//! kittest tests of the clone generator, driven by the keyboard and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::clone::{Layout, MAX_CLONES};
use modelcraft_engine::scene::{Cloner, ObjectKind};

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn clone_count(h: &mut Harness<'static, ModelApp>) -> usize {
    let id = h.state().selection[0];
    let state = h.state_mut();
    state.doc.eval.evaluate(&state.doc.scene, id).instances.map_or(0, |i| i.clones.len())
}

#[test]
fn the_palette_adds_clones_and_the_tree_and_properties_show_them() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Linear Clones");
    assert_eq!(h.state().doc.scene.len(), 2, "the generator and its default source");
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Cloner { .. }));
    assert_eq!(clone_count(&mut h), 5);
    h.run();
    assert!(h.get_by_role_and_label(Role::TreeItem, "Clones").accesskit_node().is_selected().unwrap_or(false));
    h.get_by_role_and_label(Role::TreeItem, "Clone source");
    // The Properties panel names the generator and the clone count.
    h.get_by_label("5 clones of one mesh");
    // The viewport lists the generator for screen readers.
    assert!(h.get_all_by_role(Role::ListItem).any(|n| n.accesskit_node().label().as_deref() == Some("Clones")));
}

#[test]
fn the_clone_count_changes_from_the_properties_panel_with_one_undo_step() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Linear Clones");
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Clone count").set_value_accesskit(9.0);
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), 9);
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.get_by_label("9 clones of one mesh");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), 5);
    // The offset has its own named fields, so it works without dragging.
    h.get_by_label("Offset X").set_value_accesskit(3.0);
    h.run();
    let id = h.state().selection[0];
    let inst = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, id).instances.unwrap()
    };
    assert!((inst.clones.instances[1].matrix.w_axis.x - 3.0).abs() < 1e-4);
}

#[test]
fn the_layout_combo_switches_layouts_by_keyboard_and_shows_that_layouts_fields() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Radial Clones");
    assert_eq!(clone_count(&mut h), 8);
    h.get_by_role_and_label(Role::SpinButton, "Radius");
    h.get_by_label("Face outwards");
    h.get_by_role_and_label(Role::ComboBox, "Layout").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Grid").click();
    h.run();
    h.run();
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Cloner { cloner: Cloner { layout: Layout::Grid { .. }, .. }, .. }));
    assert_eq!(clone_count(&mut h), 16);
    h.get_by_role_and_label(Role::SpinButton, "Count X");
    h.get_by_label("Spacing X");
    assert!(h.query_by_role_and_label(Role::SpinButton, "Radius").is_none(), "the radial fields are gone");
}

#[test]
fn a_surface_layout_asks_for_a_target_and_then_fills_it() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Surface Clones");
    h.run();
    assert_eq!(clone_count(&mut h), 0, "no target yet");
    h.get_by_label_contains("needs a target object");
    h.get_by_role_and_label(Role::ComboBox, "Target object");
    h.get_by_role_and_label(Role::SpinButton, "Seed");
    // Choose the target through the command the combo issues.
    let gen_id = h.state().selection[0];
    let cube = h.state().doc.scene.objects().iter().find(|o| o.name == "Cube").map(|o| o.id).unwrap();
    let cloner = match &h.state().doc.scene.get(gen_id).unwrap().kind {
        ObjectKind::Cloner { cloner, .. } => *cloner,
        _ => unreachable!(),
    };
    h.state_mut().run(Command::SetCloner { id: gen_id, cloner, target: Some(cube) });
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), 60);
    h.get_by_label("60 clones of one mesh");
}

#[test]
fn a_huge_request_is_capped_and_said_so() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Grid Clones");
    let id = h.state().selection[0];
    let cloner = Cloner { layout: Layout::Grid { count: [200, 200, 1], spacing: glam::Vec3::ONE }, seed: 1 };
    h.state_mut().run(Command::SetCloner { id, cloner, target: None });
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), MAX_CLONES);
    h.get_by_label_contains("more than");
}

#[test]
fn deleting_the_source_leaves_a_message_not_a_crash() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Grid Clones");
    let src = h.state().doc.scene.objects().iter().find(|o| o.name == "Clone source").map(|o| o.id).unwrap();
    h.state_mut().run(Command::Delete { ids: vec![src] });
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), 0);
    h.get_by_label_contains("needs a source object");
}
