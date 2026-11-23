//! Cutting tools: the knife (drag a line across the mesh), slice by plane, connect vertices and
//! the polygon pen (click points, snapped to the mesh).
//!
//! Every tool has a way without the pointer: a by-numbers window (Slice by Plane, Knife by
//! Numbers, Polygon by Numbers) and Connect on the selected vertices. All of them add a build
//! history step (`SlicePlane`, `CutLine`, `Connect`, `AddPolygon`), so each is one undo step.
//! A tool checks its result before it adds the step: nothing is added when the cut would change
//! nothing, and the status line says why.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Stroke};
use glam::Vec3;
use modelcraft_engine::mesh::PolyMesh;
use modelcraft_engine::ops::cut::{self, PenPoint, Slice};
use modelcraft_engine::scene::{ObjectId, StepOp, pick};

use crate::app::{ModelApp, SelectMode};
use crate::panels::{set_name, vec3_row};
use crate::theme;
use crate::viewport::{to_screen, world_ray};

/// The commands of this module (menus, palette, shortcuts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Draw a line across the mesh with the pointer.
    Knife,
    KnifeNumbers,
    Slice,
    Connect,
    /// Click the corners of a new polygon.
    Pen,
    PenNumbers,
}

pub const ALL: [Op; 6] = [Op::Knife, Op::KnifeNumbers, Op::Slice, Op::Connect, Op::Pen, Op::PenNumbers];

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::Knife => tl!("Knife"),
            Op::KnifeNumbers => tl!("Knife by Numbers…"),
            Op::Slice => tl!("Slice by Plane…"),
            Op::Connect => tl!("Connect Vertices"),
            Op::Pen => tl!("Polygon Pen"),
            Op::PenNumbers => tl!("Polygon by Numbers…"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::Knife => "mesh.knife",
            Op::KnifeNumbers => "mesh.knife_numbers",
            Op::Slice => "mesh.slice_plane",
            Op::Connect => "mesh.connect",
            Op::Pen => "mesh.polygon_pen",
            Op::PenNumbers => "mesh.polygon_numbers",
        }
    }
    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            Op::Knife => "Model: C",
            Op::KnifeNumbers => "Model: Alt+C",
            Op::Slice => "Model: Shift+C",
            Op::Connect => "Model: Shift+J",
            Op::Pen => "Model: P",
            Op::PenNumbers => "Model: Shift+P",
        })
    }
}

/// The tool that has the viewport's pointer.
#[derive(Default)]
pub enum Tool {
    #[default]
    None,
    /// Dragging draws the line; `from` is where the drag began, in screen points.
    Knife { from: Option<Pos2> },
    /// Each click adds a point; Enter (or a click on the first point) makes the polygon.
    Pen { points: Vec<PenPoint>, world: Vec<Vec3> },
}

impl Tool {
    pub fn is_on(&self) -> bool {
        !matches!(self, Tool::None)
    }
}

pub struct SliceWindow {
    point: Vec3,
    normal: Vec3,
    selected_only: bool,
    focus_pending: bool,
}

pub struct KnifeWindow {
    a: Vec3,
    b: Vec3,
    view: Vec3,
    front_only: bool,
    focus_pending: bool,
}

pub struct PenWindow {
    text: String,
    focus_pending: bool,
}

#[derive(Default)]
pub struct State {
    pub tool: Tool,
    pub slice: Option<SliceWindow>,
    pub knife: Option<KnifeWindow>,
    pub pen: Option<PenWindow>,
    /// The knife also cuts faces that turn away from the view.
    pub through: bool,
}

impl State {
    /// Anything of this module open (a tool or a window).
    pub fn is_open(&self) -> bool {
        self.tool.is_on() || self.slice.is_some() || self.knife.is_some() || self.pen.is_some()
    }
}

fn mesh(app: &mut ModelApp) -> Option<(ObjectId, Arc<PolyMesh>)> {
    let id = app.edit_target()?;
    Some((id, app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh))
}

