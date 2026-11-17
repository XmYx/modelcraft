//! The Render View: a progressive path-traced image of the scene from the active viewport's
//! camera, in a window that keeps refining and (with "Update while editing") restarts whenever
//! the scene or the camera changes.
//!
//! The render runs on a worker thread through the UI-free engine (`craft-render-core` on the
//! CPU, `craft-render-gpu` on a GPU); this module only starts and stops it and shows its film.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Key, RichText};
use modelcraft_render::pathtrace::{RenderSettings, Renderer, SceneBuilder};
use modelcraft_render::pathtrace_gpu::{GpuContext, GpuRenderer};
use modelcraft_render::trace::{ExportOptions, Lighting, export_builder};

use crate::app::ModelApp;
use crate::theme;

/// Where the render runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Device {
    /// A hardware GPU when there is one, otherwise the CPU.
    Auto,
    Cpu,
    Gpu,
}

impl Device {
    fn label(self) -> String {
        match self {
            Device::Auto => tl!("Automatic"),
            Device::Cpu => tl!("Processor (CPU)"),
            Device::Gpu => tl!("Graphics card (GPU)"),
        }
    }
}

/// What lights the render (the scene graph has no light objects yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Light {
    /// A soft sky and one key light sized to the scene.
    Studio,
    /// A physical sky with a sun.
    Daylight,
    /// A panorama picture of the surroundings.
    Panorama,
}

impl Light {
    fn label(self) -> String {
        match self {
            Light::Studio => tl!("Studio"),
            Light::Daylight => tl!("Daylight"),
            Light::Panorama => tl!("Panorama image"),
        }
    }
}

/// What the worker shares with the window.
#[derive(Default)]
struct Shared {
    /// sRGB pixels of the latest pass, or empty before the first one.
    pixels: Vec<u8>,
    samples: u32,
    /// Counts film updates, so the window uploads a texture only when there is a new one.
    generation: u64,
    device: String,
    seconds: f64,
    done: bool,
    error: Option<String>,
}

