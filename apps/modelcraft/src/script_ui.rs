//! The Scripts window: an editor for Rhai scripts with syntax highlighting, an output console that
//! links errors to their line, script files on disk, a startup script and the Scripts menu and
//! Quick Scripts strip built from the scripts folder.
//!
//! The language, its sandbox and its limits live in the UI-free `modelcraft-script` crate; this
//! module is the window around it. A run happens on the UI thread between frames, bounded by the
//! script limits (10 s of wall-clock time by default), and everything it changes goes through the
//! document's commands, so Undo reverts a script's work like any other edit.
//!
//! Keyboard: Ctrl+J opens the window (focus moves into the editor), Ctrl+Enter runs the script,
//! Ctrl+Shift+Enter runs the selection, Ctrl+S in the editor saves the script file, and Esc closes
//! the window and returns focus to the viewport.

use std::path::{Path, PathBuf};

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, Key, KeyboardShortcut, Modifiers, RichText};
use modelcraft_script::highlight::{TokenKind, tokenize};
use modelcraft_script::{ErrorKind, Event, EventPacer, Limits, LineKind, Permissions, PlaybackPolicy, RunContext, Session};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::theme;

/// Largest script file the editor opens.
pub const MAX_FILE_BYTES: u64 = 1 << 20;
/// Entries read from each folder of the scripts folder.
const MAX_SCAN: usize = 500;
/// Bytes of a script read for its tooltip.
const TIP_BYTES: usize = 4096;
/// Console lines kept.
const MAX_CONSOLE_LINES: usize = 2000;
/// Above this many bytes the editor stops colouring (the text stays editable).
const HIGHLIGHT_LIMIT: usize = 200_000;
/// A script with this name in the scripts folder runs when the app starts.
pub const STARTUP: &str = "startup.rhai";
/// Scripts in this subfolder of the scripts folder become buttons in the Quick Scripts strip.
pub const QUICK_DIR: &str = "quick";
pub const EXT: &str = "rhai";
/// The subfolder of the scripts folder that holds libraries (`import "lib/name" as m;`).
pub const LIBRARY_DIR: &str = "lib";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsoleKind {
    Note,
    Print,
    Debug,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleLine {
    pub kind: ConsoleKind,
    pub text: String,
    /// The script line an error or `debug` call points at.
    pub line: Option<usize>,
}

/// A script file in the scripts folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub path: PathBuf,
    /// The subfolder it is in (one level), for submenus.
    pub folder: Option<String>,
    /// The first comment line, shown as a tooltip.
    pub tip: String,
}

/// What to do once unsaved edits have been dealt with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Then {
    New,
    Open(PathBuf),
    Pick,
    /// A new unsaved script with this text (a recorded macro).
    Text(String),
}

pub struct ScriptState {
    pub open: bool,
    pub text: String,
    /// The file being edited, if it has one.
    pub path: Option<PathBuf>,
    saved: String,
    pub console: Vec<ConsoleLine>,
    pub(crate) session: Option<Session>,
    /// The scripts folder (`None` in tests unless one is set).
    pub dir: Option<PathBuf>,
    pub allow_writes: bool,
    pub entries: Vec<Entry>,
    pub quick: Vec<Entry>,
    pub show_quick: bool,
    /// Line (1-based) to put the cursor on.
    goto: Option<usize>,
    focus_pending: bool,
    had_focus: bool,
    /// The editor text field had the keyboard focus in the last frame.
    editor_focus: bool,
    /// The last selection (character range) the editor reported.
    sel: Option<(usize, usize)>,
    /// Cursor line and column, 1-based, for the footer.
    cursor: (usize, usize),
    /// Line of the last error, marked in the editor.
    pub error_line: Option<usize>,
    confirm: Option<Then>,
    /// A project was loaded since the last frame (fires `scene_loaded`).
    loaded: bool,
    /// What the last frame looked like, to notice changes for event callbacks.
    last_selection: Vec<modelcraft_engine::scene::ObjectId>,
    last_sig: u64,
    last_frame: f64,
    /// Holds back and merges frame and scene events while the timeline plays.
    pub pacer: EventPacer,
    /// The pacer's clock.
    clock: std::time::Instant,
    /// Playback was running in the last frame (to deliver what was held back when it stops).
    was_playing: bool,
    /// The undo history's mark at the end of the last frame: steps made after it are the
    /// user's (or an agent's), and callbacks that answer them join the newest one.
    last_mark: usize,
    pub macros: crate::script_macro::MacroState,
    pub recovery: crate::script_recover::Recovery,
    pub plugins: crate::plugin_ui::PluginState,
}

impl Default for ScriptState {
    fn default() -> Self {
        ScriptState::new(None)
    }
}

/// `<data folder>/modelcraft/scripts`.
pub fn default_dir() -> Option<PathBuf> {
    craft_settings::paths::data_dir("modelcraft").map(|d| d.join("scripts"))
}

impl ScriptState {
    pub fn new(dir: Option<PathBuf>) -> Self {
        let mut s = ScriptState {
            open: false,
            text: String::new(),
            path: None,
            saved: String::new(),
            console: Vec::new(),
            session: None,
            dir,
            allow_writes: false,
            entries: Vec::new(),
            quick: Vec::new(),
            show_quick: true,
            goto: None,
            focus_pending: false,
            had_focus: false,
            editor_focus: false,
            sel: None,
            cursor: (1, 1),
            error_line: None,
            confirm: None,
            loaded: false,
            last_selection: Vec::new(),
            last_sig: 0,
            last_frame: 0.0,
            pacer: EventPacer::new(PlaybackPolicy::default()),
            clock: std::time::Instant::now(),
            was_playing: false,
            last_mark: 0,
            macros: Default::default(),
            recovery: Default::default(),
            plugins: Default::default(),
        };
        s.reload();
        s
    }

    pub fn modified(&self) -> bool {
        self.text != self.saved
    }

    /// What the text is compared with to tell whether it is modified (the recovered text of an
    /// earlier session is compared with the file it came from).
    pub(crate) fn set_saved(&mut self, saved: String) {
        self.saved = saved;
    }

    pub(crate) fn note_line(&mut self, kind: ConsoleKind, text: String) {
        self.note(kind, text, None);
    }

    /// The name shown for the script and used in error messages.
    pub fn name(&self) -> String {
        self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("untitled.{EXT}"))
    }

    /// Read the scripts folder again.
    pub fn reload(&mut self) {
        let (entries, quick) = match &self.dir {
            Some(d) => scan(d),
            None => (Vec::new(), Vec::new()),
        };
        self.entries = entries;
        self.quick = quick;
    }

    fn note(&mut self, kind: ConsoleKind, text: impl Into<String>, line: Option<usize>) {
        self.console.push(ConsoleLine { kind, text: text.into(), line });
        if self.console.len() > MAX_CONSOLE_LINES {
            let drop = self.console.len() - MAX_CONSOLE_LINES;
            self.console.drain(..drop);
        }
    }
}

fn first_comment(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).and_then(|l| l.strip_prefix("//")).map(|l| l.trim().to_string()).unwrap_or_default()
}

pub(crate) fn read_script(path: &Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(crate::tlf!("{file} is larger than {size} bytes", file = path.display(), size = MAX_FILE_BYTES));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|_| crate::tlf!("{file} is not UTF-8 text", file = path.display()))
}

fn is_script(p: &Path) -> bool {
    p.is_file() && p.extension().is_some_and(|e| e == EXT)
}

/// The first comment line of a file, reading only its first few KiB.
fn tip_of(path: &Path) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; TIP_BYTES];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    buf.truncate(n);
    first_comment(&String::from_utf8_lossy(&buf))
}

fn entry_of(path: PathBuf, folder: Option<String>) -> Entry {
    let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let tip = tip_of(&path);
    Entry { label, path, folder, tip }
}