fn close_all(app: &mut ModelApp) {
    let c = &mut app.modeling.cut;
    c.tool = Tool::None;
    (c.slice, c.knife, c.pen) = (None, None, None);
}

/// Run a command of this module.
pub fn run(app: &mut ModelApp, op: Op) {
    let Some((_, m)) = mesh(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let (lo, hi) = m.bounds();
    let (centre, size) = ((lo + hi) * 0.5, (hi - lo).max_element().max(0.1));
    match op {
        Op::Connect => connect(app, &m),
        Op::Knife => {
            if matches!(app.modeling.cut.tool, Tool::Knife { .. }) {
                close_all(app);
                app.status = tl!("Tool off");
            } else {
                close_all(app);
                app.modeling.cut.tool = Tool::Knife { from: None };
                app.status = tl!("Knife: drag a line across the mesh. Esc stops.");
            }
        }
        Op::Pen => {
            if matches!(app.modeling.cut.tool, Tool::Pen { .. }) {
                close_all(app);
                app.status = tl!("Tool off");
            } else {
                close_all(app);
                app.modeling.cut.tool = Tool::Pen { points: Vec::new(), world: Vec::new() };
                app.status = tl!("Polygon pen: click the corners, Enter makes the polygon. Esc stops.");
            }
        }
        Op::Slice => {
            close_all(app);
            app.modeling.cut.slice = Some(SliceWindow { point: centre, normal: Vec3::Y, selected_only: false, focus_pending: true });
        }
        Op::KnifeNumbers => {
            close_all(app);
            let a = Vec3::new(lo.x - size * 0.1, centre.y, hi.z + size * 0.1);
            let b = Vec3::new(hi.x + size * 0.1, centre.y, hi.z + size * 0.1);
            app.modeling.cut.knife = Some(KnifeWindow { a, b, view: -Vec3::Z, front_only: !app.modeling.cut.through, focus_pending: true });
        }
        Op::PenNumbers => {
            close_all(app);
            app.modeling.cut.pen = Some(PenWindow { text: String::new(), focus_pending: true });
        }
    }
}

/// Add the step if it changes the mesh; otherwise say why not.
fn add_checked(app: &mut ModelApp, m: &PolyMesh, op: StepOp, result: Result<PolyMesh, modelcraft_engine::ops::OpError>) -> bool {
    match result {
        Ok(out) if out != *m => {
            app.add_step(op);
            true
        }
        Ok(_) => {
            app.status = tl!("Nothing to cut: the cut crosses no face");
            false
        }
        Err(e) => {
            app.status = e.to_string();
            false
        }
    }
}

fn connect(app: &mut ModelApp, m: &PolyMesh) {
    if app.mode != SelectMode::Vertex || app.components.len() < 2 {
        app.status = tl!("Connect needs two or more selected vertices (vertex mode)");
        return;
    }
    let verts: Vec<u32> = app.components.iter().copied().collect();
    let result = cut::connect(m, &verts);
    add_checked(app, m, StepOp::Connect { verts }, result);
}

fn apply_slice(app: &mut ModelApp, w: &SliceWindow) -> bool {
    let Some((_, m)) = mesh(app) else { return false };
    let faces: Vec<u32> = if w.selected_only && app.mode == SelectMode::Face { app.components.iter().copied().collect() } else { Vec::new() };
    let result = cut::slice(&m, &Slice { point: w.point, normal: w.normal, limits: Vec::new(), faces: faces.clone() });
    add_checked(app, &m, StepOp::SlicePlane { point: w.point, normal: w.normal, faces }, result)
}

fn apply_knife(app: &mut ModelApp, a: Vec3, b: Vec3, view: Vec3, front_only: bool) -> bool {
    let Some((_, m)) = mesh(app) else { return false };
    let result = cut::cut_line(&m, a, b, view, front_only);
    add_checked(app, &m, StepOp::CutLine { a, b, view, front_only }, result)
}

fn apply_pen(app: &mut ModelApp, points: Vec<PenPoint>, toward: Option<Vec3>) -> bool {
    let Some((_, m)) = mesh(app) else { return false };
    let result = cut::add_polygon(&m, &points, toward);
    add_checked(app, &m, StepOp::AddPolygon { points, toward }, result)
}

/// Read the text of the polygon window: one point per line, `v 12` for vertex 12 or `x y z` for
/// a position, in the mesh's own coordinates.
pub fn parse_points(text: &str) -> Result<Vec<PenPoint>, String> {
    let mut out = Vec::new();
    let text = text.replace(';', "\n");
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
        let bad = || crate::tlf!("Line {n}: write v 12 for vertex 12, or x y z for a point", n = n + 1);
        match words.as_slice() {
            [v, i] if v.eq_ignore_ascii_case("v") => out.push(PenPoint::Vertex { v: i.parse().map_err(|_| bad())? }),
            [x, y, z] => {
                let p = Vec3::new(x.parse().map_err(|_| bad())?, y.parse().map_err(|_| bad())?, z.parse().map_err(|_| bad())?);
                if !p.is_finite() {
                    return Err(bad());
                }
                out.push(PenPoint::Position { p });
            }
            _ => return Err(bad()),
        }
        if out.len() > cut::MAX_PEN_POINTS {
            return Err(crate::tlf!("At most {n} points", n = cut::MAX_PEN_POINTS));
        }
    }
    Ok(out)
}

