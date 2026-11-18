//! Plugins in the window: the Plugins window (install, permission prompt, enable, remove), the
//! plugins' menu tools and their event callbacks. The packages, manifests, grants and sandbox are
//! `modelcraft_script::plugin`; see `docs/plugins.md`.

use std::path::PathBuf;

use egui::RichText;
use modelcraft_script::RunContext;
use modelcraft_script::plugin::{self, Grants, Installation, Installed, LoadedPlugin, Permission, Status};

use crate::app::ModelApp;
use crate::script_ui::ConsoleKind;
use crate::theme;

#[derive(Default)]
pub struct PluginState {
    /// `<data folder>/modelcraft/plugins` (none in tests unless set).
    pub dir: Option<PathBuf>,
    pub open: bool,
    pub installed: Vec<Installed>,
    pub broken: Vec<(PathBuf, String)>,
    pub grants: Grants,
    pub loaded: Vec<LoadedPlugin>,
    /// How each plugin's entry script went (an error text, or none).
    pub errors: std::collections::BTreeMap<String, String>,
    /// The plugin the user is asked to confirm removing.
    pub confirm_remove: Option<String>,
    /// The last thing that happened, for the window and screen readers.
    pub message: String,
    focus_prompt: bool,
}

impl PluginState {
    /// Some plugin listens to events.
    pub fn listening(&self) -> bool {
        self.loaded.iter().any(|p| p.session.as_ref().is_some_and(|s| s.has_callbacks()))
    }

    /// The plugin the user has to decide about, if any.
    pub fn pending(&self) -> Option<(&Installed, Vec<Permission>)> {
        self.installed.iter().find_map(|i| match self.grants.status(&i.manifest) {
            Status::NeedsApproval(p) => Some((i, p)),
            _ => None,
        })
    }
}

/// `<data folder>/modelcraft/plugins`.
pub fn default_dir() -> Option<PathBuf> {
    craft_settings::paths::data_dir("modelcraft").map(|d| d.join("plugins"))
}

/// Use `dir` for plugins, read them and start the ones the user has allowed.
pub fn init(app: &mut ModelApp, dir: Option<PathBuf>) {
    app.script.plugins.dir = dir;
    reload(app);
}

/// Read the plugins folder again: start what is allowed, stop what is gone or switched off, and
/// open the window when a plugin needs a decision.
pub fn reload(app: &mut ModelApp) {
    let Some(dir) = app.script.plugins.dir.clone() else { return };
    let (installed, broken) = plugin::scan(&dir);
    let p = &mut app.script.plugins;
    p.grants = Grants::load(&dir);
    p.broken = broken;
    p.installed = installed.clone();
    p.loaded.retain(|l| installed.iter().any(|i| i.manifest.id == l.manifest.id && i.manifest.version == l.manifest.version));
    for i in &installed {
        match app.script.plugins.grants.status(&i.manifest) {
            Status::Ready(granted) => {
                if !app.script.plugins.loaded.iter().any(|l| l.manifest.id == i.manifest.id) {
                    start(app, i, &granted);
                }
            }
            Status::Disabled => app.script.plugins.loaded.retain(|l| l.manifest.id != i.manifest.id),
            Status::NeedsApproval(_) => {
                app.script.plugins.loaded.retain(|l| l.manifest.id != i.manifest.id);
                app.script.plugins.open = true;
                app.script.plugins.focus_prompt = true;
            }
        }
    }
}

fn project_dir(app: &ModelApp) -> Option<PathBuf> {
    app.doc.path.as_ref().and_then(|p| p.parent()).map(std::path::Path::to_path_buf).filter(|p| !p.as_os_str().is_empty())
}