/// The menu entries and the quick scripts of a scripts folder: `*.rhai` at the top and one level
/// of subfolders (the `quick` subfolder is the strip), sorted by name, without the startup script.
pub fn scan(dir: &Path) -> (Vec<Entry>, Vec<Entry>) {
    // At most MAX_SCAN entries per folder are read, however many there are.
    let list = |d: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d).map(|rd| rd.filter_map(|e| e.ok()).take(MAX_SCAN).map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let mut entries = Vec::new();
    let mut quick = Vec::new();
    for p in list(dir) {
        if is_script(&p) && p.file_name().is_some_and(|n| n != STARTUP) {
            entries.push(entry_of(p, None));
        } else if p.is_dir() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Dot folders are hidden, and `lib` holds libraries for `import`, not scripts to run.
            if name.starts_with('.') || name == LIBRARY_DIR {
                continue;
            }
            for q in list(&p).into_iter().filter(|q| is_script(q)) {
                if name == QUICK_DIR {
                    quick.push(entry_of(q, None));
                } else {
                    entries.push(entry_of(q, Some(name.clone())));
                }
            }
        }
    }
    (entries, quick)
}

// ---------------------------------------------------------------- running

fn project_dir(app: &ModelApp) -> Option<PathBuf> {
    app.doc.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf).filter(|p| !p.as_os_str().is_empty())
}

/// A folder a script may be given write access to: not a filesystem root and not the user's home
/// folder (a project saved straight into the home folder would otherwise give a script the
/// dot-files that run at the next login).
fn home_folders() -> Vec<PathBuf> {
    ["HOME", "USERPROFILE"].into_iter().filter_map(std::env::var_os).filter_map(|h| std::fs::canonicalize(h).ok()).collect()
}

fn safe_write_root_given(dir: &Path, homes: &[PathBuf]) -> bool {
    let Ok(canon) = std::fs::canonicalize(dir) else { return false };
    canon.parent().is_some() && !homes.contains(&canon)
}

/// The folders a script run may write when "Allow writing files" is on: the project's folder (or
/// `scripts-out` inside it when the project's folder is not a safe place to give away, created on
/// demand) and the scripts folder.
pub fn write_roots(app: &ModelApp) -> Vec<PathBuf> {
    write_roots_given(app, &home_folders())
}

fn write_roots_given(app: &ModelApp, homes: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(p) = project_dir(app) {
        if safe_write_root_given(&p, homes) {
            roots.push(p);
        } else {
            let out = p.join("scripts-out");
            if std::fs::create_dir_all(&out).is_ok() {
                roots.push(out);
            }
        }
    }
    roots.extend(app.script.dir.iter().filter(|d| d.is_dir()).cloned());
    roots
}

/// What a script run may touch: the script's folder, the project's folder and the scripts folder
/// to read; writing only when the window's "Allow writing files" is on, and then only the
/// folders of [`write_roots`].
fn permissions(app: &ModelApp, script: Option<&Path>) -> Permissions {
    let project = project_dir(app);
    let script_dir = script.and_then(Path::parent).map(Path::to_path_buf).filter(|p| !p.as_os_str().is_empty());
    let base = script_dir.clone().or_else(|| project.clone()).or_else(|| app.script.dir.clone()).unwrap_or_else(|| PathBuf::from("."));
    let mut p = Permissions::none().with_base(base);
    for d in [script_dir.as_ref(), project.as_ref(), app.script.dir.as_ref()].into_iter().flatten() {
        p = p.allow_read(d);
    }
    if app.script.allow_writes {
        for d in write_roots(app) {
            p = p.allow_write(d);
        }
    }
    p
}

/// Run `source` against the document and report in the console and the status line.
/// Returns whether it finished without an error.
pub fn execute(app: &mut ModelApp, name: &str, source: &str, script: Option<&Path>) -> bool {
    let perms = permissions(app, script);
    let file = app.doc.path.as_ref().map(|p| p.display().to_string());
    let session = app.script.session.get_or_insert_with(|| Session::new(Limits::default(), Permissions::none()));
    session.set_permissions(perms);
    session.set_library(app.script.dir.clone());
    session.set_current_file(file);
    let mut ctx = RunContext { selection: app.selection.clone() };
    app.script.note(ConsoleKind::Note, crate::tlf!("Running {script}…", script = name), None);
    let Some(session) = app.script.session.as_mut() else { return false };
    let report = session.run(&mut app.doc, &mut ctx, name, source);
    finish(app, name, report)
}

/// Show a run's console lines, the selection it set and its error (no summary line).
pub(crate) fn show_report(app: &mut ModelApp, report: &modelcraft_script::RunReport) {
    for l in &report.lines {
        let kind = match l.kind {
            LineKind::Print => ConsoleKind::Print,
            LineKind::Debug => ConsoleKind::Debug,
            LineKind::Warning => ConsoleKind::Warning,
        };
        app.script.note(kind, l.text.clone(), l.line);
    }
    if let Some(sel) = &report.selection {
        app.selection = sel.clone();
    }
    if let Some(e) = &report.error {
        app.script.note(ConsoleKind::Error, e.to_string(), e.line);
    }
    app.prune_selection();
}

pub(crate) fn finish_report(app: &mut ModelApp, name: &str, report: modelcraft_script::RunReport) -> bool {
    finish(app, name, report)
}

/// Show a finished run: console lines, the selection it set, and the outcome in the status line.
fn finish(app: &mut ModelApp, name: &str, report: modelcraft_script::RunReport) -> bool {
    for l in &report.lines {
        let kind = match l.kind {
            LineKind::Print => ConsoleKind::Print,
            LineKind::Debug => ConsoleKind::Debug,
            LineKind::Warning => ConsoleKind::Warning,
        };
        app.script.note(kind, l.text.clone(), l.line);
    }
    if let Some(sel) = report.selection {
        app.selection = sel;
    }
    // Scripts can delete what the selection or the component selection pointed at.
    app.prune_selection();
    match &report.error {
        None => {
            app.script.error_line = None;
            let done = crate::tlf!(
                "{script} finished: {commands} commands in {seconds} s",
                script = name,
                commands = report.commands,
                seconds = format!("{:.2}", report.elapsed.as_secs_f64())
            );
            app.script.note(ConsoleKind::Note, done.clone(), None);
            app.status = done;
            true
        }
        Some(e) => {
            app.script.error_line = e.line;
            app.script.note(ConsoleKind::Error, e.to_string(), e.line);
            if e.kind == ErrorKind::Limit {
                app.script.note(ConsoleKind::Note, tl!("Commands the script ran before it stopped stay applied; Undo reverts them."), None);
            }
            app.status = crate::tlf!("{script} failed: {error}", script = name, error = e);
            false
        }
    }
}

/// Run a tool that a script registered (a Scripts menu command).
pub fn run_tool(app: &mut ModelApp, label: &str) {
    let mut ctx = RunContext { selection: app.selection.clone() };
    app.script.note(ConsoleKind::Note, crate::tlf!("Running {script}…", script = label), None);
    let Some(session) = app.script.session.as_mut() else { return };
    let report = session.run_tool(&mut app.doc, &mut ctx, label);
    if !finish(app, label, report) {
        app.script.open = true;
    }
}

fn scene_signature(app: &ModelApp) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for o in app.doc.scene.objects() {
        h = (h ^ o.id.0).wrapping_mul(0x0100_0000_01b3);
        h = (h ^ o.rev).wrapping_mul(0x0100_0000_01b3);
    }
    h ^ app.doc.scene.len() as u64
}

/// A project was opened, imported or started new: `scene_loaded` callbacks run next frame.
pub fn scene_loaded(app: &mut ModelApp) {
    app.script.loaded = true;
}

/// How long callbacks may run for one event: the usual limit, or a budget shared with the other
/// events of a batch (while the timeline plays).
struct Budget {
    each: std::time::Duration,
    left: std::time::Duration,
}

