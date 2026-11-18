//! Script libraries in the window (egui_kittest): imports from the scripts folder, and the `lib`
//! folder staying out of the Scripts menu.

use std::path::PathBuf;

use egui_kittest::Harness;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::script_ui::ScriptState;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-lib-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

fn names(h: &Harness<'static, ModelApp>) -> Vec<String> {
    h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect()
}

#[test]
fn a_script_in_the_menu_uses_a_library_and_the_library_is_not_a_menu_entry() {
    let dir = tmp("menu");
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib/names.rhai"), "fn tag(s) { \"lib-\" + s }").unwrap();
    std::fs::write(dir.join("Make.rhai"), "// Makes a tagged group\nimport \"lib/names\" as names;\nadd_group(names::tag(\"one\"));\nregister_tool(\"Tagged\", || { add_group(names::tag(\"tool\")); });").unwrap();
    let mut h = harness();
    h.run();
    h.state_mut().script = ScriptState::new(Some(dir.clone()));
    assert!(h.state().script.entries.iter().all(|e| e.folder.as_deref() != Some("lib")), "libraries are not scripts to run from the menu");
    assert!(h.state().script.entries.iter().any(|e| e.label == "Make"));
    crate::script_ui::run_file(h.state_mut(), &dir.join("Make.rhai"));
    assert_eq!(names(&h), vec!["lib-one".to_string()], "{:?}", h.state().script.console);
    // The tool the script registered calls the library after the script has ended.
    crate::script_ui::run_tool(h.state_mut(), "Tagged");
    assert_eq!(names(&h), vec!["lib-one".to_string(), "lib-tool".to_string()], "{:?}", h.state().script.console);
}

#[test]
fn a_missing_library_is_an_error_in_the_console_with_the_name() {
    let dir = tmp("missing");
    let mut h = harness();
    h.run();
    h.state_mut().script = ScriptState::new(Some(dir));
    let ok = crate::script_ui::execute(h.state_mut(), "t.rhai", "import \"lib/ghost\" as g;", None);
    assert!(!ok);
    assert!(h.state().script.console.iter().any(|l| l.text.contains("ghost")), "{:?}", h.state().script.console);
    // Without a scripts folder at all, imports are refused and say why.
    h.state_mut().script = ScriptState::new(None);
    assert!(!crate::script_ui::execute(h.state_mut(), "t.rhai", "import \"lib/x\" as g;", None));
}
