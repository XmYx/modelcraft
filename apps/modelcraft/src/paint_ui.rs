//! Vertex colour painting in the Paint workspace: a colour (red, green, blue and alpha fields, a
//! hex field and swatches), a brush (radius, strength, falloff, paint or erase) and fills.
//!
//! A brush stroke is one history step (`PaintStroke`) holding the points it touched, so it
//! replays exactly and undoes as one step. While the pointer is down the stroke is previewed
//! through a temporary step. Without the pointer, *Fill Selection* and *Fill All* set colours
//! from the keyboard (`FillColor` steps). Colours are shown in sRGB here and kept linear in the
//! mesh (what glTF `COLOR_0` and the viewport shader use).

use egui::{Color32, Pos2, Rect, Stroke};
use glam::Vec3;
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene, Step, StepOp, pick};
use modelcraft_engine::sculpt::brush::Falloff;

use crate::app::{ModelApp, SelectMode, Workspace};
use crate::panels::{drag, set_name};
use crate::theme;
use crate::viewport::{to_screen, world_ray};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Switch the brush on or off.
    Brush,
    FillSelection,
    FillAll,
    RadiusUp,
    RadiusDown,
}

pub const ALL: [Op; 5] = [Op::Brush, Op::FillSelection, Op::FillAll, Op::RadiusUp, Op::RadiusDown];

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::Brush => tl!("Vertex Paint Brush"),
            Op::FillSelection => tl!("Fill Selection with Colour"),
            Op::FillAll => tl!("Fill All with Colour"),
            Op::RadiusUp => tl!("Paint Radius Bigger"),
            Op::RadiusDown => tl!("Paint Radius Smaller"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::Brush => "paint.brush",
            Op::FillSelection => "paint.fill_selection",
            Op::FillAll => "paint.fill_all",
            Op::RadiusUp => "paint.radius_up",
            Op::RadiusDown => "paint.radius_down",
        }
    }
    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            Op::Brush => "Paint: B",
            Op::FillSelection => "Paint: Alt+F",
            Op::FillAll => "Paint: Shift+F",
            Op::RadiusUp => "Paint: Alt+]",
            Op::RadiusDown => "Paint: Alt+[",
        })
    }
}

/// A stroke in progress.
pub struct Active {
    object: ObjectId,
    before: Box<Scene>,
    /// The points touched, in the mesh's space.
    points: Vec<Vec3>,
}

/// Swatches offered next to the colour fields (sRGB).
pub const SWATCHES: [[u8; 3]; 8] = [
    [0xE5, 0x5B, 0x5B],
    [0xE8, 0x8A, 0x3C],
    [0xF2, 0xD0, 0x4E],
    [0x7C, 0xD0, 0x5C],
    [0x58, 0x8E, 0xF0],
    [0x8B, 0x5C, 0xE0],
    [0x20, 0x20, 0x24],
    [0xF5, 0xF5, 0xF5],
];

pub struct State {
    /// sRGB, 0 to 255 (red, green, blue, alpha).
    pub rgba: [u8; 4],
    pub radius: f32,
    pub strength: f32,
    pub erase: bool,
    pub falloff: Falloff,
    pub brush: bool,
    pub stroke: Option<Active>,
    /// The hex field's text while it is being edited.
    pub hex: String,
}

impl Default for State {
    fn default() -> Self {
        State { rgba: [0xE5, 0x5B, 0x5B, 0xFF], radius: 0.4, strength: 0.6, erase: false, falloff: Falloff::Smooth, brush: false, stroke: None, hex: String::new() }
    }
}

fn srgb_to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

impl State {
    /// The colour as the mesh keeps it: linear RGBA.
    pub fn linear(&self) -> [f32; 4] {
        [srgb_to_linear(self.rgba[0]), srgb_to_linear(self.rgba[1]), srgb_to_linear(self.rgba[2]), f32::from(self.rgba[3]) / 255.0]
    }
    pub fn hex_text(&self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.rgba[0], self.rgba[1], self.rgba[2])
    }
}

/// Read `#RRGGBB` or `RRGGBB`.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(t.get(i..i + 2)?, 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

const MIN_RADIUS: f32 = 0.001;
const MAX_RADIUS: f32 = 1e5;

fn painting(app: &ModelApp) -> bool {
    app.workspace == Workspace::Paint
}

fn target(app: &ModelApp) -> Option<ObjectId> {
    let id = app.edit_target()?;
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })).then_some(id)
}

