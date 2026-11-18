//! What script callbacks change is not an undo step of its own (egui_kittest).

use egui_kittest::Harness;
use modelcraft_engine::Command;

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn install(h: &mut Harness<'static, ModelApp>, src: &str) {
    h.run();
    assert!(crate::script_ui::execute(h.state_mut(), "cb.rhai", src, None), "{:?}", h.state().script.console);
    h.run();
}

fn names(h: &Harness<'static, ModelApp>) -> Vec<String> {
    h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect()
}

fn user_adds_group(h: &mut Harness<'static, ModelApp>, name: &str) {
    h.state_mut().doc.execute(Command::AddGroup { name: Some(name.into()) }).unwrap();
    h.run();
}

fn undo(h: &mut Harness<'static, ModelApp>) {
    h.state_mut().doc.execute(Command::Undo).unwrap();
    h.run();
}

#[test]
fn a_callback_reacting_to_a_users_step_joins_that_step() {
    let mut h = harness();
    install(&mut h, "on_scene_changed(|e| { if object_count() == 1 { add_group(\"echo\"); } });");
    let before = h.state().doc.undo.len();
    user_adds_group(&mut h, "mine");
    assert_eq!(names(&h), vec!["mine".to_string(), "echo".to_string()]);
    assert_eq!(h.state().doc.undo.len(), before + 1, "the callback added no step of its own: {:?}", h.state().doc.undo.labels());
    assert_eq!(h.state().doc.undo.labels().last().map(String::as_str), Some("Add group"), "the user's step keeps its name");
    // One Undo takes both away, one Redo brings both back.
    undo(&mut h);
    assert!(names(&h).is_empty(), "{:?}", names(&h));
    h.state_mut().doc.execute(Command::Redo).unwrap();
    h.run();
    h.run();
    assert_eq!(names(&h), vec!["mine".to_string(), "echo".to_string()]);
}

#[test]
fn a_callback_that_fires_without_a_user_step_makes_one_script_callbacks_step() {
    let mut h = harness();
    // Selecting something is not an undo step, but the callback changes the scene in response.
    install(&mut h, "on_selection_changed(|e| { add_group(\"a\"); add_group(\"b\"); add_group(\"c\"); });");
    let id = h.state_mut().doc.execute(Command::AddGroup { name: Some("target".into()) }).unwrap().created[0];
    h.run();
    let before = h.state().doc.undo.len();
    h.state_mut().selection = vec![id];
    h.run();
    h.run();
    assert_eq!(names(&h).len(), 4, "{:?}", names(&h));
    assert_eq!(h.state().doc.undo.len(), before + 1, "three changes, one step: {:?}", h.state().doc.undo.labels());
    assert_eq!(h.state().doc.undo.labels().last().map(String::as_str), Some("Script callbacks"));
    undo(&mut h);
    assert_eq!(names(&h), vec!["target".to_string()]);
}

#[test]
fn reacting_to_an_undo_does_not_destroy_the_redo_history() {
    let mut h = harness();
    install(&mut h, "on_scene_changed(|e| { if object_count() < 3 { add_group(\"echo\"); } });");
    user_adds_group(&mut h, "one");
    user_adds_group(&mut h, "two");
    assert_eq!(names(&h).len(), 3, "{:?}", names(&h));
    undo(&mut h);
    // The scene changed because of the Undo and the callback reacted to it (an echo came back),
    // but Redo still works.
    assert!(h.state().doc.undo.can_redo(), "{:?}", h.state().doc.undo.redo_labels());
    assert!(names(&h).iter().filter(|n| *n == "echo").count() >= 1, "{:?}", names(&h));
    h.state_mut().doc.execute(Command::Redo).unwrap();
    h.run();
    assert!(names(&h).contains(&"two".to_string()));
}

#[test]
fn many_events_do_not_pile_up_steps() {
    let mut h = harness();
    install(&mut h, "on_scene_changed(|e| { if object_count() < 4 { add_group(\"echo\"); } });");
    let before = h.state().doc.undo.len();
    for i in 0..3 {
        user_adds_group(&mut h, &format!("u{i}"));
    }
    assert_eq!(h.state().doc.undo.len(), before + 3, "one step per user step however many callbacks ran: {:?}", h.state().doc.undo.labels());
}
