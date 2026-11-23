//! Render settings, the frame queue and the Render menu.
//!
//! The Render Settings window edits what the project keeps for its renders
//! ([`modelcraft_engine::scene::stage::RenderConfig`]): the camera, picture size, the range of
//! frames, how files are saved, which passes an EXR holds, the region kept, and quality. Every
//! change is a `stage` command, so it is undoable and saved with the project.
//!
//! The queue renders the project's frame range on a worker thread from a copy of the scene
//! ([`modelcraft_render::batch`]); it writes numbered files, hands each finished frame to the
//! Picture Viewer, reports its progress through a live region and can be cancelled from the
//! window, the palette or Esc.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use egui::{Key, RichText};
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::scene::stage::{MAX_SIDE, OutputFormat, Pass, RenderConfig};
use modelcraft_engine::{Command, StageCommand};
use modelcraft_render::batch::{self, Frame, Job, Progress};
use modelcraft_render::trace::Lighting;

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderAction {
    /// The Render Settings window.
    Settings,
    /// The Picture Viewer window.
    Pictures,
    /// Render the project's frame range to numbered files.
    RenderFrames,
    CancelFrames,
}

impl RenderAction {
    pub const ALL: [RenderAction; 4] = [RenderAction::Settings, RenderAction::Pictures, RenderAction::RenderFrames, RenderAction::CancelFrames];

    pub fn label(self) -> String {
        match self {
            RenderAction::Settings => tl!("Render Settings…"),
            RenderAction::Pictures => tl!("Picture Viewer"),
            RenderAction::RenderFrames => tl!("Render Frames to Files"),
            RenderAction::CancelFrames => tl!("Cancel Frame Render"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            RenderAction::Settings => "render.settings",
            RenderAction::Pictures => "render.pictures",
            RenderAction::RenderFrames => "render.frames",
            RenderAction::CancelFrames => "render.cancel_frames",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            RenderAction::Settings => Some("Cmd+Shift+F9"),
            RenderAction::Pictures => Some("Shift+F9"),
            _ => None,
        }
    }
}

/// The Render menu.
pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Rendering"), |ui| {
        let ctx = ui.ctx().clone();
        for a in [crate::actions::Action::RenderView]
            .into_iter()
            .chain(RenderAction::ALL.map(crate::actions::Action::Render))
            .chain(crate::bake_ui::BakeAction::ALL.map(crate::actions::Action::Bake))
            .chain(crate::matgraph_ui::MatAction::ALL.map(crate::actions::Action::Material))
        {
            let b = egui::Button::new(a.label()).shortcut_text(a.shortcut_text(app));
            if ui.add(b).clicked() {
                app.act(&ctx, a);
                ui.close();
            }
        }
    });
}

