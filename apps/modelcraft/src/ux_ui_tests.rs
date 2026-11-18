//! UI and accessibility tests of the shared interface code: the object organisation (tags,
//! layers, pivots, user attributes, presets, instances) and display modes.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::scene::{DisplayMode, ObjectKind, TagKind, UserValue};

use crate::app::ModelApp;
use crate::ui_tests::{harness, palette};

/// A harness with one cube selected and the Organize section of Properties open.
fn organizing() -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    h.get_by_label("Organization").click();
    h.run();
    h
}

fn menu_item(h: &mut Harness<'static, ModelApp>, menu: &str, item: &str) {
    h.get_by_label(menu).click();
    h.run();
    h.get_by_label(item).click();
    h.run();
}

#[test]
fn a_protection_tag_is_added_from_properties_and_refuses_a_typed_position() {
    let mut h = organizing();
    let id = h.state().selection[0];
    menu_item(&mut h, "Add Tag", "Protection Tag");
    assert!(h.state().doc.scene.tag(id, TagKind::Protection).is_some());
    h.get_by_label("Protect position");
    h.get_by_label("Position X").set_value_accesskit(2.0);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().transform.translation.x, 0.0, "the protected position did not move");
    assert!(h.state().status.contains("protected"), "{}", h.state().status);
    // Unprotect the position: the same edit goes through.
    // One more frame so the pointer lands on the checkbox, not on the field that was just edited.
    h.run();
    h.get_by_label("Protect position").click();
    h.run();
    h.get_by_label("Position X").set_value_accesskit(2.0);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().transform.translation.x, 2.0);
    // The tag can be removed again from its own button.
    h.get_by_label("Remove Protection Tag").click();
    h.run();
    assert!(h.state().doc.scene.tag(id, TagKind::Protection).is_none());
}

#[test]
fn a_display_tag_and_the_view_mode_are_set_from_the_interface() {
    let mut h = organizing();
    let id = h.state().selection[0];
    menu_item(&mut h, "Add Tag", "Display Tag");
    assert_eq!(h.state().doc.scene.display_mode_of(id), DisplayMode::Wireframe, "a new display tag starts as wireframe");
    h.get_by_label("Display tag mode").click();
    h.run();
    h.get_by_label("Bounding Box").click();
    h.run();
    assert_eq!(h.state().doc.scene.display_mode_of(id), DisplayMode::BoundingBox);
    // The view's own mode, from Organize ▸ Display Mode.
    h.get_by_label("Organize").click();
    h.run();
    h.get_by_label("Display Mode").click();
    h.run();
    h.get_by_label("Flat Shaded").click();
    h.run();
    assert_eq!(h.state().display_mode, DisplayMode::Flat);
}

#[test]
fn layers_are_made_from_the_selection_and_switched_in_the_layers_window() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let id = h.state().selection[0];
    palette(&mut h, "New Layer from Selection");
    assert_eq!(h.state().doc.scene.layers.len(), 1);
    assert_eq!(h.state().doc.scene.get(id).unwrap().layer, Some(h.state().doc.scene.layers[0].id));
    palette(&mut h, "Layers…");
    assert!(h.state().organize.layers_open);
    let name = h.state().doc.scene.layers[0].name.clone();
    h.get_by_label(&format!("Render: {name}")).click();
    h.run();
    assert!(h.state().doc.scene.visible(id) && !h.state().doc.scene.renders(id), "viewport only");
    h.get_by_label(&format!("Show: {name}")).click();
    h.run();
    assert!(!h.state().doc.scene.visible(id), "hidden with its layer");
    h.get_by_label(&format!("Show: {name}")).click();
    h.run();
    h.get_by_label(&format!("Lock: {name}")).click();
    h.run();
    assert!(h.state().doc.scene.is_locked(id));
    h.get_by_label(&format!("Select objects on {name}")).click();
    h.run();
    assert_eq!(h.state().selection, vec![id]);
    h.get_by_label(&format!("Delete layer {name}")).click();
    h.run();
    assert!(h.state().doc.scene.layers.is_empty());
    assert!(!h.state().doc.scene.is_locked(id));
}

