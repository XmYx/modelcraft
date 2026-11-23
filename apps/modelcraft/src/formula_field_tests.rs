//! kittest tests of formula fields in Properties, all by keyboard: typing `=` and a formula into a
//! number, the formula shown as text with its value in the tree, a formula that does not parse
//! kept with its error, Escape, a cycle reported in place, a number replacing the formula, and
//! one undo step per change.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{AnimCommand, Command};

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> (Harness<'static, ModelApp>, ObjectId) {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let id = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Box".into()), parent: None }).unwrap().created[0];
    app.select_only(id);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    (h, id)
}

/// Focus the field named `name` (a number or a formula), replace its text and press `key`.
fn type_into(h: &mut Harness<'static, ModelApp>, role: Role, name: &str, text: &str, key: Key) {
    h.get_by_role_and_label(role, name).focus();
    h.run();
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

fn formula(h: &Harness<'static, ModelApp>, id: ObjectId, path: &str) -> Option<String> {
    h.state().doc.scene.constraints.formula(id, path).and_then(|c| match &c.kind {
        modelcraft_engine::scene::constraint::ConstraintKind::Expression { expr, .. } => Some(expr.clone()),
        _ => None,
    })
}

fn field(h: &Harness<'static, ModelApp>, name: &str) -> (Option<String>, Option<String>) {
    let n = h.get_by_role_and_label(Role::TextInput, name);
    (n.accesskit_node().value(), n.accesskit_node().description())
}

#[test]
fn typing_equals_into_a_number_gives_it_a_formula() {
    let (mut h, id) = harness();
    let undo = h.state().doc.undo.len();
    type_into(&mut h, Role::SpinButton, "Position X", "=frame / 2", Key::Enter);
    assert_eq!(formula(&h, id, "translate.x").as_deref(), Some("frame / 2"));
    assert_eq!(h.state().doc.undo.len(), undo + 1, "one undo step");
    assert!(h.state().status.contains("follows a formula"), "{}", h.state().status);
    // The field is now the formula's text, and the tree says what it gives.
    h.state_mut().run(Command::Anim(AnimCommand::SetTime { frame: 10.0 }));
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.attr(id, "translate.x"), Some(5.0));
    let (value, description) = field(&h, "Position X");
    assert_eq!(value.as_deref(), Some("=frame / 2"));
    assert_eq!(description.as_deref(), Some("Formula, now 5.000"));
    // Leaving the field with Escape after typing changes nothing.
    let steps = h.state().doc.undo.len();
    type_into(&mut h, Role::TextInput, "Position X", "=frame * 9", Key::Escape);
    assert_eq!(formula(&h, id, "translate.x").as_deref(), Some("frame / 2"));
    assert_eq!(h.state().doc.undo.len(), steps);
}

#[test]
fn a_formula_that_does_not_parse_stays_with_its_error_until_escape() {
    let (mut h, id) = harness();
    let undo = h.state().doc.undo.len();
    type_into(&mut h, Role::SpinButton, "Position Y", "=1 +", Key::Enter);
    assert!(formula(&h, id, "translate.y").is_none(), "nothing is set");
    assert_eq!(h.state().doc.undo.len(), undo);
    let (value, description) = field(&h, "Position Y");
    assert_eq!(value.as_deref(), Some("=1 +"), "the text stays to be fixed");
    assert!(description.is_some_and(|d| d.starts_with("Formula error:")));
    // Fixing it sets it.
    type_into(&mut h, Role::TextInput, "Position Y", "=1 + 2", Key::Enter);
    assert_eq!(formula(&h, id, "translate.y").as_deref(), Some("1 + 2"));
    assert_eq!(h.state().doc.scene.attr(id, "translate.y"), Some(3.0));
    // A bad edit of an existing formula, dropped with Escape.
    type_into(&mut h, Role::TextInput, "Position Y", "=(", Key::Enter);
    assert!(field(&h, "Position Y").1.is_some_and(|d| d.starts_with("Formula error:")));
    h.get_by_role_and_label(Role::TextInput, "Position Y").focus();
    h.run();
    h.key_press(Key::Escape);
    h.run();
    h.run();
    assert_eq!(h.state().selection, vec![id], "Escape in the field cancels the edit, not the selection");
    assert_eq!(field(&h, "Position Y").0.as_deref(), Some("=1 + 2"));
    assert_eq!(formula(&h, id, "translate.y").as_deref(), Some("1 + 2"));
}