fn start(app: &mut ModelApp, installed: &Installed, granted: &[Permission]) {
    let mut ctx = RunContext { selection: app.selection.clone() };
    let project = project_dir(app);
    let (loaded, report) = plugin::start(installed, granted, project.as_deref(), &mut app.doc, &mut ctx);
    let id = installed.manifest.id.clone();
    crate::script_ui::show_report(app, &report);
    match &report.error {
        Some(e) => {
            app.script.plugins.errors.insert(id, e.to_string());
        }
        None => {
            app.script.plugins.errors.remove(&id);
        }
    }
    app.script.plugins.loaded.retain(|l| l.manifest.id != installed.manifest.id);
    app.script.plugins.loaded.push(loaded);
}

/// The user allowed what the plugin asked for.
pub fn approve(app: &mut ModelApp, id: &str) {
    let Some(i) = app.script.plugins.installed.iter().find(|i| i.manifest.id == id).cloned() else { return };
    app.script.plugins.grants.approve(id, &i.manifest.requested());
    save_grants(app);
    app.script.plugins.message = crate::tlf!("{name} is allowed and running", name = i.manifest.name);
    reload(app);
}

/// The user said no: the plugin stays installed and off.
pub fn deny(app: &mut ModelApp, id: &str) {
    let name = app.script.plugins.installed.iter().find(|i| i.manifest.id == id).map(|i| i.manifest.name.clone()).unwrap_or_default();
    app.script.plugins.grants.deny(id);
    save_grants(app);
    app.script.plugins.loaded.retain(|l| l.manifest.id != id);
    app.script.plugins.message = crate::tlf!("{name} stays off", name = name);
}

pub fn set_enabled(app: &mut ModelApp, id: &str, enabled: bool) {
    app.script.plugins.grants.set_enabled(id, enabled);
    save_grants(app);
    if !enabled {
        app.script.plugins.loaded.retain(|l| l.manifest.id != id);
    }
    reload(app);
}

pub fn remove(app: &mut ModelApp, id: &str) {
    let Some(dir) = app.script.plugins.dir.clone() else { return };
    match plugin::remove(&dir, id) {
        Ok(()) => {
            app.script.plugins.grants.forget(id);
            save_grants(app);
            app.script.plugins.errors.remove(id);
            app.script.plugins.message = crate::tlf!("{id} was removed", id = id);
        }
        Err(e) => app.script.plugins.message = e,
    }
    app.script.plugins.confirm_remove = None;
    reload(app);
}

fn save_grants(app: &mut ModelApp) {
    let Some(dir) = app.script.plugins.dir.clone() else { return };
    if let Err(e) = app.script.plugins.grants.save(&dir) {
        app.script.plugins.message = crate::tlf!("The choice could not be saved: {error}", error = e);
    }
}

/// Install the package folder `src` (the plugin then asks for permission before it runs).
pub fn install_from(app: &mut ModelApp, src: &std::path::Path) {
    let Some(dir) = app.script.plugins.dir.clone() else {
        app.script.plugins.message = tl!("No plugins folder on this system");
        return;
    };
    match plugin::install(src, &dir) {
        Ok(Installation::New(i)) => {
            app.script.plugins.message = crate::tlf!("Installed {name} {version}", name = i.manifest.name, version = i.manifest.version)
        }
        Ok(Installation::Updated(i, old)) => {
            app.script.plugins.loaded.retain(|l| l.manifest.id != i.manifest.id);
            app.script.plugins.message = crate::tlf!("Updated {name} from {old} to {version}", name = i.manifest.name, old = old, version = i.manifest.version);
        }
        Err(e) => app.script.plugins.message = crate::tlf!("Could not install: {error}", error = e),
    }
    app.script.plugins.open = true;
    reload(app);
}

/// Ask for a package folder and install it.
pub fn pick_and_install(app: &mut ModelApp) {
    if let Some(p) = rfd::FileDialog::new().set_title(tl!("Choose a plugin folder")).pick_folder() {
        install_from(app, &p);
    }
}

pub fn toggle(app: &mut ModelApp) {
    app.script.plugins.open = !app.script.plugins.open;
}

