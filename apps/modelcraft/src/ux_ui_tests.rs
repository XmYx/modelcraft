//! UI and accessibility tests of the shared interface code: keyboard shortcuts, the object
//! organisation (tags, layers, pivots, user attributes, presets, instances) and display modes.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::scene::{DisplayMode, ObjectKind, TagKind, UserValue};

use crate::app::ModelApp;

use crate::ui_tests::{harness, palette};

/// Something focused that is not a text field: the viewport, or any other widget.
fn focus(h: &egui_kittest::Harness<'static, crate::app::ModelApp>, id: egui::Id) {
    h.ctx.memory_mut(|m| m.request_focus(id));
}

#[test]
fn undo_and_select_all_work_while_the_viewport_or_another_widget_has_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    assert_eq!(h.state().doc.scene.len(), 1);
    // The viewport after a click: egui says it wants the keyboard, but it is not typing.
    focus(&h, egui::Id::unique(("viewport", 0)));
    h.run();
    assert!(h.ctx.egui_wants_keyboard_input() && !h.ctx.text_edit_focused());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 0, "Ctrl+Z undoes with the viewport focused");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1, "and redoes");
    h.state_mut().selection.clear();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    assert_eq!(h.state().selection.len(), 1, "Ctrl+A selects with the viewport focused");
    // Any other focusable widget is the same: a panel header here.
    h.state_mut().selection.clear();
    focus(&h, egui::Id::unique(("panel-focus", 0)));
    h.run();
    assert!(h.ctx.egui_wants_keyboard_input() && !h.ctx.text_edit_focused());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 0, "Ctrl+Z undoes with a panel focused");
}

#[test]
fn a_single_letter_shortcut_does_nothing_while_a_text_field_has_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let before = h.state().tool;
    // Typing in the object search field: E (Rotate tool) is a letter there, not a command.
    h.get_by_label("Search objects").focus();
    h.run();
    assert!(h.ctx.text_edit_focused());
    // A real backend reports the key press, then the text it typed.
    h.key_press(Key::E);
    h.event(egui::Event::Text("e".into()));
    h.run();
    assert_eq!(h.state().tool, before, "the tool did not change while typing");
    assert!(h.state().objects.filter.contains('e'), "the letter went to the field: {:?}", h.state().objects.filter);
    // Ctrl+Z in a text field is the field's own undo, not the document's.
    let len = h.state().doc.scene.len();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), len, "the document's undo is not triggered from a text field");
    // Control: with the field released the same key switches the tool.
    h.ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique(("none", 0))));
    h.ctx.memory_mut(|m| {
        if let Some(f) = m.focused() {
            m.surrender_focus(f);
        }
    });
    h.run();
    assert!(!h.ctx.text_edit_focused());
    h.key_press(Key::E);
    h.run();
    assert_eq!(h.state().tool, crate::app::Tool::Rotate, "E switches to Rotate with nothing focused");
}

#[test]
fn a_focused_control_keeps_its_own_keys_but_tool_letters_and_command_chords_still_run() {
    let mut h = harness();
    h.run();
    focus(&h, egui::Id::unique(("panel-focus", 0)));
    h.run();
    let quad = h.state().quad;
    h.key_press(Key::Space);
    h.run();
    assert_eq!(h.state().quad, quad, "Space belongs to the focused control, it does not toggle four views");
    h.key_press(Key::E);
    h.run();
    assert_eq!(h.state().tool, crate::app::Tool::Rotate, "a tool letter still runs");
    // With the viewport focused Space is the viewport's.
    focus(&h, egui::Id::unique(("viewport", 0)));
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_ne!(h.state().quad, quad);
}

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
    h.get_by_label("Remove Protection Tag").scroll_to_me();
    h.run();
    h.get_by_label("Remove Protection Tag").click_accesskit();
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

#[test]
fn the_file_menu_offers_3mf_and_usd_export() {
    let mut h = harness();
    h.run();
    h.get_by_label("File").click();
    h.run();
    h.get_by_label("Export").click();
    h.run();
    h.run();
    h.get_by_label_contains("3MF");
    h.get_by_label_contains("USD");
    for a in [crate::actions::Action::Export3mf, crate::actions::Action::ExportUsda] {
        assert!(crate::actions::all().contains(&a), "{a:?} is in the palette and the shortcuts dialog");
    }
}
