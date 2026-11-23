//! The slim header of each view: View, Display, Options and Camera menus, and an optional
//! statistics overlay with a grid-spacing readout.
//!
//! Every setting here belongs to its own view ([`ViewOptions`] lives in [`View`]); a view that has
//! not been changed follows the app-wide setting (View menu, Organize ▸ Display Mode, Preferences),
//! so a header only ever changes the view it is in. The menus are keyboard reachable like any
//! menu; each button's name starts with its label and says which view it belongs to.

use egui::{Align, Color32, FontId, Id, Layout, Rect, RichText, Sense, Ui, UiBuilder, vec2};
use modelcraft_engine::scene::DisplayMode;
use modelcraft_render::ViewPreset;

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

/// Height of the header strip.
pub const HEADER_H: f32 = 22.0;

/// Spacing of the ground grid in metres (the grid has 20 cells each way).
pub const GRID_SPACING_M: f32 = 0.5;

/// Most objects the statistics add up when nothing is selected.
const MAX_STAT_OBJECTS: usize = 4000;

/// What one view overrides. `None` follows the app-wide setting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewOptions {
    pub display: Option<DisplayMode>,
    pub grid: Option<bool>,
    pub axes: Option<bool>,
    pub patches: Option<bool>,
    /// Show the statistics overlay.
    pub stats: bool,
}

pub fn display_of(app: &ModelApp, i: usize) -> DisplayMode {
    app.views.get(i).and_then(|v| v.options.display).unwrap_or(app.display_mode)
}
pub fn grid_of(app: &ModelApp, i: usize) -> bool {
    app.views.get(i).and_then(|v| v.options.grid).unwrap_or(app.show_grid)
}
pub fn axes_of(app: &ModelApp, i: usize) -> bool {
    app.views.get(i).and_then(|v| v.options.axes).unwrap_or(app.show_axes)
}
pub fn patches_of(app: &ModelApp, i: usize) -> bool {
    app.views.get(i).and_then(|v| v.options.patches).unwrap_or(app.show_patches)
}

fn preset_label(p: ViewPreset) -> String {
    match p {
        ViewPreset::Perspective => tl!("Perspective"),
        ViewPreset::Front => tl!("Front"),
        ViewPreset::Right => tl!("Right"),
        ViewPreset::Top => tl!("Top"),
    }
}

/// Draw the header of view `i` over the top of `rect`.
pub fn header(app: &mut ModelApp, ui: &mut Ui, i: usize, rect: Rect) {
    if i >= app.views.len() {
        return;
    }
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_H));
    ui.painter().rect_filled(bar, 0.0, Color32::from_rgba_unmultiplied(12, 13, 15, 150));
    let view = preset_label(app.views[i].preset);
    let active = app.active_view == i;
    ui.scope_builder(UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        ui.spacing_mut().button_padding = vec2(6.0, 1.0);
        let menu = |ui: &mut Ui, app: &mut ModelApp, label: String, body: &dyn Fn(&mut Ui, &mut ModelApp)| {
            let r = ui.menu_button(label.clone(), |ui| body(ui, app));
            set_name(ui, &r.response, crate::tlf!("{menu} menu of the {view} view", menu = label, view = view));
        };
        menu(ui, app, tl!("View"), &|ui, app| view_menu(ui, app, i));
        menu(ui, app, tl!("Display"), &|ui, app| display_menu(ui, app, i));
        menu(ui, app, tl!("Options"), &|ui, app| options_menu(ui, app, i));
        menu(ui, app, tl!("Camera"), &|ui, app| camera_menu(ui, app, i));
        ui.add_space(6.0);
        ui.label(RichText::new(view.clone()).color(if active { theme::ACCENT } else { theme::TEXT_DIM }).size(12.0));
    });
}

fn view_menu(ui: &mut Ui, app: &mut ModelApp, i: usize) {
    for p in [ViewPreset::Perspective, ViewPreset::Front, ViewPreset::Right, ViewPreset::Top] {
        if ui.radio(app.views[i].preset == p, preset_label(p)).clicked() {
            app.set_view_of(i, p);
            app.active_view = i;
            ui.close();
        }
    }
    ui.separator();
    if ui.button(tl!("Frame Selection")).clicked() {
        app.active_view = i;
        app.frame_selection();
        ui.close();
    }
}

fn display_menu(ui: &mut Ui, app: &mut ModelApp, i: usize) {
    let current = app.views[i].options.display;
    if ui.radio(current.is_none(), tl!("Follow the Scene Setting")).clicked() {
        app.views[i].options.display = None;
        ui.close();
    }
    ui.separator();
    for m in [DisplayMode::Smooth, DisplayMode::Flat, DisplayMode::Wireframe, DisplayMode::SeeThrough, DisplayMode::BoundingBox] {
        if ui.radio(current == Some(m), crate::organize_ui::mode_label(m)).clicked() {
            app.views[i].options.display = Some(m);
            ui.close();
        }
    }
    ui.separator();
    let mut patches = patches_of(app, i);
    if ui.checkbox(&mut patches, tl!("Show Patches")).changed() {
        app.views[i].options.patches = Some(patches);
    }
}