/// Run a command of this module.
pub fn run(app: &mut ModelApp, op: Op) {
    match op {
        Op::Brush => {
            if target(app).is_none() {
                app.status = tl!("Select a mesh to paint");
                return;
            }
            app.paint.brush = !app.paint.brush;
            app.status = if app.paint.brush { tl!("Paint brush on: drag over the mesh") } else { tl!("Paint brush off") };
        }
        Op::RadiusUp | Op::RadiusDown => {
            let r = if op == Op::RadiusUp { app.paint.radius * 1.25 } else { app.paint.radius / 1.25 };
            app.paint.radius = r.clamp(MIN_RADIUS, MAX_RADIUS);
            app.status = crate::tlf!("Paint radius {radius}", radius = format!("{:.3}", app.paint.radius));
        }
        Op::FillAll | Op::FillSelection => {
            if target(app).is_none() {
                app.status = tl!("Select a mesh to paint");
                return;
            }
            let verts = if op == Op::FillSelection {
                if app.mode == SelectMode::Object || app.components.is_empty() {
                    app.status = tl!("Select vertices, edges or faces to fill (component mode)");
                    return;
                }
                app.component_vertices()
            } else {
                Vec::new()
            };
            let color = app.paint.linear();
            app.add_step(StepOp::FillColor { color, verts });
        }
    }
}

/// Take the pointer while the brush is on in the Paint workspace.
pub fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, view: usize, rect: Rect) -> bool {
    if !painting(app) || !app.paint.brush {
        return false;
    }
    let Some(id) = target(app) else { return false };
    if resp.clicked_by(egui::PointerButton::Secondary) {
        end_stroke(app, false);
        app.paint.brush = false;
        app.status = tl!("Paint brush off");
        return true;
    }
    let Some(cam) = app.views.get(view).map(|v| v.camera) else { return false };
    let down = resp.dragged_by(egui::PointerButton::Primary) || resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked_by(egui::PointerButton::Primary);
    if down && let Some(at) = resp.interact_pointer_pos() {
        let mesh = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
        let w = app.doc.scene.world_matrix(id);
        let ray = world_ray(&cam, rect, at).transformed(w.inverse());
        if let Some(hit) = pick::pick_face(&mesh, &ray) {
            if app.paint.stroke.is_none() {
                // The scene to go back to when the stroke ends: the preview step is not kept.
                let base = Box::new(app.doc.scene.clone());
                app.paint.stroke = Some(Active { object: id, before: base, points: Vec::new() });
            }
            let spacing = (app.paint.radius * 0.25).max(1e-6);
            if let Some(s) = app.paint.stroke.as_mut()
                && s.points.last().is_none_or(|l| l.distance(hit.point) >= spacing)
                && s.points.len() < modelcraft_engine::ops::paint::MAX_STAMPS
            {
                s.points.push(hit.point);
                preview(app);
            }
        }
    }
    if resp.drag_stopped() || resp.clicked_by(egui::PointerButton::Primary) {
        end_stroke(app, true);
    }
    // The brush: a ring the size of the stamp at the pointer.
    if let Some(at) = resp.hover_pos() {
        let ring = ring_radius(app, &cam, rect, id, at);
        ui.painter_at(rect).circle_stroke(at, ring, Stroke::new(1.5, Color32::from_rgb(app.paint.rgba[0], app.paint.rgba[1], app.paint.rgba[2])));
        ui.painter_at(rect).circle_stroke(at, ring + 1.5, Stroke::new(1.0, theme::TEXT_DIM));
    }
    true
}

/// How many points on screen the brush radius is, at the surface under the pointer.
fn ring_radius(app: &mut ModelApp, cam: &modelcraft_render::Camera, rect: Rect, id: ObjectId, at: Pos2) -> f32 {
    let mesh = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    let w = app.doc.scene.world_matrix(id);
    let ray = world_ray(cam, rect, at).transformed(w.inverse());
    let centre = pick::pick_face(&mesh, &ray).map(|h| w.transform_point3(h.point)).unwrap_or(cam.target);
    // One radius to the side of the camera's view.
    let right = (cam.target - cam.eye()).cross(Vec3::Y).normalize_or_zero();
    let scale = w.transform_vector3(Vec3::X).length().max(1e-9);
    match (to_screen(cam, rect, centre), to_screen(cam, rect, centre + right * app.paint.radius * scale)) {
        (Some(a), Some(b)) => a.distance(b).clamp(3.0, 600.0),
        _ => 12.0,
    }
}

fn stroke_op(app: &ModelApp, points: Vec<Vec3>) -> StepOp {
    StepOp::PaintStroke {
        points,
        radius: app.paint.radius,
        strength: app.paint.strength,
        color: app.paint.linear(),
        erase: app.paint.erase,
        falloff: app.paint.falloff,
    }
}

/// Show the stroke so far: the scene as it was plus the stroke as one more step.
fn preview(app: &mut ModelApp) {
    let Some(s) = &app.paint.stroke else { return };
    let (object, mut scene, points) = (s.object, (*s.before).clone(), s.points.clone());
    let op = stroke_op(app, points);
    if let Ok(o) = scene.edit(object)
        && let ObjectKind::Mesh { steps, .. } = &mut o.kind
    {
        steps.push(Step { id: u64::MAX, enabled: true, op });
    }
    app.doc.scene = scene;
}

