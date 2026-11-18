//! Snapping to geometry, gizmo orientations and the measure tool.
//!
//! - **Snap to** vertices, edges and surfaces of the other objects while moving with the gizmo
//!   (View ▸ Snapping, or the palette). The search is [`modelcraft_engine::scene::snap`]; this
//!   module feeds it the scene and draws the marker.
//! - **Gizmo orientation**: world, local (the active object, or the edited mesh), parent, or the
//!   normal of the selected components. Move and rotate handles follow the orientation.
//! - **Measure**: click two points (vertices, edge points or surface points); the distance and
//!   its parts are shown in the length unit of the preferences, and announced in the status bar.

use egui::{Color32, Pos2, Rect, Stroke};
use glam::{Mat4, Quat, Vec2, Vec3};
use modelcraft_engine::scene::pick::Ray;
use modelcraft_engine::scene::snap::{self, SnapHit, SnapKind, SnapTargets};
use modelcraft_engine::scene::{ObjectId, ObjectKind};

use crate::app::{ModelApp, SelectMode};
use crate::prefs_ui::Unit;
use crate::theme;

/// The frame the gizmo handles line up with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GizmoSpace {
    #[default]
    World,
    /// The active object (or the mesh being edited).
    Local,
    /// The active object's parent.
    Parent,
    /// The average normal of the selected components, else the same as Local.
    Normal,
}

impl GizmoSpace {
    pub const ALL: [GizmoSpace; 4] = [GizmoSpace::World, GizmoSpace::Local, GizmoSpace::Parent, GizmoSpace::Normal];
    pub fn label(self) -> String {
        match self {
            GizmoSpace::World => tl!("World Axes"),
            GizmoSpace::Local => tl!("Local Axes"),
            GizmoSpace::Parent => tl!("Parent Axes"),
            GizmoSpace::Normal => tl!("Normal Axes"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    SnapVertices,
    SnapEdges,
    SnapSurfaces,
    Space(GizmoSpace),
    Measure,
    ClearMeasure,
}

pub fn all_ops() -> Vec<Op> {
    let mut v = vec![Op::SnapVertices, Op::SnapEdges, Op::SnapSurfaces];
    v.extend(GizmoSpace::ALL.iter().map(|s| Op::Space(*s)));
    v.extend([Op::Measure, Op::ClearMeasure]);
    v
}

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::SnapVertices => tl!("Snap to Vertices"),
            Op::SnapEdges => tl!("Snap to Edges"),
            Op::SnapSurfaces => tl!("Snap to Surfaces"),
            Op::Space(s) => s.label(),
            Op::Measure => tl!("Measure"),
            Op::ClearMeasure => tl!("Clear Measurement"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::SnapVertices => "snap.vertices",
            Op::SnapEdges => "snap.edges",
            Op::SnapSurfaces => "snap.surfaces",
            Op::Space(GizmoSpace::World) => "gizmo.world",
            Op::Space(GizmoSpace::Local) => "gizmo.local",
            Op::Space(GizmoSpace::Parent) => "gizmo.parent",
            Op::Space(GizmoSpace::Normal) => "gizmo.normal",
            Op::Measure => "tools.measure",
            Op::ClearMeasure => "tools.measure_clear",
        }
    }
}

/// A measurement in progress: up to two points.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Measurement {
    pub a: Option<Vec3>,
    pub b: Option<Vec3>,
}

#[derive(Default)]
pub struct State {
    pub targets: SnapTargets,
    pub space: GizmoSpace,
    /// The measure tool takes the viewport clicks.
    pub measuring: bool,
    pub measurement: Measurement,
    /// Where the move drag snapped to, for the marker.
    pub hint: Option<SnapHit>,
}

/// What a measurement reads as: the distance and its x, y, z parts, in `unit`.
pub fn describe(a: Vec3, b: Vec3, unit: Unit) -> String {
    let d = b - a;
    let per = unit.per_metre();
    let n = |v: f32| format!("{:.3}", v * per);
    format!("{}{}  (X {}  Y {}  Z {})", n(d.length()), unit.suffix(), n(d.x.abs()), n(d.y.abs()), n(d.z.abs()))
}

pub fn run(app: &mut ModelApp, op: Op) {
    match op {
        Op::SnapVertices => app.snaps.targets.vertex = !app.snaps.targets.vertex,
        Op::SnapEdges => app.snaps.targets.edge = !app.snaps.targets.edge,
        Op::SnapSurfaces => app.snaps.targets.surface = !app.snaps.targets.surface,
        Op::Space(s) => app.snaps.space = s,
        Op::Measure => {
            app.snaps.measuring = !app.snaps.measuring;
            app.status = if app.snaps.measuring { tl!("Measure: click two points") } else { tl!("Measure off") };
        }
        Op::ClearMeasure => app.snaps.measurement = Measurement::default(),
    }
    if let Op::SnapVertices | Op::SnapEdges | Op::SnapSurfaces = op {
        let t = app.snaps.targets;
        let names: Vec<String> =
            [(t.vertex, tl!("vertices")), (t.edge, tl!("edges")), (t.surface, tl!("surfaces"))].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
        app.status = if names.is_empty() { tl!("Snapping to geometry is off") } else { crate::tlf!("Snapping to {what}", what = names.join(", ")) };
    }
}

