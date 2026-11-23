//! The Picture Viewer: the pictures of past renders, with a histogram and a comparison.
//!
//! Finished renders of the Render View and the frame queue are kept in a history bounded by memory
//! ([`modelcraft_render::output::History`]). The viewer shows one, draws its histogram and compares
//! it with another by a split, by the difference, or in numbers. Everything is reachable from the
//! keyboard: the list takes Up and Down, and the comparison is chosen from named controls.

use egui::{Key, RichText};
use modelcraft_render::output::{self, BINS, Histogram, History, Picture};

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The selected picture alone.
    Single,
    /// The selected picture left of a line, the one it is compared with right of it.
    Split,
    /// How the two differ.
    Difference,
}

impl Mode {
    fn label(self) -> String {
        match self {
            Mode::Single => tl!("Picture"),
            Mode::Split => tl!("Split"),
            Mode::Difference => tl!("Difference"),
        }
    }
}

pub struct PictureViewer {
    pub open: bool,
    pub history: History,
    pub selected: usize,
    /// The picture the selected one is compared with.
    pub other: Option<usize>,
    pub mode: Mode,
    /// Where the split line is, 0 to 1 across the picture.
    pub split: f32,
    /// Multiplier of the difference image.
    pub gain: f32,
    texture: Option<(egui::TextureHandle, u64)>,
    focus_pending: bool,
    /// Bumped whenever the history changes, so the shown texture is rebuilt.
    revision: u64,
    had_focus: bool,
}

impl Default for PictureViewer {
    fn default() -> Self {
        PictureViewer {
            open: false,
            history: History::default(),
            selected: 0,
            other: None,
            mode: Mode::Single,
            split: 0.5,
            gain: 8.0,
            texture: None,
            focus_pending: false,
            revision: 0,
            had_focus: false,
        }
    }
}

impl PictureViewer {
    /// Keep a finished picture and select it.
    pub fn add(&mut self, picture: Picture) {
        if self.history.push(picture) {
            self.selected = self.history.len().saturating_sub(1);
            self.other = self.other.filter(|o| *o < self.history.len() && *o != self.selected);
            self.revision += 1;
        }
    }
}

pub fn toggle(app: &mut ModelApp) {
    app.pictures.open = !app.pictures.open;
    app.pictures.focus_pending = app.pictures.open;
}

/// Esc closes the window when focus is inside it.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    let v = &mut app.pictures;
    if v.open && v.had_focus && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        v.open = false;
        v.had_focus = false;
        return true;
    }
    false
}

/// The pixels the viewer shows for the current mode.
fn shown(v: &PictureViewer) -> Option<(Vec<u8>, [u32; 2])> {
    let a = v.history.get(v.selected)?;
    let b = v.other.and_then(|o| v.history.get(o));
    match (v.mode, b) {
        (Mode::Split, Some(b)) => output::split(a, b, v.split).map(|p| (p, a.size)),
        (Mode::Difference, Some(b)) => output::difference_image(a, b, v.gain).map(|p| (p, a.size)),
        _ => Some((a.pixels.clone(), a.size)),
    }
}

fn draw_histogram(ui: &mut egui::Ui, h: &Histogram) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(320.0), 64.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(24, 26, 29));
    let peak = h.peak().max(1) as f32;
    for (bins, color) in
        [(&h.red, egui::Color32::from_rgb(230, 80, 80)), (&h.green, egui::Color32::from_rgb(90, 200, 100)), (&h.blue, egui::Color32::from_rgb(90, 140, 240))]
    {
        let pts: Vec<egui::Pos2> = bins
            .iter()
            .enumerate()
            .map(|(i, n)| egui::pos2(rect.left() + rect.width() * (i as f32 + 0.5) / BINS as f32, rect.bottom() - rect.height() * (*n as f32 / peak)))
            .collect();
        p.add(egui::Shape::line(pts, egui::Stroke::new(1.2, color.gamma_multiply(0.9))));
    }
    // Words for those who cannot see the drawing: how the pixels spread over dark, middle, light.
    let third = BINS / 3;
    let share = |a: usize, b: usize| (h.luma[a..b].iter().sum::<u32>() as f32 * 100.0 / h.pixels.max(1) as f32).round() as u32;
    let text = crate::tlf!(
        "Histogram: {dark} percent of pixels dark, {mid} percent middle, {light} percent light",
        dark = share(0, third),
        mid = share(third, 2 * third),
        light = share(2 * third, BINS)
    );
    ui.ctx().accesskit_node_builder(resp.id, |n| {
        n.set_role(egui::accesskit::Role::Image);
        n.set_label(text);
    });
}