fn options_menu(ui: &mut Ui, app: &mut ModelApp, i: usize) {
    let (mut grid, mut axes) = (grid_of(app, i), axes_of(app, i));
    if ui.checkbox(&mut grid, tl!("Grid")).changed() {
        app.views[i].options.grid = Some(grid);
    }
    if ui.checkbox(&mut axes, tl!("Axes")).changed() {
        app.views[i].options.axes = Some(axes);
    }
    ui.checkbox(&mut app.views[i].options.stats, tl!("Statistics"));
    ui.separator();
    if ui.button(tl!("Reset View Options")).clicked() {
        app.views[i].options = ViewOptions::default();
        ui.close();
    }
}

fn camera_menu(ui: &mut Ui, app: &mut ModelApp, i: usize) {
    // The editor camera is the only camera until camera objects exist.
    let _ = ui.radio(true, tl!("Editor Camera"));
    ui.separator();
    if ui.button(tl!("Reset Camera")).clicked() {
        let preset = app.views[i].preset;
        app.views[i].camera = modelcraft_render::Camera::preset(preset);
        ui.close();
    }
}

/// The lines of the statistics overlay for view `i`.
pub fn stats_lines(app: &mut ModelApp, i: usize) -> Vec<String> {
    let mut out = Vec::new();
    let selected = app.active();
    let (name, points, polygons) = match selected {
        Some(id) => {
            let e = app.doc.eval.evaluate(&app.doc.scene, id);
            let name = app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
            (name, e.mesh.vertex_count(), e.mesh.face_count())
        }
        None => {
            let ids: Vec<_> = app.doc.scene.objects().iter().map(|o| o.id).filter(|id| !app.doc.scene.is_cage_input(*id)).take(MAX_STAT_OBJECTS).collect();
            let n = ids.len();
            let (mut v, mut f) = (0, 0);
            for id in ids {
                let e = app.doc.eval.evaluate(&app.doc.scene, id);
                v += e.mesh.vertex_count();
                f += e.mesh.face_count();
            }
            (crate::tlf!("{count} objects", count = n), v, f)
        }
    };
    out.push(name);
    out.push(crate::tlf!("{count} points", count = crate::panels::group_digits(points)));
    out.push(crate::tlf!("{count} polygons", count = crate::panels::group_digits(polygons)));
    if let Some(id) = selected
        && let Some(layer) = app.doc.sculpt_layer(id)
    {
        out.push(crate::tlf!("Detail level {level}", level = layer.level));
    }
    // A mesh holds a position and about a dozen bytes of connectivity per point and polygon corner.
    let bytes = points * 12 + polygons * 4 * 12;
    out.push(crate::tlf!("About {size} of mesh data", size = size_text(bytes)));
    if grid_of(app, i) && !matches!(app.views[i].preset, ViewPreset::Front | ViewPreset::Right) {
        let per = app.unit.per_metre();
        out.push(crate::tlf!("Grid spacing {value}{unit}", value = number_text(GRID_SPACING_M * per), unit = app.unit.suffix()));
    }
    out
}

fn number_text(v: f32) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() { "0".into() } else { s.to_string() }
}

fn size_text(bytes: usize) -> String {
    match bytes {
        b if b >= 10 << 20 => crate::tlf!("{n} MB", n = b >> 20),
        b if b >= 10 << 10 => crate::tlf!("{n} KB", n = b >> 10),
        b => crate::tlf!("{n} bytes", n = b),
    }
}

/// Draw the statistics overlay when the view shows it. The text is one node in the accessibility
/// tree with a polite live region, so it is read out when it changes and not on every frame.
pub fn stats_overlay(app: &mut ModelApp, ui: &mut Ui, i: usize, rect: Rect) {
    if !app.views.get(i).is_some_and(|v| v.options.stats) {
        return;
    }
    let lines = stats_lines(app, i);
    let text = lines.join("\n");
    let font = FontId::monospace(11.0);
    let origin = rect.min + vec2(8.0, HEADER_H + 4.0);
    let galley = ui.painter().layout(text.clone(), font, theme::TEXT, 260.0);
    let area = Rect::from_min_size(origin, galley.size()).expand2(vec2(6.0, 3.0));
    let p = ui.painter_at(rect);
    p.rect_filled(area, 3.0, Color32::from_rgba_unmultiplied(12, 13, 15, 170));
    p.galley(origin, galley, theme::TEXT);
    let resp = ui.interact(area, Id::unique(("view-statistics", i)), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &text));
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_live(egui::accesskit::Live::Polite));
}