fn is_on(app: &ModelApp, op: Op) -> bool {
    match op {
        Op::SnapVertices => app.snaps.targets.vertex,
        Op::SnapEdges => app.snaps.targets.edge,
        Op::SnapSurfaces => app.snaps.targets.surface,
        Op::Space(s) => app.snaps.space == s,
        Op::Measure => app.snaps.measuring,
        Op::ClearMeasure => false,
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Snapping and Axes"), |ui| {
        for op in all_ops() {
            if matches!(op, Op::Space(GizmoSpace::World) | Op::Measure) {
                ui.separator();
            }
            let on = is_on(app, op);
            let r = ui.add(egui::Button::selectable(on, op.label()));
            if r.clicked() {
                run(app, op);
                ui.close();
            }
        }
    });
}

fn rotation_of(m: Mat4) -> Quat {
    let (_, r, _) = m.to_scale_rotation_translation();
    if r.is_finite() { r.normalize() } else { Quat::IDENTITY }
}

fn frame(r: Quat) -> [Vec3; 3] {
    [r * Vec3::X, r * Vec3::Y, r * Vec3::Z]
}

/// The three axes the gizmo handles follow, in world space.
pub fn gizmo_axes(app: &mut ModelApp) -> [Vec3; 3] {
    let world = [Vec3::X, Vec3::Y, Vec3::Z];
    if app.snaps.space == GizmoSpace::World {
        return world;
    }
    let target = if app.mode == SelectMode::Object { app.active() } else { app.edit_target() };
    let Some(id) = target else { return world };
    match app.snaps.space {
        GizmoSpace::World => world,
        GizmoSpace::Parent => match app.doc.scene.get(id).and_then(|o| o.parent) {
            Some(p) => frame(rotation_of(app.doc.scene.world_matrix(p))),
            None => world,
        },
        GizmoSpace::Local => frame(rotation_of(app.doc.scene.world_matrix(id))),
        GizmoSpace::Normal => {
            let local = frame(rotation_of(app.doc.scene.world_matrix(id)));
            if app.mode == SelectMode::Object || app.components.is_empty() {
                return local;
            }
            let Some(n) = component_normal(app) else { return local };
            // z along the normal; x as near the local x as the normal allows.
            let w = app.doc.scene.world_matrix(id);
            let n = w.transform_vector3(n).normalize_or(Vec3::Y);
            let x = (local[0] - n * local[0].dot(n)).try_normalize().unwrap_or_else(|| n.any_orthonormal_vector());
            let y = n.cross(x);
            // Z along the normal: a right-handed frame (x cross y is the normal).
            [x, y, n]
        }
    }
}

/// The average (object-space) normal of the selected faces, or of the faces around selected
/// edges and vertices.
fn component_normal(app: &mut ModelApp) -> Option<Vec3> {
    let id = app.edit_target()?;
    let m = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    let mut sum = Vec3::ZERO;
    match app.mode {
        SelectMode::Face => {
            for f in app.components.iter().filter(|f| (**f as usize) < m.face_count()) {
                sum += m.face_normal(*f);
            }
        }
        _ => {
            let verts = app.component_vertices();
            let normals = m.vertex_normals();
            for v in verts {
                sum += normals.get(v as usize).copied().unwrap_or(Vec3::ZERO);
            }
        }
    }
    sum.try_normalize()
}

/// The meshes a snap can land on: every drawn object that is not being moved.
fn candidates(app: &mut ModelApp, skip: &[ObjectId]) -> Vec<(Mat4, std::sync::Arc<modelcraft_engine::mesh::PolyMesh>)> {
    let ids: Vec<ObjectId> = app.doc.scene.objects().iter().map(|o| o.id).collect();
    let mut out = Vec::new();
    for id in ids {
        let Some(o) = app.doc.scene.get(id) else { continue };
        if skip.contains(&id) || !app.doc.scene.visible(id) || app.doc.scene.is_cage_input(id) {
            continue;
        }
        if !matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. } | ObjectKind::Instance { .. }) {
            continue;
        }
        let mesh = app.doc.eval.evaluate(&app.doc.scene, id).mesh;
        out.push((app.doc.scene.world_matrix(id), mesh));
    }
    out
}

