//! The pen tool and point editing for splines.
//!
//! With a spline selected its points show in the viewport (squares), and its Bezier handles
//! (dots on lines) for the selected points. Drag a point or a handle to move it, click to select
//! (Shift adds), double-click the curve to insert a point, Delete removes the selected points.
//! The pen tool adds a point with each click and pulls out a smooth handle when the click is
//! dragged; clicking the first point of a segment with at least three points closes it, Enter
//! ends the segment, Backspace takes the last point back, Escape or a right click stops.
//!
//! Every change is a `spline edit` command, so it is undoable, journaled and scriptable. The
//! pointer is never the only way: Properties lists the points with numeric fields, buttons add,
//! insert and delete them, and a combo sets the handle mode.

use std::sync::Arc;

use egui::{Pos2, Rect, Stroke};
use glam::{Mat4, Vec3};
use modelcraft_engine::curves::{CurveKind, HandleMode, Knot, KnotRef, Side, Spline, SplineEdit};
use modelcraft_engine::scene::splines::SplineSource;
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene};
use modelcraft_engine::{Command, SplineCommand};
use modelcraft_render::{Camera, ViewPreset};

use crate::app::ModelApp;
use crate::theme;

/// How near (in points on screen) the pointer must be to pick a point or a handle.
const POINT_REACH: f32 = 9.0;
/// How near to the first point a click closes the segment.
const CLOSE_REACH: f32 = 10.0;
/// A pen press moved further than this (in points) before release pulls out a handle.
const HANDLE_DRAG: f32 = 4.0;
/// Most points whose handles are drawn when none is selected.
const HANDLES_FOR_ALL: usize = 200;
/// Most points drawn as squares (beyond that every n-th is skipped).
const MAX_MARKS: usize = 20_000;

/// What the pen is doing.
pub struct Pen {
    /// The spline being drawn (made on the first click when there is none).
    pub target: Option<ObjectId>,
    /// The segment being drawn; none: the next click starts a new one.
    pub segment: Option<usize>,
    press: Option<Press>,
}

struct Press {
    view: usize,
    at: Pos2,
}

/// A grabbed point or handle being moved: the scene as it was and where the grab began.
struct Drag {
    view: usize,
    object: ObjectId,
    what: Grab,
    base: Box<Scene>,
    /// The plane facing the camera through the grabbed point (world), and where the pointer's ray
    /// met it at the start.
    plane: (Vec3, Vec3),
    start: Vec3,
}

enum Grab {
    Knots(Vec<KnotRef>),
    Handle(KnotRef, Side),
}

/// Pointer state of the spline tools; kept in the app.
#[derive(Default)]
pub struct SplineUi {
    pub pen: Option<Pen>,
    /// The selected points of `owner`.
    pub selected: Vec<KnotRef>,
    /// The segment Properties lists.
    pub segment: usize,
    owner: Option<ObjectId>,
    drag: Option<Drag>,
}

impl SplineUi {
    pub fn pen_on(&self) -> bool {
        self.pen.is_some()
    }
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Knot(KnotRef),
    Handle(KnotRef, Side),
    /// The curve itself: segment and parameter.
    Curve(usize, f32),
}

// ---------------------------------------------------------------- helpers

pub(crate) fn spline_of(app: &mut ModelApp, id: ObjectId) -> Option<Arc<Spline>> {
    Some(app.doc.eval.spline(&app.doc.scene, id)?.spline.clone())
}

/// The selected object when it is a spline.
pub fn active_spline(app: &ModelApp) -> Option<ObjectId> {
    app.active().filter(|id| app.doc.scene.is_spline(*id))
}

/// Forget the selected points when another object is active.
fn sync_owner(app: &mut ModelApp) {
    let now = active_spline(app);
    if app.spline.owner != now {
        app.spline.owner = now;
        app.spline.selected.clear();
        app.spline.segment = 0;
        app.spline.drag = None;
    }
    // Points that no longer exist (undo, removal) drop out of the selection.
    if let Some(id) = now
        && let Some(s) = spline_of(app, id)
    {
        app.spline.selected.retain(|k| s.segments.get(k.segment).is_some_and(|g| k.knot < g.knots.len()));
        app.spline.segment = app.spline.segment.min(s.segments.len().saturating_sub(1));
    }
}

