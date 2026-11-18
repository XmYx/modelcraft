//! The UV editor: a window with a 2D view of the selected mesh's UV layout.
//!
//! - Select UV vertices, edges, faces or whole islands (click, Shift+click, box, Ctrl+A) and move,
//!   rotate or scale them by dragging, by arrow and bracket keys, or by typing numbers.
//! - Pin vertices (they keep their place when the mesh is unwrapped again), weld the two sides of
//!   seams, mark and clear seams, mark seams by angle, and unwrap.
//! - A checker background and a distortion heat map (angle error, area, stretch), with numbers for
//!   the same measures, so nothing depends on colour alone.
//!
//! Every edit is a step of the object's build history (`Seams`, `TransformUvs`, `Weld UVs`,
//! `Unwrap UVs`), so it is saved, undone and redone like any other. Corners are named by their
//! number in the mesh the step receives; pins and the selection are editor state and are dropped
//! when the mesh changes under them.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Color32, Key, Mesh, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use glam::Vec2;
use modelcraft_engine::Command;
use modelcraft_engine::mesh::{NONE, PolyMesh};
use modelcraft_engine::scene::{Density, Fit, ObjectId, ObjectKind, Projection, Rotation, StepOp, UnwrapMethod, UvPin};
use modelcraft_engine::uv::RelaxMode;
use modelcraft_engine::uv::{
    Charts, Distortion, SeamSet, TriangleMetric, UvTransform, expand_to_uv_vertices, face_triangles, island_corners, measure, per_face, uv_vertex_corners,
};
use modelcraft_engine::uvpack::Axis;
use modelcraft_render::buffers::LineVertex;

use crate::app::ModelApp;
use crate::theme;

/// Most faces the view draws; above it the view says so and draws nothing (bounded work per frame).
pub const MAX_DRAW_FACES: usize = 300_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvSelect {
    Vertex,
    Edge,
    Face,
    Island,
}

impl UvSelect {
    pub const ALL: [UvSelect; 4] = [UvSelect::Vertex, UvSelect::Edge, UvSelect::Face, UvSelect::Island];
    pub fn label(self) -> String {
        match self {
            UvSelect::Vertex => tl!("UV Vertex"),
            UvSelect::Edge => tl!("UV Edge"),
            UvSelect::Face => tl!("UV Face"),
            UvSelect::Island => tl!("UV Island"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvTool {
    Select,
    Move,
    Rotate,
    Scale,
}

impl UvTool {
    pub const ALL: [UvTool; 4] = [UvTool::Select, UvTool::Move, UvTool::Rotate, UvTool::Scale];
    pub fn label(self) -> String {
        match self {
            UvTool::Select => tl!("Select"),
            UvTool::Move => tl!("Move"),
            UvTool::Rotate => tl!("Rotate"),
            UvTool::Scale => tl!("Scale"),
        }
    }
}

/// What the heat map shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heat {
    Off,
    Angle,
    Area,
    Stretch,
}

impl Heat {
    pub const ALL: [Heat; 4] = [Heat::Off, Heat::Angle, Heat::Area, Heat::Stretch];
    pub fn label(self) -> String {
        match self {
            Heat::Off => tl!("No heat map"),
            Heat::Angle => tl!("Angle error"),
            Heat::Area => tl!("Area scale"),
            Heat::Stretch => tl!("Stretch"),
        }
    }
}

/// The point a rotation or scale turns about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pivot {
    Selection,
    Centre,
    Origin,
}

impl Pivot {
    pub const ALL: [Pivot; 3] = [Pivot::Selection, Pivot::Centre, Pivot::Origin];
    pub fn label(self) -> String {
        match self {
            Pivot::Selection => tl!("Selection centre"),
            Pivot::Centre => tl!("Middle of the square"),
            Pivot::Origin => tl!("Top left corner"),
        }
    }
}

/// Commands of the UV editor that are also menu entries and shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UvAction {
    Editor,
    MarkSeam,
    ClearSeam,
    SeamsByAngle,
    Unwrap,
    UnwrapAbf,
    UnwrapGroups,
    Relax,
    Straighten,
    Stitch,
    Transfer,
    Pack,
    ProjectPlanar,
    ProjectBox,
    ProjectCylindrical,
    ProjectSpherical,
    ProjectCamera,
    ExportLayout,
}

impl UvAction {
    pub const ALL: [UvAction; 18] = [
        UvAction::Editor,
        UvAction::MarkSeam,
        UvAction::ClearSeam,
        UvAction::SeamsByAngle,
        UvAction::Unwrap,
        UvAction::UnwrapAbf,
        UvAction::UnwrapGroups,
        UvAction::Relax,
        UvAction::Straighten,
        UvAction::Stitch,
        UvAction::Transfer,
        UvAction::Pack,
        UvAction::ProjectPlanar,
        UvAction::ProjectBox,
        UvAction::ProjectCylindrical,
        UvAction::ProjectSpherical,
        UvAction::ProjectCamera,
        UvAction::ExportLayout,
    ];