pub fn act(app: &mut ModelApp, ctx: &egui::Context, a: RenderAction) {
    match a {
        RenderAction::Settings => {
            app.render_ui.open = !app.render_ui.open;
            app.render_ui.focus_pending = app.render_ui.open;
        }
        RenderAction::Pictures => crate::picture_viewer::toggle(app),
        RenderAction::RenderFrames => start_queue(app, ctx),
        RenderAction::CancelFrames => cancel(app),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Output,
    Save,
    Passes,
    Region,
    Quality,
    Frames,
}

impl Category {
    const ALL: [Category; 6] = [Category::Output, Category::Save, Category::Passes, Category::Region, Category::Quality, Category::Frames];

    fn label(self) -> String {
        match self {
            Category::Output => tl!("Output"),
            Category::Save => tl!("Save"),
            Category::Passes => tl!("Passes"),
            Category::Region => tl!("Region"),
            Category::Quality => tl!("Quality"),
            Category::Frames => tl!("Frame Queue"),
        }
    }
}

enum Message {
    Frame(Frame),
    Done(Result<Vec<std::path::PathBuf>, String>),
}

struct Queue {
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    rx: Mutex<Receiver<Message>>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The window's state and the queue in progress.
pub struct RenderUi {
    pub open: bool,
    pub category: Category,
    pub focus_pending: bool,
    had_focus: bool,
    queue: Option<Queue>,
    /// The line the window announces: progress, the result, or why the queue stopped.
    pub state: String,
    /// Files the last queue wrote.
    pub written: Vec<std::path::PathBuf>,
    /// The output path while it is being typed.
    output_edit: Option<String>,
}

impl Default for RenderUi {
    fn default() -> Self {
        RenderUi {
            open: false,
            category: Category::Output,
            focus_pending: false,
            had_focus: false,
            queue: None,
            state: String::new(),
            written: Vec::new(),
            output_edit: None,
        }
    }
}

impl RenderUi {
    /// Whether a queue is rendering.
    pub fn busy(&self) -> bool {
        self.queue.is_some()
    }

    /// Frames done and frames in all, while a queue runs.
    pub fn progress(&self) -> Option<(u32, u32)> {
        self.queue.as_ref().map(|q| (q.progress.done.load(Ordering::Relaxed), q.progress.total.load(Ordering::Relaxed)))
    }
}

fn start_queue(app: &mut ModelApp, ctx: &egui::Context) {
    if app.render_ui.queue.is_some() {
        app.status = tl!("A frame render is already running");
        return;
    }
    let config = app.doc.scene.render.sanitized();
    if config.output.trim().is_empty() {
        app.render_ui.state = tl!("Choose where the frames are written first (Render Settings, Save).");
        app.render_ui.open = true;
        app.render_ui.category = Category::Save;
        app.render_ui.focus_pending = true;
        return;
    }
    app.doc.sync_sculpt();
    let job = Job {
        scene: app.doc.scene.clone(),
        viewport: app.views[app.active_view.min(3)].camera,
        lighting: lighting_of(app),
        config,
        device: device_of(app),
        write: true,
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    let (c, p, repaint) = (cancel.clone(), progress.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("render-frames".into()).spawn(move || {
        let tx_frame = tx.clone();
        let result = batch::run(&job, &c, &p, &mut |f| {
            let _ = tx_frame.send(Message::Frame(f));
            repaint.request_repaint();
        });
        let _ = tx.send(Message::Done(result));
        repaint.request_repaint();
    });
    match spawned {
        Ok(handle) => {
            app.render_ui.queue = Some(Queue { cancel, progress, rx: Mutex::new(rx), handle: Some(handle) });
            app.render_ui.written.clear();
            app.render_ui.state = tl!("Rendering frames…");
        }
        Err(e) => app.render_ui.state = crate::tlf!("Frame render failed: {error}", error = e),
    }
}

fn cancel(app: &mut ModelApp) {
    if let Some(q) = &app.render_ui.queue {
        q.cancel.store(true, Ordering::Relaxed);
        app.render_ui.state = tl!("Cancelling the frame render…");
    }
}

/// The lighting the Render View is set to, so frames match what it shows.
fn lighting_of(app: &ModelApp) -> Lighting {
    let v = &app.render_view;
    match v.light {
        crate::render_view::Light::Studio => Lighting::Studio,
        crate::render_view::Light::Daylight => Lighting::Daylight { elevation: v.sun_elevation.to_radians(), azimuth: v.sun_azimuth.to_radians() },
        crate::render_view::Light::Panorama => match &v.panorama {
            Some((_, image)) => Lighting::Image { image: image.clone(), rotation: v.sun_azimuth.to_radians() },
            None => Lighting::Studio,
        },
    }
}

fn device_of(app: &ModelApp) -> batch::Device {
    match app.render_view.device {
        crate::render_view::Device::Auto => batch::Device::Auto,
        crate::render_view::Device::Cpu => batch::Device::Cpu,
        crate::render_view::Device::Gpu => batch::Device::Gpu,
    }
}

/// Take what the queue has finished: pictures for the viewer, and the end of the run.
pub fn tick(app: &mut ModelApp, ctx: &egui::Context) {
    let mut frames = Vec::new();
    let mut done = None;
    if let Some(q) = &app.render_ui.queue {
        let rx = q.rx.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            match rx.try_recv() {
                Ok(Message::Frame(f)) => frames.push(f),
                Ok(Message::Done(r)) => done = Some(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    done.get_or_insert(Err(tl!("the frame render stopped unexpectedly")));
                    break;
                }
            }
        }
    }
    for f in frames {
        if let Some(p) = &f.path {
            app.render_ui.written.push(p.clone());
        }
        app.pictures.add(f.picture);
    }
    if let Some(result) = done {
        let cancelled = app.render_ui.queue.as_ref().is_some_and(|q| q.cancel.load(Ordering::Relaxed));
        app.render_ui.queue = None;
        let n = app.render_ui.written.len();
        app.render_ui.state = match result {
            Err(e) => crate::tlf!("Frame render failed: {error}", error = e),
            Ok(_) if cancelled => crate::tlf!("Frame render cancelled after {count} files", count = n),
            Ok(_) => crate::tlf!("Frame render finished: {count} files", count = n),
        };
        app.status = app.render_ui.state.clone();
    } else if app.render_ui.queue.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// Esc closes the settings window when focus is inside it, and cancels a running queue first.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if !app.render_ui.open || !app.render_ui.had_focus {
        return false;
    }
    if !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        return false;
    }
    if app.render_ui.queue.is_some() {
        cancel(app);
    } else {
        app.render_ui.open = false;
        app.render_ui.had_focus = false;
    }
    true
}

/// A change to the project's render settings: previewed while a drag lasts, one undo step after.
fn commit(app: &mut ModelApp, r: &egui::Response, config: &RenderConfig) {
    let new = config.sanitized();
    if r.changed() && r.dragged() {
        if app.prop_snapshot.is_none() {
            app.prop_snapshot = Some(Box::new(app.doc.scene.clone()));
        }
        app.doc.scene.render = new;
        return;
    }
    if r.drag_stopped() || (r.changed() && !r.dragged()) {
        if let Some(s) = app.prop_snapshot.take() {
            app.doc.scene = *s;
        }
        if app.doc.scene.render != new {
            app.run(Command::Stage(StageCommand::SetRenderSettings { config: new }));
        }
    }
}

fn drag_u32(
    app: &mut ModelApp,
    ui: &mut egui::Ui,
    c: &mut RenderConfig,
    name: String,
    range: std::ops::RangeInclusive<u32>,
    pick: impl Fn(&mut RenderConfig) -> &mut u32,
) {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(pick(c)).range(range));
        set_name(ui, &r, name);
        commit(app, &r, c);
    });
}

