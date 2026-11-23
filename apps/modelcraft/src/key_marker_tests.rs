//! kittest tests of the key markers in Properties: the three states, keying and un-keying from the
//! keyboard, the accessibility tree (toggle state and value), Shift to show the curve, and
//! auto-key.

use egui::accesskit::Role;
use egui::accesskit::Toggled;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{AnimCommand, Command};

use crate::app::ModelApp;
use crate::args::Cli;
use crate::key_marker::{KeyState, state_of};

fn harness_with(commands: Vec<Command>) -> (Harness<'static, ModelApp>, ObjectId) {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let mut id = ObjectId(0);
    for c in commands {
        if let Ok(o) = app.doc.execute(c)
            && let Some(first) = o.created.first()
        {
            id = *first;
        }
    }
    app.select_only(id);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    (h, id)
}

fn cube() -> (Harness<'static, ModelApp>, ObjectId) {
    harness_with(vec![Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }])
}

fn marker_state(h: &Harness<'static, ModelApp>, name: &str) -> (Option<Toggled>, Option<String>) {
    let n = h.get_by_role_and_label(Role::Button, name);
    (n.accesskit_node().toggled(), n.accesskit_node().value())
}

fn press_marker(h: &mut Harness<'static, ModelApp>, name: &str, key: Key, mods: Modifiers) {
    h.get_by_role_and_label(Role::Button, name).focus();
    h.run();
    h.key_press_modifiers(mods, key);
    h.run();
    h.run();
}

#[test]
fn a_field_is_plain_animated_or_keyed_by_what_its_curve_says() {
    let (mut h, id) = cube();
    let app = h.state_mut();
    assert_eq!(state_of(&app.doc.scene, id, "translate.x"), KeyState::Plain);
    app.doc.execute(Command::Anim(AnimCommand::SetKey { id, path: "translate.x".into(), frame: Some(0.0), value: None })).unwrap();
    assert_eq!(state_of(&app.doc.scene, id, "translate.x"), KeyState::Keyed);
    app.doc.scene.set_frame(10.0);
    assert_eq!(state_of(&app.doc.scene, id, "translate.x"), KeyState::Animated, "a curve with no key on this frame");
    assert_eq!(state_of(&app.doc.scene, id, "translate.y"), KeyState::Plain, "only the keyed channel is animated");
    app.doc.scene.set_frame(0.0);
    assert_eq!(state_of(&app.doc.scene, id, "translate.x"), KeyState::Keyed);
}

#[test]
fn the_marker_keys_and_unkeys_from_the_keyboard_and_says_so_in_the_tree() {
    let (mut h, id) = cube();
    let name = "Position X key";
    assert_eq!(marker_state(&h, name), (Some(Toggled::False), Some("Not animated".into())));
    let undo = h.state().doc.undo.len();
    press_marker(&mut h, name, Key::Enter, Modifiers::NONE);
    assert_eq!(h.state().doc.scene.anim.curve(id, "translate.x").map(|c| c.len()), Some(1), "Enter keys the field on this frame");
    assert_eq!(marker_state(&h, name), (Some(Toggled::True), Some("Key on this frame".into())));
    assert_eq!(h.state().doc.undo.len(), undo + 1, "one undo step");
    assert!(h.state().status.contains("keyed at frame 0"), "{}", h.state().status);
    assert!(h.get_by_role_and_label(Role::Button, name).is_focused(), "focus stays on the marker");
    // Space removes the key again; with no key left the field is plain.
    press_marker(&mut h, name, Key::Space, Modifiers::NONE);
    assert!(h.state().doc.scene.anim.curve(id, "translate.x").is_none_or(|c| c.is_empty()));
    assert_eq!(marker_state(&h, name), (Some(Toggled::False), Some("Not animated".into())));
    // Undo brings the key back.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(marker_state(&h, name), (Some(Toggled::True), Some("Key on this frame".into())));
}

#[test]
fn an_animated_field_off_its_key_says_animated_and_keys_here() {
    let (mut h, id) = cube();
    press_marker(&mut h, "Position X key", Key::Enter, Modifiers::NONE);
    h.state_mut().doc.scene.set_frame(12.0);
    h.run();
    h.run();
    assert_eq!(marker_state(&h, "Position X key"), (Some(Toggled::False), Some("Animated".into())));
    press_marker(&mut h, "Position X key", Key::Enter, Modifiers::NONE);
    assert_eq!(marker_state(&h, "Position X key"), (Some(Toggled::True), Some("Key on this frame".into())));
    let curve = h.state().doc.scene.anim.curve(id, "translate.x").map(|c| c.len());
    assert_eq!(curve, Some(2));
}