fn ray_plane(cam: &Camera, rect: Rect, at: Pos2, plane: (Vec3, Vec3)) -> Option<Vec3> {
    let ray = crate::viewport::world_ray(cam, rect, at);
    let denom = ray.dir.dot(plane.1);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane.0 - ray.origin).dot(plane.1) / denom;
    let p = ray.origin + ray.dir * t;
    (t > 0.0 && p.is_finite()).then_some(p)
}

/// The plane the pen draws on: through `anchor`, across the view's direction in a straight view
/// and the ground in the perspective view.
fn pen_plane(preset: ViewPreset, anchor: Vec3) -> (Vec3, Vec3) {
    let normal = match preset {
        ViewPreset::Front => Vec3::Z,
        ViewPreset::Right => Vec3::X,
        ViewPreset::Top | ViewPreset::Perspective => Vec3::Y,
    };
    (anchor, normal)
}

fn world_of(app: &ModelApp, id: ObjectId) -> Mat4 {
    app.doc.scene.world_matrix(id)
}

fn screen(cam: &Camera, rect: Rect, w: Mat4, p: Vec3) -> Option<Pos2> {
    crate::viewport::to_screen(cam, rect, w.transform_point3(p))
}

// ---------------------------------------------------------------- picking

/// The point, handle or curve under `at` for the active spline. Handles of selected points (or of
/// every point of a small spline) come first, then points, then the curve.
pub fn pick(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2) -> Option<Hit> {
    let id = active_spline(app)?;
    let spline = spline_of(app, id)?;
    let cam = app.views.get(view)?.camera;
    let w = world_of(app, id);
    let few = spline.knot_count() <= HANDLES_FOR_ALL;
    let mut best: Option<(f32, Hit)> = None;
    let mut consider = |d: f32, h: Hit, bias: f32| {
        if d <= POINT_REACH && best.is_none_or(|(b, _)| d - bias < b) {
            best = Some((d - bias, h));
        }
    };
    for (si, seg) in spline.segments.iter().enumerate() {
        let handles = seg.effective_handles();
        for (ki, k) in seg.knots.iter().enumerate() {
            let r = KnotRef { segment: si, knot: ki };
            if let Some(p) = screen(&cam, rect, w, k.pos) {
                consider(p.distance(at), Hit::Knot(r), 2.0);
            }
            if seg.kind == CurveKind::Bezier && (few || app.spline.selected.contains(&r)) {
                let (hin, hout) = handles.get(ki).copied().unwrap_or_default();
                for (side, off) in [(Side::In, hin), (Side::Out, hout)] {
                    if off.length_squared() > 1e-12
                        && let Some(p) = screen(&cam, rect, w, k.pos + off)
                    {
                        consider(p.distance(at), Hit::Handle(r, side), 0.0);
                    }
                }
            }
        }
    }
    if best.is_some() {
        return best.map(|(_, h)| h);
    }
    // The curve: the nearest point of any segment within reach on screen.
    let mut curve: Option<(f32, Hit)> = None;
    for (si, seg) in spline.segments.iter().enumerate() {
        let pieces = seg.pieces();
        for (pi, piece) in pieces.iter().enumerate() {
            let mut prev: Option<(Pos2, f32)> = None;
            for s in 0..=12 {
                let t = s as f32 / 12.0;
                let p = modelcraft_engine::curves::eval_piece(piece, t);
                let Some(sp) = screen(&cam, rect, w, p) else {
                    prev = None;
                    continue;
                };
                if let Some((q, tq)) = prev {
                    let ab = sp - q;
                    let f = ((at - q).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                    let d = (q + ab * f).distance(at);
                    if d <= POINT_REACH * 0.8 && curve.is_none_or(|(b, _)| d < b) {
                        curve = Some((d, Hit::Curve(si, pi as f32 + tq + (t - tq) * f)));
                    }
                }
                prev = Some((sp, t));
            }
        }
    }
    curve.map(|(_, h)| h)
}

// ---------------------------------------------------------------- selecting, deleting, inserting

pub fn select_knot(app: &mut ModelApp, k: KnotRef, add: bool) {
    let sel = &mut app.spline.selected;
    if add {
        match sel.iter().position(|s| *s == k) {
            Some(p) => {
                sel.remove(p);
            }
            None => sel.push(k),
        }
    } else {
        *sel = vec![k];
    }
    app.spline.segment = k.segment;
}

/// Delete the selected points of the active spline. Returns whether there was anything to delete
/// (so Delete falls through to deleting objects when no point is selected).
pub fn delete_points(app: &mut ModelApp) -> bool {
    sync_owner(app);
    let Some(id) = active_spline(app) else { return false };
    if app.spline.selected.is_empty() {
        return false;
    }
    let knots = std::mem::take(&mut app.spline.selected);
    app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::Remove { knots } }));
    true
}

