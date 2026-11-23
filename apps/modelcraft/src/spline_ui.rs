//! Splines in the app: the Spline menu, the Properties section of a spline object, how curves are
//! drawn in the viewport and how a click picks one.
//!
//! Everything a control does is a `spline` command (undoable, journaled, scriptable); every control
//! has an accessible name, the pickers are combo boxes that work from the keyboard, and the actions
//! are in the command palette.

use egui::{Pos2, Rect, RichText};
use glam::Vec3;
use modelcraft_engine::curves::primitives::{KINDS, MAX_CORNERS};
use modelcraft_engine::curves::{ArcClose, Plane, SplinePrimitive, Tessellation};
use modelcraft_engine::scene::splines::SplineSource;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, SplineCommand};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplineAction {
    /// Add a spline primitive (`circle`, `rectangle`, …) or `empty`.
    Add(&'static str),
    /// Turn the selected primitive spline into editable points.
    MakeEditable,
    /// The pen tool: click to add points, drag to pull out handles.
    Pen,
    /// A generator (`extrude`, `revolve`, `loft`, `sweep`) around the selected splines.
    AddGenerator(&'static str),
    /// Run the selected spline's curves the other way.
    Reverse,
    /// Cut the selected points' corners with a straight edge or a round one.
    Chamfer,
    Fillet,
    /// Flatten the selected spline's curves into straight edges.
    ToPolygon,
    /// Fill the selected spline's closed curves with a mesh.
    ToMesh,
}

pub fn kind_label(kind: &str) -> String {
    match kind {
        "circle" => tl!("Circle"),
        "rectangle" => tl!("Rectangle"),
        "star" => tl!("Star"),
        "n_side" => tl!("N-Side"),
        "arc" => tl!("Arc"),
        "helix" => tl!("Helix"),
        "empty" => tl!("Empty"),
        _ => kind.to_string(),
    }
}

impl SplineAction {
    pub fn all() -> Vec<SplineAction> {
        let mut v: Vec<SplineAction> = KINDS.iter().map(|k| SplineAction::Add(k)).collect();
        v.push(SplineAction::Add("empty"));
        v.push(SplineAction::MakeEditable);
        v.push(SplineAction::Pen);
        v.extend(modelcraft_engine::scene::spline_gen::KINDS.iter().map(|k| SplineAction::AddGenerator(k)));
        v.extend([SplineAction::Reverse, SplineAction::Chamfer, SplineAction::Fillet, SplineAction::ToPolygon, SplineAction::ToMesh]);
        v
    }

    pub fn label(self) -> String {
        match self {
            SplineAction::Add(k) => crate::tlf!("Add {shape} Spline", shape = kind_label(k)),
            SplineAction::MakeEditable => tl!("Make Spline Editable"),
            SplineAction::Pen => tl!("Pen Tool"),
            SplineAction::AddGenerator(k) => crate::tlf!("Add {kind}", kind = crate::spline_gen_ui::kind_label(k)),
            SplineAction::Reverse => tl!("Reverse Spline"),
            SplineAction::Chamfer => tl!("Chamfer Points"),
            SplineAction::Fillet => tl!("Fillet Points"),
            SplineAction::ToPolygon => tl!("Convert Spline to Polygon"),
            SplineAction::ToMesh => tl!("Convert Spline to Mesh"),
        }
    }

    pub fn id(self) -> String {
        match self {
            SplineAction::Add(k) => format!("spline.add.{k}"),
            SplineAction::MakeEditable => "spline.make_editable".into(),
            SplineAction::Pen => "spline.pen".into(),
            SplineAction::AddGenerator(k) => format!("spline.add_generator.{k}"),
            SplineAction::Reverse => "spline.reverse".into(),
            SplineAction::Chamfer => "spline.chamfer".into(),
            SplineAction::Fillet => "spline.fillet".into(),
            SplineAction::ToPolygon => "spline.to_polygon".into(),
            SplineAction::ToMesh => "spline.to_mesh".into(),
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            SplineAction::Pen => Some("Model: Shift+P"),
            _ => None,
        }
    }
}

pub fn act(app: &mut ModelApp, a: SplineAction) {
    match a {
        SplineAction::Add(kind) => {
            app.run(Command::Spline(SplineCommand::Add { kind: kind.into(), parent: None, name: None }));
        }
        SplineAction::Pen => crate::spline_pen::toggle_pen(app),
        SplineAction::Reverse | SplineAction::Chamfer | SplineAction::Fillet | SplineAction::ToPolygon | SplineAction::ToMesh => modify(app, a),
        SplineAction::AddGenerator(kind) => {
            let inputs: Vec<ObjectId> = app.selection.iter().copied().filter(|id| app.doc.scene.is_spline(*id)).collect();
            app.run(Command::Spline(SplineCommand::AddGenerator { kind: kind.into(), inputs }));
        }
        SplineAction::MakeEditable => match app.active().filter(|id| app.doc.scene.is_spline(*id)) {
            Some(id) => {
                app.run(Command::Spline(SplineCommand::MakeEditable { id }));
            }
            None => app.status = tl!("Select a spline first"),
        },
    }
}

/// Reverse, cut corners, flatten and fill: the operations on the selected spline.
fn modify(app: &mut ModelApp, a: SplineAction) {
    use modelcraft_engine::curves::SplineEdit;
    let Some(id) = crate::spline_pen::active_spline(app) else {
        app.status = tl!("Select a spline first");
        return;
    };
    let Some(spline) = crate::spline_pen::spline_of(app, id) else { return };
    let selected = app.spline.selected.clone();
    match a {
        SplineAction::Reverse => {
            // The curves with a selected point, or every curve.
            let mut segs: Vec<usize> = if selected.is_empty() { (0..spline.segments.len()).collect() } else { selected.iter().map(|k| k.segment).collect() };
            segs.sort_unstable();
            segs.dedup();
            let commands = segs.into_iter().map(|segment| Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::Reverse { segment } })).collect();
            app.run(Command::Batch { label: Some("Reverse spline".into()), commands });
        }
        SplineAction::Chamfer | SplineAction::Fillet => {
            if selected.is_empty() {
                app.status = tl!("Select the points to cut first");
                return;
            }
            let size = app.spline.corner;
            let edit = if a == SplineAction::Chamfer {
                SplineEdit::Chamfer { knots: selected, distance: size }
            } else {
                SplineEdit::Fillet { knots: selected, radius: size }
            };
            if app.run(Command::Spline(SplineCommand::Edit { id, edit })) {
                app.spline.selected.clear();
            }
        }
        SplineAction::ToPolygon => {
            let tess = match app.doc.scene.get(id).map(|o| &o.kind) {
                Some(ObjectKind::Spline { tess, .. }) => *tess,
                _ => return,
            };
            app.run(Command::Spline(SplineCommand::Edit { id, edit: SplineEdit::ToPolygon { tess } }));
        }
        _ => {
            app.run(Command::Spline(SplineCommand::ToMesh { id }));
        }
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Spline"), |ui| {
        for a in SplineAction::all() {
            if a == SplineAction::MakeEditable || matches!(a, SplineAction::Pen | SplineAction::AddGenerator("extrude") | SplineAction::Reverse) {
                ui.separator();
            }
            let action = crate::actions::Action::Spline(a);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                act(app, a);
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------- the Properties section

fn plane_label(p: Plane) -> String {
    match p {
        Plane::Xy => tl!("XY plane"),
        Plane::Yz => tl!("YZ plane"),
        Plane::Zx => tl!("ZX plane"),
    }
}

fn close_label(c: ArcClose) -> String {
    match c {
        ArcClose::Open => tl!("Open"),
        ArcClose::Chord => tl!("Chord"),
        ArcClose::Pie => tl!("Pie slice"),
    }
}

fn mode_label(kind: &str) -> String {
    match kind {
        "fixed" => tl!("Fixed steps"),
        "uniform" => tl!("Uniform length"),
        _ => tl!("Adaptive"),
    }
}

/// A combo box over `options`; returns the index picked this frame. Focus goes back to the combo
/// after a pick (its popup closes).
fn pick(ui: &mut egui::Ui, salt: impl std::hash::Hash + std::fmt::Debug, name: String, current: usize, options: &[String]) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = egui::ComboBox::from_id_salt(salt).selected_text(options.get(current).cloned().unwrap_or_default()).show_ui(ui, |ui| {
            for (i, o) in options.iter().enumerate() {
                if ui.selectable_label(i == current, o).clicked() && i != current {
                    picked = Some(i);
                }
            }
        });
        set_name(ui, &r.response, name);
        if picked.is_some() {
            r.response.request_focus();
        }
    });
    picked
}

