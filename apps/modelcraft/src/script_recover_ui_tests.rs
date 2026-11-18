//! The Scripts window's unsaved text survives quitting (egui_kittest).

use std::path::PathBuf;
use std::time::Duration;

use egui_kittest::Harness;
use modelcraft_script::draft;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::script_ui::ScriptState;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-recover-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

fn start(dir: &PathBuf, scripts: &PathBuf) -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    h.state_mut().script = ScriptState::new(Some(scripts.clone()));
    crate::script_recover::init(h.state_mut(), Some(dir.clone()));
    h.state_mut().script.recovery.delay = Duration::ZERO;
    h
}

#[test]
fn typed_text_is_kept_on_disk_and_comes_back_after_a_quit() {
    let (dir, scripts) = (tmp("a-rec"), tmp("a-scripts"));
    let mut h = start(&dir, &scripts);
    assert!(draft::load(&dir).is_none());
    h.state_mut().script.text = "print(\"unsaved work\");".into();
    h.run();
    assert_eq!(draft::load(&dir).map(|d| d.text), Some("print(\"unsaved work\");".into()), "written a moment after the change");
    // More typing replaces the draft.
    h.state_mut().script.text.push_str("\nprint(2);");
    h.run();
    assert_eq!(draft::load(&dir).map(|d| d.text), Some("print(\"unsaved work\");\nprint(2);".into()));
    // Quit: the app flushes; the next start gets the text back, unsaved, with the window open.
    crate::script_recover::flush(h.state_mut());
    drop(h);
    let mut h2 = start(&dir, &scripts);
    assert_eq!(h2.state().script.text, "print(\"unsaved work\");\nprint(2);");
    assert!(h2.state().script.modified(), "it is still unsaved");
    assert!(h2.state().script.open, "the window shows it");
    assert!(h2.state().status.contains("Recovered"), "{}", h2.state().status);
    // Saving it removes the draft; so does emptying the editor.
    let target = scripts.join("saved.rhai");
    assert!(crate::script_ui::save_to(h2.state_mut(), &target));
    h2.run();
    assert!(draft::load(&dir).is_none(), "saved text needs no recovery");
    h2.state_mut().script.text = "x".into();
    h2.run();
    assert!(draft::load(&dir).is_some());
    h2.state_mut().script.text.clear();
    h2.run();
    assert!(draft::load(&dir).is_none());
}

#[test]
fn edits_to_a_saved_script_come_back_on_that_script() {
    let (dir, scripts) = (tmp("b-rec"), tmp("b-scripts"));
    let file = scripts.join("tool.rhai");
    std::fs::write(&file, "print(\"on disk\");").unwrap();
    let mut h = start(&dir, &scripts);
    crate::script_ui::load(h.state_mut(), &file);
    h.state_mut().script.text = "print(\"edited\");".into();
    h.run();
    crate::script_recover::flush(h.state_mut());
    drop(h);
    let h2 = start(&dir, &scripts);
    assert_eq!(h2.state().script.text, "print(\"edited\");");
    assert_eq!(h2.state().script.path.as_deref(), Some(file.as_path()), "Save writes back to the same file");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "print(\"on disk\");", "the file itself is untouched until the user saves");
    assert!(h2.state().script.modified());
}

#[test]
fn a_draft_equal_to_the_file_or_for_a_vanished_file_is_handled() {
    let (dir, scripts) = (tmp("c-rec"), tmp("c-scripts"));
    let file = scripts.join("same.rhai");
    std::fs::write(&file, "print(1);").unwrap();
    // The text on disk is what the draft holds: nothing to recover, and the draft is removed.
    draft::save(&dir, &draft::Draft { text: "print(1);".into(), path: Some(file.display().to_string()) }).unwrap();
    let h = start(&dir, &scripts);
    assert!(h.state().script.text.is_empty() && !h.state().script.open);
    assert!(draft::load(&dir).is_none());
    // The file was deleted since: the text is recovered as an unsaved, unnamed script.
    draft::save(&dir, &draft::Draft { text: "print(2);".into(), path: Some(scripts.join("gone.rhai").display().to_string()) }).unwrap();
    let h = start(&dir, &scripts);
    assert_eq!(h.state().script.text, "print(2);");
    assert!(h.state().script.path.is_none());
}

#[test]
fn nothing_is_written_while_the_text_is_unchanged_since_the_last_write_and_failures_do_not_loop() {
    let (dir, scripts) = (tmp("d-rec"), tmp("d-scripts"));
    let mut h = start(&dir, &scripts);
    h.state_mut().script.text = "a".into();
    h.run();
    let f = draft::path_in(&dir);
    let first = std::fs::metadata(&f).unwrap().modified().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    h.run();
    h.run();
    assert_eq!(std::fs::metadata(&f).unwrap().modified().unwrap(), first, "unchanged text is not written again");
    // A text too large to keep is reported once and the old draft stays.
    h.state_mut().script.text = "z".repeat(draft::MAX_DRAFT_BYTES + 1);
    h.run();
    h.run();
    assert!(h.state().status.contains("could not be kept"), "{}", h.state().status);
    assert_eq!(draft::load(&dir).map(|d| d.text), Some("a".into()));
    // With the debounce, a change is not written until the text has been still for the delay.
    h.state_mut().script.text = "b".into();
    h.state_mut().script.recovery.delay = Duration::from_secs(60);
    h.run();
    assert_eq!(draft::load(&dir).map(|d| d.text), Some("a".into()));
}

#[test]
fn without_a_recovery_folder_nothing_happens() {
    let mut h = harness();
    h.run();
    h.state_mut().script.text = "print(1);".into();
    h.run();
    crate::script_recover::flush(h.state_mut());
    assert!(h.state().script.recovery.dir.is_none());
}
