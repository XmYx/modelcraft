//! Bake Maps: capture a dense mesh's normals, height, occlusion, curvature, cavities and colour
//! onto the UV layout of a simpler one ([`modelcraft_bake`]).
//!
//! The window takes the selected object as the target and bakes from every other visible surface
//! (or from the other selected objects). The bake runs on a worker thread from a copy of the
//! meshes, reports its rows through a live progress bar, can be cancelled from the window, the
//! palette or Esc, writes one file per map and hands each map to the Picture Viewer.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use egui::{Key, RichText};
use modelcraft_bake::{BakeError, MapKind, Options, Output, Prepared, Progress};
use modelcraft_engine::scene::ObjectId;

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakeAction {
    /// The Bake Maps window.
    Window,
    /// Bake the selected object now, with the window's settings.
    Run,
    Cancel,
}

impl BakeAction {
    pub const ALL: [BakeAction; 3] = [BakeAction::Window, BakeAction::Run, BakeAction::Cancel];

    pub fn label(self) -> String {
        match self {
            BakeAction::Window => tl!("Bake Maps…"),
            BakeAction::Run => tl!("Bake Maps Now"),
            BakeAction::Cancel => tl!("Cancel Bake"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            BakeAction::Window => "uv.bake_maps",
            BakeAction::Run => "uv.bake_maps_now",
            BakeAction::Cancel => "uv.cancel_bake",
        }
    }
}

pub fn act(app: &mut ModelApp, ctx: &egui::Context, a: BakeAction) {
    match a {
        BakeAction::Window => {
            app.bake.open = !app.bake.open;
            app.bake.focus_pending = app.bake.open;
        }
        BakeAction::Run => start(app, ctx),
        BakeAction::Cancel => cancel(app),
    }
}

enum Message {
    Done(Result<Output, BakeError>),
}

struct Running {
    cancel: Arc<AtomicBool>,
    progress: Arc<Progress>,
    rx: Mutex<Receiver<Message>>,
    handle: Option<JoinHandle<()>>,
    name: String,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub struct BakeUi {
    pub open: bool,
    pub focus_pending: bool,
    had_focus: bool,
    pub options: Options,
    /// Bake from every other visible surface (else from the other selected objects).
    pub from_all: bool,
    /// Write EXR files instead of PNG.
    pub exr: bool,
    /// Where the files go: this plus `_<map>.png`.
    pub prefix: String,
    job: Option<Running>,
    /// What the window announces.
    pub state: String,
    pub files: Vec<PathBuf>,
}

impl Default for BakeUi {
    fn default() -> Self {
        BakeUi {
            open: false,
            focus_pending: false,
            had_focus: false,
            options: Options { width: 1024, height: 1024, maps: vec![MapKind::Normal, MapKind::Occlusion], ..Options::default() },
            from_all: true,
            exr: false,
            prefix: String::new(),
            job: None,
            state: String::new(),
            files: Vec::new(),
        }
    }
}

impl BakeUi {
    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Rows done and rows in all, while a bake runs.
    pub fn progress(&self) -> Option<(u32, u32)> {
        self.job.as_ref().map(|j| (j.progress.rows_done.load(Ordering::Relaxed), j.progress.rows_total.load(Ordering::Relaxed)))
    }
}

fn target_of(app: &ModelApp) -> Option<ObjectId> {
    app.active().filter(|id| {
        app.doc
            .scene
            .get(*id)
            .is_some_and(|o| matches!(o.kind, modelcraft_engine::scene::ObjectKind::Mesh { .. } | modelcraft_engine::scene::ObjectKind::SmoothCage { .. }))
    })
}

fn start(app: &mut ModelApp, ctx: &egui::Context) {
    if app.bake.job.is_some() {
        app.status = tl!("A bake is already running");
        return;
    }
    let Some(target) = target_of(app) else {
        app.bake.state = tl!("Select the mesh to bake onto first.");
        return;
    };
    if app.bake.prefix.trim().is_empty() {
        app.bake.state = tl!("Choose where the maps are written first.");
        app.bake.open = true;
        return;
    }
    let sources: Vec<ObjectId> = if app.bake.from_all { Vec::new() } else { app.selection.iter().copied().filter(|id| *id != target).collect() };
    if !app.bake.from_all && sources.is_empty() {
        app.bake.state = tl!("Select the dense meshes too, or bake from every other surface.");
        return;
    }
    app.doc.sync_sculpt();
    let prepared = match Prepared::from_scene(&app.doc.scene, &mut app.doc.eval, target, &sources) {
        Ok(p) => p,
        Err(e) => {
            app.bake.state = crate::tlf!("Bake failed: {error}", error = e);
            return;
        }
    };
    if prepared.source_count() == 0 {
        app.bake.state = tl!("There is no dense mesh to bake from.");
        return;
    }
    let name = app.doc.scene.get(target).map_or_else(String::new, |o| o.name.clone());
    let options = app.bake.options.sanitized();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Progress::default());
    let (tx, rx) = channel();
    let (c, p, o, repaint) = (cancel.clone(), progress.clone(), options.clone(), ctx.clone());
    let spawned = std::thread::Builder::new().name("bake-maps".into()).spawn(move || {
        let _ = tx.send(Message::Done(prepared.bake(&o, &c, &p)));
        repaint.request_repaint();
    });
    match spawned {
        Ok(handle) => {
            app.bake.job = Some(Running { cancel, progress, rx: Mutex::new(rx), handle: Some(handle), name });
            app.bake.files.clear();
            app.bake.state = tl!("Baking…");
        }
        Err(e) => app.bake.state = crate::tlf!("Bake failed: {error}", error = e),
    }
}

fn cancel(app: &mut ModelApp) {
    if let Some(j) = &app.bake.job {
        j.cancel.store(true, Ordering::Relaxed);
        app.bake.state = tl!("Cancelling the bake…");
    }
}

/// Take the finished bake: write its files and keep its pictures.
pub fn tick(app: &mut ModelApp, ctx: &egui::Context) {
    let mut done = None;
    if let Some(j) = &app.bake.job {
        let rx = j.rx.lock().unwrap_or_else(|p| p.into_inner());
        match rx.try_recv() {
            Ok(Message::Done(r)) => done = Some(r),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => done = Some(Err(BakeError::Cancelled)),
        }
    }
    let Some(result) = done else {
        if app.bake.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        return;
    };
    let job = app.bake.job.take();
    let (name, exr) = (job.as_ref().map(|j| j.name.clone()).unwrap_or_default(), app.bake.exr);
    app.bake.state = match result {
        Err(BakeError::Cancelled) => tl!("Bake cancelled"),
        Err(e) => crate::tlf!("Bake failed: {error}", error = e),
        Ok(out) => {
            let mut failed = None;
            for map in &out.maps {
                let path = PathBuf::from(format!("{}_{}.{}", app.bake.prefix, map.kind.id(), if exr { "exr" } else { "png" }));
                let written = if exr { map.write_exr(&path) } else { map.write_png(&path) };
                match written {
                    Ok(()) => app.bake.files.push(path),
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
                app.pictures.add(modelcraft_render::output::Picture {
                    label: crate::tlf!("Bake of {name}: {map}", name = name, map = tl!(map.kind.title())),
                    size: [map.width, map.height],
                    pixels: map.to_rgba8(),
                    samples: 0,
                    seconds: 0.0,
                });
            }
            match failed {
                Some(e) => crate::tlf!("Bake failed: {error}", error = e),
                None => crate::tlf!("Bake finished: {count} maps, {missed} texels found no surface", count = out.maps.len(), missed = out.missed),
            }
        }
    };
    app.status = app.bake.state.clone();
}

/// Esc cancels a running bake, or closes the window when focus is inside it.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if !app.bake.open || !app.bake.had_focus {
        return false;
    }
    if !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        return false;
    }
    if app.bake.job.is_some() {
        cancel(app);
    } else {
        app.bake.open = false;
        app.bake.had_focus = false;
    }
    true
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.bake.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Bake Maps")).id(egui::Id::unique("bake-maps")).open(&mut open).collapsible(false).default_size([440.0, 420.0]).show(ctx, |ui| {
        let target = target_of(app);
        let app_busy = app.bake.busy();
        let shown = target.and_then(|t| app.doc.scene.get(t)).map_or_else(|| tl!("none selected"), |o| o.name.clone());
        ui.label(RichText::new(crate::tlf!("Bake onto: {name}", name = shown)).strong());
        let b = &mut app.bake;
        let r = ui.checkbox(&mut b.from_all, tl!("Bake from every other visible surface"));
        if std::mem::take(&mut b.focus_pending) {
            r.request_focus();
        }
        ui.label(
            RichText::new(tl!("The target needs a UV layout. Without the box ticked, the other selected objects are the dense meshes."))
                .color(theme::TEXT_DIM)
                .small(),
        );
        ui.separator();
        ui.label(RichText::new(tl!("Maps")).strong());
        for kind in MapKind::ALL {
            let mut on = b.options.maps.contains(&kind);
            if ui.checkbox(&mut on, tl!(kind.title())).changed() {
                if on {
                    b.options.maps.push(kind);
                } else {
                    b.options.maps.retain(|k| *k != kind);
                }
            }
        }
        ui.separator();
        let side = |ui: &mut egui::Ui, v: &mut u32, name: String, range: std::ops::RangeInclusive<u32>| {
            ui.horizontal(|ui| {
                ui.label(name.clone());
                let r = ui.add(egui::DragValue::new(v).range(range));
                set_name(ui, &r, name);
            });
        };
        side(ui, &mut b.options.width, tl!("Width"), 1..=modelcraft_bake::MAX_SIDE);
        side(ui, &mut b.options.height, tl!("Height"), 1..=modelcraft_bake::MAX_SIDE);
        side(ui, &mut b.options.rays, tl!("Occlusion rays"), 1..=modelcraft_bake::MAX_RAYS);
        side(ui, &mut b.options.padding, tl!("Padding (texels)"), 0..=256);
        for (v, name, speed) in [(&mut b.options.cage, tl!("Cage distance"), 0.005f32), (&mut b.options.distance, tl!("Ray distance"), 0.005)] {
            ui.horizontal(|ui| {
                ui.label(name.clone());
                let r = ui.add(egui::DragValue::new(v).speed(speed).range(0.0..=1e6).max_decimals(4));
                set_name(ui, &r, name);
            });
        }
        ui.checkbox(&mut b.options.flip_green, tl!("Flip the normal map's green channel"));
        ui.checkbox(&mut b.exr, tl!("Write EXR files"));
        ui.horizontal(|ui| {
            ui.label(tl!("Write to"));
            let r = ui.add(egui::TextEdit::singleline(&mut b.prefix).hint_text(tl!("a path such as /folder/rock (then _normal.png …)")).desired_width(240.0));
            set_name(ui, &r, tl!("Write to"));
            if ui.button(tl!("Choose…")).clicked()
                && let Some(path) = rfd::FileDialog::new().set_file_name("bake").save_file()
            {
                b.prefix = path.with_extension("").display().to_string();
            }
        });
        ui.separator();
        let busy = app_busy;
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(tl!("Bake"))).clicked() {
                let ctx = ui.ctx().clone();
                act(app, &ctx, BakeAction::Run);
            }
            if ui.add_enabled(busy, egui::Button::new(tl!("Cancel"))).clicked() {
                cancel(app);
            }
        });
        if let Some((done, total)) = app.bake.progress() {
            let bar = ui.add(egui::ProgressBar::new(if total > 0 { done as f32 / total as f32 } else { 0.0 }).text(crate::tlf!(
                "{done} of {total} rows",
                done = done,
                total = total
            )));
            set_name(ui, &bar, crate::tlf!("Bake progress: {done} of {total} rows", done = done, total = total));
        }
        let line = ui.label(RichText::new(&app.bake.state).color(theme::TEXT));
        ui.ctx().accesskit_node_builder(line.id, |n| n.set_live(egui::accesskit::Live::Polite));
        for f in app.bake.files.iter().take(8) {
            ui.label(RichText::new(f.display().to_string()).color(theme::TEXT_DIM).small());
        }
        let layer = ui.layer_id();
        app.bake.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
    });
    if !open {
        app.bake.open = false;
    }
}
