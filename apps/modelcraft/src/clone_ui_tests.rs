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

/// Open a combo box from the keyboard: focus it, press Space, then Arrow Down lands on the first
/// option and Tab walks to the next ones.
fn open_combo(h: &mut Harness<'static, ModelApp>, name: &str) {
    h.get_by_role_and_label(Role::ComboBox, name).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(h.get_by_role_and_label(Role::ComboBox, name).is_focused(), "focus starts on the {name} combo");
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn the_layout_combo_switches_layouts_by_keyboard_and_shows_that_layouts_fields() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Radial Clones");
    assert_eq!(clone_count(&mut h), 8);
    h.get_by_role_and_label(Role::SpinButton, "Radius");
    h.get_by_label("Face outwards");
    open_combo(&mut h, "Layout");
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Linear"), "Arrow Down enters the list");
    for expected in ["Radial", "Grid"] {
        h.key_press(Key::Tab);
        h.run();
        assert_eq!(focused_label(&h).as_deref(), Some(expected));
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Cloner { cloner: Cloner { layout: Layout::Grid { .. }, .. }, .. }));
    assert_eq!(clone_count(&mut h), 16);
    h.get_by_role_and_label(Role::SpinButton, "Count X");
    h.get_by_label("Spacing X");
    assert!(h.query_by_role_and_label(Role::SpinButton, "Radius").is_none(), "the radial fields are gone");
    // The status line is a live region: it announces the change; focus is back on the combo.
    assert_eq!(h.state().status, "Clone settings");
    assert!(h.get_by_role_and_label(Role::ComboBox, "Layout").is_focused(), "focus returns to the combo, now on {:?}", focused_label(&h));
}

#[test]
fn a_surface_layout_asks_for_a_target_and_the_target_combo_works_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Surface Clones"); // the selected cube becomes the source
    h.run();
    assert_eq!(clone_count(&mut h), 0, "no target yet");
    assert!(h.get_all_by_label_contains("needs a target object").count() >= 1, "shown in Properties");
    assert!(h.state().status.contains("target"), "announced through the status line: {}", h.state().status);
    h.get_by_role_and_label(Role::SpinButton, "Seed");
    let gen_id = h.state().selection[0];
    let cube = h.state().doc.scene.objects().iter().find(|o| o.name == "Cube").map(|o| o.id).unwrap();
    assert_eq!(h.state().doc.scene.get(cube).unwrap().parent, Some(gen_id));
    palette(&mut h, "Add Sphere");
    h.state_mut().select_only(gen_id);
    h.run();
    h.run();
    open_combo(&mut h, "Target object");
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("None"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Sphere"), "the next candidate is the sphere");
    // Nothing from the generator's own subtree is offered as a target.
    let offered: Vec<String> = h.get_all_by_role(Role::Button).filter_map(|n| n.accesskit_node().label()).collect();
    for own in ["Clones", "Cube"] {
        assert!(!offered.iter().any(|l| l == own), "{own} must not be offered: {offered:?}");
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(clone_count(&mut h), 60);
    h.get_by_label("60 clones of one mesh");
    let sphere = h.state().doc.scene.objects().iter().find(|o| o.name == "Sphere").map(|o| o.id);
    assert!(matches!(h.state().doc.scene.get(gen_id).unwrap().kind, ObjectKind::Cloner { target, .. } if target == sphere));
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
    assert!(h.get_all_by_label_contains("more than").count() >= 1, "shown in Properties and announced in the status line");
    assert!(h.state().status.contains("more than"), "{}", h.state().status);
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
    assert!(h.get_all_by_label_contains("needs a source object").count() >= 1);
}