    pub fn label(self) -> String {
        match self {
            UvAction::Editor => tl!("UV Editor"),
            UvAction::MarkSeam => tl!("Mark Seam"),
            UvAction::ClearSeam => tl!("Clear Seam"),
            UvAction::SeamsByAngle => tl!("Mark Seams by Angle"),
            UvAction::Unwrap => tl!("Unwrap UVs"),
            UvAction::UnwrapAbf => tl!("Unwrap UVs (Angle-Based)"),
            UvAction::UnwrapGroups => tl!("Unwrap UVs by Polygon Groups"),
            UvAction::Relax => tl!("Relax UVs"),
            UvAction::Straighten => tl!("Straighten UVs"),
            UvAction::Stitch => tl!("Stitch UV Islands"),
            UvAction::Transfer => tl!("Transfer UVs"),
            UvAction::Pack => tl!("Pack UV Islands"),
            UvAction::ProjectPlanar => tl!("Project UVs: Planar"),
            UvAction::ProjectBox => tl!("Project UVs: Box"),
            UvAction::ProjectCylindrical => tl!("Project UVs: Cylindrical"),
            UvAction::ProjectSpherical => tl!("Project UVs: Spherical"),
            UvAction::ProjectCamera => tl!("Project UVs: From View"),
            UvAction::ExportLayout => tl!("Export UV Layout…"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            UvAction::Editor => "uv.editor",
            UvAction::MarkSeam => "uv.mark_seam",
            UvAction::ClearSeam => "uv.clear_seam",
            UvAction::SeamsByAngle => "uv.seams_by_angle",
            UvAction::Unwrap => "uv.unwrap",
            UvAction::UnwrapAbf => "uv.unwrap_abf",
            UvAction::UnwrapGroups => "uv.unwrap_groups",
            UvAction::Relax => "uv.relax",
            UvAction::Straighten => "uv.straighten",
            UvAction::Stitch => "uv.stitch",
            UvAction::Transfer => "uv.transfer",
            UvAction::Pack => "uv.pack",
            UvAction::ProjectPlanar => "uv.project_planar",
            UvAction::ProjectBox => "uv.project_box",
            UvAction::ProjectCylindrical => "uv.project_cylindrical",
            UvAction::ProjectSpherical => "uv.project_spherical",
            UvAction::ProjectCamera => "uv.project_camera",
            UvAction::ExportLayout => "uv.export_layout",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            UvAction::Editor => "Cmd+Shift+U",
            UvAction::MarkSeam => "Model: Alt+M",
            UvAction::ClearSeam => "Model: Alt+Shift+M",
            UvAction::Unwrap => "Model: Alt+U",
            UvAction::Pack => "Model: Alt+P",
            _ => return None,
        })
    }
}

/// Everything about a mesh's layout that the view needs; recomputed when the mesh changes.
pub(crate) struct Derived {
    pub(crate) mesh: Arc<PolyMesh>,
    pub(crate) charts: Charts,
    metrics: Vec<TriangleMetric>,
    summary: Distortion,
    /// Texel density of each island: UV area over surface area, as a square root (UV units per metre).
    density: Vec<f32>,
}

fn derive(mesh: Arc<PolyMesh>) -> Derived {
    let seams = SeamSet::from_uv_discontinuities(&mesh);
    let charts = Charts::build(&mesh, &seams).unwrap_or_default();
    let (summary, tri) = measure(&mesh);
    let metrics = per_face(&mesh, &tri);
    let mut density = Vec::with_capacity(charts.islands.len());
    for isl in &charts.islands {
        let (mut a3, mut auv) = (0.0f32, 0.0f32);
        let mut tris = Vec::new();
        for &f in &isl.faces {
            tris.clear();
            face_triangles(&mesh, f, &mut tris);
            for t in &tris {
                let p = t.map(|c| mesh.position(mesh.origin(c)));
                let u = t.map(|c| mesh.uv(c));
                a3 += 0.5 * (p[1] - p[0]).cross(p[2] - p[0]).length();
                auv += 0.5 * (u[1] - u[0]).perp_dot(u[2] - u[0]).abs();
            }
        }
        density.push(if a3 > 0.0 { (auv / a3).sqrt() } else { 0.0 });
    }
    Derived { mesh, charts, metrics, summary, density }
}

/// A pointer gesture in the canvas.
enum Gesture {
    /// Box selection from this screen point.
    Box(Pos2),
    /// Moving, rotating or scaling the selection; `t` is the transform so far.
    Transform { tool: UvTool, start: Pos2, t: UvTransform },
}

pub struct UvUi {
    pub open: bool,
    /// The editor takes keyboard focus when it is drawn next.
    pub focus_pending: bool,
    /// The canvas had keyboard focus last frame (it then handles its keys before the app does).
    pub had_focus: bool,
    pub mode: UvSelect,
    pub tool: UvTool,
    /// Selected corners (every corner of each selected UV vertex, edge end, face or island).
    pub selection: BTreeSet<u32>,
    /// Corners held in place when the mesh is unwrapped again.
    pub pinned: BTreeSet<u32>,
    /// UV at the top left of the canvas and screen pixels per UV unit.
    pub origin: Vec2,
    pub zoom: f32,
    pub fitted: bool,
    pub checker: bool,
    /// Draw a checker from the UVs on the meshes in the 3D viewport.
    pub viewport_checker: bool,
    pub heat: Heat,
    pub show_seams: bool,
    pub pivot: Pivot,
    /// Numeric fields: move, rotate (degrees), scale.
    pub move_by: Vec2,
    pub rotate_deg: f32,
    pub scale_by: Vec2,
    /// Distance of one arrow-key nudge, in UV units.
    pub nudge: f32,
    /// Angle for "mark seams by angle", in degrees.
    pub seam_angle: f32,
    /// The solver `Unwrap UVs` uses.
    pub unwrap_method: UnwrapMethod,
    /// Iterations and fit of `Relax UVs`.
    pub relax_iterations: u32,
    pub relax_mode: RelaxMode,
    /// Pack options: gap between islands (fraction of a tile), rotation, density, and a texel
    /// density (UV units per metre) that decides how many tiles are used.
    pub pack_margin: f32,
    pub pack_rotation: Rotation,
    pub pack_density: Density,
    pub pack_fixed_density: bool,
    pub pack_texel_density: f32,
    /// Width over height of the active 3D view, as last drawn (for a projection from the view).
    pub view_aspect: f32,
    /// UDIM tiles `Pack UV Islands` spreads the islands over (1 is the unit square).
    pub pack_tiles: u32,
    /// `Stitch UV Islands` also resizes the moved island so the edges are as long as its neighbour's.
    pub stitch_scale: bool,
    /// Position of the keyboard element cursor (Page Up and Page Down walk the elements).
    cursor: usize,
    gesture: Option<Gesture>,
    target: Option<ObjectId>,
    derived: Option<(usize, Arc<Derived>)>,
    hover_text: String,
}

impl Default for UvUi {
    fn default() -> Self {
        UvUi {
            open: false,
            focus_pending: false,
            had_focus: false,
            mode: UvSelect::Vertex,
            tool: UvTool::Move,
            selection: BTreeSet::new(),
            pinned: BTreeSet::new(),
            origin: Vec2::new(-0.1, -0.1),
            zoom: 400.0,
            fitted: false,
            checker: true,
            viewport_checker: false,
            heat: Heat::Off,
            show_seams: true,
            pivot: Pivot::Selection,
            move_by: Vec2::ZERO,
            rotate_deg: 0.0,
            scale_by: Vec2::ONE,
            nudge: 0.01,
            seam_angle: 30.0,
            unwrap_method: UnwrapMethod::Lscm,
            relax_iterations: 10,
            relax_mode: RelaxMode::Rigid,
            stitch_scale: false,
            pack_tiles: 1,
            pack_margin: 0.01,
            pack_rotation: Rotation::Fit,
            pack_density: Density::Uniform,
            pack_fixed_density: false,
            pack_texel_density: 1.0,
            view_aspect: 16.0 / 9.0,
            cursor: 0,
            gesture: None,
            target: None,
            derived: None,
            hover_text: String::new(),
        }
    }
}

// ------------------------------------------------------------------ data

/// The object whose layout the editor shows and its mesh before the next step (the mesh that the
/// corner numbers of new steps refer to).
pub(crate) fn subject(app: &mut ModelApp) -> Option<(ObjectId, Arc<PolyMesh>)> {
    let id = app.edit_target()?;
    let mesh = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    Some((id, mesh))
}

/// Steps that leave vertex numbers alone, so seams recorded before them still name the same edges.
fn keeps_vertex_numbers(op: &StepOp) -> bool {
    matches!(
        op,
        StepOp::Seams { .. }
            | StepOp::TransformUvs { .. }
            | StepOp::StitchUvs { .. }
            | StepOp::RelaxUvs { .. }
            | StepOp::StraightenUvs { .. }
            | StepOp::Unwrap { .. }
            | StepOp::SetPatch { .. }
            | StepOp::Flip { .. }
    )
}

/// The seams of the latest `Seams` step, if only steps that keep vertex numbers follow it.
pub fn current_seams(app: &ModelApp, id: ObjectId) -> SeamSet {
    let Some(o) = app.doc.scene.get(id) else { return SeamSet::new() };
    for (i, s) in o.steps().iter().enumerate().rev() {
        if let StepOp::Seams { edges } = &s.op
            && s.enabled
        {
            return if o.steps().iter().skip(i + 1).all(|t| !t.enabled || keeps_vertex_numbers(&t.op)) {
                SeamSet::from_vertex_pairs(edges)
            } else {
                SeamSet::new()
            };
        }
    }
    SeamSet::new()
}

pub(crate) fn derived(app: &mut ModelApp, mesh: &Arc<PolyMesh>) -> Arc<Derived> {
    let key = Arc::as_ptr(mesh) as usize;
    if let Some((k, d)) = &app.uv.derived
        && *k == key
    {
        return d.clone();
    }
    let d = Arc::new(derive(mesh.clone()));
    app.uv.derived = Some((key, d.clone()));
    d
}

/// Edges of `mesh` both of whose ends (on one side) are in the selection: what an edge, weld or
/// seam command acts on.
fn selected_edges(mesh: &PolyMesh, selection: &BTreeSet<u32>) -> Vec<u32> {
    let mut out = BTreeSet::new();
    for &h in selection {
        if (h as usize) < mesh.corner_count() && selection.contains(&mesh.next(h)) {
            out.insert(mesh.edge_of(h));
        }
    }
    out.into_iter().collect()
}

// ------------------------------------------------------------------ actions

fn name(ui: &egui::Ui, r: &egui::Response, label: impl Into<String>) {
    let label = label.into();
    ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(label));
}

pub fn act(app: &mut ModelApp, a: UvAction) {
    match a {
        UvAction::Editor => toggle(app),
        UvAction::MarkSeam => mark_seams(app, true),
        UvAction::ClearSeam => mark_seams(app, false),
        UvAction::SeamsByAngle => seams_by_angle(app),
        UvAction::Unwrap => unwrap(app),
        UvAction::UnwrapAbf => unwrap_with(app, UnwrapMethod::Abf),
        UvAction::UnwrapGroups => unwrap_op(app, app.uv.unwrap_method, true),
        UvAction::Relax => relax(app),
        UvAction::Straighten => straighten(app),
        UvAction::Stitch => stitch(app),
        UvAction::Transfer => transfer(app),
        UvAction::Pack => pack(app),
        UvAction::ProjectPlanar => project(app, Projection::Planar { from: Axis::PosY }),
        UvAction::ProjectBox => project(app, Projection::Box),
        UvAction::ProjectCylindrical => project(app, Projection::Cylindrical { axis: Axis::PosY }),
        UvAction::ProjectSpherical => project(app, Projection::Spherical { axis: Axis::PosY }),
        UvAction::ProjectCamera => project_from_view(app),
        UvAction::ExportLayout => export_layout(app),
    }
}

pub fn toggle(app: &mut ModelApp) {
    app.uv.open = !app.uv.open;
    if app.uv.open {
        app.uv.focus_pending = true;
        app.uv.fitted = false;
    } else {
        app.uv.had_focus = false;
    }
}

/// Write `seams` as the object's current `Seams` step: edit the last step when it is one, add a new one otherwise.
fn write_seams(app: &mut ModelApp, id: ObjectId, seams: &SeamSet) {
    let op = StepOp::Seams { edges: seams.to_vertex_pairs() };
    let last =
        app.doc.scene.get(id).and_then(|o| o.steps().len().checked_sub(1).map(|i| (i, matches!(o.steps().get(i).map(|s| &s.op), Some(StepOp::Seams { .. })))));
    let ok = match last {
        Some((index, true)) => app.run(Command::SetStep { id, index, op }),
        _ => app.run(Command::AddStep { id, op }),
    };
    if ok {
        app.selection = vec![id];
    }
}