/// Insert a point on the curve at the segment and parameter.
pub fn insert_point(app: &mut ModelApp, id: ObjectId, segment: usize, u: f32) {
    if app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::Insert { segment, u } })) {
        // The new point is the one after the piece's start.
        let knot = (u.floor() as usize + 1).min(spline_of(app, id).and_then(|s| s.segments.get(segment).map(|g| g.knots.len().saturating_sub(1))).unwrap_or(0));
        app.spline.selected = vec![KnotRef { segment, knot }];
    }
}

// ---------------------------------------------------------------- moving

fn begin_drag(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2, hit: Hit, add: bool) -> bool {
    let Some(id) = active_spline(app) else { return false };
    let Some(spline) = spline_of(app, id) else { return false };
    let cam = app.views[view].camera;
    let w = world_of(app, id);
    let (what, grab_local) = match hit {
        Hit::Knot(k) => {
            if !app.spline.selected.contains(&k) {
                select_knot(app, k, add);
            }
            let Some(p) = spline.segments.get(k.segment).and_then(|s| s.knots.get(k.knot)).map(|n| n.pos) else { return false };
            (Grab::Knots(app.spline.selected.clone()), p)
        }
        Hit::Handle(k, side) => {
            let Some(seg) = spline.segments.get(k.segment) else { return false };
            let Some(kn) = seg.knots.get(k.knot) else { return false };
            let (hin, hout) = seg.effective_handles().get(k.knot).copied().unwrap_or_default();
            (Grab::Handle(k, side), kn.pos + if side == Side::In { hin } else { hout })
        }
        Hit::Curve(..) => return false,
    };
    let origin = w.transform_point3(grab_local);
    let plane = (origin, cam.back());
    let Some(start) = ray_plane(&cam, rect, at, plane) else { return false };
    app.spline.drag = Some(Drag { view, object: id, what, base: Box::new(app.doc.scene.clone()), plane, start });
    true
}

/// The edit a drag stands for with the pointer at `at`: `None` when the pointer's ray misses the plane.
fn drag_edit(app: &ModelApp, d: &Drag, rect: Rect, at: Pos2, spline: &Spline) -> Option<SplineEdit> {
    let cam = app.views.get(d.view)?.camera;
    let now = ray_plane(&cam, rect, at, d.plane)?;
    let w = d.base.world_matrix(d.object);
    let inv = w.inverse();
    if !inv.is_finite() {
        return None;
    }
    match &d.what {
        Grab::Knots(k) => Some(SplineEdit::Translate { knots: k.clone(), by: inv.transform_vector3(now - d.start) }),
        Grab::Handle(k, side) => {
            let seg = spline.segments.get(k.segment)?;
            let kn = seg.knots.get(k.knot)?;
            let target = inv.transform_point3(now);
            Some(SplineEdit::SetHandle { at: *k, side: *side, offset: target - kn.pos })
        }
    }
}