fn count_field(ui: &mut egui::Ui, v: &mut u32, range: std::ops::RangeInclusive<u32>, name: String) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(v).speed(0.2).range(range));
        set_name(ui, &r, name);
        r
    })
    .inner
}

fn plane_field(ui: &mut egui::Ui, id: ObjectId, plane: &mut Plane) -> bool {
    let labels: Vec<String> = Plane::ALL.iter().map(|p| plane_label(*p)).collect();
    let cur = Plane::ALL.iter().position(|p| p == plane).unwrap_or(0);
    match pick(ui, ("spline-plane", id.0), tl!("Plane"), cur, &labels) {
        Some(i) => {
            *plane = Plane::ALL.get(i).copied().unwrap_or(*plane);
            true
        }
        None => false,
    }
}

/// The Properties section of a spline object.
pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, source: &SplineSource, tess: Tessellation) {
    match source {
        SplineSource::Primitive { primitive } => primitive_fields(app, ui, id, *primitive, tess),
        SplineSource::Curve { spline } => {
            ui.label(crate::tlf!("{segments} segments, {points} points", segments = spline.segments.len(), points = spline.knot_count()));
            crate::spline_points_ui::section(app, ui, id);
        }
    }
    tess_fields(app, ui, id, tess, source);
    status(app, ui, id);
}