/// Mark or clear the seams on the selected edges: the edges selected in the UV editor when it has
/// an edge selection, otherwise those selected in the viewport (edge mode).
pub fn mark_seams(app: &mut ModelApp, mark: bool) {
    let Some((id, mesh)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let from_uv = app.uv.open && !app.uv.selection.is_empty() && mesh.has_uvs();
    let edges: Vec<u32> = if from_uv {
        selected_edges(&mesh, &app.uv.selection)
    } else if app.mode == crate::app::SelectMode::Edge {
        app.components.iter().copied().collect()
    } else {
        Vec::new()
    };
    if edges.is_empty() {
        app.status = tl!("Select edges first (edge mode, or UV edges in the UV editor)");
        return;
    }
    let mut seams = current_seams(app, id);
    let changed = if mark { seams.mark_edges(&mesh, &edges) } else { seams.clear_edges(&mesh, &edges) };
    if changed == 0 {
        app.status = if mark { tl!("Those edges are already seams") } else { tl!("None of those edges is a seam") };
        return;
    }
    write_seams(app, id, &seams);
}

pub fn seams_by_angle(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let mut seams = current_seams(app, id);
    let n = seams.mark_by_angle(&mesh, app.uv.seam_angle);
    if n == 0 {
        app.status = tl!("No edge is sharper than that angle");
        return;
    }
    write_seams(app, id, &seams);
}

/// Unwrap the mesh along its seams with the chosen solver, keeping the pinned corners where they are.
pub fn unwrap(app: &mut ModelApp) {
    unwrap_with(app, app.uv.unwrap_method);
}

/// Unwrap with a given solver (see [`UnwrapMethod`]).
pub fn unwrap_with(app: &mut ModelApp, method: UnwrapMethod) {
    unwrap_op(app, method, false);
}

/// Unwrap, cutting along the borders between polygon groups too when `from_patches` is set, so
/// that every group is an island.
fn unwrap_op(app: &mut ModelApp, method: UnwrapMethod, from_patches: bool) {
    let Some((id, mesh)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let seams = current_seams(app, id);
    let pins: Vec<UvPin> = app
        .uv
        .pinned
        .iter()
        .filter(|c| (**c as usize) < mesh.corner_count() && mesh.has_uvs())
        .map(|c| UvPin { corner: *c, uv: mesh.uv(*c).to_array() })
        .collect();
    let op = StepOp::Unwrap { seams: seams.to_vertex_pairs(), angle: None, pins, auto_cut: true, margin: 0.01, method, from_patches };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
        app.uv.selection.clear();
    }
}

/// Pack the islands the UVs form into the unit square (margin, best-fit rotation, even density).
pub fn pack(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    if !mesh.has_uvs() {
        app.status = tl!("This mesh has no UVs yet. Project or unwrap it to make some.");
        return;
    }
    let u = &app.uv;
    let op = StepOp::PackUvs {
        margin: u.pack_margin,
        rotation: u.pack_rotation,
        density: if u.pack_fixed_density { Density::Uniform } else { u.pack_density },
        tiles: u.pack_tiles,
        texel_density: u.pack_fixed_density.then_some(u.pack_texel_density),
    };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
        app.uv.selection.clear();
        app.uv.pinned.clear();
        app.uv.fitted = false;
    }
}

/// Project UVs onto the whole mesh, or onto the faces selected in the viewport (face mode).
pub fn project(app: &mut ModelApp, projection: Projection) {
    let Some((id, _)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let faces: Vec<u32> = if app.mode == crate::app::SelectMode::Face { app.components.iter().copied().collect() } else { Vec::new() };
    let op = StepOp::ProjectUvs { projection, fit: Fit::Uniform, faces };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
        app.uv.selection.clear();
        app.uv.pinned.clear();
        app.uv.fitted = false;
    }
}

/// Project UVs as the 3D view sees the mesh (the view's camera and the object's transform), so the
/// texture lines up with what is on the screen.
pub fn project_from_view(app: &mut ModelApp) {
    let Some((id, _)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let cam = app.views[app.active_view.min(3)].camera;
    let aspect = if app.uv.view_aspect.is_finite() && app.uv.view_aspect > 0.0 { app.uv.view_aspect } else { 1.0 };
    let view_proj = (cam.view_proj(aspect) * app.doc.scene.world_matrix(id)).to_cols_array();
    project(app, Projection::Camera { view_proj });
}

/// Ask for a file and write the layout of the selected mesh as SVG or PNG.
pub fn export_layout(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let Some(path) = rfd::FileDialog::new().add_filter("SVG", &["svg"]).add_filter("PNG", &["png"]).set_file_name("uv-layout.svg").save_file() else { return };
    // A layout over several UDIM tiles is one picture per tile, named name.1001.png and so on.
    if modelcraft_engine::uvpack::udim_tiles(&mesh).len() > 1 {
        app.status = match app.doc.export_uv_layout_udim(&path, Some(id), 2048) {
            Ok(files) => crate::tlf!("Exported {count} UDIM tiles next to {file}", count = files.len(), file = path.display()),
            Err(e) => crate::tlf!("Export failed: {error}", error = e),
        };
        return;
    }
    app.status = match app.doc.export_uv_layout(&path, Some(id), 2048) {
        Ok(()) => crate::tlf!("Exported {file}", file = path.display()),
        Err(e) => crate::tlf!("Export failed: {error}", error = e),
    };
}

fn transform_selection(app: &mut ModelApp, t: UvTransform) {
    let Some((id, mesh)) = subject(app) else { return };
    if app.uv.selection.is_empty() || !mesh.has_uvs() {
        app.status = tl!("Select UVs first");
        return;
    }
    if t.is_identity() {
        return;
    }
    let corners: Vec<u32> = app.uv.selection.iter().copied().collect();
    let op =
        StepOp::TransformUvs { corners, translate: t.translate.to_array(), rotate_deg: t.rotate_deg, scale: t.scale.to_array(), pivot: t.pivot.to_array() };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
    }
}

fn pivot_point(ui: &UvUi, mesh: &PolyMesh) -> Vec2 {
    match ui.pivot {
        Pivot::Selection => modelcraft_engine::uv::bounds(mesh, &ui.selection).map_or(Vec2::splat(0.5), |(lo, hi)| 0.5 * (lo + hi)),
        Pivot::Centre => Vec2::splat(0.5),
        Pivot::Origin => Vec2::ZERO,
    }
}

fn apply_numeric(app: &mut ModelApp) {
    let Some((_, mesh)) = subject(app) else { return };
    let u = &app.uv;
    let t = UvTransform { translate: u.move_by, rotate_deg: u.rotate_deg, scale: u.scale_by, pivot: pivot_point(u, &mesh) };
    transform_selection(app, t);
    app.uv.move_by = Vec2::ZERO;
    app.uv.rotate_deg = 0.0;
    app.uv.scale_by = Vec2::ONE;
}

fn flip(app: &mut ModelApp, horizontal: bool) {
    let Some((_, mesh)) = subject(app) else { return };
    let scale = if horizontal { Vec2::new(-1.0, 1.0) } else { Vec2::new(1.0, -1.0) };
    let t = UvTransform { scale, pivot: pivot_point(&app.uv, &mesh), ..UvTransform::default() };
    transform_selection(app, t);
}

fn nudge(app: &mut ModelApp, dx: f32, dy: f32, big: bool) {
    let d = app.uv.nudge * if big { 10.0 } else { 1.0 };
    transform_selection(app, UvTransform { translate: Vec2::new(dx, dy) * d, ..UvTransform::default() });
}

fn rotate_by(app: &mut ModelApp, deg: f32) {
    let Some((_, mesh)) = subject(app) else { return };
    let t = UvTransform { rotate_deg: deg, pivot: pivot_point(&app.uv, &mesh), ..UvTransform::default() };
    transform_selection(app, t);
}

fn scale_by(app: &mut ModelApp, f: f32) {
    let Some((_, mesh)) = subject(app) else { return };
    let t = UvTransform { scale: Vec2::splat(f), pivot: pivot_point(&app.uv, &mesh), ..UvTransform::default() };
    transform_selection(app, t);
}

fn weld(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else { return };
    let edges = selected_edges(&mesh, &app.uv.selection);
    if edges.is_empty() {
        app.status = tl!("Select the UV edges to weld first");
        return;
    }
    if app.run(Command::AddStep { id, op: StepOp::StitchUvs { edges, align: false, scale: false } }) {
        app.selection = vec![id];
        app.uv.selection.clear();
    }
}

/// Sew the selected edges: move the island on one side onto the other, then weld.
fn stitch(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else { return };
    let edges = selected_edges(&mesh, &app.uv.selection);
    if edges.is_empty() {
        app.status = tl!("Select the UV edges to stitch first");
        return;
    }
    let op = StepOp::StitchUvs { edges, align: true, scale: app.uv.stitch_scale };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
        app.uv.selection.clear();
    }
}