#[test]
fn a_cycle_and_a_missing_object_are_shown_in_the_field() {
    let (mut h, id) = harness();
    type_into(&mut h, Role::SpinButton, "Scale X", "=self.scale.x + 1", Key::Enter);
    assert_eq!(formula(&h, id, "scale.x").as_deref(), Some("self.scale.x + 1"));
    let (_, d) = field(&h, "Scale X");
    assert!(d.as_deref().is_some_and(|d| d.starts_with("Formula error:") && d.contains("cycle")), "{d:?}");
    assert_eq!(h.state().doc.scene.attr(id, "scale.x"), Some(1.0), "a cycle is skipped, not run");
    type_into(&mut h, Role::SpinButton, "Rotation Z", "=@Nobody.rotate.z", Key::Enter);
    let (_, d) = field(&h, "Rotation Z");
    assert!(d.as_deref().is_some_and(|d| d.starts_with("Formula error:") && d.contains("Nobody")), "{d:?}");
    // Another object's attribute by name.
    let other = h.state_mut().doc.execute(Command::AddGroup { name: Some("Lever".into()) }).unwrap().created[0];
    h.state_mut().run(Command::Anim(AnimCommand::SetAttr { id: other, path: "rotate.z".into(), value: 30.0 }));
    h.state_mut().select_only(id);
    h.run();
    type_into(&mut h, Role::TextInput, "Rotation Z", "=@Lever.rotate.z * 2", Key::Enter);
    assert!((h.state().doc.scene.attr(id, "rotate.z").unwrap() - 60.0).abs() < 1e-3);
    assert_eq!(field(&h, "Rotation Z").1.as_deref(), Some("Formula, now 60.000"));
}

#[test]
fn a_number_replaces_the_formula_and_undo_brings_it_back() {
    let (mut h, id) = harness();
    type_into(&mut h, Role::SpinButton, "Position Z", "=2 * 3", Key::Enter);
    assert_eq!(h.state().doc.scene.attr(id, "translate.z"), Some(6.0));
    let undo = h.state().doc.undo.len();
    type_into(&mut h, Role::TextInput, "Position Z", "1.5", Key::Tab);
    assert!(formula(&h, id, "translate.z").is_none());
    assert_eq!(h.state().doc.scene.attr(id, "translate.z"), Some(1.5));
    assert!(h.query_by_role_and_label(Role::SpinButton, "Position Z").is_some(), "a plain number again");
    assert_eq!(h.state().doc.undo.len(), undo + 1, "removing the formula and setting the value is one step");
    h.state_mut().run(Command::Undo);
    h.run();
    assert_eq!(formula(&h, id, "translate.z").as_deref(), Some("2 * 3"));
    assert_eq!(h.state().doc.scene.attr(id, "translate.z"), Some(6.0));
    // An empty field removes the formula and keeps the value.
    type_into(&mut h, Role::TextInput, "Position Z", "", Key::Enter);
    assert!(formula(&h, id, "translate.z").is_none());
    assert_eq!(h.state().doc.scene.attr(id, "translate.z"), Some(6.0));
}

#[test]
fn drawing_the_same_frame_again_does_not_solve_again() {
    let (mut h, id) = harness();
    type_into(&mut h, Role::SpinButton, "Position X", "=sin(time) + frame", Key::Enter);
    assert!(formula(&h, id, "translate.x").is_some());
    let runs = h.state().doc.scene.constraints.solves_run();
    for _ in 0..20 {
        h.run();
    }
    assert_eq!(h.state().doc.scene.constraints.solves_run(), runs, "twenty redraws, no solve");
    h.state_mut().run(Command::Anim(AnimCommand::SetTime { frame: 4.0 }));
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.constraints.solves_run(), runs + 1, "one solve for the new frame");
}