/// Escape closes the tool or window that is open.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if !app.modeling.cut.is_open() || !ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        return false;
    }
    // A pen that has points loses them first; the next Escape stops the tool.
    if let Tool::Pen { points, world } = &mut app.modeling.cut.tool
        && !points.is_empty()
    {
        points.clear();
        world.clear();
        return true;
    }
    close_all(app);
    app.status = tl!("Cancelled");
    true
}

// ---------------------------------------------------------------- the pointer tools

/// How near (points on screen) a click snaps to a vertex or an edge of the mesh.
const VERTEX_REACH: f32 = 10.0;
const EDGE_REACH: f32 = 6.0;
/// Most elements searched for a snap.
const MAX_SNAP: usize = 400_000;

/// The pen point under the pointer and where it is in world space.
fn pen_point(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2, last: Option<Vec3>) -> Option<(PenPoint, Vec3)> {
    let (id, m) = mesh(app)?;
    let w = app.doc.scene.world_matrix(id);
    let cam = app.views.get(view)?.camera;
    // A vertex of the mesh.
    let mut best: Option<(f32, u32)> = None;
    for v in 0..m.vertex_count().min(MAX_SNAP) as u32 {
        if let Some(p) = to_screen(&cam, rect, w.transform_point3(m.position(v))) {
            let d = p.distance(at);
            if d <= VERTEX_REACH && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, v));
            }
        }
    }
    if let Some((_, v)) = best {
        return Some((PenPoint::Vertex { v }, w.transform_point3(m.position(v))));
    }
    // A point on an edge.
    let mut best: Option<(f32, u32, f32)> = None;
    for e in 0..m.edge_count().min(MAX_SNAP) as u32 {
        let (a, b) = m.edge_vertices(e);
        let (Some(pa), Some(pb)) = (to_screen(&cam, rect, w.transform_point3(m.position(a))), to_screen(&cam, rect, w.transform_point3(m.position(b)))) else {
            continue;
        };
        let ab = pb - pa;
        let t = if ab.length_sq() > 1e-6 { ((at - pa).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
        let d = (pa + ab * t).distance(at);
        if d <= EDGE_REACH && t > 0.02 && t < 0.98 && best.is_none_or(|(b, _, _)| d < b) {
            best = Some((d, e, t));
        }
    }
    if let Some((_, e, t)) = best {
        let (a, b) = m.edge_vertices(e);
        let p = m.position(a).lerp(m.position(b), t);
        return Some((PenPoint::Edge { a, b, t }, w.transform_point3(p)));
    }
    // The surface, else a plane facing the camera through the last point.
    let ray = world_ray(&cam, rect, at);
    let inv = w.inverse();
    let local = ray.transformed(inv);
    if let Some(h) = pick::pick_face(&m, &local) {
        return Some((PenPoint::Position { p: h.point }, w.transform_point3(h.point)));
    }
    let through = last.unwrap_or(w.transform_point3(Vec3::ZERO));
    let n = (cam.target - cam.eye()).normalize_or_zero();
    let denom = ray.dir.dot(n);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (through - ray.origin).dot(n) / denom;
    let p = ray.origin + ray.dir * t;
    Some((PenPoint::Position { p: inv.transform_point3(p) }, p))
}

/// The knife's plane through the camera and the two screen points, as the step stores it: two
/// points of the line (in the mesh's space) and the direction looked along.
fn knife_line(app: &mut ModelApp, view: usize, rect: Rect, from: Pos2, to: Pos2) -> Option<(Vec3, Vec3, Vec3)> {
    let (id, m) = mesh(app)?;
    let w = app.doc.scene.world_matrix(id);
    let cam = app.views.get(view)?.camera;
    let (ra, rb) = (world_ray(&cam, rect, from), world_ray(&cam, rect, to));
    // The depth of the mesh's centre along the middle ray.
    let (lo, hi) = m.bounds();
    let centre = w.transform_point3((lo + hi) * 0.5);
    let mid = world_ray(&cam, rect, from.lerp(to, 0.5));
    let depth = ((centre - mid.origin).dot(mid.dir) / mid.dir.length_squared().max(1e-12)).max(1e-3);
    let (a, b) = (ra.origin + ra.dir * depth, rb.origin + rb.dir * depth);
    let inv = w.inverse();
    let dir = inv.transform_vector3(mid.dir).normalize_or_zero();
    (dir != Vec3::ZERO).then(|| (inv.transform_point3(a), inv.transform_point3(b), dir))
}

/// Give the tool the pointer. Returns true when it did.
pub fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, view: usize, rect: Rect) -> bool {
    if !app.modeling.cut.tool.is_on() {
        return false;
    }
    if resp.clicked_by(egui::PointerButton::Secondary) {
        close_all(app);
        app.status = tl!("Tool off");
        return true;
    }
    if app.edit_target().is_none() {
        close_all(app);
        return false;
    }
    match &app.modeling.cut.tool {
        Tool::Knife { .. } => {
            if resp.drag_started_by(egui::PointerButton::Primary)
                && let Some(at) = resp.interact_pointer_pos()
            {
                app.modeling.cut.tool = Tool::Knife { from: Some(at - resp.drag_delta()) };
            }
            if resp.drag_stopped()
                && let Tool::Knife { from: Some(from) } = &app.modeling.cut.tool
                && let Some(to) = resp.interact_pointer_pos().or_else(|| resp.hover_pos())
            {
                let from = *from;
                app.modeling.cut.tool = Tool::Knife { from: None };
                if from.distance(to) > 4.0
                    && let Some((a, b, dir)) = knife_line(app, view, rect, from, to)
                {
                    let front_only = !app.modeling.cut.through;
                    apply_knife(app, a, b, dir, front_only);
                }
            }
        }
        Tool::Pen { points, world } => {
            let (n, last) = (points.len(), world.last().copied());
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = resp.interact_pointer_pos()
                && let Some((p, wp)) = pen_point(app, view, rect, at, last)
            {
                // A click on the first point closes the polygon.
                let closes = n >= 3 && matches!(&app.modeling.cut.tool, Tool::Pen { points, .. } if points.first() == Some(&p));
                if closes {
                    finish_pen(app, view, rect);
                } else if let Tool::Pen { points, world } = &mut app.modeling.cut.tool
                    && !points.contains(&p)
                {
                    points.push(p);
                    world.push(wp);
                }
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                finish_pen(app, view, rect);
            }
            if resp.has_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Backspace))
                && let Tool::Pen { points, world } = &mut app.modeling.cut.tool
            {
                points.pop();
                world.pop();
            }
        }
        Tool::None => {}
    }
    true
}