#[test]
fn an_instance_is_created_shown_in_properties_and_made_unique() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let master = h.state().selection[0];
    palette(&mut h, "Create Instance");
    let inst = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(inst).unwrap().kind, ObjectKind::Instance { of } if of == master));
    h.get_by_role_and_label(Role::TreeItem, "Cube instance");
    h.get_by_label("Organization").click();
    h.run();
    h.get_by_label("Instance of Cube");
    h.get_by_label("Select Master").click();
    h.run();
    assert_eq!(h.state().selection, vec![master]);
    h.state_mut().selection = vec![inst];
    h.run();
    h.get_by_label("Make Unique").click();
    h.run();
    assert!(matches!(h.state().doc.scene.get(inst).unwrap().kind, ObjectKind::Mesh { .. }));
}

#[test]
fn the_pivot_is_typed_or_centred_without_moving_the_object() {
    let mut h = organizing();
    let id = h.state().selection[0];
    let world = |h: &Harness<'static, ModelApp>| h.state().doc.scene.world_matrix(id).transform_point3(glam::Vec3::new(0.5, 0.5, 0.5));
    let before = world(&h);
    h.get_by_label("Pivot Y").set_value_accesskit(1.5);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().pivot.y, 1.5);
    assert!((world(&h) - before).length() < 1e-4, "typing a pivot keeps the object in place");
    h.get_by_label("Reset Pivot").click();
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().pivot, glam::Vec3::ZERO);
    assert!((world(&h) - before).length() < 1e-4);
    palette(&mut h, "Pivot to Center");
    assert_eq!(h.state().doc.scene.get(id).unwrap().pivot, glam::Vec3::ZERO, "a cube is centred already");
}

#[test]
fn user_attributes_are_added_edited_and_removed_by_name() {
    let mut h = organizing();
    let id = h.state().selection[0];
    h.get_by_label("New attribute name").focus();
    h.run();
    h.event(egui::Event::Text("mass".into()));
    h.run();
    menu_item(&mut h, "Add Attribute", "Number");
    assert_eq!(h.state().doc.scene.get(id).unwrap().user.get("mass"), Some(&UserValue::Number(0.0)));
    h.get_by_role_and_label(Role::SpinButton, "mass").set_value_accesskit(4.5);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().user.get("mass"), Some(&UserValue::Number(4.5)));
    assert_eq!(modelcraft_engine::scene::attr::get(h.state().doc.scene.get(id).unwrap(), "user.mass"), Some(4.5));
    h.get_by_label("Remove attribute mass").click();
    h.run();
    assert!(h.state().doc.scene.get(id).unwrap().user.is_empty());
}

#[test]
fn a_preset_is_saved_from_one_object_and_applied_to_another() {
    let mut h = organizing();
    let a = h.state().selection[0];
    let mut m = h.state().doc.scene.get(a).unwrap().material.clone();
    m.metallic = 1.0;
    h.state_mut().run(modelcraft_engine::Command::SetMaterial { id: a, material: m });
    h.run();
    h.get_by_label("New preset name").focus();
    h.run();
    h.event(egui::Event::Text("Shiny".into()));
    h.run();
    h.get_by_label("Save Preset").click();
    h.run();
    assert_eq!(h.state().doc.scene.presets.len(), 1);
    assert_eq!(h.state().doc.scene.presets[0].name, "Shiny");
    palette(&mut h, "Add Cube");
    let b = h.state().selection[0];
    assert_ne!(a, b);
    assert_eq!(h.state().doc.scene.get(b).unwrap().material.metallic, 0.0);
    h.get_by_label("Apply Preset: Shiny").click();
    h.run();
    assert_eq!(h.state().doc.scene.get(b).unwrap().material.metallic, 1.0);
    h.get_by_label("Remove Preset: Shiny").click();
    h.run();
    assert!(h.state().doc.scene.presets.is_empty());
}
