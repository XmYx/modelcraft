//! Keyboard and accessibility-tree tests of the Animation panel (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::animation::{Interp, Tangent};
use modelcraft_engine::{AnimCommand, Command};

use crate::anim_ui::AnimView;
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

fn spin(h: &mut Harness<'static, ModelApp>, label: &str, value: f64) {
    h.get_by_role_and_label(Role::SpinButton, label).set_value_accesskit(value);
    h.run();
}

/// A cube keyed at frames 0 and 24 on Position X (0 → 6), panel open, cube selected.
fn animated_cube() -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let id = h.state().selection[0];
    for (frame, value) in [(0.0, 0.0), (24.0, 6.0)] {
        assert!(h.state_mut().run(Command::Anim(AnimCommand::SetKey { id, path: "translate.x".into(), frame: Some(frame), value: Some(value) })));
    }
    h.state_mut().anim.open = true;
    h.run();
    h
}

fn x(h: &Harness<'static, ModelApp>) -> f32 {
    let id = h.state().selection[0];
    h.state().doc.scene.get(id).unwrap().transform.translation.x
}

#[test]
fn key_the_transform_and_scrub_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let id = h.state().selection[0];
    assert!(!h.state().anim.open);
    h.key_press(Key::K);
    h.run();
    assert!(h.state().anim.open, "keying opens the Animation panel");
    assert_eq!(h.state().doc.scene.anim.tracks().len(), 9);
    // Auto-key from the keyboard, go to frame 24 by typing, move the cube by typing.
    h.key_press_modifiers(Modifiers::SHIFT, Key::K);
    h.run();
    assert!(h.state().doc.scene.anim.auto_key);
    spin(&mut h, "Frame", 24.0);
    assert_eq!(h.state().doc.scene.anim.frame, 24.0);
    spin(&mut h, "Position X", 6.0);
    assert_eq!(h.state().doc.scene.anim.curve(id, "translate.x").unwrap().len(), 2, "the edit set a key");
    // The ruler is a slider: Home, End and the arrows move the frame.
    let ruler = h.get_by_role_and_label(Role::Slider, "Current frame");
    assert_eq!(ruler.accesskit_node().numeric_value(), Some(24.0));
    ruler.focus();
    h.run();
    h.key_press(Key::Home);
    h.run();
    assert_eq!(h.state().doc.scene.anim.frame, 0.0);
    assert_eq!(x(&h), 0.0);
    for _ in 0..12 {
        h.key_press(Key::ArrowRight);
        h.run();
    }
    assert_eq!(h.state().doc.scene.anim.frame, 12.0);
    assert!((x(&h) - 3.0).abs() < 1e-4, "halfway between the keys");
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowRight);
    h.run();
    assert_eq!(h.state().doc.scene.anim.frame, 22.0);
    // One undo removes the auto-keyed edit and its key together.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.anim.curve(id, "translate.x").unwrap().len(), 1);
}

#[test]
fn key_sheet_keys_are_reachable_selectable_and_editable_without_dragging() {
    let mut h = animated_cube();
    let id = h.state().selection[0];
    let first = h.get_by_role_and_label(Role::Button, "Cube · Position X key, frame 0, value 0");
    assert_eq!(first.accesskit_node().is_selected(), Some(false));
    first.focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(h.state().anim.selected.len(), 1);
    assert_eq!(h.get_by_role_and_label(Role::Button, "Cube · Position X key, frame 0, value 0").accesskit_node().is_selected(), Some(true));
    // Right arrow: focus moves to the next key of the row.
    h.key_press(Key::ArrowRight);
    h.run();
    h.run();
    assert!(h.get_by_role_and_label(Role::Button, "Cube · Position X key, frame 24, value 6").is_focused());
    h.key_press(Key::Space);
    h.run();
    // Alt+Left nudges the selected key one frame, Alt+Shift+Left ten; focus follows the key.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowLeft);
    h.run();
    h.run();
    h.key_press_modifiers(Modifiers::ALT | Modifiers::SHIFT, Key::ArrowLeft);
    h.run();
    h.run();
    let curve = h.state().doc.scene.anim.curve(id, "translate.x").unwrap().clone();
    assert_eq!(curve.keys.iter().map(|k| k.t).collect::<Vec<_>>(), vec![0.0, 13.0]);
    assert!(h.get_by_role_and_label(Role::Button, "Cube · Position X key, frame 13, value 6").is_focused());
    // The fields edit the key by number: frame, value, slopes.
    spin(&mut h, "Key frame", 20.0);
    spin(&mut h, "Key value", 8.0);
    spin(&mut h, "Slope in", 0.5);
    let k = h.state().doc.scene.anim.curve(id, "translate.x").unwrap().keys[1];
    assert_eq!((k.t, k.v, k.tangent, k.in_slope, k.out_slope), (20.0, 8.0, Tangent::Free, 0.5, 0.5));
    // Shift Keys moves the selection by the typed amount.
    spin(&mut h, "Shift by frames", 4.0);
    h.get_by_label("Shift Keys").click();
    h.run();
    assert_eq!(h.state().doc.scene.anim.curve(id, "translate.x").unwrap().keys[1].t, 24.0);
    // Delete removes the selected key while a key has focus, never the object.
    h.run();
    h.key_press(Key::Delete);
    h.run();
    assert_eq!(h.state().doc.scene.anim.curve(id, "translate.x").unwrap().len(), 1);
    assert_eq!(h.state().doc.scene.len(), 1, "the cube is still there");
    assert!(h.state().anim.selected.is_empty());
}