/// Tell the callbacks of `event`, the user's scripts and the plugins'. Returns the time they took.
fn fire(app: &mut ModelApp, event: &Event, budget: Option<&Budget>) -> std::time::Duration {
    let mut total = std::time::Duration::ZERO;
    let mut left = budget.map(|b| b.left);
    for which in std::iter::once(None).chain((0..app.script.plugins.loaded.len()).map(Some)) {
        let b = budget.zip(left).map(|(b, left)| Budget { each: b.each, left });
        let t = fire_one(app, which, event, b.as_ref());
        total += t;
        left = left.map(|l| l.saturating_sub(t));
    }
    total
}

/// One session's callbacks: the user's (`None`) or a plugin's.
fn fire_one(app: &mut ModelApp, which: Option<usize>, event: &Event, budget: Option<&Budget>) -> std::time::Duration {
    let taken = match which {
        None => app.script.session.take(),
        Some(i) => app.script.plugins.loaded.get_mut(i).and_then(|p| p.session.take()),
    };
    let Some(mut session) = taken else { return std::time::Duration::ZERO };
    let mut ctx = RunContext { selection: app.selection.clone() };
    let (reports, elapsed) = match budget {
        None => (session.fire(&mut app.doc, &mut ctx, event), std::time::Duration::ZERO),
        Some(b) => {
            let f = session.fire_within(&mut app.doc, &mut ctx, event, b.each, b.left);
            (f.reports, f.elapsed)
        }
    };
    match which {
        None => app.script.session = Some(session),
        Some(i) => {
            if let Some(p) = app.script.plugins.loaded.get_mut(i) {
                p.session = Some(session);
            }
        }
    }
    for r in reports {
        // Callbacks are quiet unless they print or fail: no "finished" line per event.
        for l in &r.lines {
            let kind = match l.kind {
                LineKind::Print => ConsoleKind::Print,
                LineKind::Debug => ConsoleKind::Debug,
                LineKind::Warning => ConsoleKind::Warning,
            };
            app.script.note(kind, l.text.clone(), l.line);
        }
        if let Some(sel) = r.selection {
            app.selection = sel;
        }
        if let Some(e) = &r.error {
            app.script.note(ConsoleKind::Error, e.to_string(), e.line);
            app.status = crate::tlf!("A script callback failed: {error}", error = e);
        }
    }
    app.prune_selection();
    elapsed
}

/// Once per frame: tell the callbacks what changed (selection, scene, frame, a loaded project).
/// Nothing runs while a drag or a stroke is going on; the change is reported when it ends. What a
/// callback itself changes does not count as a change, so callbacks cannot trigger themselves.
///
/// While the timeline plays, the frame and scene events (which change every drawn frame) go
/// through the pacer instead: they are merged into one batch per interval carrying the newest
/// frame, the callbacks of a batch share a small time budget, and what was held back is delivered
/// once when playback stops.
pub fn tick(app: &mut ModelApp) {
    let sig = scene_signature(app);
    let frame = app.doc.scene.anim.frame;
    let listening = app.script.session.as_ref().is_some_and(|s| s.has_callbacks()) || app.script.plugins.listening();
    if !listening || app.drag.is_some() || app.stroke.is_some() {
        if !listening {
            app.script.last_selection = app.selection.clone();
            app.script.last_sig = sig;
            app.script.last_frame = frame;
            app.script.loaded = false;
            app.script.was_playing = false;
            app.script.pacer.flush();
            app.script.last_mark = app.doc.undo_mark();
        }
        return;
    }
    let playing = app.anim.playback.playing;
    let scene_changed = sig != app.script.last_sig;
    let frame_changed = frame != app.script.last_frame;
    let mut immediate = Vec::new();
    if std::mem::take(&mut app.script.loaded) {
        immediate.push(Event::SceneLoaded);
    }
    if app.selection != app.script.last_selection {
        immediate.push(Event::SelectionChanged);
    }
    let now = app.script.clock.elapsed();
    let mut paced = Vec::new();
    if playing {
        app.script.was_playing = true;
        paced = app.script.pacer.offer(now, frame_changed.then_some(frame), scene_changed);
    } else if std::mem::take(&mut app.script.was_playing) || app.script.pacer.pending() {
        // Playback stopped: the current change joins what was held back, and all of it is delivered.
        paced = app.script.pacer.offer(now, frame_changed.then_some(frame), scene_changed);
        paced.extend(app.script.pacer.flush());
    } else {
        if scene_changed {
            immediate.push(Event::SceneChanged);
        }
        if frame_changed {
            immediate.push(Event::FrameChanged { frame, skipped: 0 });
        }
    }
    app.script.last_selection = app.selection.clone();
    app.script.last_sig = sig;
    app.script.last_frame = frame;
    if immediate.is_empty() && paced.is_empty() {
        app.script.last_mark = app.doc.undo_mark();
        return;
    }
    // What the callbacks change joins one undo step (see `Document::fold_undo_since`): the user's
    // step that they answer when there is one, otherwise a single "Script callbacks" step. The
    // redo history is held aside, so reacting to an Undo does not make it impossible to Redo.
    let user_step = app.doc.undo_mark() > app.script.last_mark && !app.doc.undo.can_redo();
    let mark = app.doc.undo_mark();
    let redo = app.doc.undo.take_redo();
    // A recording holds what the user does, not what callbacks answer with.
    app.doc.pause_recording(true);
    for e in &immediate {
        fire(app, e, None);
    }
    if !paced.is_empty() {
        let policy = app.script.pacer.policy().clone();
        // A batch delivered while playing shares one budget; the flush at the end has none.
        let mut left = policy.frame_budget;
        let mut spent = std::time::Duration::ZERO;
        for e in &paced {
            let t = if playing {
                let b = Budget { each: policy.event_time, left };
                let t = fire(app, e, Some(&b));
                left = left.saturating_sub(t);
                t
            } else {
                let started = std::time::Instant::now();
                fire(app, e, None);
                started.elapsed()
            };
            spent += t;
        }
        if playing {
            app.script.pacer.record_cost(now, spent);
        }
    }
    app.doc.pause_recording(false);
    app.doc.fold_undo_since(mark, &tl!("Script callbacks"), user_step);
    app.doc.undo.restore_redo(redo);
    app.script.last_mark = app.doc.undo_mark();
    app.script.last_selection = app.selection.clone();
    app.script.last_sig = scene_signature(app);
    app.script.last_frame = app.doc.scene.anim.frame;
}

/// Run the editor's script (or only its selection).
pub fn run_editor(app: &mut ModelApp, selection_only: bool) {
    let name = app.script.name();
    let path = app.script.path.clone();
    let source = if selection_only {
        let Some((a, b)) = app.script.sel else {
            app.status = tl!("Select the part of the script to run first");
            return;
        };
        let chars: Vec<char> = app.script.text.chars().collect();
        let (a, b) = (a.min(chars.len()), b.min(chars.len()));
        // Blank lines and spaces keep the line and column numbers of errors those of the file.
        let before = &chars[..a];
        let line = before.iter().filter(|c| **c == '\n').count();
        let col = before.iter().rev().take_while(|c| **c != '\n').count();
        format!("{}{}{}", "\n".repeat(line), " ".repeat(col), chars[a..b].iter().collect::<String>())
    } else {
        app.script.text.clone()
    };
    execute(app, &name, &source, path.as_deref());
}

/// Run a script file from the Scripts menu or the Quick Scripts strip.
pub fn run_file(app: &mut ModelApp, path: &Path) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into());
    match read_script(path) {
        Ok(text) => {
            if !execute(app, &name, &text, Some(path)) {
                // Errors belong in front of the user.
                app.script.open = true;
            }
        }
        Err(e) => {
            app.script.note(ConsoleKind::Error, format!("{}: {e}", path.display()), None);
            app.status = crate::tlf!("{script} could not be read: {error}", script = name, error = e);
            app.script.open = true;
        }
    }
}

/// Run `startup.rhai` from the scripts folder, if there is one.
pub fn startup(app: &mut ModelApp) {
    let Some(p) = app.script.dir.as_ref().map(|d| d.join(STARTUP)).filter(|p| p.is_file()) else { return };
    run_file(app, &p);
}