struct Job {
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    size: [u32; 2],
    target: u32,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct RenderView {
    pub open: bool,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub device: Device,
    /// Restart when the scene or the camera changes.
    pub live: bool,
    pub light: Light,
    /// Sun height above the horizon and compass direction, in degrees (Daylight).
    pub sun_elevation: f32,
    pub sun_azimuth: f32,
    /// The panorama picture and where it came from (Panorama); without one the studio light is used.
    pub panorama: Option<(String, modelcraft_render::pathtrace::texture::Image)>,
    /// The state line, announced to assistive technology when it changes.
    pub state: String,
    job: Option<Job>,
    /// What the running job renders: scene revisions, camera and settings.
    key: u64,
    texture: Option<(egui::TextureHandle, u64)>,
    focus_pending: bool,
    /// Whether the keyboard focus was inside the window on the previous frame.
    had_focus: bool,
}

impl Default for RenderView {
    fn default() -> Self {
        RenderView {
            open: false,
            width: 960,
            height: 540,
            samples: 256,
            device: Device::Auto,
            live: true,
            light: Light::Studio,
            sun_elevation: 40.0,
            sun_azimuth: 30.0,
            panorama: None,
            state: String::new(),
            job: None,
            key: 0,
            texture: None,
            focus_pending: false,
            had_focus: false,
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn mix(h: u64, v: u64) -> u64 {
    (h ^ v).wrapping_mul(0x0000_0100_0000_01b3).rotate_left(23)
}

/// The worker: build the scene, then add passes until the target, an error or a cancel.
fn worker(
    builder: SceneBuilder,
    settings: RenderSettings,
    target: u32,
    device: Device,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    ctx: egui::Context,
) {
    let start = Instant::now();
    let scene = Arc::new(builder.build());
    let publish = |film: &modelcraft_render::pathtrace::Film, label: &str, done: bool| {
        let mut s = lock(&shared);
        s.pixels = film.to_srgb8(0.0);
        s.samples = film.samples;
        s.generation += 1;
        s.device = label.to_string();
        s.seconds = start.elapsed().as_secs_f64();
        s.done = done;
        drop(s);
        ctx.request_repaint();
    };
    let fail = |message: String| {
        let mut s = lock(&shared);
        s.error = Some(message);
        s.done = true;
        drop(s);
        ctx.request_repaint();
    };
    // Passes start at one sample (a first image at once) and grow, so later passes spend
    // their time rendering rather than copying the film.
    let pass_size = |done: u32, most: u32| (done.max(1)).min(most).min(target - done);

    let gpu = match device {
        Device::Cpu => None,
        Device::Gpu => GpuContext::new(),
        Device::Auto => GpuContext::new().filter(|c| !c.caps.software),
    };
    if device == Device::Gpu && gpu.is_none() {
        return fail(tl!("No graphics card is available; choose the processor instead."));
    }
    if let Some(gpu) = gpu {
        let label = crate::tlf!("GPU: {name}", name = gpu.caps.adapter);
        match GpuRenderer::new(&gpu, &scene, settings) {
            Ok(mut r) => {
                let mut failed = None;
                while r.samples() < target && !cancel.load(Ordering::Relaxed) {
                    let n = pass_size(r.samples(), 64);
                    match r.render(&gpu, n).and_then(|()| r.film(&gpu)) {
                        Ok(film) => publish(&film, &label, film.samples >= target),
                        Err(e) => {
                            failed = Some(e.to_string());
                            break;
                        }
                    }
                }
                match failed {
                    None => return,
                    Some(e) if device == Device::Gpu => return fail(e),
                    Some(e) => log::warn!("render view: the GPU failed ({e}); continuing on the CPU"),
                }
            }
            Err(e) if device == Device::Gpu => return fail(e.to_string()),
            Err(e) => log::warn!("render view: the GPU cannot render this scene ({e}); using the CPU"),
        }
    }
    let label = crate::tlf!("CPU: {count} threads", count = std::thread::available_parallelism().map_or(1, usize::from));
    let mut r = match Renderer::new(scene, settings) {
        Ok(r) => r,
        Err(e) => return fail(e.to_string()),
    };
    while r.film().samples < target {
        let n = pass_size(r.film().samples, 16);
        if !r.render(n, &cancel) {
            return;
        }
        publish(r.film(), &label, r.film().samples >= target);
    }
}

impl RenderView {
    /// Whether a render is running.
    pub fn rendering(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !lock(&j.shared).done)
    }

    /// Samples in the image on screen and the number the render aims for.
    pub fn progress(&self) -> (u32, u32) {
        self.job.as_ref().map_or((0, self.samples), |j| (lock(&j.shared).samples, j.target))
    }

    pub fn stop(&mut self) {
        if let Some(j) = &self.job {
            j.cancel.store(true, Ordering::Relaxed);
            lock(&j.shared).done = true;
        }
    }

    /// The latest image as sRGB bytes with its size, if a pass has finished.
    pub fn image(&self) -> Option<(Vec<u8>, [u32; 2])> {
        let j = self.job.as_ref()?;
        let s = lock(&j.shared);
        (!s.pixels.is_empty()).then(|| (s.pixels.clone(), j.size))
    }
}

/// What a render of the current scene, camera and settings would be identified by.
fn scene_key(app: &ModelApp) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for o in app.doc.scene.objects() {
        h = mix(mix(h, o.id.0), o.rev);
    }
    let c = &app.views[app.active_view.min(3)].camera;
    for v in [c.target.x, c.target.y, c.target.z, c.yaw, c.pitch, c.distance, c.fov_y] {
        h = mix(h, u64::from(v.to_bits()));
    }
    h = mix(h, c.projection as u64);
    let v = &app.render_view;
    for n in [v.width, v.height, v.samples, v.device as u32, v.light as u32, v.sun_elevation.to_bits(), v.sun_azimuth.to_bits()] {
        h = mix(h, u64::from(n));
    }
    for b in v.panorama.iter().flat_map(|(path, _)| path.bytes()) {
        h = mix(h, u64::from(b));
    }
    h
}

/// Start (or restart) rendering the scene as it is now.
pub fn start(app: &mut ModelApp, ctx: &egui::Context) {
    app.render_view.job = None;
    app.doc.sync_sculpt();
    let camera = app.views[app.active_view.min(3)].camera;
    let lighting = match app.render_view.light {
        Light::Studio => Lighting::Studio,
        Light::Daylight => Lighting::Daylight { elevation: app.render_view.sun_elevation.to_radians(), azimuth: app.render_view.sun_azimuth.to_radians() },
        Light::Panorama => match &app.render_view.panorama {
            Some((_, image)) => Lighting::Image { image: image.clone(), rotation: app.render_view.sun_azimuth.to_radians() },
            None => Lighting::Studio,
        },
    };
    let builder = match export_builder(&app.doc.scene, &mut app.doc.eval, &camera, &ExportOptions { lighting, ..ExportOptions::default() }) {
        Ok(b) => b,
        Err(e) => {
            app.render_view.state = crate::tlf!("Render failed: {error}", error = e);
            return;
        }
    };
    let v = &mut app.render_view;
    let settings = RenderSettings { width: v.width.clamp(16, 8192), height: v.height.clamp(16, 8192), ..RenderSettings::default() };
    let target = v.samples.clamp(1, 1 << 20);
    let cancel = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (c, s, device, repaint) = (cancel.clone(), shared.clone(), v.device, ctx.clone());
    let spawned = std::thread::Builder::new().name("render-view".into()).spawn(move || worker(builder, settings, target, device, c, s, repaint));
    v.key = 0;
    match spawned {
        Ok(_) => {
            v.job = Some(Job { cancel, shared, size: [settings.width, settings.height], target });
            v.state = tl!("Rendering…");
        }
        Err(e) => v.state = crate::tlf!("Render failed: {error}", error = e),
    }
}

/// Open the window (starting a render) or close it.
pub fn toggle(app: &mut ModelApp, ctx: &egui::Context) {
    app.render_view.open = !app.render_view.open;
    if app.render_view.open {
        app.render_view.focus_pending = true;
        start(app, ctx);
        app.render_view.key = scene_key(app);
    } else {
        app.render_view.stop();
    }
}

/// Esc closes the window when the keyboard focus is on one of its controls. Called before the
/// app's own shortcuts, where Esc clears the selection; returns whether it used the key.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    let v = &mut app.render_view;
    if v.open && v.had_focus && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        v.open = false;
        v.had_focus = false;
        v.stop();
        return true;
    }
    false
}