#[test]
fn curve_editor_edits_interpolation_and_tangents_by_number() {
    let mut h = animated_cube();
    let id = h.state().selection[0];
    h.get_by_label("Curve Editor").click();
    h.run();
    assert_eq!(h.state().anim.view, AnimView::Curves);
    // The curve can be hidden and shown with its switch.
    let toggle = h.get_by_role_and_label(Role::CheckBox, "Show curve Cube · Position X");
    toggle.click();
    h.run();
    assert_eq!(h.state().anim.hidden.len(), 1);
    h.get_by_role_and_label(Role::CheckBox, "Show curve Cube · Position X").click();
    h.run();
    assert!(h.state().anim.hidden.is_empty());
    let key = h.get_by_role_and_label(Role::Button, "Cube · Position X key, frame 0, value 0");
    key.focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    // Alt+Up nudges the value in the Curve Editor.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    h.run();
    h.run();
    let k = h.state().doc.scene.anim.curve(id, "translate.x").unwrap().keys[0];
    assert!(k.v > 0.0, "{}", k.v);
    spin(&mut h, "Slope out", 1.5);
    spin(&mut h, "Handle out", 0.8);
    let k = h.state().doc.scene.anim.curve(id, "translate.x").unwrap().keys[0];
    assert_eq!((k.tangent, k.out_slope, k.out_weight), (Tangent::Free, 1.5, 0.8));
    // The handle is in the tree; its numbers are the fields above.
    h.get_by_role_and_label(Role::Button, "Outgoing tangent handle");
    let keys = h.state().anim.selected.clone();
    assert!(h.state_mut().run(Command::Anim(AnimCommand::SetKeyShape { keys, interp: Some(Interp::Stepped), tangent: None })));
    h.run();
    assert!(h.query_by_role_and_label(Role::Button, "Outgoing tangent handle").is_none(), "a stepped segment has no handle");
}

#[test]
fn playback_and_transport_from_the_keyboard_and_the_palette() {
    let mut h = animated_cube();
    palette(&mut h, "Go to End");
    assert_eq!(h.state().doc.scene.anim.frame, 120.0);
    palette(&mut h, "Previous Key");
    assert_eq!(h.state().doc.scene.anim.frame, 24.0);
    assert_eq!(x(&h), 6.0);
    h.key_press_modifiers(Modifiers::ALT, Key::Comma);
    h.run();
    assert_eq!(h.state().doc.scene.anim.frame, 0.0);
    h.key_press(Key::Period);
    h.run();
    assert_eq!(h.state().doc.scene.anim.frame, 1.0);
    // The range and rate are typed fields; one undo step each.
    let undo = h.state().doc.undo.len();
    spin(&mut h, "End", 48.0);
    spin(&mut h, "Frames per second", 30.0);
    let t = h.state().doc.scene.anim.time;
    assert_eq!((t.end, t.fps), (48.0, 30.0));
    assert_eq!(h.state().doc.undo.len(), undo + 2);
    // Play, let time pass, pause: the frame advanced and going to a frame was never an undo step.
    h.key_press_modifiers(Modifiers::SHIFT, Key::Space);
    h.step();
    assert!(h.state().anim.playback.playing);
    h.get_by_label("Pause");
    std::thread::sleep(std::time::Duration::from_millis(120));
    h.step();
    h.step();
    assert!(h.state().doc.scene.anim.frame > 1.0);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Space);
    h.step();
    assert!(!h.state().anim.playback.playing);
    assert_eq!(h.state().doc.undo.len(), undo + 2);
}

#[test]
fn any_attribute_can_be_keyed_from_the_panel() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Torus");
    let id = h.state().selection[0];
    palette(&mut h, "Animation Panel");
    assert!(h.state().anim.open);
    h.get_by_label("No animation yet. Select an object and press Key Transform, or turn on Auto-Key and move something.");
    h.state_mut().anim.attribute = "primitive.tube".into();
    h.run();
    h.get_by_label("Key Attribute").click();
    h.run();
    assert_eq!(h.state().doc.scene.anim.curve(id, "primitive.tube").map(|c| c.len()), Some(1));
    h.get_by_role_and_label(Role::Button, "Torus · primitive.tube key, frame 0, value 0.35");
}