fn finish_pen(app: &mut ModelApp, view: usize, _rect: Rect) {
    let Tool::Pen { points, .. } = &app.modeling.cut.tool else { return };
    let points = points.clone();
    if points.len() < 3 {
        app.status = tl!("A polygon needs at least three points");
        return;
    }
    // The polygon faces the camera unless it joins the mesh along an edge.
    let toward = app.edit_target().and_then(|id| {
        let w = app.doc.scene.world_matrix(id);
        let cam = app.views.get(view)?.camera;
        let d = (cam.eye() - cam.target).normalize_or_zero();
        Some(w.inverse().transform_vector3(d).normalize_or_zero()).filter(|v| *v != Vec3::ZERO)
    });
    if apply_pen(app, points, toward)
        && let Tool::Pen { points, world } = &mut app.modeling.cut.tool
    {
        points.clear();
        world.clear();
    }
}

/// The line, the points and the ring under the pointer.
pub fn overlay(app: &ModelApp, painter: &egui::Painter, view: usize, rect: Rect, hover: Option<Pos2>) {
    let Some(cam) = app.views.get(view).map(|v| v.camera) else { return };
    match &app.modeling.cut.tool {
        Tool::Knife { from: Some(from) } => {
            if let Some(to) = hover {
                painter.line_segment([*from, to], Stroke::new(1.5, theme::ACCENT));
            }
        }
        Tool::Pen { world, .. } => {
            let pts: Vec<Pos2> = world.iter().filter_map(|p| to_screen(&cam, rect, *p)).collect();
            for w in pts.windows(2) {
                if let [a, b] = w {
                    painter.line_segment([*a, *b], Stroke::new(1.5, theme::ACCENT));
                }
            }
            if let (Some(last), Some(at)) = (pts.last(), hover) {
                painter.line_segment([*last, at], Stroke::new(1.0, theme::TEXT_DIM));
            }
            for p in &pts {
                painter.circle_filled(*p, 3.5, theme::SELECT);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- the by-numbers windows

/// Draw the windows of this module.
pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    slice_window(app, ctx);
    knife_window(app, ctx);
    pen_window(app, ctx);
}

/// What a window did this frame.
#[derive(Default)]
struct Outcome {
    apply: bool,
    cancel: bool,
}

/// The Apply and Cancel buttons, with Enter applying when it did not just finish typing.
fn buttons(ui: &mut egui::Ui, fields: &[egui::Response], enter_applies: bool) -> Outcome {
    let mut out = Outcome::default();
    ui.horizontal(|ui| {
        out.apply |= ui.button(tl!("Apply")).clicked();
        out.cancel |= ui.button(tl!("Cancel")).clicked();
    });
    let committed = fields.iter().any(|r| r.lost_focus());
    let nothing_focused = ui.ctx().memory(|m| m.focused()).is_none();
    if enter_applies && ui.input(|i| i.key_pressed(egui::Key::Enter) && (i.modifiers.shift || (nothing_focused && !committed))) {
        out.apply = true;
    }
    out
}

fn slice_window(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut w) = app.modeling.cut.slice.take() else { return };
    let mut out = Outcome::default();
    let faces_mode = app.mode == SelectMode::Face && !app.components.is_empty();
    egui::Window::new(tl!("Slice by Plane")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, [-12.0, 60.0]).show(ctx, |ui| {
        ui.label(tl!("Cuts every face the plane crosses and keeps both sides."));
        let mut rs = vec3_row(ui, &tl!("Point"), &mut w.point, 0.01);
        rs.extend(vec3_row(ui, &tl!("Normal"), &mut w.normal, 0.01));
        ui.horizontal(|ui| {
            for (name, n) in [(tl!("Plane X"), Vec3::X), (tl!("Plane Y"), Vec3::Y), (tl!("Plane Z"), Vec3::Z)] {
                let r = ui.button(name.clone());
                set_name(ui, &r, crate::tlf!("{plane}: set the normal", plane = name));
                if r.clicked() {
                    w.normal = n;
                }
            }
        });
        if faces_mode {
            let r = ui.checkbox(&mut w.selected_only, tl!("Selected faces only"));
            set_name(ui, &r, tl!("Selected faces only"));
        } else {
            w.selected_only = false;
        }
        if w.focus_pending
            && let Some(first) = rs.first()
        {
            first.request_focus();
            w.focus_pending = false;
        }
        out = buttons(ui, &rs, true);
    });
    if out.cancel {
        return;
    }
    if out.apply {
        if !apply_slice(app, &w) {
            // The status says why; the window stays so the numbers can change.
            app.modeling.cut.slice = Some(w);
        }
        return;
    }
    app.modeling.cut.slice = Some(w);
}

fn knife_window(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut w) = app.modeling.cut.knife.take() else { return };
    let mut out = Outcome::default();
    egui::Window::new(tl!("Knife by Numbers")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, [-12.0, 60.0]).show(ctx, |ui| {
        ui.label(tl!("Cuts between the two points, along the direction looked in."));
        let mut rs = vec3_row(ui, &tl!("From"), &mut w.a, 0.01);
        rs.extend(vec3_row(ui, &tl!("To"), &mut w.b, 0.01));
        rs.extend(vec3_row(ui, &tl!("View"), &mut w.view, 0.01));
        let r = ui.checkbox(&mut w.front_only, tl!("Visible faces only"));
        set_name(ui, &r, tl!("Visible faces only"));
        if w.focus_pending
            && let Some(first) = rs.first()
        {
            first.request_focus();
            w.focus_pending = false;
        }
        out = buttons(ui, &rs, true);
    });
    if out.cancel {
        return;
    }
    if out.apply {
        if apply_knife(app, w.a, w.b, w.view, w.front_only) {
            app.modeling.cut.through = !w.front_only;
        } else {
            app.modeling.cut.knife = Some(w);
        }
        return;
    }
    app.modeling.cut.knife = Some(w);
}

fn pen_window(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut w) = app.modeling.cut.pen.take() else { return };
    let mut out = Outcome::default();
    let mut error = None;
    egui::Window::new(tl!("Polygon by Numbers")).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, [-12.0, 60.0]).show(ctx, |ui| {
        ui.label(tl!("One point per line (or separated by ;): v 12 for vertex 12, or x y z for a new point."));
        let r = ui.add(egui::TextEdit::multiline(&mut w.text).desired_rows(5).desired_width(240.0));
        set_name(ui, &r, tl!("Points"));
        if w.focus_pending {
            r.request_focus();
            w.focus_pending = false;
        }
        if let Err(e) = parse_points(&w.text)
            && !w.text.trim().is_empty()
        {
            ui.colored_label(Color32::from_rgb(240, 180, 60), &e);
            error = Some(e);
        }
        // Enter adds a line here; Tab to Apply and Enter applies.
        out = buttons(ui, &[], false);
    });
    if out.cancel {
        return;
    }
    if out.apply {
        match parse_points(&w.text) {
            Ok(points) => {
                let toward = Some(Vec3::Y);
                if !apply_pen(app, points, toward) {
                    app.modeling.cut.pen = Some(w);
                }
            }
            Err(e) => {
                app.status = error.unwrap_or(e);
                app.modeling.cut.pen = Some(w);
            }
        }
        return;
    }
    app.modeling.cut.pen = Some(w);
}