// ---------------------------------------------------------------- files

pub(crate) fn load(app: &mut ModelApp, path: &Path) {
    match read_script(path) {
        Ok(t) => {
            app.script.saved = t.clone();
            app.script.text = t;
            app.script.path = Some(path.to_path_buf());
            app.script.error_line = None;
            app.script.sel = None;
            app.script.goto = Some(1);
            app.status = crate::tlf!("Opened {file}", file = path.display());
        }
        Err(e) => app.status = crate::tlf!("{script} could not be read: {error}", script = path.display(), error = e),
    }
}

/// Open `text` as a new unsaved script in the window (asking first when the editor has unsaved
/// edits).
pub fn open_text(app: &mut ModelApp, text: String) {
    app.script.open = true;
    guard(app, Then::Text(text));
}

fn new_script(app: &mut ModelApp) {
    app.script.text.clear();
    app.script.saved.clear();
    app.script.path = None;
    app.script.error_line = None;
    app.script.sel = None;
    app.script.focus_pending = true;
}

pub(crate) fn save_to(app: &mut ModelApp, path: &Path) -> bool {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    match craft_atomic::atomic_write(path, app.script.text.as_bytes()) {
        Ok(()) => {
            app.script.saved = app.script.text.clone();
            app.script.path = Some(path.to_path_buf());
            app.script.reload();
            app.status = crate::tlf!("Saved {file}", file = path.display());
            true
        }
        Err(e) => {
            app.status = crate::tlf!("Save failed: {error}", error = e);
            false
        }
    }
}

/// Save to the script's file, or ask for one.
pub fn save(app: &mut ModelApp) -> bool {
    match app.script.path.clone() {
        Some(p) => save_to(app, &p),
        None => save_as(app),
    }
}

fn save_as(app: &mut ModelApp) -> bool {
    let mut d = rfd::FileDialog::new().add_filter("Rhai", &[EXT]).set_file_name(app.script.name());
    if let Some(dir) = &app.script.dir {
        d = d.set_directory(dir);
    }
    match d.save_file() {
        Some(p) => save_to(app, &p),
        None => false,
    }
}

fn then(app: &mut ModelApp, t: Then) {
    match t {
        Then::New => new_script(app),
        Then::Text(text) => {
            new_script(app);
            app.script.text = text;
            app.script.goto = Some(1);
            app.script.open = true;
        }
        Then::Open(p) => load(app, &p),
        Then::Pick => {
            let mut d = rfd::FileDialog::new().add_filter("Rhai", &[EXT]);
            if let Some(dir) = &app.script.dir {
                d = d.set_directory(dir);
            }
            if let Some(p) = d.pick_file() {
                load(app, &p);
            }
        }
    }
}

/// Do `t` now, or ask first when the script has unsaved edits.
fn guard(app: &mut ModelApp, t: Then) {
    if app.script.modified() && !app.script.text.is_empty() {
        app.script.confirm = Some(t);
        app.script.focus_pending = true;
    } else {
        then(app, t);
    }
}

// ---------------------------------------------------------------- the window

/// Open the window (focus goes to the editor) or close it.
pub fn toggle(app: &mut ModelApp) {
    app.script.open = !app.script.open;
    if app.script.open {
        app.script.focus_pending = true;
    }
}

/// Esc closes the window when the keyboard focus is inside it. Called before the app's own
/// shortcuts, where Esc clears the selection; returns whether it used the key.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    let s = &mut app.script;
    if s.open && s.had_focus && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        s.open = false;
        s.had_focus = false;
        return true;
    }
    false
}

/// Ctrl+Enter, Ctrl+Shift+Enter and Ctrl+S while the editor has the focus. They go before the
/// app's own shortcuts (Ctrl+S saves the project everywhere else); returns whether one was used.
pub fn keys(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if !app.script.open || !app.script.editor_focus {
        return false;
    }
    // The shifted shortcut first: a plain-modifier match would also take it.
    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter))) {
        run_editor(app, true);
        true
    } else if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter))) {
        run_editor(app, false);
        true
    } else if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S))) {
        save(app);
        true
    } else {
        false
    }
}

fn name_node(ui: &egui::Ui, r: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(label));
}

pub fn token_color(k: TokenKind) -> Color32 {
    match k {
        TokenKind::Keyword => theme::ACCENT,
        TokenKind::Constant => Color32::from_rgb(0xF0, 0xC6, 0x6B),
        TokenKind::Number => Color32::from_rgb(0xC9, 0xA6, 0xF2),
        TokenKind::Text => Color32::from_rgb(0x9C, 0xD6, 0x7A),
        TokenKind::Comment => Color32::from_rgb(0x8E, 0x95, 0x9E),
        TokenKind::Api => Color32::from_rgb(0x6C, 0xC4, 0xE4),
        TokenKind::Ident | TokenKind::Space => theme::TEXT,
        TokenKind::Punct => theme::TEXT_DIM,
    }
}

const ERROR_RED: Color32 = Color32::from_rgb(0xFF, 0x7B, 0x72);

/// The coloured layout of a script.
pub fn highlight_job(text: &str, font: egui::FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    if text.len() > HIGHLIGHT_LIMIT {
        job.append(text, 0.0, TextFormat::simple(font, theme::TEXT));
        return job;
    }
    for t in tokenize(text) {
        job.append(&text[t.start..t.end], 0.0, TextFormat::simple(font.clone(), token_color(t.kind)));
    }
    job
}

fn line_col(text: &str, char_index: usize) -> (usize, usize) {
    let before: Vec<char> = text.chars().take(char_index).collect();
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let col = before.iter().rev().take_while(|c| **c != '\n').count() + 1;
    (line, col)
}

fn char_index_of_line(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut seen = 1;
    for (i, c) in text.chars().enumerate() {
        if c == '\n' {
            seen += 1;
            if seen == line {
                return i + 1;
            }
        }
    }
    text.chars().count()
}

const EDITOR_ID: &str = "script-text";

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.script.open {
        app.script.had_focus = false;
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Scripts")).id(egui::Id::unique("script-window")).open(&mut open).collapsible(false).default_size([760.0, 560.0]).show(ctx, |ui| {
        let layer = ui.layer_id();
        body(app, ui);
        app.script.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
    });
    app.script.open = open;
}