fn save_image(app: &mut ModelApp, path: &Path) {
    let Some((pixels, [w, h])) = app.render_view.image() else { return };
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &pixels);
    app.status = match crate::viewport::save_png(path, &img) {
        Ok(()) => crate::tlf!("Saved {file}", file = path.display()),
        Err(e) => crate::tlf!("Save failed: {error}", error = e),
    };
}

fn name(ui: &egui::Ui, r: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(label));
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.render_view.open {
        return;
    }
    // Live update: restart when anything the image depends on changed.
    let key = scene_key(app);
    if app.render_view.live && app.render_view.job.is_some() && app.render_view.key != key {
        start(app, ctx);
    }
    app.render_view.key = key;

    // Take what the worker has published.
    let mut finished_now = None;
    if let Some(j) = &app.render_view.job {
        let s = lock(&j.shared);
        let stale = app.render_view.texture.as_ref().is_none_or(|(_, g)| *g != s.generation);
        if stale && !s.pixels.is_empty() {
            let img = egui::ColorImage::from_rgba_unmultiplied([j.size[0] as usize, j.size[1] as usize], &s.pixels);
            let tex = ctx.load_texture("render-view", img, egui::TextureOptions::LINEAR);
            app.render_view.texture = Some((tex, s.generation));
        }
        if let Some(e) = &s.error {
            finished_now = Some(crate::tlf!("Render failed: {error}", error = e));
        } else if s.done && s.samples >= j.target {
            finished_now = Some(crate::tlf!(
                "Render finished: {samples} samples in {seconds} s ({device})",
                samples = s.samples,
                seconds = format!("{:.1}", s.seconds),
                device = s.device
            ));
        } else if s.done {
            finished_now = Some(crate::tlf!("Render stopped at {samples} samples", samples = s.samples));
        }
    }
    if let Some(text) = finished_now
        && app.render_view.state != text
    {
        app.render_view.state = text;
    }

    let mut open = true;
    let mut save_to = None;
    egui::Window::new(tl!("Render View")).id(egui::Id::unique("render-view")).open(&mut open).collapsible(false).default_size([720.0, 520.0]).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            let render = ui.button(tl!("Render"));
            if std::mem::take(&mut app.render_view.focus_pending) {
                render.request_focus();
            }
            if render.clicked() {
                start(app, ui.ctx());
                app.render_view.key = scene_key(app);
            }
            if ui.add_enabled(app.render_view.rendering(), egui::Button::new(tl!("Stop"))).clicked() {
                app.render_view.stop();
            }
            if ui.add_enabled(app.render_view.image().is_some(), egui::Button::new(tl!("Save Image…"))).clicked() {
                save_to = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("render.png").save_file();
            }
            ui.separator();
            let v = &mut app.render_view;
            ui.label(tl!("Width"));
            let r = ui.add(egui::DragValue::new(&mut v.width).range(16..=8192));
            name(ui, &r, tl!("Width"));
            ui.label(tl!("Height"));
            let r = ui.add(egui::DragValue::new(&mut v.height).range(16..=8192));
            name(ui, &r, tl!("Height"));
            ui.label(tl!("Samples"));
            let r = ui.add(egui::DragValue::new(&mut v.samples).range(1..=65_536));
            name(ui, &r, tl!("Samples"));
            ui.label(tl!("Device"));
            let r = egui::ComboBox::from_id_salt("render-device").selected_text(v.device.label()).show_ui(ui, |ui| {
                for d in [Device::Auto, Device::Cpu, Device::Gpu] {
                    ui.selectable_value(&mut v.device, d, d.label());
                }
            });
            name(ui, &r.response, tl!("Device"));
            ui.label(tl!("Lighting"));
            let r = egui::ComboBox::from_id_salt("render-light").selected_text(v.light.label()).show_ui(ui, |ui| {
                for l in [Light::Studio, Light::Daylight, Light::Panorama] {
                    ui.selectable_value(&mut v.light, l, l.label());
                }
            });
            name(ui, &r.response, tl!("Lighting"));
            if v.light == Light::Daylight {
                ui.label(tl!("Sun height"));
                let r = ui.add(egui::DragValue::new(&mut v.sun_elevation).range(-10.0..=90.0).suffix("°"));
                name(ui, &r, tl!("Sun height"));
                ui.label(tl!("Sun direction"));
                let r = ui.add(egui::DragValue::new(&mut v.sun_azimuth).range(-360.0..=360.0).suffix("°"));
                name(ui, &r, tl!("Sun direction"));
            }
            if v.light == Light::Panorama {
                let shown = v.panorama.as_ref().map_or_else(|| tl!("none"), |(path, _)| path.clone());
                ui.label(RichText::new(shown).color(theme::TEXT_DIM));
                if ui.button(tl!("Choose Panorama…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter(tl!("Radiance HDR"), &["hdr"]).pick_file()
                {
                    let path = path.display().to_string();
                    match modelcraft_render::trace::load_panorama(&path) {
                        Ok(image) => v.panorama = Some((path, image)),
                        Err(e) => v.state = crate::tlf!("Render failed: {error}", error = e),
                    }
                }
                ui.label(tl!("Turn"));
                let r = ui.add(egui::DragValue::new(&mut v.sun_azimuth).range(-360.0..=360.0).suffix("°"));
                name(ui, &r, tl!("Turn"));
            }
            ui.checkbox(&mut v.live, tl!("Update while editing"));
        });
        let (done, target) = app.render_view.progress();
        ui.horizontal(|ui| {
            let state = ui.label(RichText::new(&app.render_view.state).color(theme::TEXT));
            ui.ctx().accesskit_node_builder(state.id, |n| n.set_live(egui::accesskit::Live::Polite));
            if app.render_view.rendering() {
                ui.label(RichText::new(crate::tlf!("{done} of {target} samples", done = done, target = target)).color(theme::TEXT_DIM));
            }
        });
        match &app.render_view.texture {
            Some((tex, _)) => {
                let size = tex.size_vec2();
                let avail = ui.available_size().max(egui::vec2(64.0, 64.0));
                let scale = (avail.x / size.x).min(avail.y / size.y).min(1.0);
                let r = ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), size * scale)));
                let label =
                    crate::tlf!("Rendered image, {width} by {height} pixels, {samples} samples", width = size.x as u32, height = size.y as u32, samples = done);
                ui.ctx().accesskit_node_builder(r.id, |n| {
                    n.set_role(egui::accesskit::Role::Image);
                    n.set_label(label);
                });
            }
            None => {
                ui.label(RichText::new(tl!("The image appears after the first pass.")).color(theme::TEXT_DIM));
            }
        }
        let layer = ui.layer_id();
        app.render_view.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
    });
    if let Some(path) = save_to {
        save_image(app, &path);
    }
    if !open {
        app.render_view.open = false;
        app.render_view.stop();
    } else if app.render_view.rendering() {
        // Keep the progress line moving even if a repaint request was missed.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Cli;
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use modelcraft_engine::Command;

    fn harness() -> Harness<'static, ModelApp> {
        let ctx = egui::Context::default();
        let mut app = ModelApp::with_context(&ctx, Cli::default());
        // Small and on the CPU, so the test is quick and the same everywhere.
        app.render_view = RenderView { width: 64, height: 48, samples: 4, device: Device::Cpu, ..RenderView::default() };
        app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap();
        Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
    }

    fn wait_until_done(h: &mut Harness<'static, ModelApp>) {
        // `step`, not `run`: a running render asks for repaints, which `run` would wait out.
        for _ in 0..1500 {
            h.step();
            if !h.state().render_view.rendering() {
                h.step();
                h.step();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the render did not finish: {}", h.state().render_view.state);
    }

    #[test]
    fn f9_opens_the_render_view_renders_and_esc_closes_it() {
        let mut h = harness();
        h.run();
        h.key_press(Key::F9);
        h.step();
        h.step();
        assert!(h.state().render_view.open);
        // Focus moves into the window, onto its first control.
        assert!(h.get_by_role_and_label(Role::Button, "Render").accesskit_node().is_focused());
        for label in ["Width", "Height", "Samples", "Device"] {
            assert!(h.get_all_by_label(label).count() >= 1, "{label} has an accessible name");
        }
        h.get_by_role_and_label(Role::CheckBox, "Update while editing");
        wait_until_done(&mut h);
        let state = h.state().render_view.state.clone();
        assert!(state.starts_with("Render finished: 4 samples"), "{state}");
        assert!(state.contains("CPU"), "{state}");
        // The state line is a live region and the image says what it is.
        let line = h.get_by_label(&state);
        assert_eq!(line.accesskit_node().live(), egui::accesskit::Live::Polite);
        let image = h.get_by_role(Role::Image);
        assert!(image.accesskit_node().label().is_some_and(|l| l.starts_with("Rendered image, 64 by 48 pixels, 4 samples")));
        let (pixels, size) = h.state().render_view.image().unwrap();
        assert_eq!((pixels.len(), size), (64 * 48 * 4, [64, 48]));
        // The cube is lit: the picture is not one flat colour.
        assert!(pixels.chunks(4).any(|p| p[0] > 120) && pixels.chunks(4).any(|p| p[0] < 100));
        // Esc, with the focus still on the Render button, closes the window.
        h.key_press(Key::Escape);
        h.step();
        assert!(!h.state().render_view.open);
    }

    #[test]
    fn editing_the_scene_restarts_a_live_render_only() {
        let mut h = harness();
        h.run();
        h.key_press(Key::F9);
        wait_until_done(&mut h);
        let id = h.state().doc.scene.objects()[0].id;
        let recolour = |h: &mut Harness<'static, ModelApp>, red: f32| {
            let material = modelcraft_engine::scene::Material { base_color: [red, 0.1, 0.1, 1.0], metallic: 0.0, roughness: 0.5, ..Default::default() };
            h.state_mut().doc.execute(Command::SetMaterial { id, material }).unwrap();
        };
        let centre = |h: &Harness<'static, ModelApp>| {
            let (pixels, _) = h.state().render_view.image().unwrap();
            let i = (24 * 64 + 32) * 4;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        let grey = centre(&h);
        recolour(&mut h, 0.9);
        h.step();
        assert!(h.state().render_view.rendering() || h.state().render_view.progress().0 == 4, "a change restarts the render");
        wait_until_done(&mut h);
        let red = centre(&h);
        assert!(red[0] > red[1].saturating_add(40) && grey[0].abs_diff(grey[1]) < 30, "grey {grey:?} then red {red:?}");
        // With live update off the image stays until Render is pressed.
        h.state_mut().render_view.live = false;
        recolour(&mut h, 0.05);
        h.step();
        h.step();
        assert!(!h.state().render_view.rendering());
        assert_eq!(centre(&h), red);
        h.get_by_role_and_label(Role::Button, "Render").click();
        h.step();
        wait_until_done(&mut h);
        assert!(centre(&h)[0] < red[0]);
        // Daylight shows its two sun fields and renders a different picture.
        let studio = centre(&h);
        assert!(h.query_by_label("Sun height").is_none());
        h.state_mut().render_view.light = Light::Daylight;
        h.step();
        assert!(h.get_all_by_label("Sun height").count() >= 1 && h.get_all_by_label("Sun direction").count() >= 1);
        h.get_by_role_and_label(Role::Button, "Render").click();
        h.step();
        wait_until_done(&mut h);
        assert_ne!(centre(&h), studio);
        assert!(h.state().render_view.state.starts_with("Render finished"), "{}", h.state().render_view.state);
        // A panorama lights the scene with its own colour; the picker is a named button.
        let image = modelcraft_render::pathtrace::texture::Image::filled(8, 4, modelcraft_render::pathtrace::texture::Kind::Color, [0.0, 0.0, 3.0, 1.0]);
        h.state_mut().render_view.panorama = Some(("blue.hdr".into(), image));
        h.state_mut().render_view.light = Light::Panorama;
        h.step();
        h.get_by_role_and_label(Role::Button, "Choose Panorama…");
        assert!(h.get_all_by_label("Turn").count() >= 1);
        h.get_by_role_and_label(Role::Button, "Render").click();
        h.step();
        wait_until_done(&mut h);
        let blue = centre(&h);
        assert!(blue[2] > blue[0].saturating_add(30), "lit by a blue panorama: {blue:?}");
        // Stop is a keyboard-reachable button and reports where it stopped.
        h.state_mut().render_view.samples = 60_000;
        h.get_by_role_and_label(Role::Button, "Render").click();
        h.step();
        h.step();
        assert!(h.state().render_view.rendering());
        h.get_by_role_and_label(Role::Button, "Stop").click();
        h.step();
        h.step();
        assert!(!h.state().render_view.rendering());
        assert!(h.state().render_view.state.starts_with("Render stopped at"), "{}", h.state().render_view.state);
    }
}