fn label_of(p: &Picture) -> String {
    crate::tlf!("{label}: {width} by {height}, {samples} samples", label = p.label, width = p.size[0], height = p.size[1], samples = p.samples)
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.pictures.open {
        return;
    }
    let mut open = true;
    let mut save = None;
    egui::Window::new(tl!("Picture Viewer")).id(egui::Id::unique("picture-viewer")).open(&mut open).collapsible(false).default_size([760.0, 520.0]).show(
        ctx,
        |ui| {
            let v = &mut app.pictures;
            if v.history.is_empty() {
                let r = ui.label(RichText::new(tl!("No pictures yet. Finished renders appear here.")).color(theme::TEXT_DIM));
                ui.ctx().accesskit_node_builder(r.id, |n| n.set_live(egui::accesskit::Live::Polite));
            }
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(210.0);
                    ui.label(RichText::new(tl!("History")).strong());
                    egui::ScrollArea::vertical().max_height(220.0).id_salt("picture-history").show(ui, |ui| {
                        let mut pick = None;
                        for (i, p) in v.history.iter().enumerate() {
                            let name = label_of(p);
                            let r = ui.selectable_label(v.selected == i, &p.label);
                            set_name(ui, &r, name);
                            if std::mem::take(&mut v.focus_pending) && v.selected == i {
                                r.request_focus();
                            }
                            if r.clicked() {
                                pick = Some(i);
                            }
                        }
                        if let Some(i) = pick {
                            v.selected = i;
                            if v.other == Some(i) {
                                v.other = None;
                            }
                            v.revision += 1;
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let have = v.history.get(v.selected).is_some();
                        if ui.add_enabled(have, egui::Button::new(tl!("Remove"))).clicked() {
                            v.history.remove(v.selected);
                            v.selected = v.selected.min(v.history.len().saturating_sub(1));
                            v.other = None;
                            v.revision += 1;
                        }
                        if ui.add_enabled(!v.history.is_empty(), egui::Button::new(tl!("Clear History"))).clicked() {
                            v.history.clear();
                            v.selected = 0;
                            v.other = None;
                            v.revision += 1;
                        }
                        if ui.add_enabled(have, egui::Button::new(tl!("Save Picture…"))).clicked() {
                            save = rfd::FileDialog::new().add_filter("PNG", &["png"]).set_file_name("picture.png").save_file();
                        }
                    });
                    ui.label(
                        RichText::new(crate::tlf!("{count} pictures, {mb} MB kept", count = v.history.len(), mb = v.history.bytes() >> 20))
                            .color(theme::TEXT_DIM)
                            .small(),
                    );
                    if let Some(p) = v.history.get(v.selected) {
                        draw_histogram(ui, &Histogram::of(&p.pixels));
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    // The comparison: with which picture, and how it is shown.
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tl!("Compare with"));
                        let shown_name = v.other.and_then(|o| v.history.get(o)).map_or_else(|| tl!("nothing"), |p| p.label.clone());
                        let r = egui::ComboBox::from_id_salt("picture-compare").selected_text(shown_name).show_ui(ui, |ui| {
                            if ui.selectable_label(v.other.is_none(), tl!("nothing")).clicked() {
                                v.other = None;
                                v.mode = Mode::Single;
                                v.revision += 1;
                            }
                            for (i, p) in v.history.iter().enumerate() {
                                if i != v.selected && ui.selectable_label(v.other == Some(i), &p.label).clicked() {
                                    v.other = Some(i);
                                    v.mode = if v.mode == Mode::Single { Mode::Split } else { v.mode };
                                    v.revision += 1;
                                }
                            }
                        });
                        set_name(ui, &r.response, tl!("Compare with"));
                        if v.other.is_some() {
                            for m in [Mode::Single, Mode::Split, Mode::Difference] {
                                if ui.selectable_label(v.mode == m, m.label()).clicked() {
                                    v.mode = m;
                                    v.revision += 1;
                                }
                            }
                        }
                    });
                    if v.other.is_some() && v.mode == Mode::Split {
                        ui.horizontal(|ui| {
                            ui.label(tl!("Split position"));
                            let r = ui.add(egui::Slider::new(&mut v.split, 0.0..=1.0).show_value(true));
                            set_name(ui, &r, tl!("Split position"));
                            if r.changed() {
                                v.revision += 1;
                            }
                        });
                    }
                    if v.other.is_some() && v.mode == Mode::Difference {
                        ui.horizontal(|ui| {
                            ui.label(tl!("Difference gain"));
                            let r = ui.add(egui::DragValue::new(&mut v.gain).range(1.0..=100.0).speed(0.1));
                            set_name(ui, &r, tl!("Difference gain"));
                            if r.changed() {
                                v.revision += 1;
                            }
                        });
                    }
                    if let (Some(a), Some(b)) = (v.history.get(v.selected), v.other.and_then(|o| v.history.get(o))) {
                        let text = match output::difference(a, b) {
                            Some(d) => crate::tlf!(
                                "Mean difference {mean} percent, largest {max} percent, {changed} percent of pixels differ",
                                mean = format!("{:.2}", d.mean * 100.0),
                                max = format!("{:.1}", d.max * 100.0),
                                changed = format!("{:.1}", d.changed * 100.0)
                            ),
                            None => tl!("The pictures differ in size, so they cannot be compared."),
                        };
                        let r = ui.label(RichText::new(text).color(theme::TEXT));
                        ui.ctx().accesskit_node_builder(r.id, |n| n.set_live(egui::accesskit::Live::Polite));
                    }
                    // The picture.
                    if let Some((pixels, size)) = shown(v) {
                        let stale = v.texture.as_ref().is_none_or(|(_, r)| *r != v.revision);
                        if stale {
                            let img = egui::ColorImage::from_rgba_unmultiplied([size[0] as usize, size[1] as usize], &pixels);
                            v.texture = Some((ui.ctx().load_texture("picture-viewer", img, egui::TextureOptions::LINEAR), v.revision));
                        }
                        if let Some((tex, _)) = &v.texture {
                            let full = tex.size_vec2();
                            let avail = ui.available_size().max(egui::vec2(64.0, 64.0));
                            let scale = (avail.x / full.x).min(avail.y / full.y).min(1.0);
                            let r = ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), full * scale)));
                            let name =
                                v.history.get(v.selected).map(|p| crate::tlf!("Picture shown: {description}", description = label_of(p))).unwrap_or_default();
                            ui.ctx().accesskit_node_builder(r.id, |n| {
                                n.set_role(egui::accesskit::Role::Image);
                                n.set_label(name);
                            });
                        }
                    }
                });
            });
            let layer = ui.layer_id();
            v.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
        },
    );
    if let Some(path) = save
        && let Some(p) = app.pictures.history.get(app.pictures.selected)
    {
        let img = egui::ColorImage::from_rgba_unmultiplied([p.size[0] as usize, p.size[1] as usize], &p.pixels);
        app.status = match crate::viewport::save_png(&path, &img) {
            Ok(()) => crate::tlf!("Saved {file}", file = path.display()),
            Err(e) => crate::tlf!("Save failed: {error}", error = e),
        };
    }
    if !open {
        app.pictures.open = false;
    }
}
