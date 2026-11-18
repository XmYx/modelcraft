//! Recorded macros in the window (egui_kittest): record from the keyboard, replay, save as a tool.

use std::path::PathBuf;

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::Command;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::script_macro::{MACRO_FOLDER, MACRO_MARK, file_stem, tool_path};
use crate::script_ui::ScriptState;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-macro-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn names(h: &Harness<'static, ModelApp>) -> Vec<String> {
    h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect()
}

fn add(h: &mut Harness<'static, ModelApp>, name: &str) {
    h.state_mut().doc.execute(Command::AddPrimitive { kind: "cube".into(), name: Some(name.into()), parent: None }).unwrap();
    h.run();
}

#[test]
fn the_command_palette_records_a_macro_and_the_window_shows_it_as_a_script() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Record Macro");
    assert!(h.state().doc.is_recording(), "the palette started the recording");
    add(&mut h, "Box");
    h.state_mut().doc.execute(Command::Rename { id: modelcraft_engine::scene::ObjectId(1), name: "Crate".into() }).unwrap();
    h.run();
    assert_eq!(h.state().doc.recording_steps(), 2);
    palette(&mut h, "Record Macro");
    assert!(!h.state().doc.is_recording());
    assert!(h.state().script.open, "the macro opens in the Scripts window");
    let text = h.state().script.text.clone();
    assert!(text.starts_with(MACRO_MARK), "{text}");
    assert!(text.contains("cmd::add_primitive") && text.contains("cmd::rename"), "{text}");
    // Replay: Run Script on a scene that already has other objects; the macro makes its own.
    h.state_mut().doc.execute(Command::Undo).unwrap();
    h.state_mut().doc.execute(Command::Undo).unwrap();
    add(&mut h, "Other");
    h.get_by_label_contains("Run Script").click();
    h.run();
    assert_eq!(names(&h), vec!["Other".to_string(), "Crate".to_string()], "{:?}", h.state().script.console);
    // One undo takes the whole replay back.
    h.state_mut().doc.execute(Command::Undo).unwrap();
    assert_eq!(names(&h), vec!["Other".to_string()]);
}

#[test]
fn nothing_recorded_opens_nothing_and_says_so() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Record Macro");
    palette(&mut h, "Record Macro");
    assert!(!h.state().script.open);
    assert!(h.state().script.text.is_empty());
    assert_eq!(h.state().status, "Nothing was recorded");
}

#[test]
fn a_macro_is_saved_as_a_tool_in_the_menu_folder_or_the_quick_strip() {
    let dir = tmp("save");
    let mut h = harness();
    h.run();
    h.state_mut().script = ScriptState::new(Some(dir.clone()));
    h.state_mut().script.macros.tool_name = "Make Box".into();
    palette(&mut h, "Record Macro");
    add(&mut h, "Boxy");
    palette(&mut h, "Record Macro");
    h.state_mut().script.macros.tool_name = "Make Box".into();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save as Tool").click();
    h.run();
    let file = dir.join(MACRO_FOLDER).join("Make Box.rhai");
    assert!(file.is_file(), "{:?} {}", std::fs::read_dir(&dir).map(|d| d.count()), h.state().status);
    assert!(std::fs::read_to_string(&file).unwrap().starts_with(MACRO_MARK));
    assert!(h.state().script.entries.iter().any(|e| e.label.contains("Make Box") && e.folder.as_deref() == Some(MACRO_FOLDER)), "the Scripts menu lists it");
    // Running the saved file from the menu entry makes the box again.
    let path = file.clone();
    crate::script_ui::run_file(h.state_mut(), &path);
    assert_eq!(names(&h).iter().filter(|n| n.starts_with("Boxy")).count(), 2, "{:?}", names(&h));
    // The strip: a second save with the strip checkbox goes to `quick`.
    h.state_mut().script.macros.quick = true;
    h.state_mut().script.macros.tool_name = "Strip Box".into();
    assert!(crate::script_macro::save_tool(h.state_mut()));
    assert!(dir.join("quick").join("Strip Box.rhai").is_file());
    assert!(h.state().script.quick.iter().any(|e| e.label.contains("Strip Box")), "the strip lists it");
}

#[test]
fn callbacks_do_not_end_up_in_the_recording() {
    let mut h = harness();
    h.run();
    assert!(crate::script_ui::execute(h.state_mut(), "cb.rhai", "on_scene_changed(|e| { if object_count() == 1 { add_group(\"echo\"); } });", None));
    h.run();
    palette(&mut h, "Record Macro");
    add(&mut h, "Mine");
    h.run();
    assert!(names(&h).contains(&"echo".to_string()), "the callback ran: {:?}", names(&h));
    palette(&mut h, "Record Macro");
    let text = h.state().script.text.clone();
    assert!(text.contains("Mine") && !text.contains("echo"), "{text}");
}

#[test]
fn recording_while_the_editor_has_unsaved_edits_asks_before_replacing_them() {
    let mut h = harness();
    h.run();
    h.state_mut().script.open = true;
    h.state_mut().script.text = "print(\"my draft\");".into();
    h.run();
    palette(&mut h, "Record Macro");
    add(&mut h, "Z");
    palette(&mut h, "Record Macro");
    assert_eq!(h.state().script.text, "print(\"my draft\");", "the draft is still there until the user decides");
    h.get_by_label("Discard Changes").click();
    h.run();
    assert!(h.state().script.text.starts_with(MACRO_MARK));
}

#[test]
fn tool_names_become_safe_file_names_and_other_files_are_never_replaced() {
    assert_eq!(file_stem("  ../../etc/passwd  "), "etcpasswd");
    assert_eq!(file_stem("Ünï Tool-1_x"), "Ünï Tool-1_x");
    assert_eq!(file_stem("\n\t"), "");
    let dir = tmp("names");
    assert_eq!(tool_path(&dir, "", false), None);
    let p = tool_path(&dir, "Mine", false).unwrap();
    assert_eq!(p, dir.join(MACRO_FOLDER).join("Mine.rhai"));
    // A file of the user's own is left alone: the name gets a number.
    std::fs::create_dir_all(dir.join(MACRO_FOLDER)).unwrap();
    std::fs::write(&p, "print(\"hand written\");").unwrap();
    assert_eq!(tool_path(&dir, "Mine", false).unwrap(), dir.join(MACRO_FOLDER).join("Mine 2.rhai"));
    // A macro this feature wrote may be saved over (the same tool, edited).
    std::fs::write(&p, format!("{MACRO_MARK} old\n")).unwrap();
    assert_eq!(tool_path(&dir, "Mine", false).unwrap(), p);
    assert_eq!(tool_path(&dir, "Mine", true).unwrap(), dir.join("quick").join("Mine.rhai"));
}