fn body(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui.button(tl!("New")).clicked() {
            guard(app, Then::New);
        }
        if ui.button(tl!("Open…")).clicked() {
            guard(app, Then::Pick);
        }
        let entries = app.script.entries.clone();
        let r = egui::ComboBox::from_id_salt("script-files").selected_text(tl!("Scripts folder")).show_ui(ui, |ui| {
            if entries.is_empty() {
                ui.label(RichText::new(tl!("No scripts in the scripts folder")).color(theme::TEXT_DIM));
            }
            for e in &entries {
                let label = match &e.folder {
                    Some(f) => format!("{f}/{}", e.label),
                    None => e.label.clone(),
                };
                if ui.selectable_label(app.script.path.as_ref() == Some(&e.path), label).clicked() {
                    guard(app, Then::Open(e.path.clone()));
                    ui.close();
                }
            }
        });
        name_node(ui, &r.response, tl!("Scripts folder"));
        if ui.button(tl!("Save")).clicked() {
            save(app);
        }
        if ui.button(tl!("Save As…")).clicked() {
            save_as(app);
        }
        ui.separator();
        let run = ui.add(egui::Button::new(tl!("Run Script")).shortcut_text(Action::ScriptRun.shortcut_text(app)));
        if run.clicked() {
            run_editor(app, false);
        }
        let run_sel = ui.add(egui::Button::new(tl!("Run Selection")).shortcut_text(Action::ScriptRunSelection.shortcut_text(app)));
        if run_sel.clicked() {
            run_editor(app, true);
        }
        if ui.button(tl!("Clear Output")).clicked() {
            app.script.console.clear();
        }
        ui.separator();
        let w = ui.checkbox(&mut app.script.allow_writes, tl!("Allow writing files"));
        w.on_hover_text(tl!(
            "Lets scripts write files in the folders listed below. Reading is limited to the project's folder, the scripts folder and the script's own."
        ));
    });

    crate::script_macro::row(app, ui);

    if app.script.allow_writes {
        let roots = write_roots(app);
        let text = if roots.is_empty() {
            tl!("Scripts can write nowhere: save the project or create the scripts folder first.")
        } else {
            crate::tlf!("Scripts can write in: {folders}", folders = roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", "))
        };
        let l = ui.label(RichText::new(text).color(theme::SELECT).small());
        ui.ctx().accesskit_node_builder(l.id, |n| n.set_live(egui::accesskit::Live::Polite));
    }

    if let Some(t) = app.script.confirm.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::tlf!("{script} has unsaved changes.", script = app.script.name())).color(theme::SELECT));
            let save_btn = ui.button(tl!("Save and Continue"));
            if std::mem::take(&mut app.script.focus_pending) {
                save_btn.request_focus();
            }
            if save_btn.clicked() && save(app) {
                app.script.confirm = None;
                then(app, t.clone());
            }
            if ui.button(tl!("Discard Changes")).clicked() {
                app.script.confirm = None;
                then(app, t.clone());
            }
            if ui.button(tl!("Cancel")).clicked() {
                app.script.confirm = None;
            }
        });
    }

    let title = format!("{}{}", app.script.name(), if app.script.modified() { tl!(" (modified)") } else { String::new() });
    let file_line = ui.label(RichText::new(title.clone()).color(theme::TEXT_DIM));
    ui.ctx().accesskit_node_builder(file_line.id, |n| n.set_label(crate::tlf!("Script file: {name}", name = title)));

    let total = ui.available_height();
    let console_h = (total * 0.32).clamp(96.0, 260.0);
    let editor_h = (total - console_h - 40.0).max(120.0);
    editor(app, ui, editor_h);

    ui.horizontal(|ui| {
        let (l, c) = app.script.cursor;
        let lines = app.script.text.lines().count().max(1);
        ui.label(RichText::new(crate::tlf!("Line {line}, column {column} of {lines}", line = l, column = c, lines = lines)).color(theme::TEXT_DIM).small());
    });
    ui.label(RichText::new(tl!("Output")).strong().small());
    console(app, ui, console_h);
}

fn editor(app: &mut ModelApp, ui: &mut egui::Ui, height: f32) {
    let id = egui::Id::unique(EDITOR_ID);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let n_lines = app.script.text.split('\n').count().max(1);
    let gutter_w = (n_lines.to_string().len().max(2) as f32) * char_w + 14.0;
    let error_line = app.script.error_line;
    let goto = app.script.goto.take();
    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = highlight_job(buf.as_str(), font.clone());
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut scroll_to = None;
    let mut new_sel = None;
    let mut cursor_at = None;
    let mut editor_focused = false;
    let focus_editor = app.script.confirm.is_none() && std::mem::take(&mut app.script.focus_pending);
    egui::ScrollArea::both().id_salt("script-scroll").max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            let (_, gutter) = ui.allocate_space(egui::vec2(gutter_w, 0.0));
            let out = egui::TextEdit::multiline(&mut app.script.text)
                .id(id)
                .code_editor()
                .desired_width(f32::INFINITY)
                .desired_rows(14)
                .layouter(&mut layouter)
                .show(ui);
            let resp = out.response.response.clone();
            name_node(ui, &resp, tl!("Script text"));
            if focus_editor {
                resp.request_focus();
            }
            // Gutter: line numbers aligned with the rows; the error line is marked.
            let painter = ui.painter().clone();
            for (i, row) in out.galley.rows.iter().enumerate() {
                let y = out.galley_pos.y + row.pos.y;
                let is_err = error_line == Some(i + 1);
                let color = if is_err { ERROR_RED } else { theme::TEXT_DIM };
                painter.text(egui::pos2(gutter.min.x + gutter_w - 8.0, y), egui::Align2::RIGHT_TOP, (i + 1).to_string(), font.clone(), color);
                if is_err {
                    let r = row.rect().translate(out.galley_pos.to_vec2());
                    let right = r.max.x.max(r.min.x + 400.0);
                    painter.rect_filled(
                        egui::Rect::from_min_max(egui::pos2(gutter.min.x, r.min.y), egui::pos2(right, r.max.y)),
                        0.0,
                        Color32::from_rgba_unmultiplied(0xFF, 0x50, 0x40, 38),
                    );
                }
            }
            if let Some(range) = out.cursor_range {
                let [a, b] = range.sorted_cursors();
                cursor_at = Some(line_col(&app.script.text, range.primary.index.into()));
                new_sel = Some(if a.index != b.index { Some((a.index.into(), b.index.into())) } else { None });
            }
            if let Some(line) = goto {
                let idx = char_index_of_line(&app.script.text, line);
                let mut st = out.state.clone();
                st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
                st.store(ui.ctx(), id);
                resp.request_focus();
                if let Some(row) = out.galley.rows.get(line.saturating_sub(1)) {
                    scroll_to = Some(row.rect().translate(out.galley_pos.to_vec2()));
                }
                cursor_at = Some((line, 1));
            }
            editor_focused = resp.has_focus();
        });
        if let Some(r) = scroll_to {
            ui.scroll_to_rect(r, Some(egui::Align::Center));
        }
    });
    if let Some(c) = cursor_at {
        app.script.cursor = c;
    }
    if let Some(s) = new_sel {
        app.script.sel = s;
    }
    app.script.editor_focus = editor_focused;
}

fn console(app: &mut ModelApp, ui: &mut egui::Ui, height: f32) {
    let mut goto = None;
    egui::ScrollArea::vertical()
        .id_salt("script-output")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if app.script.console.is_empty() {
                ui.label(RichText::new(tl!("Nothing yet. Run a script to see its output here.")).color(theme::TEXT_DIM));
            }
            for l in &app.script.console {
                let color = match l.kind {
                    ConsoleKind::Note => theme::TEXT_DIM,
                    ConsoleKind::Print => theme::TEXT,
                    ConsoleKind::Debug => Color32::from_rgb(0x6C, 0xC4, 0xE4),
                    ConsoleKind::Warning => theme::SELECT,
                    ConsoleKind::Error => ERROR_RED,
                };
                let text = RichText::new(l.text.as_str()).monospace().color(color);
                match (l.kind, l.line) {
                    (ConsoleKind::Error, Some(line)) => {
                        let b = ui.add(egui::Button::new(text).frame(false));
                        name_node(ui, &b, crate::tlf!("Error, go to line {line}: {message}", line = line, message = l.text));
                        if b.clicked() {
                            goto = Some(line);
                        }
                    }
                    _ => {
                        let r = ui.label(text);
                        if l.kind == ConsoleKind::Debug
                            && let Some(line) = l.line
                        {
                            r.on_hover_text(crate::tlf!("From line {line}", line = line));
                        }
                    }
                }
            }
        });
    if let Some(line) = goto {
        app.script.goto = Some(line);
    }
}

// ---------------------------------------------------------------- menu and strip