/// Copy the UVs of the first selected mesh onto the second, by closest point on the surface.
fn transfer(app: &mut ModelApp) {
    let ids = app.selection.clone();
    let [source, id] = ids[..] else {
        app.status = tl!("Select two meshes: the one with the UVs first, then the one to give them to");
        return;
    };
    if app.run(Command::TransferUvs { id, source, max_distance: None, per_island: true }) {
        app.selection = vec![id];
        app.uv.selection.clear();
        app.uv.fitted = false;
    }
}

/// Relax the selected UVs (the whole layout when nothing is selected), the rest staying put.
fn relax(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else { return };
    if !mesh.has_uvs() {
        app.status = tl!("This mesh has no UVs yet. Project or unwrap it to make some.");
        return;
    }
    let corners: Vec<u32> = app.uv.selection.iter().copied().collect();
    let op = StepOp::RelaxUvs { corners, iterations: app.uv.relax_iterations, mode: app.uv.relax_mode };
    if app.run(Command::AddStep { id, op }) {
        app.selection = vec![id];
    }
}

/// Lay each selected chain of UV edges on the line between its ends.
fn straighten(app: &mut ModelApp) {
    let Some((id, mesh)) = subject(app) else { return };
    if selected_edges(&mesh, &app.uv.selection).is_empty() {
        app.status = tl!("Select a chain of UV edges to straighten first");
        return;
    }
    let corners: Vec<u32> = app.uv.selection.iter().copied().collect();
    if app.run(Command::AddStep { id, op: StepOp::StraightenUvs { corners, strength: 1.0 } }) {
        app.selection = vec![id];
    }
}

fn pin(app: &mut ModelApp, on: bool) {
    let sel = app.uv.selection.clone();
    if sel.is_empty() {
        app.status = tl!("Select UVs first");
        return;
    }
    if on {
        app.uv.pinned.extend(sel);
        app.status = crate::tlf!("{count} UV points pinned", count = app.uv.pinned.len());
    } else {
        for c in &sel {
            app.uv.pinned.remove(c);
        }
        app.status = crate::tlf!("{count} UV points pinned", count = app.uv.pinned.len());
    }
}

// ------------------------------------------------------------------ keys

/// Keys while the canvas has focus. Called before the app's own shortcuts; returns whether it
/// used a key, so that "1" there selects UV vertices and not objects.
pub fn keys(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if !app.uv.open || !app.uv.had_focus {
        return false;
    }
    let mut used = true;
    let m = ctx.input(|i| i.modifiers);
    let (shift, cmd) = (m.shift, m.command);
    let pressed = |k: Key, mods: egui::Modifiers| ctx.input_mut(|i| i.consume_key(mods, k));
    let plain = egui::Modifiers::NONE;
    let sh = egui::Modifiers::SHIFT;
    if pressed(Key::Escape, plain) {
        if app.uv.selection.is_empty() {
            app.uv.open = false;
            app.uv.had_focus = false;
        } else {
            app.uv.selection.clear();
        }
    } else if pressed(Key::A, egui::Modifiers::COMMAND) {
        select_all(app);
    } else if pressed(Key::Z, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT) {
        // The keymap leaves Cmd+Z to a focused widget (it may be a text field); the canvas is not one.
        app.act(ctx, crate::actions::Action::Redo);
    } else if pressed(Key::Z, egui::Modifiers::COMMAND) {
        app.act(ctx, crate::actions::Action::Undo);
    } else if pressed(Key::Num1, plain) {
        app.uv.mode = UvSelect::Vertex;
    } else if pressed(Key::Num2, plain) {
        app.uv.mode = UvSelect::Edge;
    } else if pressed(Key::Num3, plain) {
        app.uv.mode = UvSelect::Face;
    } else if pressed(Key::Num4, plain) {
        app.uv.mode = UvSelect::Island;
    } else if pressed(Key::Q, plain) {
        app.uv.tool = UvTool::Select;
    } else if pressed(Key::W, plain) {
        app.uv.tool = UvTool::Move;
    } else if pressed(Key::E, plain) {
        app.uv.tool = UvTool::Rotate;
    } else if pressed(Key::R, plain) {
        app.uv.tool = UvTool::Scale;
    } else if pressed(Key::P, sh) {
        pin(app, false);
    } else if pressed(Key::P, plain) {
        pin(app, true);
    } else if pressed(Key::J, plain) {
        weld(app);
    } else if pressed(Key::M, sh) {
        mark_seams(app, false);
    } else if pressed(Key::M, plain) {
        mark_seams(app, true);
    } else if pressed(Key::U, plain) {
        unwrap(app);
    } else if pressed(Key::K, plain) {
        pack(app);
    } else if pressed(Key::PageDown, plain) || pressed(Key::PageDown, sh) {
        step_selection(app, true, shift);
    } else if pressed(Key::PageUp, plain) || pressed(Key::PageUp, sh) {
        step_selection(app, false, shift);
    } else if pressed(Key::F, plain) || pressed(Key::Home, plain) {
        app.uv.fitted = false;
    } else if pressed(Key::Plus, plain) || pressed(Key::Plus, sh) || pressed(Key::Equals, plain) {
        app.uv.zoom *= 1.25;
    } else if pressed(Key::Minus, plain) {
        app.uv.zoom /= 1.25;
    } else if pressed(Key::OpenBracket, plain) || pressed(Key::OpenBracket, sh) {
        rotate_by(app, if shift { -90.0 } else { -15.0 });
    } else if pressed(Key::CloseBracket, plain) || pressed(Key::CloseBracket, sh) {
        rotate_by(app, if shift { 90.0 } else { 15.0 });
    } else if pressed(Key::Comma, plain) || pressed(Key::Comma, sh) {
        scale_by(app, if shift { 0.5 } else { 1.0 / 1.1 });
    } else if pressed(Key::Period, plain) || pressed(Key::Period, sh) {
        scale_by(app, if shift { 2.0 } else { 1.1 });
    } else if let Some((dx, dy)) = arrow(ctx, plain).or_else(|| arrow(ctx, sh)) {
        nudge(app, dx, dy, shift);
    } else {
        used = false;
    }
    let _ = cmd;
    used
}

fn arrow(ctx: &egui::Context, mods: egui::Modifiers) -> Option<(f32, f32)> {
    for (k, dx, dy) in [(Key::ArrowLeft, -1.0, 0.0), (Key::ArrowRight, 1.0, 0.0), (Key::ArrowUp, 0.0, -1.0), (Key::ArrowDown, 0.0, 1.0)] {
        if ctx.input_mut(|i| i.consume_key(mods, k)) {
            return Some((dx, dy));
        }
    }
    None
}

/// Select the next or previous element in the current mode (the keyboard way to pick one):
/// UV vertices, edges, faces or islands in mesh order, wrapping around. With `extend` the
/// element is added to the selection.
fn step_selection(app: &mut ModelApp, forward: bool, extend: bool) {
    let Some((_, mesh)) = subject(app) else { return };
    if !mesh.has_uvs() {
        return;
    }
    let d = derived(app, &mesh);
    let count = match app.uv.mode {
        UvSelect::Vertex => d.charts.vertex_source.len(),
        UvSelect::Edge => mesh.corner_count(),
        UvSelect::Face => mesh.face_count(),
        UvSelect::Island => d.charts.islands.len(),
    };
    if count == 0 {
        return;
    }
    let next = if app.uv.selection.is_empty() && forward {
        0
    } else if forward {
        (app.uv.cursor + 1) % count
    } else {
        (app.uv.cursor + count - 1) % count
    };
    app.uv.cursor = next;
    let corners: BTreeSet<u32> = match app.uv.mode {
        UvSelect::Vertex => (0..mesh.corner_count() as u32).filter(|c| d.charts.corner_vertex.get(*c as usize) == Some(&(next as u32))).collect(),
        UvSelect::Edge => {
            let h = next as u32;
            expand_to_uv_vertices(&mesh, &d.charts, &[h, mesh.next(h)].into())
        }
        UvSelect::Face => mesh.face_half_edges(next as u32).collect(),
        UvSelect::Island => d.charts.islands.get(next).map(|i| i.faces.iter().flat_map(|f| mesh.face_half_edges(*f)).collect()).unwrap_or_default(),
    };
    if extend {
        app.uv.selection.extend(corners);
    } else {
        app.uv.selection = corners;
    }
    app.status = crate::tlf!("{mode}: {index} of {count}", mode = app.uv.mode.label(), index = next + 1, count = count);
}

fn select_all(app: &mut ModelApp) {
    if let Some((_, mesh)) = subject(app) {
        app.uv.selection = (0..mesh.corner_count() as u32).collect();
    }
}

// ------------------------------------------------------------------ viewport overlay