/// The geometry snap for a cursor in a view: the nearest vertex, edge point or surface point of
/// the objects not in `skip`, within 14 pixels (surfaces answer anywhere over them).
pub fn geometry_snap(
    app: &mut ModelApp,
    targets: SnapTargets,
    skip: &[ObjectId],
    cam: &modelcraft_render::Camera,
    rect: Rect,
    cursor: Pos2,
) -> Option<SnapHit> {
    if !targets.any() {
        return None;
    }
    let cands = candidates(app, skip);
    let placed: Vec<snap::Placed<'_>> = cands.iter().map(|(w, m)| (*w, m.as_ref())).collect();
    let aspect = rect.aspect_ratio();
    let project = |p: Vec3| {
        let n = cam.project(p, aspect)?;
        Some(Vec2::new(rect.left() + (n.x * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - n.y * 0.5) * rect.height()))
    };
    let ndc = Vec2::new((cursor.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (cursor.y - rect.top()) / rect.height() * 2.0);
    let (origin, dir) = cam.ray(ndc, aspect);
    snap::snap(&placed, targets, &project, Vec2::new(cursor.x, cursor.y), 14.0, &Ray { origin, dir })
}

/// A viewport click while the measure tool is on: the first and second points of a measurement;
/// a third click starts a new one. Returns whether the click was used.
pub fn measure_click(app: &mut ModelApp, cam: &modelcraft_render::Camera, rect: Rect, at: Pos2) -> bool {
    if !app.snaps.measuring {
        return false;
    }
    let all = SnapTargets { vertex: true, edge: true, surface: true };
    let Some(hit) = geometry_snap(app, all, &[], cam, rect, at) else {
        app.status = tl!("Measure: nothing under the pointer");
        return true;
    };
    let m = &mut app.snaps.measurement;
    match (m.a, m.b) {
        (Some(_), None) => m.b = Some(hit.point),
        _ => *m = Measurement { a: Some(hit.point), b: None },
    }
    if let (Some(a), Some(b)) = (m.a, m.b) {
        app.status = crate::tlf!("Distance {text}", text = describe(a, b, app.unit));
    } else {
        app.status = tl!("Measure: first point set, click the second");
    }
    true
}

/// The snap marker, the measurement and the measure tool's hint, over a view.
pub fn overlay(app: &ModelApp, painter: &egui::Painter, cam: &modelcraft_render::Camera, rect: Rect) {
    let to = |p: Vec3| -> Option<Pos2> {
        let n = cam.project(p, rect.aspect_ratio())?;
        Some(Pos2::new(rect.left() + (n.x * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - n.y * 0.5) * rect.height()))
    };
    if let Some(h) = app.snaps.hint
        && let Some(p) = to(h.point)
    {
        let c = Color32::from_rgb(255, 220, 90);
        match h.kind {
            SnapKind::Vertex => {
                painter.rect_stroke(Rect::from_center_size(p, egui::vec2(11.0, 11.0)), 0.0, Stroke::new(2.0, c), egui::StrokeKind::Middle);
            }
            SnapKind::Edge => {
                painter.circle_stroke(p, 6.0, Stroke::new(2.0, c));
            }
            SnapKind::Surface => {
                painter.circle_filled(p, 4.0, c);
            }
        }
    }
    let m = app.snaps.measurement;
    let c = Color32::from_rgb(120, 220, 255);
    for p in [m.a, m.b].into_iter().flatten() {
        if let Some(s) = to(p) {
            painter.circle_filled(s, 4.0, c);
        }
    }
    if let (Some(a), Some(b)) = (m.a, m.b)
        && let (Some(sa), Some(sb)) = (to(a), to(b))
    {
        painter.line_segment([sa, sb], Stroke::new(1.5, c));
        let mid = Pos2::new((sa.x + sb.x) * 0.5, (sa.y + sb.y) * 0.5);
        painter.text(mid + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, describe(a, b, app.unit), egui::FontId::proportional(12.0), c);
    }
    if app.snaps.measuring && m.a.is_none() {
        painter.text(
            rect.center_top() + egui::vec2(0.0, 24.0),
            egui::Align2::CENTER_TOP,
            tl!("Measure: click two points"),
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measurement_reads_distance_and_parts_in_the_chosen_unit() {
        let t = describe(Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0), Unit::M);
        assert!(t.starts_with("5.000 m") && t.contains("X 3.000") && t.contains("Y 4.000") && t.contains("Z 0.000"), "{t}");
        let t = describe(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Unit::Cm);
        assert!(t.starts_with("100.000 cm"), "{t}");
        let t = describe(Vec3::ZERO, Vec3::new(0.0254, 0.0, 0.0), Unit::In);
        assert!(t.starts_with("1.000 in"), "{t}");
    }
}
