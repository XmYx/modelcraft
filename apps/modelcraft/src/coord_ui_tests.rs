//! kittest tests of the Coordinates panel: opening it from the palette, typing numbers and applying
//! them by keyboard, the Object/World and Size/Scale switches, several objects, components, and
//! one history step per edit.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::Command;
use modelcraft_engine::scene::ObjectId;

use crate::app::{ModelApp, SelectMode};
use crate::args::Cli;

fn harness() -> (Harness<'static, ModelApp>, ObjectId) {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let id = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    app.select_only(id);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    palette(&mut h, "Coordinates Panel");
    (h, id)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
}

fn type_into(h: &mut Harness<'static, ModelApp>, name: &str, v: f64) {
    h.get_by_role_and_label(Role::SpinButton, name).set_value_accesskit(v);
    h.run();
    h.run();
}

fn apply_by_keyboard(h: &mut Harness<'static, ModelApp>) {
    h.get_by_role_and_label(Role::Button, "Apply coordinates").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
}

fn translation(h: &Harness<'static, ModelApp>, id: ObjectId) -> Vec3 {
    h.state().doc.scene.get(id).unwrap().transform.translation
}

#[test]
fn the_palette_opens_the_panel_off_by_default_and_it_shows_the_selection() {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    assert!(!app.layouts.shown(app.workspace, crate::app::PanelId::Coordinates), "off by default, so saved layouts are untouched");
    assert_eq!(crate::dock::Layouts::default().to_saved(), app.layouts.to_saved());
    let (h, _) = harness();
    assert!(h.state().layouts.shown(h.state().workspace, crate::app::PanelId::Coordinates));
    for n in ["Coordinate position X", "Coordinate rotation Y", "Coordinate size Z"] {
        h.get_by_role_and_label(Role::SpinButton, n);
    }
    h.get_by_label("Applies to: Cube");
    assert!(h.get_by_role_and_label(Role::Button, "Apply coordinates").accesskit_node().is_disabled(), "nothing typed yet");
}

#[test]
fn typed_position_is_applied_by_keyboard_as_one_undo_step() {
    let (mut h, id) = harness();
    let steps = h.state().doc.undo.len();
    type_into(&mut h, "Coordinate position X", 3.0);
    type_into(&mut h, "Coordinate position Z", -2.0);
    assert_eq!(translation(&h, id), Vec3::ZERO, "nothing is applied before Apply");
    apply_by_keyboard(&mut h);
    assert_eq!(translation(&h, id), Vec3::new(3.0, 0.0, -2.0));
    assert_eq!(h.state().doc.undo.len(), steps + 1, "one history step for both numbers");
    assert!(h.get_by_role_and_label(Role::Button, "Apply coordinates").accesskit_node().is_disabled());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(translation(&h, id), Vec3::ZERO);
}

#[test]
fn revert_drops_what_was_typed() {
    let (mut h, id) = harness();
    type_into(&mut h, "Coordinate position Y", 5.0);
    h.get_by_role_and_label(Role::Button, "Revert coordinates").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert_eq!(translation(&h, id), Vec3::ZERO);
    assert!(h.get_by_role_and_label(Role::Button, "Apply coordinates").accesskit_node().is_disabled());
}

#[test]
fn rotation_and_size_apply_and_the_scale_switch_shows_the_scale() {
    let (mut h, id) = harness();
    type_into(&mut h, "Coordinate rotation Y", 90.0);
    type_into(&mut h, "Coordinate size X", 4.0);
    apply_by_keyboard(&mut h);
    let t = h.state().doc.scene.get(id).unwrap().transform;
    assert!((t.euler_degrees().y - 90.0).abs() < 1e-3, "{:?}", t.euler_degrees());
    assert!((t.scale.x - 2.0).abs() < 1e-4, "the cube is 2 wide, so size 4 doubles the scale: {:?}", t.scale);
    assert_eq!(t.scale.y, 1.0);
    // Scale shows the scale itself; typing it sets it.
    h.get_by_role_and_label(Role::Button, "Scale numbers").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    type_into(&mut h, "Coordinate scale Z", 3.0);
    apply_by_keyboard(&mut h);
    assert!((h.state().doc.scene.get(id).unwrap().transform.scale.z - 3.0).abs() < 1e-4);
}

#[test]
fn world_position_goes_through_the_parent() {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let parent = app.doc.execute(Command::AddGroup { name: None }).unwrap().created[0];
    app.doc
        .execute(Command::SetTransform {
            id: parent,
            transform: modelcraft_engine::scene::Transform { translation: Vec3::new(10.0, 0.0, 0.0), ..Default::default() },
        })
        .unwrap();
    let child = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: Some(parent) }).unwrap().created[0];
    app.select_only(child);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    palette(&mut h, "Coordinates Panel");
    h.get_by_role_and_label(Role::Button, "World position").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.get_by_role_and_label(Role::SpinButton, "Coordinate position X").accesskit_node().numeric_value(), Some(10.0), "world x is the parent's");
    type_into(&mut h, "Coordinate position X", 12.0);
    apply_by_keyboard(&mut h);
    assert_eq!(translation(&h, child), Vec3::new(2.0, 0.0, 0.0), "the local move is the world move seen from the parent");
}

#[test]
fn every_selected_object_moves_by_the_same_change() {
    let (mut h, a) = harness();
    let b = h.state_mut().doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }).unwrap().created[0];
    h.state_mut().selection = vec![a, b];
    h.run();
    h.run();
    let steps = h.state().doc.undo.len();
    type_into(&mut h, "Coordinate position Y", 1.5);
    apply_by_keyboard(&mut h);
    assert_eq!(translation(&h, a).y, 1.5);
    assert_eq!(translation(&h, b).y, 1.5);
    assert_eq!(h.state().doc.undo.len(), steps + 1);
}

#[test]
fn a_component_edit_writes_one_history_step() {
    let (mut h, id) = harness();
    h.state_mut().mode = SelectMode::Vertex;
    h.state_mut().components.insert(0);
    h.run();
    h.run();
    let before = first_vertex(&mut h, id);
    h.get_by_label("Applies to: selected components");
    let steps = h.state().doc.undo.len();
    let x = h.get_by_role_and_label(Role::SpinButton, "Coordinate position X").accesskit_node().numeric_value().unwrap();
    type_into(&mut h, "Coordinate position X", x + 0.5);
    apply_by_keyboard(&mut h);
    let after = first_vertex(&mut h, id);
    assert!((after.x - before.x - 0.5).abs() < 1e-4, "{before:?} -> {after:?}");
    assert_eq!((after.y, after.z), (before.y, before.z));
    assert_eq!(h.state().doc.undo.len(), steps + 1);
}

#[test]
fn changing_the_selection_drops_a_half_typed_change() {
    let (mut h, a) = harness();
    let b = h.state_mut().doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }).unwrap().created[0];
    type_into(&mut h, "Coordinate position X", 9.0);
    h.state_mut().select_only(b);
    h.run();
    h.run();
    assert!(h.get_by_role_and_label(Role::Button, "Apply coordinates").accesskit_node().is_disabled());
    assert_eq!(translation(&h, a), Vec3::ZERO);
    assert_eq!(translation(&h, b), Vec3::ZERO);
}

#[test]
fn with_nothing_selected_it_says_so() {
    let (mut h, _) = harness();
    h.state_mut().selection.clear();
    h.run();
    h.run();
    assert!(h.query_all_by_role_and_label(Role::SpinButton, "Coordinate position X").next().is_none());
}

fn first_vertex(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> Vec3 {
    let app = h.state_mut();
    app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh.positions()[0]
}