/// Seams of the selected mesh as red lines in the 3D viewport.
pub fn seam_lines(app: &mut ModelApp) -> Vec<LineVertex> {
    let mut out = Vec::new();
    let Some(id) = app.edit_target() else { return out };
    let seams = current_seams(app, id);
    if seams.is_empty() {
        return out;
    }
    let mesh = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    let w = app.doc.scene.world_matrix(id);
    let color = [0.95, 0.2, 0.2, 1.0];
    for e in seams.edge_ids(&mesh).into_iter().take(500_000) {
        let (a, b) = mesh.edge_vertices(e);
        for v in [a, b] {
            out.push(LineVertex { pos: w.transform_point3(mesh.position(v)).to_array(), color });
        }
    }
    out
}

// ------------------------------------------------------------------ menu

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.menu_button(tl!("UV"), |ui| {
        for a in UvAction::ALL {
            let b = egui::Button::new(a.label()).shortcut_text(crate::keys::shortcut_text(app, crate::actions::Action::Uv(a)));
            if ui.add(b).clicked() {
                app.act(&ctx, crate::actions::Action::Uv(a));
                ui.close();
            }
        }
    });
}

// ------------------------------------------------------------------ the window

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.uv.open {
        return;
    }
    let mut open = true;
    let focus_pending = std::mem::take(&mut app.uv.focus_pending);
    egui::Window::new(tl!("UV Editor")).open(&mut open).default_size([760.0, 600.0]).resizable(true).show(ctx, |ui| window_body(app, ui, focus_pending));
    if !open {
        app.uv.open = false;
        app.uv.had_focus = false;
    }
}

fn window_body(app: &mut ModelApp, ui: &mut egui::Ui, focus_pending: bool) {
    let Some((id, mesh)) = subject(app) else {
        ui.label(tl!("Select a mesh object to edit its UVs."));
        app.uv.had_focus = false;
        return;
    };
    if app.uv.target != Some(id) {
        app.uv.target = Some(id);
        app.uv.selection.clear();
        app.uv.pinned.clear();
        app.uv.fitted = false;
    }
    // Corner numbers belong to one mesh: drop what no longer fits it.
    let n = mesh.corner_count() as u32;
    app.uv.selection.retain(|c| *c < n);
    app.uv.pinned.retain(|c| *c < n);
    if !mesh.has_uvs() {
        ui.label(tl!("This mesh has no UVs yet. Project or unwrap it to make some."));
        let r = ui.button(tl!("Unwrap UVs"));
        name(ui, &r, tl!("Unwrap UVs"));
        if r.clicked() {
            unwrap(app);
        }
        app.uv.had_focus = false;
        return;
    }
    let d = derived(app, &mesh);

    toolbar(app, ui);
    warnings(app, ui, id);
    ui.separator();
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| canvas(app, ui, &d, focus_pending));
        ui.separator();
        ui.vertical(|ui| side_panel(app, ui, &d));
    });
}

/// What the history steps of the mesh report went less well than asked: an unwrap that fell back,
/// a solve that did not converge. Shown under the toolbar, where the cause was.
fn warnings(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let e = app.doc.eval.evaluate(&app.doc.scene, id);
    for w in e.warnings.iter().take(4) {
        let text = crate::tlf!("Warning: {message}", message = w.message);
        let r = ui.colored_label(Color32::from_rgb(240, 180, 60), text.clone());
        name(ui, &r, text);
    }
    if e.warnings.len() > 4 {
        ui.label(crate::tlf!("{count} more warnings in the build history", count = e.warnings.len() - 4));
    }
}