/// Show the drag as a preview on the scene (no undo step).
fn preview_drag(app: &mut ModelApp, rect: Rect, at: Pos2) {
    let Some(d) = app.spline.drag.take() else { return };
    let base_spline = match d.base.get(d.object).map(|o| &o.kind) {
        Some(ObjectKind::Spline { source, .. }) => source.spline(),
        _ => {
            return;
        }
    };
    if let Some(edit) = drag_edit(app, &d, rect, at, &base_spline) {
        let mut moved = (*base_spline).clone();
        if moved.apply(&edit).is_ok() {
            let mut scene = (*d.base).clone();
            if let Ok(o) = scene.edit(d.object)
                && let ObjectKind::Spline { source, .. } = &mut o.kind
            {
                *source = SplineSource::Curve { spline: Arc::new(moved) };
            }
            app.doc.scene = scene;
        }
    }
    app.spline.drag = Some(d);
}

fn finish_drag(app: &mut ModelApp, rect: Rect, at: Pos2) {
    let Some(d) = app.spline.drag.take() else { return };
    let base_spline = match d.base.get(d.object).map(|o| &o.kind) {
        Some(ObjectKind::Spline { source, .. }) => source.spline(),
        _ => return,
    };
    let edit = drag_edit(app, &d, rect, at, &base_spline);
    app.doc.scene = *d.base;
    if let Some(edit) = edit
        && edit_changes(&edit)
    {
        app.run(Command::Spline(SplineCommand::Edit { id: d.object, edit }));
    }
}

fn edit_changes(e: &SplineEdit) -> bool {
    match e {
        SplineEdit::Translate { by, .. } => by.length_squared() > 1e-14,
        _ => true,
    }
}

// ---------------------------------------------------------------- the pen

/// Switch the pen on or off. The pen adds to the selected spline when it has its own points, and
/// starts a new spline otherwise.
pub fn toggle_pen(app: &mut ModelApp) {
    if app.spline.pen.take().is_some() {
        app.status = tl!("Pen off");
        return;
    }
    let target =
        active_spline(app).filter(|id| matches!(app.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Spline { source: SplineSource::Curve { .. }, .. })));
    let segment = target.and_then(|id| {
        let s = spline_of(app, id)?;
        let last = s.segments.len().checked_sub(1)?;
        (!s.segments[last].closed).then_some(last)
    });
    app.spline.pen = Some(Pen { target, segment, press: None });
    app.status = tl!("Pen: click to add points, drag to pull a handle, Enter ends the curve, click the first point to close it, Escape stops.");
}

/// Add the point a pen click made: `press` where the button went down, `release` where it came up.
/// Returns whether a point was added (or a segment closed).
pub fn pen_place(app: &mut ModelApp, view: usize, rect: Rect, press: Pos2, release: Pos2) -> bool {
    let Some(pen) = &app.spline.pen else { return false };
    let (mut target, mut segment) = (pen.target, pen.segment);
    let cam = app.views[view].camera;
    let preset = app.views[view].preset;
    // The spline to draw into: made on the first click.
    if target.is_none() {
        if !app.run(Command::Spline(SplineCommand::Add { kind: "empty".into(), parent: None, name: None })) {
            return false;
        }
        target = app.selection.first().copied();
        segment = None;
    }
    let Some(id) = target.filter(|id| app.doc.scene.is_spline(*id)) else {
        app.spline.pen = None;
        return false;
    };
    let Some(spline) = spline_of(app, id) else { return false };
    let w = world_of(app, id);
    let inv = w.inverse();
    let last =
        segment.and_then(|s| spline.segments.get(s)).and_then(|s| s.knots.last()).map_or_else(|| w.transform_point3(Vec3::ZERO), |k| w.transform_point3(k.pos));
    let plane = pen_plane(preset, last);
    let (Some(p0), Some(p1)) = (ray_plane(&cam, rect, press, plane), ray_plane(&cam, rect, release, plane)) else {
        app.status = tl!("The pen cannot reach the drawing plane from this angle; turn the view");
        return false;
    };
    // Clicking the first point of the segment closes it.
    if let Some(si) = segment
        && let Some(seg) = spline.segments.get(si)
        && seg.knots.len() >= 3
        && let Some(first) = seg.knots.first()
        && let Some(sp) = screen(&cam, rect, w, first.pos)
        && sp.distance(press) <= CLOSE_REACH
    {
        let ok = app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::SetClosed { segment: si, closed: true } }));
        if let Some(pen) = &mut app.spline.pen {
            pen.target = Some(id);
            pen.segment = None;
        }
        return ok;
    }
    let local = inv.transform_point3(p0);
    let knot = if press.distance(release) > HANDLE_DRAG {
        let h = inv.transform_vector3(p1 - p0);
        Knot { pos: local, tan_in: -h, tan_out: h, mode: HandleMode::Mirrored }
    } else {
        Knot::new(local)
    };
    let seg_index = segment.unwrap_or(spline.segments.len());
    let ok = app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::Append { segment: seg_index, knot, kind: CurveKind::Bezier } }));
    if ok {
        app.selection = vec![id];
        app.spline.selected = vec![KnotRef { segment: seg_index, knot: spline.segments.get(seg_index).map_or(0, |s| s.knots.len()) }];
        app.spline.owner = Some(id);
        if let Some(pen) = &mut app.spline.pen {
            pen.target = Some(id);
            pen.segment = Some(seg_index);
        }
    }
    ok
}