/// The tools plugins registered: `(plugin id, plugin name, label, tip)`.
pub fn tools(app: &ModelApp) -> Vec<(String, String, String, String)> {
    let mut v = Vec::new();
    for p in &app.script.plugins.loaded {
        if let Some(s) = &p.session {
            for t in s.tools() {
                v.push((p.manifest.id.clone(), p.manifest.name.clone(), t.label, t.tip));
            }
        }
    }
    v
}

/// Run a tool of a plugin, as one undoable change.
pub fn run_tool(app: &mut ModelApp, id: &str, label: &str) {
    let Some(pos) = app.script.plugins.loaded.iter().position(|p| p.manifest.id == id) else { return };
    let Some(mut session) = app.script.plugins.loaded[pos].session.take() else { return };
    let mut ctx = RunContext { selection: app.selection.clone() };
    app.script.note_line(ConsoleKind::Note, crate::tlf!("Running {script}…", script = label));
    let report = session.run_tool(&mut app.doc, &mut ctx, label);
    if let Some(p) = app.script.plugins.loaded.get_mut(pos) {
        p.session = Some(session);
    }
    if !crate::script_ui::finish_report(app, label, report) {
        app.script.open = true;
    }
}

/// The Scripts menu's part: the Plugins window and each plugin's tools.
pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    if ui.button(tl!("Plugins…")).clicked() {
        app.script.plugins.open = true;
        ui.close();
    }
    let tools = tools(app);
    let mut run = None;
    let mut last = String::new();
    for (id, name, label, tip) in &tools {
        if *id != last {
            ui.label(RichText::new(name).color(theme::TEXT_DIM).small());
            last = id.clone();
        }
        let b = ui.button(label);
        let b = if tip.is_empty() { b } else { b.on_hover_text(tip) };
        if b.clicked() {
            run = Some((id.clone(), label.clone()));
            ui.close();
        }
    }
    if let Some((id, label)) = run {
        run_tool(app, &id, &label);
    }
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.script.plugins.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Plugins"))
        .id(egui::Id::unique("plugins-window"))
        .open(&mut open)
        .collapsible(false)
        .default_size([560.0, 480.0])
        .show(ctx, |ui| body(app, ui));
    app.script.plugins.open = open;
}

fn permission_list(ui: &mut egui::Ui, perms: &[Permission]) {
    for p in perms {
        let text = tl!(p.describe());
        let color = if p.is_sensitive() { theme::SELECT } else { theme::TEXT };
        let l = ui.label(RichText::new(format!("• {text}")).color(color));
        ui.ctx().accesskit_node_builder(l.id, |n| {
            n.set_label(if p.is_sensitive() { crate::tlf!("Needs care: {permission}", permission = text) } else { text.clone() })
        });
    }
}