fn toolbar(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Select"));
        for m in UvSelect::ALL {
            let r = ui.selectable_label(app.uv.mode == m, m.label());
            name(ui, &r, m.label());
            if r.clicked() {
                app.uv.mode = m;
            }
        }
        ui.separator();
        ui.label(tl!("Tool"));
        for t in UvTool::ALL {
            let r = ui.selectable_label(app.uv.tool == t, t.label());
            name(ui, &r, crate::tlf!("{tool} tool", tool = t.label()));
            if r.clicked() {
                app.uv.tool = t;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Unwrap solver"));
        let methods = [(UnwrapMethod::Lscm, tl!("Conformal (LSCM)")), (UnwrapMethod::Abf, tl!("Angle-based (ABF)"))];
        let shown = methods.iter().find(|(m, _)| *m == app.uv.unwrap_method).map(|(_, l)| l.clone()).unwrap_or_default();
        let r = egui::ComboBox::from_id_salt("uv-unwrap-method").selected_text(shown).show_ui(ui, |ui| {
            for (m, l) in &methods {
                ui.selectable_value(&mut app.uv.unwrap_method, *m, l.clone());
            }
        });
        name(ui, &r.response, tl!("Unwrap solver"));
        for (label, f) in
            [(tl!("Pin"), 0), (tl!("Unpin"), 1), (tl!("Weld"), 2), (tl!("Mark Seam"), 3), (tl!("Clear Seam"), 4), (tl!("Unwrap UVs"), 5), (tl!("Frame"), 6)]
        {
            let r = ui.button(label.clone());
            name(ui, &r, label);
            if r.clicked() {
                match f {
                    0 => pin(app, true),
                    1 => pin(app, false),
                    2 => weld(app),
                    3 => mark_seams(app, true),
                    4 => mark_seams(app, false),
                    5 => unwrap(app),
                    _ => app.uv.fitted = false,
                }
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Tidy"));
        for (label, a) in [
            (tl!("Relax UVs"), UvAction::Relax),
            (tl!("Straighten UVs"), UvAction::Straighten),
            (tl!("Stitch UV Islands"), UvAction::Stitch),
            (tl!("Transfer UVs"), UvAction::Transfer),
        ] {
            let r = ui.button(label.clone());
            name(ui, &r, label);
            if r.clicked() {
                act(app, a);
            }
        }
        ui.label(tl!("Relax iterations"));
        let r = ui.add(egui::DragValue::new(&mut app.uv.relax_iterations).speed(0.2).range(1..=200));
        name(ui, &r, tl!("Relax iterations"));
        let modes = [(RelaxMode::Rigid, tl!("Keep lengths")), (RelaxMode::Conformal, tl!("Keep angles"))];
        let shown = modes.iter().find(|(m, _)| *m == app.uv.relax_mode).map(|(_, l)| l.clone()).unwrap_or_default();
        let r = egui::ComboBox::from_id_salt("uv-relax-mode").selected_text(shown).show_ui(ui, |ui| {
            for (m, l) in &modes {
                ui.selectable_value(&mut app.uv.relax_mode, *m, l.clone());
            }
        });
        name(ui, &r.response, tl!("Relax fit"));
        let r = ui.checkbox(&mut app.uv.stitch_scale, tl!("Match scale when stitching"));
        name(ui, &r, tl!("Match scale when stitching"));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Layout"));
        ui.label(tl!("UDIM tiles"));
        let r = ui.add(egui::DragValue::new(&mut app.uv.pack_tiles).speed(0.1).range(1..=100));
        name(ui, &r, tl!("UDIM tiles"));
        // The visible label is the accessible name, so a voice-control user can say what they see.
        for a in [
            UvAction::Pack,
            UvAction::ProjectPlanar,
            UvAction::ProjectBox,
            UvAction::ProjectCylindrical,
            UvAction::ProjectSpherical,
            UvAction::ProjectCamera,
            UvAction::ExportLayout,
        ] {
            let label = a.label();
            let r = ui.button(label.clone());
            name(ui, &r, label);
            if r.clicked() {
                act(app, a);
            }
        }
        pack_options(app, ui);
    });
}

/// The Pack Options popover: margin, rotation, density and tiles for `Pack UV Islands`.
fn pack_options(app: &mut ModelApp, ui: &mut egui::Ui) {
    let r = ui.menu_button(tl!("Pack Options…"), |ui| {
        ui.horizontal(|ui| {
            ui.label(tl!("Margin"));
            let r = ui.add(egui::DragValue::new(&mut app.uv.pack_margin).speed(0.0005).range(0.0..=0.1).max_decimals(4));
            name(ui, &r, tl!("Pack margin"));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Rotation"));
            let rotations = [(Rotation::None, tl!("None")), (Rotation::Quarter, tl!("90 degrees")), (Rotation::Fit, tl!("Best fit"))];
            let shown = rotations.iter().find(|(m, _)| *m == app.uv.pack_rotation).map(|(_, l)| l.clone()).unwrap_or_default();
            let r = egui::ComboBox::from_id_salt("uv-pack-rotation").selected_text(shown).show_ui(ui, |ui| {
                for (m, l) in &rotations {
                    ui.selectable_value(&mut app.uv.pack_rotation, *m, l.clone());
                }
            });
            name(ui, &r.response, tl!("Pack rotation"));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Island sizes"));
            let densities = [(Density::Uniform, tl!("Even texel density")), (Density::Keep, tl!("Keep sizes"))];
            let shown = densities.iter().find(|(m, _)| *m == app.uv.pack_density).map(|(_, l)| l.clone()).unwrap_or_default();
            let r = ui.add_enabled_ui(!app.uv.pack_fixed_density, |ui| {
                egui::ComboBox::from_id_salt("uv-pack-density").selected_text(shown).show_ui(ui, |ui| {
                    for (m, l) in &densities {
                        ui.selectable_value(&mut app.uv.pack_density, *m, l.clone());
                    }
                })
            });
            name(ui, &r.inner.response, tl!("Pack island sizes"));
        });
        let r = ui.checkbox(&mut app.uv.pack_fixed_density, tl!("Fixed texel density (as many tiles as needed)"));
        name(ui, &r, tl!("Fixed texel density"));
        if app.uv.pack_fixed_density {
            ui.horizontal(|ui| {
                ui.label(tl!("Texel density (tiles per metre)"));
                let r = ui.add(egui::DragValue::new(&mut app.uv.pack_texel_density).speed(0.01).range(0.001..=1000.0).max_decimals(3));
                name(ui, &r, tl!("Texel density"));
            });
        } else {
            ui.horizontal(|ui| {
                ui.label(tl!("UDIM tiles"));
                let r = ui.add(egui::DragValue::new(&mut app.uv.pack_tiles).speed(0.1).range(1..=100));
                name(ui, &r, tl!("Pack tiles"));
            });
        }
        let b = ui.button(tl!("Pack Now"));
        name(ui, &b, tl!("Pack Now"));
        if b.clicked() {
            pack(app);
            ui.close();
        }
    });
    name(ui, &r.response, tl!("Pack Options…"));
}

fn side_panel(app: &mut ModelApp, ui: &mut egui::Ui, d: &Derived) {
    ui.set_min_width(200.0);
    ui.label(egui::RichText::new(tl!("Transform by Numbers")).strong());
    let mut apply = false;
    for (label, v, speed) in [(tl!("Move U"), &mut app.uv.move_by.x, 0.005), (tl!("Move V"), &mut app.uv.move_by.y, 0.005)] {
        ui.horizontal(|ui| {
            ui.label(label.clone());
            let r = ui.add(egui::DragValue::new(v).speed(speed).max_decimals(4));
            name(ui, &r, label);
            apply |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        });
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Rotate (degrees)"));
        let r = ui.add(egui::DragValue::new(&mut app.uv.rotate_deg).speed(0.5).max_decimals(2));
        name(ui, &r, tl!("Rotate (degrees)"));
        apply |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    });
    for (label, v) in [(tl!("Scale U"), &mut app.uv.scale_by.x), (tl!("Scale V"), &mut app.uv.scale_by.y)] {
        ui.horizontal(|ui| {
            ui.label(label.clone());
            let r = ui.add(egui::DragValue::new(v).speed(0.01).max_decimals(4));
            name(ui, &r, label);
            apply |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        });
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Pivot"));
        let r = egui::ComboBox::from_id_salt("uv-pivot").selected_text(app.uv.pivot.label()).show_ui(ui, |ui| {
            for p in Pivot::ALL {
                ui.selectable_value(&mut app.uv.pivot, p, p.label());
            }
        });
        name(ui, &r.response, tl!("Pivot"));
    });
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Apply"));
        name(ui, &r, tl!("Apply transform"));
        apply |= r.clicked();
        for (label, horizontal) in [(tl!("Flip U"), true), (tl!("Flip V"), false)] {
            let r = ui.button(label.clone());
            name(ui, &r, label);
            if r.clicked() {
                flip(app, horizontal);
            }
        }
    });
    if apply {
        apply_numeric(app);
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Nudge step"));
        let r = ui.add(egui::DragValue::new(&mut app.uv.nudge).speed(0.001).range(0.0001..=1.0).max_decimals(4));
        name(ui, &r, tl!("Nudge step"));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Seam angle"));
        let r = ui.add(egui::DragValue::new(&mut app.uv.seam_angle).speed(0.5).range(0.0..=180.0).suffix("°"));
        name(ui, &r, tl!("Seam angle"));
    });
    let b = ui.button(tl!("Mark Seams by Angle"));
    name(ui, &b, tl!("Mark Seams by Angle"));
    if b.clicked() {
        seams_by_angle(app);
    }
    ui.separator();
    ui.label(egui::RichText::new(tl!("Display")).strong());
    let r = ui.checkbox(&mut app.uv.viewport_checker, tl!("Checker in the 3D view"));
    name(ui, &r, tl!("Checker in the 3D view"));
    let r = ui.checkbox(&mut app.uv.checker, tl!("Checker background"));
    name(ui, &r, tl!("Checker background"));
    let r = ui.checkbox(&mut app.uv.show_seams, tl!("Show seams"));
    name(ui, &r, tl!("Show seams"));
    ui.horizontal(|ui| {
        ui.label(tl!("Heat map"));
        let r = egui::ComboBox::from_id_salt("uv-heat").selected_text(app.uv.heat.label()).show_ui(ui, |ui| {
            for h in Heat::ALL {
                ui.selectable_value(&mut app.uv.heat, h, h.label());
            }
        });
        name(ui, &r.response, tl!("Heat map"));
    });
    legend(ui, app.uv.heat);
    ui.separator();
    ui.label(egui::RichText::new(tl!("Measures")).strong());
    let s = d.summary;
    for line in [
        crate::tlf!("Islands: {count}", count = d.charts.islands.len()),
        crate::tlf!("Selected: {count} corners", count = app.uv.selection.len()),
        crate::tlf!("Pinned: {count}", count = app.uv.pinned.len()),
        crate::tlf!("Angle error: mean {mean}°, max {max}°", mean = format!("{:.2}", s.mean_angle_error_deg), max = format!("{:.2}", s.max_angle_error_deg)),
        crate::tlf!("Stretch: mean {mean}, max {max}", mean = format!("{:.2}", s.mean_stretch), max = format!("{:.2}", s.max_stretch)),
        crate::tlf!("Area scale spread: {value}", value = format!("{:.2}", s.area_rms_log)),
        crate::tlf!("Mirrored triangles: {count}", count = s.flipped),
        island_density_line(app, d),
    ] {
        ui.label(egui::RichText::new(line).color(theme::TEXT_DIM));
    }
    if !app.uv.hover_text.is_empty() {
        ui.label(app.uv.hover_text.clone());
    }
}