fn drag_i32(app: &mut ModelApp, ui: &mut egui::Ui, c: &mut RenderConfig, name: String, pick: impl Fn(&mut RenderConfig) -> &mut i32) {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(pick(c)).range(-1_000_000..=1_000_000));
        set_name(ui, &r, name);
        commit(app, &r, c);
    });
}

fn camera_name(app: &ModelApp, id: Option<ObjectId>) -> String {
    id.and_then(|i| app.doc.scene.get(i)).map_or_else(|| tl!("Viewport view"), |o| o.name.clone())
}

fn page(app: &mut ModelApp, ui: &mut egui::Ui, category: Category, c: &mut RenderConfig) {
    match category {
        Category::Output => {
            ui.horizontal(|ui| {
                ui.label(tl!("Camera"));
                let shown = camera_name(app, app.doc.scene.render_camera());
                let r = egui::ComboBox::from_id_salt("render-camera").selected_text(shown).show_ui(ui, |ui| {
                    let current = app.doc.scene.render_camera();
                    if ui.selectable_label(current.is_none(), tl!("Viewport view")).clicked() && current.is_some() {
                        app.run(Command::Stage(StageCommand::SetRenderCamera { id: None }));
                    }
                    for id in app.doc.scene.cameras() {
                        let name = camera_name(app, Some(id));
                        if ui.selectable_label(current == Some(id), name).clicked() && current != Some(id) {
                            app.run(Command::Stage(StageCommand::SetRenderCamera { id: Some(id) }));
                        }
                    }
                });
                set_name(ui, &r.response, tl!("Camera"));
            });
            drag_u32(app, ui, c, tl!("Width"), 16..=MAX_SIDE, |c| &mut c.width);
            drag_u32(app, ui, c, tl!("Height"), 16..=MAX_SIDE, |c| &mut c.height);
            drag_i32(app, ui, c, tl!("First frame"), |c| &mut c.frame_start);
            drag_i32(app, ui, c, tl!("Last frame"), |c| &mut c.frame_end);
            drag_u32(app, ui, c, tl!("Frame step"), 1..=10_000, |c| &mut c.frame_step);
            let [w, h] = c.output_size();
            ui.label(
                RichText::new(crate::tlf!("{count} frames, each {width} by {height} pixels", count = c.frames().len(), width = w, height = h))
                    .color(theme::TEXT_DIM),
            );
        }
        Category::Save => {
            ui.horizontal(|ui| {
                ui.label(tl!("Format"));
                let name = |f: OutputFormat| match f {
                    OutputFormat::Png => tl!("PNG (8-bit)"),
                    OutputFormat::Exr => tl!("EXR (32-bit float)"),
                };
                let r = egui::ComboBox::from_id_salt("render-format").selected_text(name(c.format)).show_ui(ui, |ui| {
                    for f in [OutputFormat::Png, OutputFormat::Exr] {
                        ui.selectable_value(&mut c.format, f, name(f));
                    }
                });
                set_name(ui, &r.response, tl!("Format"));
                if c.format != app.doc.scene.render.format {
                    app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
                    r.response.request_focus();
                }
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Frames go to"));
                // The text is kept while it is edited, and follows the project otherwise.
                let mut text = app.render_ui.output_edit.clone().unwrap_or_else(|| c.output.clone());
                let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text(tl!("a file name, such as shot.png or shot_####.png")).desired_width(280.0));
                set_name(ui, &r, tl!("Frames go to"));
                if r.changed() {
                    app.render_ui.output_edit = Some(text);
                }
                if r.lost_focus()
                    && let Some(t) = app.render_ui.output_edit.take()
                    && t != c.output
                {
                    c.output = t;
                    app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
                }
                if ui.button(tl!("Choose…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().set_file_name("render.png").save_file()
                {
                    c.output = path.display().to_string();
                    app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
                }
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Exposure (stops)"));
                let r = ui.add(egui::DragValue::new(&mut c.exposure).speed(0.05).range(-16.0..=16.0));
                set_name(ui, &r, tl!("Exposure (stops)"));
                commit(app, &r, c);
            });
            let r = ui.checkbox(&mut c.denoise, tl!("Denoise finished pictures"));
            if r.changed() {
                app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
            }
        }
        Category::Passes => {
            ui.label(RichText::new(tl!("An EXR file keeps these as layers next to the picture. A PNG holds the picture only.")).color(theme::TEXT_DIM).small());
            let enabled = c.format == OutputFormat::Exr;
            let before = c.passes.clone();
            for pass in Pass::ALL.into_iter().filter(|p| *p != Pass::Beauty) {
                let mut on = c.passes.contains(&pass);
                let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, tl!(pass.title())));
                if r.changed() {
                    if on {
                        c.passes.push(pass);
                    } else {
                        c.passes.retain(|p| *p != pass);
                    }
                }
            }
            if c.passes != before {
                app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
            }
            if !enabled {
                ui.label(RichText::new(tl!("Choose the EXR format on the Save page to keep passes.")).color(theme::TEXT_DIM).small());
            }
        }
        Category::Region => {
            let mut on = c.region.is_some();
            if ui.checkbox(&mut on, tl!("Keep only a region of the picture")).changed() {
                c.region = on.then_some([0.0, 0.0, 1.0, 1.0]);
                app.run(Command::Stage(StageCommand::SetRenderSettings { config: c.sanitized() }));
            }
            if let Some(mut r) = c.region {
                for (k, name) in [tl!("Left"), tl!("Top"), tl!("Right"), tl!("Bottom")].into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(name.clone());
                        let resp = ui.add(egui::DragValue::new(&mut r[k]).speed(0.005).range(0.0..=1.0).max_decimals(3));
                        set_name(ui, &resp, name);
                        if resp.changed() {
                            c.region = Some(r);
                        }
                        commit(app, &resp, c);
                    });
                }
                ui.label(RichText::new(tl!("The whole picture is rendered; the saved picture is cut to the region.")).color(theme::TEXT_DIM).small());
            }
        }
        Category::Quality => {
            drag_u32(app, ui, c, tl!("Samples per pixel"), 1..=65_536, |c| &mut c.samples);
            drag_u32(app, ui, c, tl!("Light bounces"), 1..=64, |c| &mut c.bounces);
            ui.label(
                RichText::new(tl!("These are used for frames written to files. The Render View has its own, lighter settings.")).color(theme::TEXT_DIM).small(),
            );
        }
        Category::Frames => {
            ui.horizontal(|ui| {
                let busy = app.render_ui.busy();
                let go = ui.add_enabled(!busy, egui::Button::new(tl!("Render Frames")));
                if std::mem::take(&mut app.render_ui.focus_pending) {
                    go.request_focus();
                }
                if go.clicked() {
                    let ctx = ui.ctx().clone();
                    act(app, &ctx, RenderAction::RenderFrames);
                }
                if ui.add_enabled(busy, egui::Button::new(tl!("Cancel"))).clicked() {
                    cancel(app);
                }
            });
            if let Some((done, total)) = app.render_ui.progress() {
                let bar = ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).text(crate::tlf!(
                    "{done} of {total} frames",
                    done = done,
                    total = total
                )));
                set_name(ui, &bar, crate::tlf!("Frame render progress: {done} of {total} frames", done = done, total = total));
            }
            for p in app.render_ui.written.iter().rev().take(5) {
                ui.label(RichText::new(p.display().to_string()).color(theme::TEXT_DIM).small());
            }
        }
    }
}

/// The Render Settings window.
pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.render_ui.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Render Settings")).id(egui::Id::unique("render-settings")).open(&mut open).collapsible(false).default_size([520.0, 360.0]).show(
        ctx,
        |ui| {
            let mut config = app.doc.scene.render.clone();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(120.0);
                    for (n, cat) in Category::ALL.into_iter().enumerate() {
                        let r = ui.selectable_label(app.render_ui.category == cat, cat.label());
                        if n == 0 && std::mem::take(&mut app.render_ui.focus_pending) && app.render_ui.category != Category::Frames {
                            r.request_focus();
                        }
                        if r.clicked() {
                            app.render_ui.category = cat;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let category = app.render_ui.category;
                    page(app, ui, category, &mut config);
                });
            });
            ui.separator();
            let line = ui.label(RichText::new(&app.render_ui.state).color(theme::TEXT));
            ui.ctx().accesskit_node_builder(line.id, |n| n.set_live(egui::accesskit::Live::Polite));
            let layer = ui.layer_id();
            app.render_ui.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
        },
    );
    if !open {
        app.render_ui.open = false;
    }
}
