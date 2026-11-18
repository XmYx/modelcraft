//! The Plugins window (egui_kittest): install, the permission prompt, running, updating, removing.

use std::path::{Path, PathBuf};

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_script::plugin::API_VERSION;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::script_ui::ConsoleKind;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-plugin-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

fn package(dir: &Path, version: &str, perms: &[&str], entry: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let m = serde_json::json!({"id": "row.tools", "name": "Row Tools", "version": version, "api": API_VERSION, "entry": "main.rhai", "description": "Makes rows of cubes", "author": "Ann", "permissions": perms});
    std::fs::write(dir.join("plugin.json"), m.to_string()).unwrap();
    std::fs::write(dir.join("main.rhai"), entry).unwrap();
}

fn start(plugins: &Path) -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    crate::plugin_ui::init(h.state_mut(), Some(plugins.to_path_buf()));
    h.run();
    h
}

fn names(h: &Harness<'static, ModelApp>) -> Vec<String> {
    h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect()
}

const ENTRY: &str = "register_tool(\"Add Row\", \"Adds a cube\", || { add_primitive(\"cube\", \"Row cube\"); });";

#[test]
fn a_new_plugin_asks_before_it_runs_and_its_tool_works_once_allowed() {
    let (plugins, src) = (tmp("a-plugins"), tmp("a-src"));
    package(&src, "1.0.0", &["register_tools", "edit_scene"], ENTRY);
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    assert!(h.state().script.plugins.open, "installing opens the window on the prompt");
    assert!(h.state().script.plugins.loaded.is_empty(), "nothing runs before the user decides");
    h.get_by_label_contains("Row Tools 1.0.0 wants permission");
    assert!(h.query_all_by_label_contains("Change your scene").count() >= 1);
    assert!(h.query_all_by_label_contains("Add commands to the Scripts menu").count() >= 1);
    // The safe answer has the keyboard focus.
    assert!(h.get_by_role_and_label(Role::Button, "Don't Allow").accesskit_node().is_focused());
    h.get_by_role_and_label(Role::Button, "Allow").click();
    h.run();
    assert_eq!(h.state().script.plugins.loaded.len(), 1);
    assert!(h.state().script.plugins.errors.is_empty(), "{:?}", h.state().script.plugins.errors);
    // The tool is in the Scripts menu and changes the scene as an undoable step.
    assert_eq!(crate::plugin_ui::tools(h.state()).len(), 1);
    crate::plugin_ui::run_tool(h.state_mut(), "row.tools", "Add Row");
    assert_eq!(names(&h), vec!["Row cube".to_string()]);
    h.state_mut().doc.execute(modelcraft_engine::Command::Undo).unwrap();
    assert!(names(&h).is_empty());
    // The decision is remembered: a new session starts the plugin without asking.
    drop(h);
    let h2 = start(&plugins);
    assert_eq!(h2.state().script.plugins.loaded.len(), 1);
    assert!(h2.state().script.plugins.pending().is_none());
}

#[test]
fn saying_no_keeps_it_off_and_turning_it_on_asks_again() {
    let (plugins, src) = (tmp("b-plugins"), tmp("b-src"));
    package(&src, "1.0.0", &["register_tools", "edit_scene"], ENTRY);
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    h.get_by_role_and_label(Role::Button, "Don't Allow").click();
    h.run();
    assert!(h.state().script.plugins.loaded.is_empty());
    assert!(h.state().script.plugins.pending().is_none());
    h.get_by_label_contains("Off");
    assert!(crate::plugin_ui::tools(h.state()).is_empty());
    // Turning it on does not grant anything by itself: the prompt comes back.
    h.get_by_role_and_label(Role::Button, "Turn On").click();
    h.run();
    assert!(h.state().script.plugins.pending().is_some(), "still nothing granted");
    assert!(h.state().script.plugins.loaded.is_empty());
}

