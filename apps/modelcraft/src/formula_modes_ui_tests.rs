//! kittest tests of the formula deformer and the formula clone-modifier mode, by keyboard: the
//! palette adds them, the formula field takes a new formula with one undo step, a formula that
//! does not parse or reads an unknown name stays with its error, Escape drops it, and the
//! direction combo works from the keyboard.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::clone::Mode;
use modelcraft_engine::ops::deform::{Along, Deformer};
use modelcraft_engine::scene::ObjectKind;

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    h
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

/// Replace the text of the field named `name` and press `key`.
fn type_into(h: &mut Harness<'static, ModelApp>, name: &str, text: &str, key: Key) {
    h.get_by_role_and_label(Role::TextInput, name).focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.key_press(Key::Backspace);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(key);
    h.run();
    h.run();
}

fn description(h: &Harness<'static, ModelApp>, name: &str) -> String {
    h.get_by_role_and_label(Role::TextInput, name).accesskit_node().description().unwrap_or_default()
}

fn deformer(h: &Harness<'static, ModelApp>) -> Deformer {
    let id = h.state().selection[0];
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Deformer { deformer, .. } => deformer.clone(),
        k => panic!("not a deformer: {k:?}"),
    }
}

fn top(h: &mut Harness<'static, ModelApp>, mesh: modelcraft_engine::scene::ObjectId) -> f32 {
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, mesh).mesh.positions().iter().map(|p| p.y).fold(f32::MIN, f32::max)
}

#[test]
fn the_formula_deformer_takes_a_formula_from_the_keyboard() {
    let mut h = harness();
    palette(&mut h, "Add Cube");
    let mesh = h.state().selection[0];
    let plain = top(&mut h, mesh);
    palette(&mut h, "Add Formula Deformer");
    assert!(matches!(deformer(&h), Deformer::Formula { .. }));
    h.get_by_role_and_label(Role::SpinButton, "Amount");
    h.get_by_role_and_label(Role::ComboBox, "Move along");
    // Along the Y axis: open the combo, two down from Normal, Enter.
    h.get_by_role_and_label(Role::ComboBox, "Move along").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    for _ in 0..2 {
        h.key_press(Key::Tab);
        h.run();
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(deformer(&h), Deformer::Formula { along: Along::Y, .. }), "{:?}", deformer(&h));
    // A constant lift of 0.25.
    let undo = h.state().doc.undo.len();
    type_into(&mut h, "Formula", "0.25", Key::Enter);
    assert!(matches!(deformer(&h), Deformer::Formula { ref expr, .. } if expr == "0.25"));
    assert_eq!(h.state().doc.undo.len(), undo + 1, "one undo step");
    assert!((top(&mut h, mesh) - (plain + 0.25)).abs() < 1e-4, "every point lifted by 0.25 along Y: {} from {plain}", top(&mut h, mesh));
    // A formula that does not parse, then one with a name a deformer does not know.
    type_into(&mut h, "Formula", "y *", Key::Enter);
    assert!(matches!(deformer(&h), Deformer::Formula { ref expr, .. } if expr == "0.25"), "nothing set");
    assert!(description(&h, "Formula").starts_with("Formula error:"));
    h.get_by_label_contains("Formula error:");
    type_into(&mut h, "Formula", "self.translate.x", Key::Enter);
    assert!(description(&h, "Formula").contains("self.translate.x"), "{}", description(&h, "Formula"));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    // Escape drops the draft and keeps the selection.
    h.get_by_role_and_label(Role::TextInput, "Formula").focus();
    h.run();
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert_eq!(h.get_by_role_and_label(Role::TextInput, "Formula").accesskit_node().value().as_deref(), Some("0.25"));
    assert!(!description(&h, "Formula").starts_with("Formula error:"));
}

#[test]
fn the_formula_clone_mode_takes_a_formula_from_the_keyboard() {
    let mut h = harness();
    palette(&mut h, "Add Linear Clones");
    palette(&mut h, "Add Formula Clone Modifier");
    let m = h.state().selection[0];
    let generator = h.state().doc.scene.get(m).and_then(|o| o.parent).unwrap();
    let heights = |h: &mut Harness<'static, ModelApp>| -> Vec<f32> {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, generator).instances.map_or(Vec::new(), |i| i.clones.instances.iter().map(|c| c.matrix.w_axis.y).collect())
    };
    let undo = h.state().doc.undo.len();
    type_into(&mut h, "Formula", "i", Key::Enter);
    let mode = match &h.state().doc.scene.get(m).unwrap().kind {
        ObjectKind::CloneModifier { modifier, .. } => modifier.mode.clone(),
        _ => panic!("not a modifier"),
    };
    assert_eq!(mode, Mode::Formula { expr: "i".into() });
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    let ys = heights(&mut h);
    assert!(ys.len() >= 3 && ys.windows(2).all(|w| (w[1] - w[0] - 0.5).abs() < 1e-4), "clone i rises by i / 2: {ys:?}");
    type_into(&mut h, "Formula", "frame", Key::Enter);
    assert!(description(&h, "Formula").contains("frame"), "a clone formula has no frame name: {}", description(&h, "Formula"));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
}