fn body(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        if ui.button(tl!("Install from Folder…")).clicked() {
            pick_and_install(app);
        }
        if ui.button(tl!("Reload Plugins")).clicked() {
            reload(app);
        }
        if let Some(d) = &app.script.plugins.dir {
            ui.label(RichText::new(crate::tlf!("Plugins folder: {folder}", folder = d.display())).color(theme::TEXT_DIM).small());
        }
    });
    if !app.script.plugins.message.is_empty() {
        let l = ui.label(RichText::new(app.script.plugins.message.clone()).color(theme::SELECT));
        ui.ctx().accesskit_node_builder(l.id, |n| n.set_live(egui::accesskit::Live::Polite));
    }
    // The permission prompt: one plugin at a time, and nothing it asks for is granted by looking at it.
    if let Some((i, new)) = app.script.plugins.pending().map(|(i, n)| (i.clone(), n)) {
        let m = &i.manifest;
        ui.separator();
        ui.group(|ui| {
            let title = ui.heading(crate::tlf!("{name} {version} wants permission", name = m.name, version = m.version));
            ui.ctx().accesskit_node_builder(title.id, |n| n.set_live(egui::accesskit::Live::Assertive));
            if !m.author.is_empty() {
                ui.label(RichText::new(crate::tlf!("by {author}", author = m.author)).color(theme::TEXT_DIM));
            }
            if !m.description.is_empty() {
                ui.label(&m.description);
            }
            if new.is_empty() {
                ui.label(tl!("It asks for no special permissions: it can only read your scene."));
            } else {
                ui.label(tl!("It asks to:"));
                permission_list(ui, &new);
            }
            ui.label(
                RichText::new(tl!("A plugin is a script: it cannot run programs, use the network or reach files outside what is listed."))
                    .color(theme::TEXT_DIM)
                    .small(),
            );
            ui.horizontal(|ui| {
                let deny = ui.button(tl!("Don't Allow"));
                if std::mem::take(&mut app.script.plugins.focus_prompt) {
                    deny.request_focus();
                }
                if deny.clicked() {
                    deny_id(app, &m.id);
                }
                if ui.button(tl!("Allow")).clicked() {
                    approve(app, &m.id);
                }
            });
        });
    }
    ui.separator();
    let installed = app.script.plugins.installed.clone();
    if installed.is_empty() && app.script.plugins.broken.is_empty() {
        ui.label(RichText::new(tl!("No plugins installed")).color(theme::TEXT_DIM));
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        for i in &installed {
            card(app, ui, i);
        }
        for (dir, why) in app.script.plugins.broken.clone() {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            ui.label(RichText::new(crate::tlf!("{name} could not be read: {why}", name = name, why = why)).color(theme::SELECT));
        }
    });
}

fn deny_id(app: &mut ModelApp, id: &str) {
    deny(app, id);
}

fn card(app: &mut ModelApp, ui: &mut egui::Ui, i: &Installed) {
    let m = &i.manifest;
    let status = app.script.plugins.grants.status(m);
    let loaded = app.script.plugins.loaded.iter().any(|l| l.manifest.id == m.id);
    ui.group(|ui| {
        ui.label(RichText::new(format!("{} {}", m.name, m.version)).strong());
        ui.label(RichText::new(&m.id).color(theme::TEXT_DIM).small());
        if !m.description.is_empty() {
            ui.label(&m.description);
        }
        let state = match (&status, loaded) {
            (Status::Ready(_), true) => tl!("Running"),
            (Status::Ready(_), false) => tl!("Allowed"),
            (Status::Disabled, _) => tl!("Off"),
            (Status::NeedsApproval(_), _) => tl!("Waiting for your decision"),
        };
        ui.label(RichText::new(state).color(theme::TEXT_DIM));
        if let Some(e) = app.script.plugins.errors.get(&m.id) {
            ui.label(RichText::new(crate::tlf!("Its start script failed: {error}", error = e)).color(theme::SELECT));
        }
        if !m.requested().is_empty() {
            ui.label(RichText::new(tl!("Permissions")).color(theme::TEXT_DIM).small());
            permission_list(ui, &m.requested());
        }
        ui.horizontal(|ui| {
            match status {
                Status::Disabled => {
                    if ui.button(tl!("Turn On")).clicked() {
                        set_enabled(app, &m.id, true);
                    }
                }
                Status::Ready(_) => {
                    if ui.button(tl!("Turn Off")).clicked() {
                        set_enabled(app, &m.id, false);
                    }
                }
                Status::NeedsApproval(_) => {}
            }
            if app.script.plugins.confirm_remove.as_deref() == Some(m.id.as_str()) {
                ui.label(RichText::new(crate::tlf!("Remove {name} and its files?", name = m.name)).color(theme::SELECT));
                if ui.button(tl!("Remove")).clicked() {
                    remove(app, &m.id);
                }
                if ui.button(tl!("Keep")).clicked() {
                    app.script.plugins.confirm_remove = None;
                }
            } else if ui.button(tl!("Remove…")).clicked() {
                app.script.plugins.confirm_remove = Some(m.id.clone());
            }
        });
    });
}