#[test]
fn a_plugin_can_only_do_what_was_allowed() {
    let (plugins, src) = (tmp("c-plugins"), tmp("c-src"));
    // It asks for menu tools only, then tries to change the scene in its start script.
    package(&src, "1.0.0", &["register_tools"], "add_group(\"sneaky\"); register_tool(\"X\", || { });");
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    h.get_by_role_and_label(Role::Button, "Allow").click();
    h.run();
    assert!(names(&h).is_empty(), "{:?}", names(&h));
    let err = h.state().script.plugins.errors.get("row.tools").cloned().unwrap_or_default();
    assert!(err.contains("edit_scene"), "the card says why: {err}");
    assert!(h.state().script.console.iter().any(|l| l.kind == ConsoleKind::Error && l.text.contains("edit_scene")));
}

#[test]
fn an_update_that_asks_for_more_asks_about_the_extra_only() {
    let (plugins, src) = (tmp("d-plugins"), tmp("d-src"));
    package(&src, "1.0.0", &["register_tools"], ENTRY);
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    crate::plugin_ui::approve(h.state_mut(), "row.tools");
    h.run();
    assert_eq!(h.state().script.plugins.loaded.len(), 1);
    // A version that asks for the same runs without a prompt; one that asks for more stops and asks.
    package(&src, "1.1.0", &["register_tools"], ENTRY);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    assert!(h.state().script.plugins.pending().is_none());
    assert_eq!(h.state().script.plugins.loaded.len(), 1);
    package(&src, "1.2.0", &["register_tools", "write_project_files"], ENTRY);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    assert!(h.state().script.plugins.loaded.is_empty(), "the new version waits for a decision");
    h.get_by_label_contains("Row Tools 1.2.0 wants permission");
    assert!(h.query_all_by_label_contains("Write files in a plugin-out folder").count() >= 1);
    // An older version is refused.
    package(&src, "0.5.0", &[], ENTRY);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    assert!(h.state().script.plugins.message.contains("already installed"), "{}", h.state().script.plugins.message);
}

#[test]
fn a_listening_plugin_hears_events_and_a_removed_one_is_gone() {
    let (plugins, src) = (tmp("e-plugins"), tmp("e-src"));
    package(&src, "1.0.0", &["listen_events"], "on_scene_changed(|e| { print(\"plugin heard the scene\"); });");
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    crate::plugin_ui::approve(h.state_mut(), "row.tools");
    h.run();
    h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: Some("g".into()) }).unwrap();
    h.run();
    h.run();
    assert!(h.state().script.console.iter().any(|l| l.kind == ConsoleKind::Print && l.text == "plugin heard the scene"), "{:?}", h.state().script.console);
    // Removing asks first.
    h.state_mut().script.plugins.open = true;
    h.run();
    h.get_by_role_and_label(Role::Button, "Remove…").click();
    h.run();
    assert!(plugins.join("row.tools").exists(), "not yet");
    h.get_by_role_and_label(Role::Button, "Keep").click();
    h.run();
    assert!(plugins.join("row.tools").exists());
    h.get_by_role_and_label(Role::Button, "Remove…").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Remove").click();
    h.run();
    assert!(!plugins.join("row.tools").exists());
    assert!(h.state().script.plugins.loaded.is_empty() && h.state().script.plugins.installed.is_empty());
}

#[test]
fn broken_packages_are_refused_with_a_reason_and_listed_when_found_on_disk() {
    let (plugins, src) = (tmp("f-plugins"), tmp("f-src"));
    package(&src, "1.0.0", &[], ENTRY);
    std::fs::write(src.join("native.so"), "x").unwrap();
    let mut h = start(&plugins);
    crate::plugin_ui::install_from(h.state_mut(), &src);
    h.run();
    assert!(h.state().script.plugins.message.contains("Could not install"), "{}", h.state().script.plugins.message);
    assert!(!plugins.join("row.tools").exists());
    // A folder dropped into the plugins folder by hand that is not a plugin is listed with why.
    std::fs::create_dir_all(plugins.join("mystery")).unwrap();
    crate::plugin_ui::reload(h.state_mut());
    h.run();
    h.get_by_label_contains("mystery could not be read");
}