#[test]
fn the_marker_comes_right_before_its_field_in_tab_order() {
    let (mut h, _) = cube();
    h.get_by_role_and_label(Role::Button, "Position X key").focus();
    h.run();
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_role_and_label(Role::SpinButton, "Position X").is_focused());
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_role_and_label(Role::Button, "Position Y key").is_focused());
}

#[test]
fn shift_shows_the_curve_in_the_curve_editor() {
    let (mut h, id) = cube();
    press_marker(&mut h, "Position Z key", Key::Enter, Modifiers::NONE);
    assert!(!h.state().anim.open || h.state().anim.view != crate::anim_ui::AnimView::Curves || h.state().anim.selected.is_empty());
    press_marker(&mut h, "Position Z key", Key::Enter, Modifiers::SHIFT);
    let app = h.state();
    assert!(app.anim.open);
    assert_eq!(app.anim.view, crate::anim_ui::AnimView::Curves);
    assert_eq!(app.anim.selected.len(), 1);
    assert_eq!((app.anim.selected[0].id, app.anim.selected[0].path.as_str()), (id, "translate.z"));
    assert!(app.doc.scene.anim.curve(id, "translate.z").is_some_and(|c| c.len() == 1), "shift did not remove or add a key");
}

#[test]
fn a_shift_press_on_an_attribute_with_no_curve_says_so() {
    let (mut h, _) = cube();
    press_marker(&mut h, "Scale Y key", Key::Enter, Modifiers::SHIFT);
    assert_eq!(h.state().status, "Scale Y has no curve yet");
    assert!(!h.state().anim.open);
}

#[test]
fn every_kind_of_field_has_a_marker_named_after_it() {
    let (mut h, _) = cube();
    h.get_by_label("Material").click();
    h.run();
    for n in ["Position X key", "Rotation Y key", "Scale Z key", "Size X key", "Size Z key", "Metallic key", "Roughness key"] {
        h.get_by_role_and_label(Role::Button, n);
    }
    let (h, _) = harness_with(vec![Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }]);
    h.get_by_role_and_label(Role::Button, "Radius key");
    let (h, _) = {
        let ctx = egui::Context::default();
        let mut app = ModelApp::with_context(&ctx, Cli::default());
        let cube = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
        let d = app.doc.execute(Command::AddDeformer { kind: "twist".into(), target: cube }).unwrap().created[0];
        app.select_only(d);
        let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
        h.run();
        (h, d)
    };
    h.get_by_role_and_label(Role::Button, "Angle key");
    let (h, _) = harness_with(vec![Command::AddCloner { kind: "linear".into(), input: None }]);
    for n in ["Clone count key", "Offset X key", "Turn per clone Y key", "Grow per clone Z key"] {
        h.get_by_role_and_label(Role::Button, n);
    }
    let (h, _) = harness_with(vec![Command::AddCloner { kind: "radial".into(), input: None }]);
    for n in ["Radius key", "Start angle key", "End angle key"] {
        h.get_by_role_and_label(Role::Button, n);
    }
    let (h, _) = harness_with(vec![Command::AddArray { kind: "linear".into(), input: None }]);
    for n in ["Copies key", "Offset Y key"] {
        h.get_by_role_and_label(Role::Button, n);
    }
}

#[test]
fn auto_key_keeps_working_with_the_markers() {
    let (mut h, id) = cube();
    press_marker(&mut h, "Position X key", Key::Enter, Modifiers::NONE);
    h.state_mut().doc.execute(Command::Anim(AnimCommand::SetAutoKey { on: true })).unwrap();
    h.state_mut().doc.scene.set_frame(6.0);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Position X").set_value_accesskit(2.0);
    h.run();
    h.run();
    let c = h.state().doc.scene.anim.curve(id, "translate.x").cloned();
    assert_eq!(c.as_ref().map(|c| c.len()), Some(2), "the edit keyed frame 6");
    assert_eq!(marker_state(&h, "Position X key"), (Some(Toggled::True), Some("Key on this frame".into())));
}

#[test]
fn no_marker_appears_outside_properties() {
    let (mut h, _) = cube();
    // Transform by Numbers uses the same number rows but is not keyable.
    h.state_mut().numeric = Some(crate::app::NumericTransform::default());
    h.run();
    h.run();
    assert!(h.get_all_by_role_and_label(Role::SpinButton, "Move X").count() >= 1, "the dialog is open");
    assert_eq!(h.query_all_by_label("Move X key").count(), 0);
}