/// Texel density of the island under the selection (or the smallest and largest overall).
fn island_density_line(app: &ModelApp, d: &Derived) -> String {
    let first = app.uv.selection.iter().next().copied();
    if let Some(c) = first
        && let Some(&isl) = d.charts.face_island.get(d.mesh.face_of(c) as usize)
        && let Some(v) = d.density.get(isl as usize)
    {
        return crate::tlf!("Texel density of the selected island: {value} UV per unit", value = format!("{v:.4}"));
    }
    let (lo, hi) = d.density.iter().fold((f32::MAX, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    if lo > hi {
        return String::new();
    }
    crate::tlf!("Texel density: {lo} to {hi} UV per unit", lo = format!("{lo:.4}"), hi = format!("{hi:.4}"))
}

fn legend(ui: &mut egui::Ui, heat: Heat) {
    if heat == Heat::Off {
        return;
    }
    let (lo, hi) = match heat {
        Heat::Angle => (tl!("0°"), tl!("10° or more")),
        Heat::Area => (tl!("smaller (a quarter)"), tl!("larger (four times)")),
        _ => (tl!("1 (even)"), tl!("2 or more")),
    };
    let (rect, _) = ui.allocate_exact_size(vec2(180.0, 10.0), Sense::hover());
    let mut mesh = Mesh::default();
    for i in 0..=16 {
        let t = i as f32 / 16.0;
        let x = rect.left() + t * rect.width();
        mesh.colored_vertex(pos2(x, rect.top()), heat_color(t));
        mesh.colored_vertex(pos2(x, rect.bottom()), heat_color(t));
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    ui.painter().add(Shape::mesh(mesh));
    ui.label(egui::RichText::new(crate::tlf!("{lo} to {hi}", lo = lo, hi = hi)).small().color(theme::TEXT_DIM));
}

/// Cold to hot: blue, cyan, green, yellow, red.
pub fn heat_color(t: f32) -> Color32 {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let stops =
        [(0.0, [40.0, 70.0, 200.0]), (0.25, [40.0, 190.0, 220.0]), (0.5, [70.0, 200.0, 80.0]), (0.75, [240.0, 220.0, 60.0]), (1.0, [230.0, 50.0, 40.0])];
    for w in stops.windows(2) {
        if t <= w[1].0 {
            let k = (t - w[0].0) / (w[1].0 - w[0].0);
            let c = |i: usize| (w[0].1[i] + (w[1].1[i] - w[0].1[i]) * k) as u8;
            return Color32::from_rgb(c(0), c(1), c(2));
        }
    }
    Color32::from_rgb(230, 50, 40)
}

/// Where a face's metric falls on the 0 to 1 heat scale.
pub fn heat_value(heat: Heat, m: &TriangleMetric) -> f32 {
    match heat {
        Heat::Off => 0.0,
        Heat::Angle => (m.angle_error_deg / 10.0).clamp(0.0, 1.0),
        Heat::Area => (0.5 + 0.25 * m.area_ratio.max(1e-6).log2()).clamp(0.0, 1.0),
        Heat::Stretch => (m.stretch - 1.0).clamp(0.0, 1.0),
    }
}

// ------------------------------------------------------------------ the canvas

/// Screen position of a UV.
pub(crate) fn to_screen(ui: &UvUi, rect: Rect, uv: Vec2) -> Pos2 {
    pos2(rect.left() + (uv.x - ui.origin.x) * ui.zoom, rect.top() + (uv.y - ui.origin.y) * ui.zoom)
}

fn to_uv(ui: &UvUi, rect: Rect, p: Pos2) -> Vec2 {
    Vec2::new((p.x - rect.left()) / ui.zoom + ui.origin.x, (p.y - rect.top()) / ui.zoom + ui.origin.y)
}

fn canvas(app: &mut ModelApp, ui: &mut egui::Ui, d: &Derived, focus_pending: bool) {
    let size = vec2((ui.available_width() - 230.0).max(260.0), (ui.available_height() - 4.0).max(300.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !app.uv.fitted {
        fit_view(&mut app.uv, rect, &d.mesh);
    }
    let id = ui.make_persistent_id("uv-canvas");
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let label = crate::tlf!(
        "UV canvas, {faces} faces in {islands} islands, {selected} corners selected",
        faces = d.mesh.face_count(),
        islands = d.charts.islands.len(),
        selected = app.uv.selection.len()
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, &label));
    if focus_pending {
        resp.request_focus();
    }
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    app.uv.had_focus = resp.has_focus();
    if resp.has_focus() {
        // Arrow keys nudge the selection and Escape clears it: keep them from moving the focus.
        ui.memory_mut(|m| m.set_focus_lock_filter(resp.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, escape: true, tab: false }));
    }

    // Zoom with the wheel about the pointer; pan with the middle button.
    let (scroll, middle) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.middle_down()));
    if resp.hovered()
        && scroll.abs() > 0.0
        && let Some(p) = resp.hover_pos()
    {
        let before = to_uv(&app.uv, rect, p);
        app.uv.zoom = (app.uv.zoom * (scroll * 0.002).exp()).clamp(8.0, 200_000.0);
        let after = to_uv(&app.uv, rect, p);
        app.uv.origin += before - after;
    }
    if middle && resp.dragged() {
        let dl = resp.drag_delta();
        app.uv.origin -= Vec2::new(dl.x, dl.y) / app.uv.zoom;
    }

    input(app, ui, &resp, rect, d);
    draw(app, ui, rect, d, &resp);
}

fn fit_view(u: &mut UvUi, rect: Rect, mesh: &PolyMesh) {
    let (mut lo, mut hi) = (Vec2::ZERO, Vec2::ONE);
    for c in mesh.corner_uvs() {
        lo = lo.min(*c);
        hi = hi.max(*c);
    }
    let span = (hi - lo).max(Vec2::splat(1e-3)) * 1.1;
    u.zoom = (rect.width() / span.x).min(rect.height() / span.y).clamp(8.0, 200_000.0);
    u.origin = 0.5 * (lo + hi) - Vec2::new(rect.width(), rect.height()) * 0.5 / u.zoom;
    u.fitted = true;
}

/// The corners an element at screen point `p` stands for, in the current selection mode.
pub(crate) fn pick(app: &ModelApp, rect: Rect, d: &Derived, p: Pos2) -> Option<Vec<u32>> {
    let u = &app.uv;
    let m = &d.mesh;
    let at = |c: u32| to_screen(u, rect, m.uv(c));
    match u.mode {
        UvSelect::Vertex => {
            let best = (0..m.corner_count() as u32).map(|c| (c, at(c).distance(p))).filter(|(_, dist)| *dist < 10.0).min_by(|a, b| a.1.total_cmp(&b.1))?;
            Some(uv_vertex_corners(m, &d.charts, best.0))
        }
        UvSelect::Edge => {
            let mut best: Option<(u32, f32)> = None;
            for h in 0..m.corner_count() as u32 {
                let (a, b) = (at(h), at(m.next(h)));
                let dist = segment_distance(p, a, b);
                if dist < 8.0 && best.is_none_or(|(_, bd)| dist < bd) {
                    best = Some((h, dist));
                }
            }
            let (h, _) = best?;
            let set: BTreeSet<u32> = [h, m.next(h)].into();
            Some(expand_to_uv_vertices(m, &d.charts, &set).into_iter().collect())
        }
        UvSelect::Face | UvSelect::Island => {
            let uv = to_uv(u, rect, p);
            let f = face_at(m, uv)?;
            if u.mode == UvSelect::Face {
                Some(m.face_half_edges(f).collect())
            } else {
                let c = m.face_half_edge(f);
                Some(island_corners(m, &d.charts, c))
            }
        }
    }
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 > 0.0 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// The face whose UV polygon contains `uv` (the last one drawn wins).
fn face_at(m: &PolyMesh, uv: Vec2) -> Option<u32> {
    let mut found = None;
    let mut tris = Vec::new();
    for f in 0..m.face_count().min(MAX_DRAW_FACES) as u32 {
        tris.clear();
        face_triangles(m, f, &mut tris);
        if tris.iter().any(|t| inside_triangle(uv, t.map(|c| m.uv(c)))) {
            found = Some(f);
        }
    }
    found
}

fn inside_triangle(p: Vec2, t: [Vec2; 3]) -> bool {
    let s = |a: Vec2, b: Vec2| (b - a).perp_dot(p - a);
    let (d1, d2, d3) = (s(t[0], t[1]), s(t[1], t[2]), s(t[2], t[0]));
    (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
}

/// Corners inside a screen box, by the selection mode.
pub(crate) fn in_box(app: &ModelApp, rect: Rect, d: &Derived, boxr: Rect) -> BTreeSet<u32> {
    let u = &app.uv;
    let m = &d.mesh;
    let at = |c: u32| to_screen(u, rect, m.uv(c));
    let mut out = BTreeSet::new();
    match u.mode {
        UvSelect::Vertex | UvSelect::Edge => {
            let inside: BTreeSet<u32> = (0..m.corner_count() as u32).filter(|c| boxr.contains(at(*c))).collect();
            if u.mode == UvSelect::Vertex {
                out = expand_to_uv_vertices(m, &d.charts, &inside);
            } else {
                let ends: BTreeSet<u32> = inside.iter().copied().filter(|c| inside.contains(&m.next(*c)) || inside.contains(&m.prev(*c))).collect();
                out = expand_to_uv_vertices(m, &d.charts, &ends);
            }
        }
        UvSelect::Face | UvSelect::Island => {
            for f in 0..m.face_count().min(MAX_DRAW_FACES) as u32 {
                let cs: Vec<u32> = m.face_half_edges(f).collect();
                let centre = cs.iter().fold(Vec2::ZERO, |s, c| s + m.uv(*c)) / cs.len().max(1) as f32;
                if boxr.contains(to_screen(u, rect, centre)) {
                    if u.mode == UvSelect::Face {
                        out.extend(cs);
                    } else {
                        out.extend(island_corners(m, &d.charts, m.face_half_edge(f)));
                    }
                }
            }
        }
    }
    out
}

fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, rect: Rect, d: &Derived) {
    let shift = ui.input(|i| i.modifiers.shift);
    let primary = ui.input(|i| i.pointer.primary_down());
    // Hover text for the face under the pointer.
    app.uv.hover_text.clear();
    if resp.hovered()
        && let Some(p) = resp.hover_pos()
        && let Some(f) = face_at(&d.mesh, to_uv(&app.uv, rect, p))
        && let Some(m) = d.metrics.get(f as usize)
    {
        app.uv.hover_text = crate::tlf!(
            "Face {face}: angle error {angle}°, stretch {stretch}, area scale {area}",
            face = f,
            angle = format!("{:.2}", m.angle_error_deg),
            stretch = format!("{:.2}", m.stretch),
            area = format!("{:.2}", m.area_ratio)
        );
    }
    // A drag starts where the button went down, not where egui noticed the pointer moving.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos())
    {
        let transform_tool = app.uv.tool != UvTool::Select && !app.uv.selection.is_empty();
        let on_selection = pick(app, rect, d, p).is_some_and(|c| c.iter().any(|x| app.uv.selection.contains(x)));
        app.uv.gesture = if transform_tool && on_selection {
            let pivot = pivot_point(&app.uv, &d.mesh);
            Some(Gesture::Transform { tool: app.uv.tool, start: p, t: UvTransform { pivot, ..UvTransform::default() } })
        } else {
            Some(Gesture::Box(p))
        };
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && primary
        && let Some(cur) = resp.interact_pointer_pos()
    {
        let zoom = app.uv.zoom;
        let pivot_screen = match &app.uv.gesture {
            Some(Gesture::Transform { t, .. }) => to_screen(&app.uv, rect, t.pivot),
            _ => cur,
        };
        if let Some(Gesture::Transform { tool, start, t }) = &mut app.uv.gesture {
            match tool {
                UvTool::Move => t.translate = Vec2::new(cur.x - start.x, cur.y - start.y) / zoom,
                UvTool::Rotate => {
                    let a0 = (start.y - pivot_screen.y).atan2(start.x - pivot_screen.x);
                    let a1 = (cur.y - pivot_screen.y).atan2(cur.x - pivot_screen.x);
                    t.rotate_deg = (a1 - a0).to_degrees();
                }
                UvTool::Scale => {
                    let (r0, r1) = (start.distance(pivot_screen).max(4.0), cur.distance(pivot_screen).max(1.0));
                    t.scale = Vec2::splat((r1 / r0).clamp(0.01, 100.0));
                }
                UvTool::Select => {}
            }
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        match app.uv.gesture.take() {
            Some(Gesture::Transform { t, .. }) => transform_selection(app, t),
            Some(Gesture::Box(start)) => {
                if let Some(end) = resp.interact_pointer_pos().or_else(|| resp.hover_pos()) {
                    let boxr = Rect::from_two_pos(start, end);
                    if boxr.width() > 3.0 || boxr.height() > 3.0 {
                        let hit = in_box(app, rect, d, boxr);
                        if shift {
                            app.uv.selection.extend(hit);
                        } else {
                            app.uv.selection = hit;
                        }
                    }
                }
            }
            None => {}
        }
    } else if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(p) = resp.interact_pointer_pos()
    {
        let hit = pick(app, rect, d, p);
        match hit {
            Some(corners) if shift => {
                if corners.iter().all(|c| app.uv.selection.contains(c)) {
                    for c in &corners {
                        app.uv.selection.remove(c);
                    }
                } else {
                    app.uv.selection.extend(corners);
                }
            }
            Some(corners) => app.uv.selection = corners.into_iter().collect(),
            None if !shift => app.uv.selection.clear(),
            None => {}
        }
    }
}

/// The UDIM tiles to draw: the unit square, and every tile the UVs reach (at most 100).
fn visible_tiles(m: &PolyMesh) -> Vec<(i32, i32)> {
    let (mut lo, mut hi) = (Vec2::ZERO, Vec2::ONE);
    if m.face_count() <= MAX_DRAW_FACES {
        for c in m.corner_uvs() {
            if c.is_finite() {
                lo = lo.min(*c);
                hi = hi.max(*c);
            }
        }
    }
    let (first, last) = (modelcraft_engine::uvpack::tile_of(lo), modelcraft_engine::uvpack::tile_of(hi));
    // v grows downwards, so the tile row of the lowest v is the highest row.
    let (c0, c1) = (first.0.max(0), last.0.clamp(0, 9));
    let (r0, r1) = (last.1.max(0), first.1.clamp(0, 9));
    let mut out = vec![(0, 0)];
    for row in r0..=r1 {
        for col in c0..=c1 {
            if (col, row) != (0, 0) && out.len() < 100 {
                out.push((col, row));
            }
        }
    }
    out
}

fn draw(app: &mut ModelApp, ui: &egui::Ui, rect: Rect, d: &Derived, resp: &egui::Response) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL_DEEP);
    let u = &app.uv;
    let m = &d.mesh;
    // The unit square (and every other UDIM tile the UVs reach), with a checker when asked.
    let tiles = visible_tiles(m);
    for &(col, row) in &tiles {
        let origin = Vec2::new(col as f32, -(row as f32));
        if u.checker {
            let cells = if tiles.len() > 4 { 8 } else { 16 };
            for j in 0..cells {
                for i in 0..cells {
                    let (p, q) = (
                        to_screen(u, rect, origin + Vec2::new(i as f32, j as f32) / cells as f32),
                        to_screen(u, rect, origin + Vec2::new(i as f32 + 1.0, j as f32 + 1.0) / cells as f32),
                    );
                    let c = if (i + j) % 2 == 0 { Color32::from_rgb(0x4A, 0x4D, 0x53) } else { Color32::from_rgb(0x34, 0x37, 0x3C) };
                    painter.rect_filled(Rect::from_two_pos(p, q), 0.0, c);
                }
            }
        }
        let (a, b) = (to_screen(u, rect, origin), to_screen(u, rect, origin + Vec2::ONE));
        painter.rect_stroke(Rect::from_two_pos(a, b), 0.0, Stroke::new(1.0, theme::TEXT_DIM), egui::StrokeKind::Outside);
        if tiles.len() > 1
            && let Some(number) = modelcraft_engine::uvpack::udim_number(col, row)
        {
            painter.text(a + vec2(4.0, 3.0), egui::Align2::LEFT_TOP, number.to_string(), egui::FontId::proportional(12.0), theme::TEXT_DIM);
        }
    }
    if m.face_count() > MAX_DRAW_FACES {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, tl!("Too many faces to draw"), egui::FontId::proportional(14.0), theme::TEXT);
        return;
    }
    // Live preview of a drag.
    let preview = match &u.gesture {
        Some(Gesture::Transform { t, .. }) => Some(*t),
        _ => None,
    };
    let disp = |c: u32| {
        let uv = m.uv(c);
        match &preview {
            Some(t) if u.selection.contains(&c) => t.apply(uv),
            _ => uv,
        }
    };
    // Faces: heat map fill.
    if u.heat != Heat::Off {
        let mut mesh = Mesh::default();
        let mut tris = Vec::new();
        for f in 0..m.face_count() as u32 {
            let color = d.metrics.get(f as usize).map_or(Color32::GRAY, |mt| {
                if mt.flipped { Color32::from_rgb(0xD0, 0x20, 0xD0) } else { heat_color(heat_value(u.heat, mt)).gamma_multiply(0.85) }
            });
            tris.clear();
            face_triangles(m, f, &mut tris);
            for t in &tris {
                let base = mesh.vertices.len() as u32;
                for c in t {
                    mesh.colored_vertex(to_screen(u, rect, disp(*c)), color);
                }
                mesh.add_triangle(base, base + 1, base + 2);
            }
        }
        painter.add(Shape::mesh(mesh));
    } else {
        // Selected faces get a light tint so a face or island selection is visible.
        let mut mesh = Mesh::default();
        let mut tris = Vec::new();
        for f in 0..m.face_count() as u32 {
            if !m.face_half_edges(f).all(|c| u.selection.contains(&c)) {
                continue;
            }
            tris.clear();
            face_triangles(m, f, &mut tris);
            for t in &tris {
                let base = mesh.vertices.len() as u32;
                for c in t {
                    mesh.colored_vertex(to_screen(u, rect, disp(*c)), Color32::from_rgba_unmultiplied(0xFF, 0xA6, 0x4D, 70));
                }
                mesh.add_triangle(base, base + 1, base + 2);
            }
        }
        painter.add(Shape::mesh(mesh));
    }
    // Edges: seams of the UV layout in red, selected edges in the selection colour.
    let seam_set = if u.show_seams { SeamSet::from_uv_discontinuities(m) } else { SeamSet::new() };
    for h in 0..m.corner_count() as u32 {
        let (c0, c1) = (h, m.next(h));
        let (p, q) = (to_screen(u, rect, disp(c0)), to_screen(u, rect, disp(c1)));
        let selected = u.selection.contains(&c0) && u.selection.contains(&c1);
        let seam = u.show_seams && !seam_set.is_empty() && seam_set.contains(m, m.edge_of(h));
        let stroke = if selected {
            Stroke::new(2.0, theme::SELECT)
        } else if seam {
            Stroke::new(1.5, Color32::from_rgb(0xF0, 0x50, 0x50))
        } else {
            Stroke::new(1.0, Color32::from_rgb(0xB8, 0xBD, 0xC4))
        };
        painter.line_segment([p, q], stroke);
    }
    // Vertices of the selection and the pinned ones (diamonds).
    for c in &u.selection {
        if (*c as usize) < m.corner_count() {
            painter.circle_filled(to_screen(u, rect, disp(*c)), 3.5, theme::SELECT);
        }
    }
    for c in &u.pinned {
        if (*c as usize) < m.corner_count() {
            let p = to_screen(u, rect, m.uv(*c));
            painter.add(Shape::convex_polygon(
                vec![p + vec2(0.0, -6.0), p + vec2(6.0, 0.0), p + vec2(0.0, 6.0), p + vec2(-6.0, 0.0)],
                Color32::from_rgb(0x58, 0x8E, 0xF0),
                Stroke::new(1.0, Color32::WHITE),
            ));
        }
    }
    // Box selection rubber band.
    if let (Some(Gesture::Box(start)), Some(cur)) = (&u.gesture, resp.interact_pointer_pos()) {
        painter.rect_stroke(Rect::from_two_pos(*start, cur), 0.0, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
    }
    if resp.has_focus() {
        painter.rect_stroke(rect, 0.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
    }
    let _ = NONE;
}

#[allow(dead_code)]
fn object_kind_is_mesh(app: &ModelApp, id: ObjectId) -> bool {
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }))
}