fn primitive_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, p: SplinePrimitive, tess: Tessellation) {
    let mut q = p;
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    const BIG: f32 = 1.0e6;
    match &mut q {
        SplinePrimitive::Circle { radius_x, radius_y, plane } => {
            ui.horizontal(|ui| drags.push(drag(ui, radius_x, 0.01, 0.0..=BIG, tl!("Radius X"))));
            ui.horizontal(|ui| drags.push(drag(ui, radius_y, 0.01, 0.0..=BIG, tl!("Radius Y"))));
            discrete |= plane_field(ui, id, plane);
        }
        SplinePrimitive::Rectangle { width, height, corner, plane } => {
            ui.horizontal(|ui| drags.push(drag(ui, width, 0.01, 0.0..=BIG, tl!("Width"))));
            ui.horizontal(|ui| drags.push(drag(ui, height, 0.01, 0.0..=BIG, tl!("Height"))));
            ui.horizontal(|ui| drags.push(drag(ui, corner, 0.01, 0.0..=BIG, tl!("Corner radius"))));
            discrete |= plane_field(ui, id, plane);
        }
        SplinePrimitive::Star { points, inner, outer, plane } => {
            drags.push(count_field(ui, points, 2..=MAX_CORNERS, tl!("Points")));
            ui.horizontal(|ui| drags.push(drag(ui, inner, 0.01, 0.0..=BIG, tl!("Inner radius"))));
            ui.horizontal(|ui| drags.push(drag(ui, outer, 0.01, 0.0..=BIG, tl!("Outer radius"))));
            discrete |= plane_field(ui, id, plane);
        }
        SplinePrimitive::NSide { sides, radius, plane } => {
            drags.push(count_field(ui, sides, 3..=MAX_CORNERS, tl!("Sides")));
            ui.horizontal(|ui| drags.push(drag(ui, radius, 0.01, 0.0..=BIG, tl!("Radius"))));
            discrete |= plane_field(ui, id, plane);
        }
        SplinePrimitive::Arc { radius, start, end, close, plane } => {
            ui.horizontal(|ui| drags.push(drag(ui, radius, 0.01, 0.0..=BIG, tl!("Radius"))));
            ui.horizontal(|ui| drags.push(drag(ui, start, 0.5, -36_000.0..=36_000.0, tl!("Start angle"))));
            ui.horizontal(|ui| drags.push(drag(ui, end, 0.5, -36_000.0..=36_000.0, tl!("End angle"))));
            let closes = [ArcClose::Open, ArcClose::Chord, ArcClose::Pie];
            let labels: Vec<String> = closes.iter().map(|c| close_label(*c)).collect();
            if let Some(i) = pick(ui, ("spline-arc-close", id.0), tl!("Closing"), closes.iter().position(|c| c == close).unwrap_or(0), &labels) {
                *close = closes.get(i).copied().unwrap_or(*close);
                discrete = true;
            }
            discrete |= plane_field(ui, id, plane);
        }
        SplinePrimitive::Helix { radius_start, radius_end, height, turns, knots_per_turn, plane } => {
            ui.horizontal(|ui| drags.push(drag(ui, radius_start, 0.01, 0.0..=BIG, tl!("Start radius"))));
            ui.horizontal(|ui| drags.push(drag(ui, radius_end, 0.01, 0.0..=BIG, tl!("End radius"))));
            ui.horizontal(|ui| drags.push(drag(ui, height, 0.01, -BIG..=BIG, tl!("Height"))));
            ui.horizontal(|ui| drags.push(drag(ui, turns, 0.02, 0.0..=512.0, tl!("Turns"))));
            drags.push(count_field(ui, knots_per_turn, 4..=64, tl!("Points per turn")));
            discrete |= plane_field(ui, id, plane);
        }
    }
    if q != p && discrete {
        app.run(Command::Spline(SplineCommand::SetPrimitive { id, primitive: q }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(
            app,
            r,
            |o| o.kind = ObjectKind::Spline { source: SplineSource::Primitive { primitive: q }, tess },
            || Command::Spline(SplineCommand::SetPrimitive { id, primitive: q }),
            id,
        );
    }
}

fn tess_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, tess: Tessellation, source: &SplineSource) {
    let kinds = modelcraft_engine::curves::MODES;
    let labels: Vec<String> = kinds.iter().map(|k| mode_label(k)).collect();
    if let Some(i) = pick(ui, ("spline-tess", id.0), tl!("Intermediate points"), kinds.iter().position(|k| *k == tess.kind()).unwrap_or(2), &labels)
        && let Some(t) = kinds.get(i).and_then(|k| Tessellation::default_of(k))
    {
        app.run(Command::Spline(SplineCommand::SetTessellation { id, tess: t }));
        return;
    }
    let mut t = tess;
    let mut drags = Vec::new();
    match &mut t {
        Tessellation::Fixed { steps } => drags.push(count_field(ui, steps, 1..=modelcraft_engine::curves::MAX_STEPS, tl!("Steps"))),
        Tessellation::Uniform { length } => {
            ui.horizontal(|ui| drags.push(drag(ui, length, 0.005, 0.0001..=1.0e6, tl!("Step length"))));
        }
        Tessellation::Adaptive { tolerance, angle_deg } => {
            ui.horizontal(|ui| drags.push(drag(ui, tolerance, 0.0005, 0.000_001..=1.0e6, tl!("Tolerance"))));
            ui.horizontal(|ui| drags.push(drag(ui, angle_deg, 0.1, 1.0..=90.0, tl!("Largest turn"))));
        }
    }
    if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(
            app,
            r,
            |o| o.kind = ObjectKind::Spline { source: source.clone(), tess: t },
            || Command::Spline(SplineCommand::SetTessellation { id, tess: t }),
            id,
        );
    }
}