/// End the stroke: the preview is taken back and the stroke becomes one history step.
pub fn end_stroke(app: &mut ModelApp, keep: bool) {
    let Some(s) = app.paint.stroke.take() else { return };
    app.doc.scene = *s.before;
    if keep && !s.points.is_empty() {
        let op = stroke_op(app, s.points);
        app.add_step(op);
    }
}

/// Escape takes back a stroke being drawn.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if app.paint.stroke.is_none() || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    end_stroke(app, false);
    true
}

/// The Paint pane.
pub fn pane(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut p = std::mem::take(&mut app.paint);
    ui.label(egui::RichText::new(tl!("Vertex colour")).strong());
    let preview_color = Color32::from_rgba_unmultiplied(p.rgba[0], p.rgba[1], p.rgba[2], p.rgba[3]);
    let (swatch, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(220.0), 18.0), egui::Sense::hover());
    ui.painter().rect_filled(swatch, 3.0, preview_color);
    ui.painter().rect_stroke(swatch, 3.0, Stroke::new(1.0, theme::TEXT_DIM), egui::StrokeKind::Inside);
    ui.horizontal_wrapped(|ui| {
        for (i, name) in [tl!("Red"), tl!("Green"), tl!("Blue"), tl!("Alpha")].into_iter().enumerate() {
            ui.label(name.clone());
            let mut v = u32::from(p.rgba[i]);
            let r = ui.add(egui::DragValue::new(&mut v).range(0..=255));
            set_name(ui, &r, name);
            p.rgba[i] = v.min(255) as u8;
        }
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Hex"));
        if p.hex.is_empty() {
            p.hex = p.hex_text();
        }
        let r = ui.add(egui::TextEdit::singleline(&mut p.hex).desired_width(80.0));
        set_name(ui, &r, tl!("Hex colour"));
        if r.lost_focus() {
            if let Some([a, b, c]) = parse_hex(&p.hex) {
                (p.rgba[0], p.rgba[1], p.rgba[2]) = (a, b, c);
            }
            p.hex = p.hex_text();
        }
    });
    ui.horizontal_wrapped(|ui| {
        for [r8, g8, b8] in SWATCHES {
            let text = format!("#{r8:02X}{g8:02X}{b8:02X}");
            let b = ui.add(egui::Button::new("      ").fill(Color32::from_rgb(r8, g8, b8)).min_size(egui::vec2(22.0, 20.0)));
            set_name(ui, &b, crate::tlf!("Swatch {hex}", hex = text));
            if b.clicked() {
                (p.rgba[0], p.rgba[1], p.rgba[2]) = (r8, g8, b8);
                p.hex = text;
            }
        }
    });
    ui.separator();
    ui.horizontal(|ui| {
        drag(ui, &mut p.radius, 0.01, MIN_RADIUS..=MAX_RADIUS, tl!("Paint radius"));
    });
    ui.horizontal(|ui| {
        drag(ui, &mut p.strength, 0.01, 0.0..=1.0, tl!("Paint strength"));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Falloff"));
        for (f, name) in [(Falloff::Smooth, tl!("Smooth")), (Falloff::Linear, tl!("Linear")), (Falloff::Sharp, tl!("Sharp")), (Falloff::Constant, tl!("Constant"))] {
            let r = ui.radio_value(&mut p.falloff, f, name.clone());
            set_name(ui, &r, crate::tlf!("Paint falloff: {shape}", shape = name));
        }
    });
    ui.horizontal(|ui| {
        let r = ui.radio_value(&mut p.erase, false, tl!("Paint"));
        set_name(ui, &r, tl!("Paint mode: Paint"));
        let r = ui.radio_value(&mut p.erase, true, tl!("Erase"));
        set_name(ui, &r, tl!("Paint mode: Erase"));
    });
    p.radius = if p.radius.is_finite() { p.radius.clamp(MIN_RADIUS, MAX_RADIUS) } else { 0.4 };
    let mut brush = p.brush;
    let r = ui.checkbox(&mut brush, tl!("Paint with the brush"));
    set_name(ui, &r, tl!("Paint with the brush"));
    app.paint = p;
    if brush != app.paint.brush {
        run(app, Op::Brush);
    }
    ui.horizontal_wrapped(|ui| {
        for op in [Op::FillSelection, Op::FillAll] {
            if ui.button(op.label()).clicked() {
                run(app, op);
            }
        }
    });
    ui.label(egui::RichText::new(tl!("Drag over the mesh with the brush, or fill from the keyboard. Each stroke is one undo step; the colours go into glTF exports and show tinted in the viewport.")).color(theme::TEXT_DIM).small());
    ui.separator();
    ui.checkbox(&mut app.show_patches, tl!("Show Patches"));
    if ui.button(tl!("Export GLB for TextureCraft…")).clicked() {
        app.act(&ctx, crate::actions::Action::ExportGlb);
    }
}
