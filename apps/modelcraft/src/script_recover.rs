//! The Scripts window keeps what you typed: an unsaved script is written to the recovery folder a
//! moment after the last change (and when the app exits), removed when it is saved or emptied, and
//! offered back at the next start. The file format and its checks are `modelcraft_script::draft`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use modelcraft_script::draft::{self, Draft};

use crate::app::ModelApp;
use crate::script_ui::ConsoleKind;

pub struct Recovery {
    /// The recovery folder (none in tests unless one is set, and when the system has none).
    pub dir: Option<PathBuf>,
    /// How long the text must stay unchanged before it is written.
    pub delay: Duration,
    /// The text last written.
    written: String,
    /// When the unwritten change was first seen.
    since: Option<Instant>,
    /// The failure was already reported.
    reported: bool,
}

impl Default for Recovery {
    fn default() -> Self {
        Recovery { dir: None, delay: Duration::from_millis(1500), written: String::new(), since: None, reported: false }
    }
}

/// Use `dir` for drafts and bring back the one an earlier session left.
pub fn init(app: &mut ModelApp, dir: Option<PathBuf>) {
    app.script.recovery.dir = dir;
    recover(app);
}

fn recover(app: &mut ModelApp) {
    let Some(dir) = app.script.recovery.dir.clone() else { return };
    let Some(d) = draft::load(&dir) else { return };
    let path = d.path.as_ref().map(PathBuf::from).filter(|p| p.is_file());
    let saved = path.as_ref().and_then(|p| crate::script_ui::read_script(p).ok()).unwrap_or_default();
    if d.text == saved {
        draft::clear(&dir);
        return;
    }
    let name = path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    app.script.recovery.written = d.text.clone();
    app.script.text = d.text;
    app.script.set_saved(saved);
    app.script.path = path;
    app.script.open = true;
    let note = match name {
        Some(n) => crate::tlf!("Recovered unsaved changes to {script} from the last session.", script = n),
        None => tl!("Recovered an unsaved script from the last session."),
    };
    app.script.note_line(ConsoleKind::Note, note.clone());
    app.status = note;
}

/// Once per frame: write the draft when the text has been still for the delay, remove it when
/// there is nothing unsaved.
pub fn tick(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(dir) = app.script.recovery.dir.clone() else { return };
    if !app.script.modified() || app.script.text.is_empty() {
        let r = &mut app.script.recovery;
        if !r.written.is_empty() {
            draft::clear(&dir);
            r.written.clear();
        }
        r.since = None;
        r.reported = false;
        return;
    }
    if app.script.text == app.script.recovery.written {
        app.script.recovery.since = None;
        return;
    }
    let delay = app.script.recovery.delay;
    let since = *app.script.recovery.since.get_or_insert_with(Instant::now);
    let waited = since.elapsed();
    if waited < delay {
        ctx.request_repaint_after(delay - waited);
        return;
    }
    write(app, &dir);
}

/// Write the draft now (the app is closing).
pub fn flush(app: &mut ModelApp) {
    let Some(dir) = app.script.recovery.dir.clone() else { return };
    if app.script.modified() && !app.script.text.is_empty() && app.script.text != app.script.recovery.written {
        write(app, &dir);
    }
}

fn write(app: &mut ModelApp, dir: &std::path::Path) {
    let d = Draft { text: app.script.text.clone(), path: app.script.path.as_ref().map(|p| p.display().to_string()) };
    match draft::save(dir, &d) {
        Ok(()) => {
            let r = &mut app.script.recovery;
            r.written = d.text;
            r.since = None;
            r.reported = false;
        }
        Err(e) => {
            let r = &mut app.script.recovery;
            r.since = Some(Instant::now());
            if !std::mem::replace(&mut r.reported, true) {
                app.status = crate::tlf!("The script could not be kept for recovery: {error}", error = e);
            }
        }
    }
}
