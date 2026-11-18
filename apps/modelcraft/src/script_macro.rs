//! Recorded macros in the window: record what the user does, replay it as a script, save it as a
//! user tool. The recording itself is the document's ([`modelcraft_engine::Document::start_recording`]);
//! the script is written by [`modelcraft_script::macros`]. A macro is an ordinary script: it opens
//! in the Scripts window, runs with Ctrl+Enter as one undo step, and "Save as Tool" puts it in the
//! scripts folder (the Scripts menu, or the Quick Scripts strip).

use std::path::{Path, PathBuf};

use egui::RichText;
use modelcraft_engine::scene::ObjectId;
use modelcraft_script::macros::{MacroError, macro_script};

use crate::app::ModelApp;
use crate::theme;

/// The folder (inside the scripts folder) saved macros go to, which becomes a Scripts submenu.
pub const MACRO_FOLDER: &str = "Macros";
/// The first line every macro starts with, which also marks a file as one this feature wrote.
pub const MACRO_MARK: &str = "// Macro:";

#[derive(Default)]
pub struct MacroState {
    /// The selection when recording started (the macro refers to it as `sel`).
    selection: Vec<ObjectId>,
    /// Macros recorded this session, for default names.
    count: u32,
    /// The name "Save as Tool" uses.
    pub tool_name: String,
    /// Save into the Quick Scripts strip instead of the Scripts menu.
    pub quick: bool,
}

/// Start or stop recording (the Record Macro action).
pub fn toggle(app: &mut ModelApp) {
    if app.doc.is_recording() { stop(app) } else { start(app) }
}

pub fn start(app: &mut ModelApp) {
    if app.doc.is_recording() {
        return;
    }
    app.script.macros.selection = app.selection.clone();
    app.doc.start_recording();
    app.status = tl!("Recording a macro: what you do is recorded as commands. Stop Recording ends it.");
}

/// Stop recording and open the macro as a script in the Scripts window.
pub fn stop(app: &mut ModelApp) {
    let Some(rec) = app.doc.stop_recording() else { return };
    let truncated = rec.truncated;
    let number = app.script.macros.count + 1;
    let name = crate::tlf!("Macro {number}", number = number);
    match macro_script(&name, rec.steps(), &app.script.macros.selection) {
        Ok(text) => {
            app.script.macros.count = number;
            app.script.macros.tool_name = name.clone();
            app.status = if truncated {
                crate::tlf!("{name} recorded; it was cut at {limit} commands", name = name, limit = modelcraft_engine::MAX_RECORDED_COMMANDS)
            } else {
                crate::tlf!("{name} recorded: {steps} steps. Run it with Ctrl+Enter, or Save as Tool.", name = name, steps = rec.len())
            };
            app.script.macros.selection.clear();
            crate::script_ui::open_text(app, text);
        }
        Err(MacroError::Empty) => {
            app.status = tl!("Nothing was recorded");
        }
        Err(e) => {
            app.status = crate::tlf!("The macro could not be written: {error}", error = e);
        }
    }
}

/// A file name for a tool: letters, digits, spaces, `-` and `_` only.
pub fn file_stem(name: &str) -> String {
    let s: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_')).collect();
    s.trim().chars().take(60).collect::<String>().trim().to_string()
}

/// Where "Save as Tool" writes `name`: inside the scripts folder, in `Macros` (menu) or `quick`
/// (strip). A file that is not a macro written here is never replaced: the name gets a number.
pub fn tool_path(dir: &Path, name: &str, quick: bool) -> Option<PathBuf> {
    let stem = file_stem(name);
    if stem.is_empty() {
        return None;
    }
    let folder = dir.join(if quick { crate::script_ui::QUICK_DIR } else { MACRO_FOLDER });
    let mut n = 1;
    loop {
        let file = if n == 1 { format!("{stem}.{}", crate::script_ui::EXT) } else { format!("{stem} {n}.{}", crate::script_ui::EXT) };
        let p = folder.join(file);
        if !p.exists() || std::fs::read_to_string(&p).is_ok_and(|t| t.starts_with(MACRO_MARK)) {
            return Some(p);
        }
        n += 1;
        if n > 99 {
            return None;
        }
    }
}

/// Save the editor's script as a user tool.
pub fn save_tool(app: &mut ModelApp) -> bool {
    let Some(dir) = app.script.dir.clone() else {
        app.status = tl!("No scripts folder on this system");
        return false;
    };
    if app.script.text.trim().is_empty() {
        app.status = tl!("There is no script to save as a tool");
        return false;
    }
    let name = app.script.macros.tool_name.clone();
    let Some(path) = tool_path(&dir, &name, app.script.macros.quick) else {
        app.status = tl!("Give the tool a name made of letters and digits");
        return false;
    };
    if !crate::script_ui::save_to(app, &path) {
        return false;
    }
    let place = if app.script.macros.quick { tl!("the Quick Scripts strip") } else { tl!("the Scripts menu") };
    app.status = crate::tlf!("Saved {name} as a tool in {place}", name = name, place = place);
    true
}

/// The row at the top of the Scripts window: record, stop, name and save as a tool.
pub fn row(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        if app.doc.is_recording() {
            let b = ui.button(tl!("Stop Recording"));
            if b.clicked() {
                stop(app);
            }
            let l = ui.label(RichText::new(crate::tlf!("Recording: {steps} steps", steps = app.doc.recording_steps())).color(theme::SELECT));
            ui.ctx().accesskit_node_builder(l.id, |n| n.set_live(egui::accesskit::Live::Polite));
        } else if ui.button(tl!("Record Macro")).clicked() {
            start(app);
        }
        ui.separator();
        ui.label(RichText::new(tl!("Tool name")).color(theme::TEXT_DIM));
        let name = ui.add(egui::TextEdit::singleline(&mut app.script.macros.tool_name).desired_width(140.0));
        ui.ctx().accesskit_node_builder(name.id, |n| n.set_label(tl!("Tool name")));
        ui.checkbox(&mut app.script.macros.quick, tl!("Quick Scripts strip"));
        if ui.button(tl!("Save as Tool")).clicked() {
            save_tool(app);
        }
    });
}