fn status(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some(e) = app.doc.eval.spline(&app.doc.scene, id) else { return };
    match &e.error {
        Some(msg) => {
            ui.label(RichText::new(msg).color(theme::TEXT_DIM).small());
        }
        None => {
            ui.label(crate::tlf!(
                "{points} points, length {length}",
                points = e.point_count(),
                length = format!("{:.3}", e.lines.iter().map(|l| l.length()).sum::<f32>())
            ));
        }
    }
}

// ---------------------------------------------------------------- drawing and picking

/// Line segments (world space, with colour) of every visible spline: the curves, brighter when
/// selected.
pub fn curve_lines(app: &mut ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let ids: Vec<ObjectId> = app.doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Spline { .. })).map(|o| o.id).collect();
    let mut out = Vec::new();
    let mut budget = 400_000usize;
    for id in ids {
        if !app.doc.scene.visible(id) {
            continue;
        }
        let Some(e) = app.doc.eval.spline(&app.doc.scene, id) else { continue };
        let w = app.doc.scene.world_matrix(id);
        let selected = app.selection.contains(&id);
        let color = if selected { [1.0, 0.62, 0.25, 1.0] } else { [0.55, 0.78, 0.95, 0.95] };
        for line in &e.lines {
            for i in 0..line.edge_count() {
                if budget == 0 {
                    return out;
                }
                budget -= 1;
                if let Some((a, b)) = line.edge(i) {
                    out.push((w.transform_point3(a).to_array(), w.transform_point3(b).to_array(), color));
                }
            }
        }
    }
    out
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