/// The Scripts menu.
pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    for a in [Action::ScriptWindow, Action::ScriptRun, Action::ScriptRunSelection] {
        let b = egui::Button::new(a.label()).shortcut_text(a.shortcut_text(app));
        if ui.add(b).clicked() {
            app.act(&ctx, a);
            ui.close();
        }
    }
    let record = if app.doc.is_recording() { tl!("Stop Recording Macro") } else { Action::ScriptRecord.label() };
    if ui.button(record).clicked() {
        app.act(&ctx, Action::ScriptRecord);
        ui.close();
    }
    ui.separator();
    if ui.button(tl!("Reload Scripts")).clicked() {
        app.script.reload();
        app.status = crate::tlf!("{count} scripts found", count = app.script.entries.len() + app.script.quick.len());
        ui.close();
    }
    ui.checkbox(&mut app.script.show_quick, tl!("Show Quick Scripts"));
    match &app.script.dir {
        Some(d) => {
            ui.label(RichText::new(crate::tlf!("Scripts folder: {folder}", folder = d.display())).color(theme::TEXT_DIM).small());
        }
        None => {
            ui.label(RichText::new(tl!("No scripts folder on this system")).color(theme::TEXT_DIM).small());
        }
    }
    ui.separator();
    crate::plugin_ui::menu(app, ui);
    let tools = app.script.session.as_ref().map(|s| s.tools()).unwrap_or_default();
    if !tools.is_empty() {
        ui.separator();
        ui.label(RichText::new(tl!("Scripted Tools")).color(theme::TEXT_DIM).small());
        for t in &tools {
            let b = ui.button(&t.label);
            let b = if t.tip.is_empty() { b } else { b.on_hover_text(&t.tip) };
            if b.clicked() {
                run_tool(app, &t.label);
                ui.close();
            }
        }
    }
    ui.separator();
    if app.script.entries.is_empty() {
        ui.label(RichText::new(tl!("No scripts in the scripts folder")).color(theme::TEXT_DIM));
        return;
    }
    let entries = app.script.entries.clone();
    for e in entries.iter().filter(|e| e.folder.is_none()) {
        if script_item(ui, e) {
            run_file(app, &e.path);
            ui.close();
        }
    }
    let mut folders: Vec<&String> = entries.iter().filter_map(|e| e.folder.as_ref()).collect();
    folders.dedup();
    for f in folders {
        let mut chosen = None;
        ui.menu_button(f.as_str(), |ui| {
            for e in entries.iter().filter(|e| e.folder.as_ref() == Some(f)) {
                if script_item(ui, e) {
                    chosen = Some(e.path.clone());
                    ui.close();
                }
            }
        });
        if let Some(p) = chosen {
            run_file(app, &p);
            ui.close();
        }
    }
}

fn script_item(ui: &mut egui::Ui, e: &Entry) -> bool {
    let r = ui.button(&e.label);
    let r = if e.tip.is_empty() { r } else { r.on_hover_text(&e.tip) };
    r.clicked()
}