/// Take back the last point the pen added.
pub fn pen_undo_point(app: &mut ModelApp) {
    let Some(pen) = &app.spline.pen else { return };
    let (Some(id), Some(si)) = (pen.target, pen.segment) else { return };
    let Some(spline) = spline_of(app, id) else { return };
    let Some(n) = spline.segments.get(si).map(|s| s.knots.len()) else { return };
    if n == 0 {
        return;
    }
    if app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::Remove { knots: vec![KnotRef { segment: si, knot: n - 1 }] } }))
        && n == 1
        && let Some(pen) = &mut app.spline.pen
    {
        pen.segment = None;
    }
}

/// End the curve being drawn: the next click starts another segment.
pub fn pen_end_segment(app: &mut ModelApp) {
    if let Some(pen) = &mut app.spline.pen {
        pen.segment = None;
    }
}

// ---------------------------------------------------------------- input

/// The pen's keys, taken before the shortcuts so that Escape stops it instead of clearing the
/// selection: Escape stops, Enter ends the curve, Backspace takes the last point back. Returns
/// whether a key was used.
pub fn keys(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if app.spline.pen.is_none() || ctx.text_edit_focused() {
        return false;
    }
    let pressed = |k: egui::Key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    if pressed(egui::Key::Escape) {
        app.spline.pen = None;
        app.status = tl!("Pen off");
        return true;
    }
    if pressed(egui::Key::Enter) {
        pen_end_segment(app);
        app.status = tl!("Curve ended; the next click starts another");
        return true;
    }
    if pressed(egui::Key::Backspace) {
        pen_undo_point(app);
        return true;
    }
    false
}

/// Pointer and keyboard input of the spline tools. Returns `true` when they took it.
pub fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, view: usize, rect: Rect) -> bool {
    sync_owner(app);
    if app.spline.pen.is_some() {
        return pen_input(app, ui, resp, view, rect);
    }
    if active_spline(app).is_none() {
        return false;
    }
    let shift = ui.input(|i| i.modifiers.shift);
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos()
    {
        let start = at - resp.drag_delta();
        if let Some(hit) = pick(app, view, rect, start)
            && begin_drag(app, view, rect, start, hit, shift)
        {
            return true;
        }
    }
    if app.spline.drag.as_ref().is_some_and(|d| d.view == view) {
        if let Some(at) = pointer {
            if resp.drag_stopped() {
                finish_drag(app, rect, at);
            } else {
                preview_drag(app, rect, at);
            }
        }
        return true;
    }
    if resp.double_clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some(Hit::Curve(seg, u)) = pick(app, view, rect, at)
        && let Some(id) = active_spline(app)
    {
        insert_point(app, id, seg, u);
        return true;
    }
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos()
    {
        match pick(app, view, rect, at) {
            Some(Hit::Knot(k)) => {
                select_knot(app, k, shift);
                return true;
            }
            Some(Hit::Handle(..)) => return true,
            _ => {
                if !shift {
                    app.spline.selected.clear();
                }
            }
        }
    }
    false
}