/// How near (in points on screen) a click must be to a curve to pick it.
const CURVE_REACH: f32 = 7.0;

/// The spline whose curve passes within reach of a click.
pub fn pick_curve(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2) -> Option<ObjectId> {
    let cam = app.views.get(view)?.camera;
    let ids: Vec<ObjectId> = app.doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Spline { .. }) && !o.template).map(|o| o.id).collect();
    let mut best: Option<(f32, ObjectId)> = None;
    for id in ids {
        if !app.doc.scene.visible(id) {
            continue;
        }
        let Some(e) = app.doc.eval.spline(&app.doc.scene, id) else { continue };
        let w = app.doc.scene.world_matrix(id);
        for line in &e.lines {
            for i in 0..line.edge_count() {
                let Some((a, b)) = line.edge(i) else { continue };
                let (Some(pa), Some(pb)) =
                    (crate::viewport::to_screen(&cam, rect, w.transform_point3(a)), crate::viewport::to_screen(&cam, rect, w.transform_point3(b)))
                else {
                    continue;
                };
                let d = seg_dist(at, pa, pb);
                if d <= CURVE_REACH && best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, id));
                }
            }
        }
    }
    best.map(|(_, id)| id)
}

/// The box around a spline object in world space (for Frame Selection).
pub fn world_bounds(app: &mut ModelApp, id: ObjectId) -> Option<(Vec3, Vec3)> {
    let e = app.doc.eval.spline(&app.doc.scene, id)?;
    let (lo, hi) = e.spline.bounds()?;
    let w = app.doc.scene.world_matrix(id);
    let corners = [lo, hi, Vec3::new(lo.x, hi.y, lo.z), Vec3::new(hi.x, lo.y, hi.z), Vec3::new(lo.x, lo.y, hi.z), Vec3::new(hi.x, hi.y, lo.z)];
    Some(corners.iter().map(|c| w.transform_point3(*c)).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p))))
}