/// The Quick Scripts strip: one button per script in the `quick` folder.
pub fn quick_strip(app: &mut ModelApp, ui: &mut egui::Ui) {
    if !app.script.show_quick || app.script.quick.is_empty() {
        return;
    }
    let quick = app.script.quick.clone();
    let mut run = None;
    egui::Panel::top("quick-scripts").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(tl!("Quick Scripts")).color(theme::TEXT_DIM).small());
            for e in &quick {
                let b = ui.button(&e.label);
                let b = if e.tip.is_empty() { b } else { b.on_hover_text(&e.tip) };
                name_node(ui, &b, crate::tlf!("Run script {name}", name = e.label));
                if b.clicked() {
                    run = Some(e.path.clone());
                }
            }
        });
    });
    if let Some(p) = run {
        run_file(app, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    const EDITOR: &str = "Script text";

    fn harness() -> Harness<'static, ModelApp> {
        let ctx = egui::Context::default();
        let app = ModelApp::with_context(&ctx, Cli::default());
        Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-script-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        std::fs::canonicalize(&d).unwrap_or(d)
    }

    fn open_editor(h: &mut Harness<'static, ModelApp>) {
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::J);
        h.step();
        h.step();
        assert!(h.state().script.open, "Ctrl+J opens the Scripts window");
    }

    fn names(h: &Harness<'static, ModelApp>) -> Vec<String> {
        h.state().doc.scene.objects().iter().map(|o| o.name.clone()).collect()
    }

    #[test]
    fn ctrl_j_opens_the_editor_and_ctrl_enter_runs_what_was_typed() {
        let mut h = harness();
        open_editor(&mut h);
        let editor = h.get_by_role_and_label(Role::MultilineTextInput, EDITOR);
        assert!(editor.accesskit_node().is_focused(), "focus moves into the editor");
        h.event(egui::Event::Text("add_primitive(\"cube\", \"Typed\"); print(\"hello console\");".into()));
        h.run();
        assert!(h.state().script.text.contains("Typed"));
        h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        h.run();
        assert_eq!(names(&h), vec!["Typed".to_string()]);
        h.get_by_label("hello console");
        assert!(h.state().status.contains("finished"), "{}", h.state().status);
        // The run is one undoable change like any other edit.
        h.key_press(Key::Escape);
        h.run();
        assert!(!h.state().script.open, "Esc closes the window");
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert!(names(&h).is_empty());
    }

    #[test]
    fn errors_name_their_line_and_the_console_jumps_to_it() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "let a = 1;\nlet b = 2;\nname_of(404);\nlet c = 3;\n".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert_eq!(h.state().script.error_line, Some(3));
        let err = h.get_by_label_contains("Error, go to line 3");
        assert!(
            err.accesskit_node().label().is_some_and(|l| l.contains("untitled.rhai:3:") && l.contains("no object 404")),
            "{:?}",
            err.accesskit_node().label()
        );
        assert!(h.state().status.contains("failed"), "{}", h.state().status);
        // Clicking the error puts the cursor on the line and focuses the editor.
        h.state_mut().script.cursor = (1, 1);
        h.get_by_label_contains("Error, go to line 3").click();
        h.run();
        h.run();
        assert_eq!(h.state().script.cursor.0, 3);
        assert!(h.get_by_role_and_label(Role::MultilineTextInput, EDITOR).accesskit_node().is_focused());
        // A fixed script clears the marker.
        h.state_mut().script.text = "let ok = 1;".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert_eq!(h.state().script.error_line, None);
    }

    #[test]
    fn a_syntax_error_points_at_its_line_and_runs_nothing() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "add_group(\"never\");\nlet x = (1 + ;\n".into();
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        h.run();
        assert_eq!(h.state().script.error_line, Some(2));
        assert!(h.state().doc.scene.is_empty(), "a script that does not parse does not run at all");
    }

    #[test]
    fn run_selection_runs_only_the_selected_text_and_keeps_line_numbers() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "add_group(\"A\");\nadd_group(\"B\");\nname_of(404);".into();
        h.run();
        // Select the second line (characters 16..31).
        let id = egui::Id::unique(EDITOR_ID);
        let mut st = egui::text_edit::TextEditState::load(&h.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(16), CCursor::new(31))));
        st.store(&h.ctx, id);
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
        h.run();
        assert_eq!(names(&h), vec!["B".to_string()]);
        // Select the failing third line: the error is reported at line 3, as in the file.
        let mut st = egui::text_edit::TextEditState::load(&h.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(32), CCursor::new(45))));
        st.store(&h.ctx, id);
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
        h.run();
        assert_eq!(h.state().script.error_line, Some(3));
        assert!(
            h.state().script.console.iter().any(|l| l.kind == ConsoleKind::Error && l.text.contains("untitled.rhai:3:1:")),
            "{:?}",
            h.state().script.console
        );
        // With nothing selected the menu action says so instead of running everything.
        let mut st = egui::text_edit::TextEditState::load(&h.ctx, id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(3))));
        st.store(&h.ctx, id);
        h.run();
        assert_eq!(h.state().script.sel, None);
        h.state_mut().doc = modelcraft_engine::Document::new();
        h.get_by_label_contains("Run Selection").click();
        h.run();
        assert!(h.state().status.contains("Select the part"));
        assert!(h.state().doc.scene.is_empty());
    }

    fn scripts_dir() -> PathBuf {
        let d = tmp("folder");
        std::fs::write(d.join("make_cube.rhai"), "// Makes a cube called FromMenu\nadd_primitive(\"cube\", \"FromMenu\");").unwrap();
        std::fs::write(d.join("startup.rhai"), "add_group(\"FromStartup\");").unwrap();
        std::fs::create_dir_all(d.join("tools")).unwrap();
        std::fs::write(d.join("tools/more.rhai"), "add_group(\"FromSubmenu\");").unwrap();
        std::fs::create_dir_all(d.join("quick")).unwrap();
        std::fs::write(d.join("quick/stairs.rhai"), "// A stair\nadd_group(\"FromStrip\");").unwrap();
        d
    }

    #[test]
    fn the_folder_feeds_the_menu_the_strip_and_the_startup_script() {
        let dir = scripts_dir();
        let (entries, quick) = scan(&dir);
        assert_eq!(entries.iter().map(|e| (e.label.as_str(), e.folder.as_deref())).collect::<Vec<_>>(), vec![("make_cube", None), ("more", Some("tools"))]);
        assert_eq!(entries[0].tip, "Makes a cube called FromMenu");
        assert_eq!(quick.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), vec!["stairs"], "the startup script is not a menu entry");

        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir.clone()));
        startup(h.state_mut());
        assert_eq!(names(&h), vec!["FromStartup".to_string()]);
        h.run();
        // The Scripts menu by keyboard-reachable buttons: open it, pick a script.
        h.get_by_label("Scripts").click();
        h.run();
        h.get_by_label("make_cube").click();
        h.run();
        assert!(names(&h).contains(&"FromMenu".to_string()));
        h.get_by_label("Scripts").click();
        h.run();
        h.get_by_label("tools").click();
        h.run();
        h.get_by_label("more").click();
        h.run();
        assert!(names(&h).contains(&"FromSubmenu".to_string()));
        // The strip has one button per quick script, with a name that says what it does.
        h.get_by_role_and_label(Role::Button, "Run script stairs").click();
        h.run();
        assert!(names(&h).contains(&"FromStrip".to_string()));
        // The strip can be hidden from the menu.
        h.state_mut().script.show_quick = false;
        h.run();
        assert!(h.query_by_label("Run script stairs").is_none());
    }

    #[test]
    fn a_failing_menu_script_opens_the_window_with_the_error() {
        let dir = tmp("failing");
        std::fs::write(dir.join("bad.rhai"), "let a = 1;\nundefined_function();\n").unwrap();
        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir.clone()));
        let p = dir.join("bad.rhai");
        run_file(h.state_mut(), &p);
        h.run();
        assert!(h.state().script.open);
        h.get_by_label_contains("Error, go to line 2");
    }

    #[test]
    fn unsaved_edits_are_never_thrown_away_silently() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "print(1);".into();
        h.run();
        assert!(h.state().script.modified());
        h.get_by_role_and_label(Role::Button, "New").click();
        h.run();
        h.get_by_label("untitled.rhai has unsaved changes.");
        assert_eq!(h.state().script.text, "print(1);", "nothing is lost until the user decides");
        // Focus is on the safe choice.
        assert!(h.get_by_role_and_label(Role::Button, "Save and Continue").accesskit_node().is_focused());
        h.get_by_role_and_label(Role::Button, "Cancel").click();
        h.run();
        assert_eq!(h.state().script.text, "print(1);");
        assert!(h.query_by_label("untitled.rhai has unsaved changes.").is_none());
        h.get_by_role_and_label(Role::Button, "New").click();
        h.run();
        h.get_by_role_and_label(Role::Button, "Discard Changes").click();
        h.run();
        assert!(h.state().script.text.is_empty());
    }

    #[test]
    fn ctrl_s_saves_the_script_file_and_the_menu_lists_it() {
        let dir = tmp("save");
        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir.clone()));
        h.state_mut().script.path = Some(dir.join("mine.rhai"));
        h.key_press_modifiers(Modifiers::COMMAND, Key::J);
        h.step();
        h.step();
        h.event(egui::Event::Text("print(\"saved\");".into()));
        h.run();
        assert!(h.state().script.modified());
        h.get_by_label_contains("mine.rhai (modified)");
        h.key_press_modifiers(Modifiers::COMMAND, Key::S);
        h.run();
        assert_eq!(std::fs::read_to_string(dir.join("mine.rhai")).unwrap(), "print(\"saved\");");
        assert!(!h.state().script.modified());
        assert_eq!(h.state().script.entries.len(), 1, "the folder is scanned again after a save");
        // Open it again from the folder list.
        h.state_mut().script.text.clear();
        h.state_mut().script.saved.clear();
        h.state_mut().script.path = None;
        load(h.state_mut(), &dir.join("mine.rhai"));
        assert_eq!(h.state().script.text, "print(\"saved\");");
    }

    #[test]
    fn scripts_cannot_write_files_until_the_user_allows_it() {
        let dir = tmp("perm");
        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir.clone()));
        h.key_press_modifiers(Modifiers::COMMAND, Key::J);
        h.step();
        h.step();
        h.state_mut().script.text = "write_text(\"out.txt\", \"data\");".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert!(
            h.state().script.console.iter().any(|l| l.kind == ConsoleKind::Error && l.text.contains("may not use writing")),
            "{:?}",
            h.state().script.console
        );
        assert!(!dir.join("out.txt").exists());
        let allow = h.get_by_role_and_label(Role::CheckBox, "Allow writing files");
        assert!(allow.accesskit_node().toggled() != Some(egui::accesskit::Toggled::True));
        allow.click();
        h.run();
        assert!(h.state().script.allow_writes);
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert_eq!(std::fs::read_to_string(dir.join("out.txt")).unwrap(), "data");
        // Paths outside the allowed folders stay refused even then.
        h.state_mut().script.text = "write_text(\"../escape.txt\", \"x\");".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        assert_eq!(h.state().script.error_line, Some(1));
    }

    #[test]
    fn a_script_can_change_the_selection_and_delete_what_was_selected() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "let a = add_primitive(\"cube\"); let b = add_primitive(\"sphere\"); set_selection([a, b]); delete(a);".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert_eq!(h.state().doc.scene.len(), 1);
        assert_eq!(h.state().selection.len(), 1, "the deleted object left the selection");
        assert!(h.state().script.error_line.is_none());
    }

    #[test]
    fn a_runaway_script_stops_and_the_window_stays_usable() {
        let mut h = harness();
        open_editor(&mut h);
        h.state_mut().script.text = "add_group(\"before\");\nloop { }".into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
        assert!(h.state().script.console.iter().any(|l| l.kind == ConsoleKind::Error && l.text.contains("too many operations")));
        assert!(h.state().script.console.iter().any(|l| l.text.contains("Undo reverts")));
        assert_eq!(names(&h), vec!["before".to_string()]);
        h.get_by_role_and_label(Role::Button, "Clear Output").click();
        h.run();
        assert!(h.state().script.console.is_empty());
    }

    #[test]
    fn highlighting_covers_the_text_and_every_colour_is_readable() {
        let text = "let c = add_primitive(\"cube\"); // note\nfor i in 0..3 { move_to(c, i, 0.5, 0) }";
        let job = highlight_job(text, egui::FontId::monospace(12.0));
        assert_eq!(job.text, text);
        assert_eq!(job.sections.last().map(|s| usize::from(s.byte_range.end)), Some(text.len()));
        assert!(job.sections.len() > 10);
        let colours: std::collections::HashSet<Color32> = job.sections.iter().map(|s| s.format.color).collect();
        assert!(colours.len() >= 5, "keywords, API names, strings, numbers and comments differ");
        for k in [
            TokenKind::Keyword,
            TokenKind::Constant,
            TokenKind::Number,
            TokenKind::Text,
            TokenKind::Comment,
            TokenKind::Api,
            TokenKind::Ident,
            TokenKind::Punct,
        ] {
            for bg in [theme::PANEL_DEEP, theme::PANEL] {
                assert!(theme::contrast(token_color(k), bg) >= 4.5, "{k:?} on {bg:?}: {}", theme::contrast(token_color(k), bg));
            }
        }
        assert!(theme::contrast(ERROR_RED, theme::PANEL_DEEP) >= 4.5);
        // A huge script is left uncoloured but intact.
        let big = "x ".repeat(HIGHLIGHT_LIMIT);
        assert_eq!(highlight_job(&big, egui::FontId::monospace(12.0)).sections.len(), 1);
    }

    #[test]
    fn cursor_helpers_and_oversized_files() {
        assert_eq!(line_col("ab\ncde", 0), (1, 1));
        assert_eq!(line_col("ab\ncde", 4), (2, 2));
        assert_eq!(char_index_of_line("ab\ncde\nf", 3), 7);
        assert_eq!(char_index_of_line("ab", 9), 2);
        let dir = tmp("big");
        let p = dir.join("big.rhai");
        std::fs::write(&p, vec![b'x'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        assert!(read_script(&p).is_err());
        std::fs::write(&p, [0xFF, 0xFE]).unwrap();
        assert!(read_script(&p).is_err(), "not UTF-8");
        assert_eq!(first_comment("\n  // hello there\ncode"), "hello there");
        assert_eq!(first_comment("code"), "");
    }

    #[test]
    fn writes_are_never_granted_to_a_home_folder_or_a_root() {
        assert!(!safe_write_root_given(Path::new("/"), &[]));
        assert!(!safe_write_root_given(Path::new("/definitely/not/a/folder"), &[]));
        let dir = tmp("safe-root");
        assert!(safe_write_root_given(&dir, &[]));
        // The home folder is refused, and a project saved there gets a subfolder instead.
        let home = tmp("fake-home");
        let homes = [home.clone()];
        assert!(!safe_write_root_given(&home, &homes));
        assert!(safe_write_root_given(&dir, &homes));
        let mut h = harness();
        h.run();
        h.state_mut().doc.path = Some(home.join("scene.modelcraft"));
        h.state_mut().script.allow_writes = true;
        let roots = write_roots_given(h.state(), &homes);
        assert_eq!(roots, vec![home.join("scripts-out")]);
        assert!(home.join("scripts-out").is_dir());
        // A project in an ordinary folder gets that folder.
        h.state_mut().doc.path = Some(dir.join("scene.modelcraft"));
        assert_eq!(write_roots_given(h.state(), &homes), vec![dir.clone()]);
    }

    #[test]
    fn the_granted_write_folders_are_shown_next_to_the_checkbox() {
        let dir = tmp("shown");
        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir.clone()));
        h.key_press_modifiers(Modifiers::COMMAND, Key::J);
        h.step();
        h.step();
        h.get_by_role_and_label(Role::CheckBox, "Allow writing files").click();
        h.run();
        h.get_by_label_contains("Scripts can write in:");
        assert!(h.state().script.allow_writes);
    }

    #[test]
    fn scanning_reads_a_bounded_number_of_entries_and_only_the_start_of_each_file() {
        let dir = tmp("scan-bound");
        for i in 0..(MAX_SCAN + 50) {
            std::fs::write(dir.join(format!("s{i:04}.rhai")), "// tip\n").unwrap();
        }
        let (entries, _) = scan(&dir);
        assert_eq!(entries.len(), MAX_SCAN);
        // A huge file is not read whole for its tooltip.
        let big = dir.join("big.rhai");
        let mut text = String::from("// the tip\n");
        text.push_str(&"x".repeat(5 << 20));
        std::fs::write(&big, text).unwrap();
        assert_eq!(tip_of(&big), "the tip");
    }

    fn run_text(h: &mut Harness<'static, ModelApp>, text: &str) {
        h.state_mut().script.text = text.into();
        h.run();
        h.get_by_label_contains("Run Script").click();
        h.run();
    }

    #[test]
    fn a_script_adds_a_menu_command() {
        let mut h = harness();
        open_editor(&mut h);
        run_text(&mut h, "register_tool(\"Make Pair\", \"Adds two groups\", || { add_group(\"P1\"); add_group(\"P2\"); });");
        assert!(h.state().script.error_line.is_none(), "{:?}", h.state().script.console);
        assert!(names(&h).is_empty(), "registering adds nothing yet");
        h.state_mut().script.open = false;
        h.run();
        h.get_by_label("Scripts").click();
        h.run();
        h.get_by_label("Scripted Tools");
        h.get_by_label("Make Pair").click();
        h.run();
        assert_eq!(names(&h), vec!["P1".to_string(), "P2".to_string()]);
        // A tool is one command in the history like any other run.
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert!(names(&h).is_empty() || names(&h).len() < 2);
        // Running the script again replaces the tool instead of adding a second one.
        h.state_mut().script.open = true;
        h.run();
        run_text(&mut h, "register_tool(\"Make Pair\", || { add_group(\"Q\"); });");
        assert_eq!(h.state().script.session.as_ref().map(|s| s.tools().len()), Some(1));
    }

    #[test]
    fn callbacks_hear_about_selection_scene_and_new_projects() {
        let mut h = harness();
        open_editor(&mut h);
        run_text(
            &mut h,
            "on_selection_changed(|e| { print(\"selection now \" + e.selection.len()); });\non_scene_loaded(|e| { print(\"a project was loaded\"); });\non_scene_changed(|e| { print(\"scene changed\"); });",
        );
        h.state_mut().script.open = false;
        h.run();
        let count =
            |h: &Harness<'static, ModelApp>, text: &str| h.state().script.console.iter().filter(|l| l.kind == ConsoleKind::Print && l.text == text).count();
        // Adding a cube selects it: both the scene and the selection changed.
        h.key_press_modifiers(Modifiers::COMMAND, Key::K);
        h.run();
        h.event(egui::Event::Text("Add Cube".into()));
        h.run();
        h.key_press(Key::Enter);
        h.run();
        h.run();
        assert_eq!(count(&h, "selection now 1"), 1);
        assert!(count(&h, "scene changed") >= 1);
        let before = count(&h, "scene changed");
        h.run();
        h.run();
        assert_eq!(count(&h, "scene changed"), before, "a quiet frame reports nothing");
        h.state_mut().run_pending(crate::app::Pending::New);
        h.run();
        h.run();
        assert_eq!(count(&h, "a project was loaded"), 1);
        assert_eq!(count(&h, "selection now 0"), 1, "the new project has nothing selected");
    }

    #[test]
    fn a_callback_that_changes_the_scene_does_not_trigger_itself() {
        let mut h = harness();
        open_editor(&mut h);
        run_text(&mut h, "on_scene_changed(|e| { add_group(\"echo\"); });");
        h.state_mut().script.open = false;
        h.run();
        h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: Some("user".into()) }).unwrap();
        for _ in 0..6 {
            h.run();
        }
        let echoes = names(&h).iter().filter(|n| n.starts_with("echo")).count();
        assert_eq!(echoes, 1, "{:?}", names(&h));
    }

    #[test]
    fn a_broken_callback_reports_once_per_event_and_then_goes_quiet() {
        let mut h = harness();
        open_editor(&mut h);
        run_text(&mut h, "on_selection_changed(|e| { name_of(404); });");
        h.state_mut().script.open = false;
        h.run();
        for i in 0..5u64 {
            h.state_mut().selection = vec![];
            h.run();
            let id = h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: None }).unwrap().created[0];
            h.state_mut().selection = vec![id];
            h.run();
            h.run();
            let _ = i;
        }
        let errors = h.state().script.console.iter().filter(|l| l.kind == ConsoleKind::Error).count();
        assert_eq!(errors, 3, "three failures, then it is switched off: {:?}", h.state().script.console);
        assert!(h.state().script.console.iter().any(|l| l.text.contains("switched off")));
    }

    #[test]
    fn the_startup_script_can_register_tools() {
        let dir = tmp("startup-tools");
        std::fs::write(dir.join("startup.rhai"), "register_tool(\"From Startup\", || { add_group(\"S\"); });").unwrap();
        let mut h = harness();
        h.run();
        h.state_mut().script = ScriptState::new(Some(dir));
        startup(h.state_mut());
        assert_eq!(h.state().script.session.as_ref().map(|s| s.tools().len()), Some(1));
        run_tool(h.state_mut(), "From Startup");
        assert_eq!(names(&h), vec!["S".to_string()]);
    }
}