fn pen_input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, view: usize, rect: Rect) -> bool {
    let (pressed, released, pos) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.interact_pos()));
    if resp.clicked_by(egui::PointerButton::Secondary) {
        app.spline.pen = None;
        app.status = tl!("Pen off");
        return true;
    }
    if pressed
        && resp.hovered()
        && let Some(at) = pos
        && let Some(pen) = &mut app.spline.pen
    {
        pen.press = Some(Press { view, at });
    }
    if released
        && let Some(release) = pos
        && let Some(pen) = &mut app.spline.pen
        && let Some(p) = pen.press.take_if(|p| p.view == view)
    {
        pen_place(app, view, rect, p.at, release);
    }
    true
}

// ---------------------------------------------------------------- overlay

/// Points, handles and the pen's rubber band over the viewport.
pub fn overlay(app: &mut ModelApp, painter: &egui::Painter, view: usize, rect: Rect) {
    let Some(id) = active_spline(app) else { return };
    let Some(spline) = spline_of(app, id) else { return };
    let Some(cam) = app.views.get(view).map(|v| v.camera) else { return };
    let w = world_of(app, id);
    let few = spline.knot_count() <= HANDLES_FOR_ALL;
    let mut drawn = 0usize;
    for (si, seg) in spline.segments.iter().enumerate() {
        let handles = seg.effective_handles();
        for (ki, k) in seg.knots.iter().enumerate() {
            let r = KnotRef { segment: si, knot: ki };
            let selected = app.spline.selected.contains(&r);
            let Some(p) = screen(&cam, rect, w, k.pos) else { continue };
            if seg.kind == CurveKind::Bezier && (selected || few) {
                let (hin, hout) = handles.get(ki).copied().unwrap_or_default();
                for off in [hin, hout] {
                    if off.length_squared() > 1e-12
                        && let Some(q) = screen(&cam, rect, w, k.pos + off)
                    {
                        let c = if selected { theme::SELECT } else { theme::TEXT_DIM };
                        painter.line_segment([p, q], Stroke::new(1.0, c.gamma_multiply(0.7)));
                        painter.circle_filled(q, 3.0, c);
                    }
                }
            }
            drawn += 1;
            if drawn > MAX_MARKS {
                continue;
            }
            let half = if selected { 4.5 } else { 3.5 };
            let r = Rect::from_center_size(p, egui::vec2(half * 2.0, half * 2.0));
            if selected {
                painter.rect_filled(r, 0.0, theme::SELECT);
            } else {
                painter.rect_filled(r, 0.0, egui::Color32::from_rgb(30, 32, 35));
                painter.rect_stroke(r, 0.0, Stroke::new(1.2, egui::Color32::from_rgb(220, 225, 230)), egui::StrokeKind::Inside);
            }
        }
        // A B-spline's control polygon: the knots are not on the curve.
        if seg.kind == CurveKind::BSpline {
            let pts: Vec<Pos2> = seg.knots.iter().filter_map(|k| screen(&cam, rect, w, k.pos)).collect();
            let mut line = pts.clone();
            if seg.closed
                && let Some(f) = pts.first()
            {
                line.push(*f);
            }
            painter.add(egui::Shape::line(line, Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.6))));
        }
    }
    // The pen: the handle being pulled and the line to the pointer.
    if let Some(pen) = &app.spline.pen
        && let Some(press) = pen.press.as_ref().filter(|p| p.view == view)
        && let Some(now) = painter.ctx().input(|i| i.pointer.hover_pos())
        && press.at.distance(now) > HANDLE_DRAG
    {
        painter.line_segment([press.at, now], Stroke::new(1.0, theme::ACCENT));
        painter.circle_filled(now, 3.0, theme::ACCENT);
        let mirrored = press.at + (press.at - now);
        painter.circle_filled(mirrored, 3.0, theme::ACCENT.gamma_multiply(0.7));
    }
}
