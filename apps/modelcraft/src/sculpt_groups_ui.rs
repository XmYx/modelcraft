//! Patches and visibility while sculpting: the actions (make patches, hide, show, isolate), the
//! rectangle and lasso tools for masking and hiding with their by-numbers dialog, clip and trim
//! by a plane, the occlusion mask and shells from the mask.
//!
//! Everything a pointer drag does here can be done from the keyboard: the region tools through
//! Region by Numbers (a rectangle or a list of corners in percent of the view), hiding a patch
//! through the patch list in the Sculpt panel.

use egui::{Color32, Pos2, Rect, RichText, Stroke};
use glam::{Vec2, Vec3};
use modelcraft_engine::scene::StepOp;
use modelcraft_engine::sculpt::ScreenRegion;
use modelcraft_engine::{Command, GroupOp, RegionAction, VisibilityOp};

use crate::app::{ModelApp, Workspace};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SculptAction {
    PatchesByAngle,
    PatchesByUv,
    PatchesByPart,
    PatchFromMask,
    HidePatch,
    IsolatePatch,
    ShowAll,
    InvertVisibility,
    HideMasked,
    MaskOcclusion,
    Tool(RegionTool),
    RegionDialog,
    PlaneDialog,
    ExtractShell,
    Decimate,
    ProjectDetail,
}

/// What a primary-button drag in the viewport does while sculpting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegionTool {
    #[default]
    Brush,
    RectMask,
    LassoMask,
    RectHide,
    LassoHide,
}

impl RegionTool {
    pub const ALL: [RegionTool; 5] = [RegionTool::Brush, RegionTool::RectMask, RegionTool::LassoMask, RegionTool::RectHide, RegionTool::LassoHide];

    pub fn label(self) -> String {
        match self {
            RegionTool::Brush => tl!("Brush"),
            RegionTool::RectMask => tl!("Rectangle Mask"),
            RegionTool::LassoMask => tl!("Lasso Mask"),
            RegionTool::RectHide => tl!("Rectangle Hide"),
            RegionTool::LassoHide => tl!("Lasso Hide"),
        }
    }
    fn lasso(self) -> bool {
        matches!(self, RegionTool::LassoMask | RegionTool::LassoHide)
    }
    /// The action of a drag, and of a drag with the command key held.
    fn actions(self) -> (RegionAction, RegionAction) {
        match self {
            RegionTool::RectHide | RegionTool::LassoHide => (RegionAction::Hide, RegionAction::Isolate),
            _ => (RegionAction::Mask, RegionAction::Unmask),
        }
    }
}

impl SculptAction {
    pub const ALL: [SculptAction; 20] = [
        SculptAction::PatchesByAngle,
        SculptAction::PatchesByUv,
        SculptAction::PatchesByPart,
        SculptAction::PatchFromMask,
        SculptAction::HidePatch,
        SculptAction::IsolatePatch,
        SculptAction::ShowAll,
        SculptAction::InvertVisibility,
        SculptAction::HideMasked,
        SculptAction::MaskOcclusion,
        SculptAction::Tool(RegionTool::Brush),
        SculptAction::Tool(RegionTool::RectMask),
        SculptAction::Tool(RegionTool::LassoMask),
        SculptAction::Tool(RegionTool::RectHide),
        SculptAction::Tool(RegionTool::LassoHide),
        SculptAction::RegionDialog,
        SculptAction::PlaneDialog,
        SculptAction::ExtractShell,
        SculptAction::Decimate,
        SculptAction::ProjectDetail,
    ];

    pub fn label(self) -> String {
        match self {
            SculptAction::PatchesByAngle => tl!("Sculpt Patches by Angle"),
            SculptAction::PatchesByUv => tl!("Sculpt Patches by UV Island"),
            SculptAction::PatchesByPart => tl!("Sculpt Patches by Part"),
            SculptAction::PatchFromMask => tl!("Sculpt Patch from Mask"),
            SculptAction::HidePatch => tl!("Hide Patch Under Pointer"),
            SculptAction::IsolatePatch => tl!("Isolate Patch Under Pointer"),
            SculptAction::ShowAll => tl!("Show All"),
            SculptAction::InvertVisibility => tl!("Invert Visibility"),
            SculptAction::HideMasked => tl!("Hide Masked"),
            SculptAction::MaskOcclusion => tl!("Mask Occlusion"),
            SculptAction::Tool(t) => crate::tlf!("{tool} Tool", tool = t.label()),
            SculptAction::RegionDialog => tl!("Region by Numbers…"),
            SculptAction::PlaneDialog => tl!("Clip or Trim by Plane…"),
            SculptAction::ExtractShell => tl!("Extract Shell from Mask"),
            SculptAction::Decimate => tl!("Decimate"),
            SculptAction::ProjectDetail => tl!("Project Detail"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SculptAction::PatchesByAngle => "sculpt.patches_by_angle",
            SculptAction::PatchesByUv => "sculpt.patches_by_uv",
            SculptAction::PatchesByPart => "sculpt.patches_by_part",
            SculptAction::PatchFromMask => "sculpt.patch_from_mask",
            SculptAction::HidePatch => "sculpt.hide_patch",
            SculptAction::IsolatePatch => "sculpt.isolate_patch",
            SculptAction::ShowAll => "sculpt.show_all",
            SculptAction::InvertVisibility => "sculpt.invert_visibility",
            SculptAction::HideMasked => "sculpt.hide_masked",
            SculptAction::MaskOcclusion => "sculpt.mask_occlusion",
            SculptAction::Tool(RegionTool::Brush) => "sculpt.tool_brush",
            SculptAction::Tool(RegionTool::RectMask) => "sculpt.tool_rect_mask",
            SculptAction::Tool(RegionTool::LassoMask) => "sculpt.tool_lasso_mask",
            SculptAction::Tool(RegionTool::RectHide) => "sculpt.tool_rect_hide",
            SculptAction::Tool(RegionTool::LassoHide) => "sculpt.tool_lasso_hide",
            SculptAction::RegionDialog => "sculpt.region_numbers",
            SculptAction::PlaneDialog => "sculpt.plane",
            SculptAction::ExtractShell => "sculpt.extract_shell",
            SculptAction::Decimate => "sculpt.decimate",
            SculptAction::ProjectDetail => "sculpt.project_detail",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            SculptAction::HidePatch => "Sculpt: Shift+H",
            SculptAction::IsolatePatch => "Sculpt: Shift+I",
            SculptAction::ShowAll => "Sculpt: Alt+H",
            SculptAction::Tool(RegionTool::Brush) => "Sculpt: Shift+B",
            SculptAction::Tool(RegionTool::RectMask) => "Sculpt: Shift+R",
            SculptAction::Tool(RegionTool::LassoMask) => "Sculpt: Shift+A",
            SculptAction::RegionDialog => "Sculpt: Cmd+Shift+R",
            SculptAction::PlaneDialog => "Sculpt: Cmd+Shift+P",
            _ => return None,
        })
    }
}

/// The outline being dragged in a viewport.
pub struct Outline {
    view: usize,
    points: Vec<Pos2>,
}

/// Region by Numbers: a rectangle or an outline in percent of the active view (0 0 is the top
/// left corner).
#[derive(Clone, Debug, PartialEq)]
pub struct RegionDialog {
    pub outline: bool,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub corners: String,
    pub action: RegionAction,
    focused: bool,
}

impl Default for RegionDialog {
    fn default() -> Self {
        RegionDialog {
            outline: false,
            left: 25.0,
            top: 25.0,
            right: 75.0,
            bottom: 75.0,
            corners: "30,30 70,30 50,70".into(),
            action: RegionAction::Mask,
            focused: false,
        }
    }
}

/// Clip or Trim by Plane: a plane across one axis of the object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlaneDialog {
    pub axis: usize,
    pub offset: f32,
    pub flip: bool,
    pub cap: bool,
    focused: bool,
}

impl Default for PlaneDialog {
    fn default() -> Self {
        PlaneDialog { axis: 1, offset: 0.0, flip: false, cap: true, focused: false }
    }
}

/// One row of the patch list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatchRow {
    pub patch: u32,
    pub triangle: u32,
    pub triangles: usize,
    pub hidden: bool,
}

/// Most patches the Sculpt panel lists.
const MAX_PATCH_ROWS: usize = 32;

pub struct State {
    pub tool: RegionTool,
    pub front_only: bool,
    pub angle: f32,
    pub thickness: f32,
    /// Share of the triangles Decimate keeps, in percent.
    pub decimate_percent: f32,
    /// The object Project Detail takes the shape from (none: the first other mesh), and how far
    /// a vertex may move as a share of the object's size, in percent.
    pub project_source: Option<modelcraft_engine::scene::ObjectId>,
    pub project_reach: f32,
    pub region: Option<RegionDialog>,
    pub plane: Option<PlaneDialog>,
    outline: Option<Outline>,
    rects: [Option<Rect>; 4],
    /// The triangle under the pointer, and the view it is in.
    hover: Option<(usize, u32)>,
    return_focus: Option<egui::Id>,
    rows: (Option<(u64, u64, u64, u64)>, Vec<PatchRow>, usize),
}

impl Default for State {
    fn default() -> Self {
        State {
            tool: RegionTool::Brush,
            front_only: false,
            angle: 30.0,
            thickness: 0.05,
            decimate_percent: 25.0,
            project_source: None,
            project_reach: 10.0,
            region: None,
            plane: None,
            outline: None,
            rects: [None; 4],
            hover: None,
            return_focus: None,
            rows: (None, Vec::new(), 0),
        }
    }
}

fn name(ui: &egui::Ui, resp: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(label));
}

fn session(app: &mut ModelApp) -> Option<modelcraft_engine::scene::ObjectId> {
    let id = app.doc.sculpt.as_ref().map(|s| s.id);
    if id.is_none() {
        app.status = tl!("Patches and visibility work in the Sculpt workspace");
    }
    id
}

/// The triangle a patch action applies to: under the pointer, or in the middle of the active
/// view when the pointer is elsewhere (so the action works from the keyboard).
fn target_triangle(app: &ModelApp) -> Option<u32> {
    if let Some((_, t)) = app.sculpt_groups.hover {
        return Some(t);
    }
    let i = app.active_view;
    let rect = app.sculpt_groups.rects.get(i).copied().flatten()?;
    let s = app.doc.sculpt.as_ref()?;
    let inv = app.doc.scene.world_matrix(s.id).inverse();
    let ray = crate::viewport::world_ray(&app.views.get(i)?.camera, rect, rect.center()).transformed(inv);
    s.mesh.raycast(ray.origin, ray.dir).map(|h| h.2)
}

pub fn act(app: &mut ModelApp, ctx: &egui::Context, a: SculptAction) {
    if let SculptAction::Tool(t) = a {
        app.sculpt_groups.tool = t;
        app.sculpt_groups.outline = None;
        app.status = match t {
            RegionTool::Brush => tl!("Brush tool"),
            RegionTool::RectMask | RegionTool::LassoMask => crate::tlf!("{tool}: drag to mask, with Ctrl to clear", tool = t.label()),
            _ => crate::tlf!("{tool}: drag to hide, with Ctrl to keep only the inside", tool = t.label()),
        };
        return;
    }
    let Some(id) = session(app) else { return };
    let patch_op = |app: &mut ModelApp, f: fn(u32) -> VisibilityOp| match target_triangle(app) {
        Some(triangle) => {
            app.run(Command::SculptVisibility { id, op: f(triangle) });
        }
        None => app.status = tl!("Point at the patch, or bring it to the middle of the view"),
    };
    match a {
        SculptAction::PatchesByAngle => {
            let angle = app.sculpt_groups.angle;
            if app.run(Command::SculptGroups { id, op: GroupOp::ByNormal { angle } }) {
                app.show_patches = true;
                app.status = crate::tlf!("{count} patches", count = patch_count(app));
            }
        }
        SculptAction::PatchesByUv | SculptAction::PatchesByPart => {
            let op = if a == SculptAction::PatchesByUv { GroupOp::ByUvIsland } else { GroupOp::ByPart };
            if app.run(Command::SculptGroups { id, op }) {
                app.show_patches = true;
                app.status = crate::tlf!("{count} patches", count = patch_count(app));
            }
        }
        SculptAction::PatchFromMask => {
            if app.run(Command::SculptGroups { id, op: GroupOp::FromMask }) {
                app.show_patches = true;
            }
        }
        SculptAction::HidePatch => patch_op(app, |triangle| VisibilityOp::HidePatch { triangle }),
        SculptAction::IsolatePatch => patch_op(app, |triangle| VisibilityOp::Isolate { triangle }),
        SculptAction::ShowAll => {
            app.run(Command::SculptVisibility { id, op: VisibilityOp::ShowAll });
        }
        SculptAction::InvertVisibility => {
            app.run(Command::SculptVisibility { id, op: VisibilityOp::Invert });
        }
        SculptAction::HideMasked => {
            app.run(Command::SculptVisibility { id, op: VisibilityOp::HideMasked });
        }
        SculptAction::MaskOcclusion => {
            app.run(Command::SculptMask { id, op: modelcraft_engine::MaskOp::Occlusion });
        }
        SculptAction::RegionDialog => {
            app.sculpt_groups.return_focus = ctx.memory(|m| m.focused());
            app.sculpt_groups.plane = None;
            app.sculpt_groups.region = Some(RegionDialog { action: app.sculpt_groups.tool.actions().0, ..RegionDialog::default() });
        }
        SculptAction::PlaneDialog => {
            app.sculpt_groups.return_focus = ctx.memory(|m| m.focused());
            app.sculpt_groups.region = None;
            app.sculpt_groups.plane = Some(PlaneDialog::default());
        }
        SculptAction::ExtractShell => {
            let thickness = app.sculpt_groups.thickness;
            // The new object is selected by `run`; sculpting stays on the original.
            let keep = app.selection.clone();
            if app.run(Command::SculptExtract { id, thickness }) {
                app.selection = keep;
            }
        }
        SculptAction::Decimate => decimate(app),
        SculptAction::ProjectDetail => project(app),
        SculptAction::Tool(_) => {}
    }
}

/// Decimate: a history step after the sculpt layer; sculpting carries on on the result.
fn decimate(app: &mut ModelApp) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    let ratio = (app.sculpt_groups.decimate_percent / 100.0).clamp(0.001, 1.0);
    app.doc.sync_sculpt();
    let before = app.doc.eval.evaluate(&app.doc.scene, id).mesh.triangle_count();
    let _ = app.doc.execute(Command::EndSculpt);
    app.run(Command::AddStep { id, op: StepOp::Decimate { ratio } });
    app.begin_sculpt();
    let after = app.doc.eval.evaluate(&app.doc.scene, id).mesh.triangle_count();
    app.status = crate::tlf!("Decimate: {after} of {before} triangles", after = after, before = before);
}

/// The other objects with a surface, for Project Detail.
pub fn project_sources(app: &ModelApp) -> Vec<(modelcraft_engine::scene::ObjectId, String)> {
    use modelcraft_engine::scene::ObjectKind;
    let me = app.doc.sculpt.as_ref().map(|s| s.id);
    app.doc
        .scene
        .objects()
        .iter()
        .filter(|o| Some(o.id) != me && matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. }) && !app.doc.scene.is_cage_input(o.id))
        .map(|o| (o.id, o.name.clone()))
        .collect()
}

fn project(app: &mut ModelApp) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    let sources = project_sources(app);
    let Some(source) = app.sculpt_groups.project_source.filter(|s| sources.iter().any(|(k, _)| k == s)).or_else(|| sources.first().map(|s| s.0)) else {
        app.status = tl!("Project Detail needs another mesh in the scene to take the shape from");
        return;
    };
    let size = app.doc.sculpt.as_ref().map(|s| {
        let (lo, hi) = s.mesh.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        (hi - lo).length()
    });
    let reach = size.map(|d| (d * app.sculpt_groups.project_reach / 100.0).max(1e-6));
    let before = app.doc.undo.len();
    if app.run(Command::SculptProject { id, source, reach }) && app.doc.undo.len() == before {
        app.status = tl!("Nothing of the other surface is within reach");
    }
}

fn patch_count(app: &ModelApp) -> usize {
    let Some(s) = &app.doc.sculpt else { return 0 };
    let mut p = s.mesh.triangle_patches().to_vec();
    p.sort_unstable();
    p.dedup();
    p.len()
}

/// The patches of the session, the first [`MAX_PATCH_ROWS`] of them, and how many there are.
pub fn patch_rows(app: &mut ModelApp) -> (Vec<PatchRow>, usize) {
    let Some(s) = &app.doc.sculpt else { return (Vec::new(), 0) };
    let key = (s.id.0, s.step_id, s.mesh.view_rev(), s.mesh.topology_rev());
    if app.sculpt_groups.rows.0 != Some(key) {
        let hidden = s.mesh.hidden_triangles();
        let mut map: std::collections::BTreeMap<u32, PatchRow> = std::collections::BTreeMap::new();
        for (t, p) in s.mesh.triangle_patches().iter().enumerate() {
            let h = hidden.get(t).copied().unwrap_or(false);
            let row = map.entry(*p).or_insert(PatchRow { patch: *p, triangle: t as u32, triangles: 0, hidden: true });
            row.triangles += 1;
            row.hidden &= h;
        }
        let total = map.len();
        app.sculpt_groups.rows = (Some(key), map.into_values().take(MAX_PATCH_ROWS).collect(), total);
    }
    (app.sculpt_groups.rows.1.clone(), app.sculpt_groups.rows.2)
}

/// The Sculpt menu's entries for this module.
pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.separator();
    ui.menu_button(tl!("Patches and Visibility"), |ui| {
        for a in SculptAction::ALL {
            if matches!(a, SculptAction::Tool(RegionTool::Brush) | SculptAction::RegionDialog) {
                ui.separator();
            }
            let a = crate::actions::Action::Sculpt(a);
            let b = egui::Button::new(a.label()).shortcut_text(a.shortcut_text(app));
            if ui.add(b).clicked() {
                app.act(&ctx, a);
                ui.close();
            }
        }
    });
}

fn action_button(app: &mut ModelApp, ui: &mut egui::Ui, a: SculptAction, tip: String) {
    let ctx = ui.ctx().clone();
    let a = crate::actions::Action::Sculpt(a);
    let keys = a.shortcut_text(app);
    let tip = if keys.is_empty() { tip } else { format!("{tip} ({keys})") };
    if ui.button(a.label()).on_hover_text(tip).clicked() {
        app.act(&ctx, a);
    }
}

/// The Sculpt panel's section: tools, patches, visibility.
pub fn panel(app: &mut ModelApp, ui: &mut egui::Ui) {
    if app.doc.sculpt.is_none() {
        return;
    }
    let ctx = ui.ctx().clone();
    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Region tools")).strong());
    ui.horizontal_wrapped(|ui| {
        for t in RegionTool::ALL {
            let a = crate::actions::Action::Sculpt(SculptAction::Tool(t));
            let keys = a.shortcut_text(app);
            let r = ui.radio(app.sculpt_groups.tool == t, t.label());
            let r = if keys.is_empty() { r } else { r.on_hover_text(keys) };
            if r.clicked() {
                app.act(&ctx, a);
            }
        }
    });
    ui.checkbox(&mut app.sculpt_groups.front_only, tl!("Front faces only")).on_hover_text(tl!("Rectangles and lassos leave out what faces away from the view"));
    if app.sculpt_groups.tool != RegionTool::Brush {
        let hint = if app.sculpt_groups.tool.actions().0 == RegionAction::Mask {
            tl!("Drag to mask, with Ctrl to clear the mask inside.")
        } else {
            tl!("Drag to hide, with Ctrl to keep only what is inside.")
        };
        ui.label(RichText::new(hint).color(theme::TEXT_DIM).small());
    }
    action_button(app, ui, SculptAction::RegionDialog, tl!("Type the rectangle or the outline instead of dragging it"));
    action_button(app, ui, SculptAction::PlaneDialog, tl!("Flatten one side onto a plane, or cut it away"));

    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Patches")).strong());
    ui.horizontal(|ui| {
        action_button(app, ui, SculptAction::PatchesByAngle, tl!("A patch for every region between creases sharper than the angle"));
        let r = ui.add(egui::DragValue::new(&mut app.sculpt_groups.angle).speed(0.5).range(1.0..=180.0).suffix("°").max_decimals(0));
        name(ui, &r, tl!("Crease angle"));
    });
    ui.horizontal_wrapped(|ui| {
        action_button(app, ui, SculptAction::PatchesByUv, tl!("A patch for every UV island"));
        action_button(app, ui, SculptAction::PatchesByPart, tl!("A patch for every connected part"));
        action_button(app, ui, SculptAction::PatchFromMask, tl!("The masked faces become one new patch"));
    });
    ui.checkbox(&mut app.show_patches, tl!("Show Patches"));

    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Visibility")).strong());
    ui.horizontal_wrapped(|ui| {
        action_button(app, ui, SculptAction::ShowAll, tl!("Show everything again"));
        action_button(app, ui, SculptAction::InvertVisibility, tl!("Swap what shows and what is hidden"));
        action_button(app, ui, SculptAction::HideMasked, tl!("Hide the masked faces"));
    });
    let (rows, total) = patch_rows(app);
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    if total > 1 {
        let mut op = None;
        for row in &rows {
            let mut shown = !row.hidden;
            let label = crate::tlf!("Patch {patch}, {count} triangles", patch = row.patch, count = row.triangles);
            if ui.checkbox(&mut shown, label).on_hover_text(tl!("Show or hide this patch")).changed() {
                op = Some(if shown { VisibilityOp::ShowPatch { triangle: row.triangle } } else { VisibilityOp::HidePatch { triangle: row.triangle } });
            }
        }
        if total > rows.len() {
            ui.label(RichText::new(crate::tlf!("{count} more patches not listed", count = total - rows.len())).color(theme::TEXT_DIM).small());
        }
        if let Some(op) = op {
            app.run(Command::SculptVisibility { id, op });
        }
    }

    ui.add_space(8.0);
    ui.label(RichText::new(tl!("From the mask")).strong());
    ui.horizontal(|ui| {
        action_button(app, ui, SculptAction::ExtractShell, tl!("A new object from the masked faces, with a thickness"));
        let r = ui.add(egui::DragValue::new(&mut app.sculpt_groups.thickness).speed(0.005).range(-100.0..=100.0).max_decimals(3));
        name(ui, &r, tl!("Shell thickness"));
    });
    action_button(app, ui, SculptAction::MaskOcclusion, tl!("Mask folds, holes and places where parts meet"));

    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Fewer triangles")).strong());
    ui.horizontal(|ui| {
        action_button(app, ui, SculptAction::Decimate, tl!("Keep this share of the triangles; the shape, UVs and patches stay"));
        let r = ui.add(egui::DragValue::new(&mut app.sculpt_groups.decimate_percent).speed(0.5).range(0.1..=100.0).suffix(" %").max_decimals(1));
        name(ui, &r, tl!("Triangles to keep"));
    });
    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Detail from another mesh")).strong());
    let sources = project_sources(app);
    if sources.is_empty() {
        ui.label(RichText::new(tl!("Add or import the mesh that has the detail.")).color(theme::TEXT_DIM).small());
    } else {
        let current = app.sculpt_groups.project_source.filter(|s| sources.iter().any(|(k, _)| k == s)).or_else(|| sources.first().map(|s| s.0));
        let shown = sources.iter().find(|(k, _)| Some(*k) == current).map(|(_, n)| n.clone()).unwrap_or_default();
        let r = egui::ComboBox::from_id_salt("project-source").selected_text(shown).show_ui(ui, |ui| {
            for (k, n) in &sources {
                if ui.selectable_label(Some(*k) == current, n).clicked() {
                    app.sculpt_groups.project_source = Some(*k);
                }
            }
        });
        name(ui, &r.response, tl!("Mesh with the detail"));
        ui.horizontal(|ui| {
            action_button(app, ui, SculptAction::ProjectDetail, tl!("Move this mesh onto the surface of the other one; the mask protects"));
            let r = ui.add(egui::DragValue::new(&mut app.sculpt_groups.project_reach).speed(0.5).range(0.1..=100.0).suffix(" %").max_decimals(1));
            name(ui, &r, tl!("Reach of the projection"));
        });
    }
}

fn region_for(app: &ModelApp, view: usize, rect: Rect, points: Vec<Vec2>) -> Option<(modelcraft_engine::scene::ObjectId, ScreenRegion)> {
    let s = app.doc.sculpt.as_ref()?;
    let cam = app.views.get(view)?.camera;
    let w = app.doc.scene.world_matrix(s.id);
    let region = ScreenRegion {
        view_proj: cam.view_proj(rect.aspect_ratio()) * w,
        points,
        front_only: app.sculpt_groups.front_only,
        eye: w.inverse().transform_point3(cam.eye()),
    };
    Some((s.id, region))
}

fn run_region(app: &mut ModelApp, view: usize, rect: Rect, points: Vec<Vec2>, action: RegionAction) {
    let Some((id, region)) = region_for(app, view, rect, points) else { return };
    let before = app.doc.undo.len();
    if app.run(Command::SculptRegion { id, region, action }) && app.doc.undo.len() == before {
        app.status = tl!("Nothing was inside the region");
    }
}

/// Pointer input of the region tools. Returns true when the drag is theirs (no brush stroke).
pub fn viewport_input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) -> bool {
    if let Some(r) = app.sculpt_groups.rects.get_mut(i) {
        *r = Some(rect);
    }
    if app.sculpt_groups.hover.is_some_and(|(v, _)| v == i) {
        app.sculpt_groups.hover = None;
    }
    let Some(s) = &app.doc.sculpt else { return false };
    if let Some(at) = resp.hover_pos()
        && let Some(view) = app.views.get(i)
    {
        let ray = crate::viewport::world_ray(&view.camera, rect, at).transformed(app.doc.scene.world_matrix(s.id).inverse());
        app.sculpt_groups.hover = s.mesh.raycast(ray.origin, ray.dir).map(|h| (i, h.2));
    }
    let tool = app.sculpt_groups.tool;
    if tool == RegionTool::Brush || app.stroke.is_some() {
        return false;
    }
    let pointer = resp.interact_pointer_pos();
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !crate::viewport::is_nav(app, ui)
        && let Some(at) = pointer
    {
        app.sculpt_groups.outline = Some(Outline { view: i, points: vec![at - resp.drag_delta(), at] });
    }
    let mine = app.sculpt_groups.outline.as_ref().is_some_and(|o| o.view == i);
    if !mine {
        return true;
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(at) = pointer
        && let Some(o) = &mut app.sculpt_groups.outline
    {
        if tool.lasso() {
            let far = o.points.last().is_none_or(|l| l.distance(at) >= 3.0);
            if far && o.points.len() < modelcraft_engine::sculpt::groups::MAX_REGION_POINTS {
                o.points.push(at);
            }
        } else if let Some(last) = o.points.get_mut(1) {
            *last = at;
        }
    }
    if resp.drag_stopped() || !ui.input(|inp| inp.pointer.primary_down()) {
        let Some(o) = app.sculpt_groups.outline.take() else { return true };
        let screen: Vec<Pos2> = if tool.lasso() {
            o.points
        } else {
            match (o.points.first(), o.points.get(1)) {
                (Some(a), Some(b)) => {
                    let r = Rect::from_two_pos(*a, *b);
                    vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
                }
                _ => Vec::new(),
            }
        };
        let (lo, hi) = screen.iter().fold((Pos2::new(f32::MAX, f32::MAX), Pos2::new(f32::MIN, f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        if screen.len() >= 3 && (hi - lo).max_elem() >= 3.0 {
            let (plain, with_ctrl) = tool.actions();
            let action = if ui.input(|inp| inp.modifiers.command) { with_ctrl } else { plain };
            run_region(app, i, rect, screen.iter().map(|p| crate::viewport::ndc(rect, *p)).collect(), action);
        }
    }
    true
}

/// The outline being dragged, and the tool's name in the corner of the view.
pub fn overlay(app: &ModelApp, painter: &egui::Painter, i: usize) {
    if app.workspace != Workspace::Sculpt || app.sculpt_groups.tool == RegionTool::Brush {
        return;
    }
    let Some(o) = app.sculpt_groups.outline.as_ref().filter(|o| o.view == i) else { return };
    let line = Stroke::new(1.5, theme::ACCENT);
    let fill = Color32::from_rgba_unmultiplied(232, 138, 60, 28);
    if app.sculpt_groups.tool.lasso() {
        let mut pts = o.points.clone();
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
        painter.add(egui::Shape::line(pts, line));
    } else if let (Some(a), Some(b)) = (o.points.first(), o.points.get(1)) {
        let r = Rect::from_two_pos(*a, *b);
        painter.rect_filled(r, 0.0, fill);
        painter.rect_stroke(r, 0.0, line, egui::StrokeKind::Inside);
    }
}

/// Corners typed as `x,y x,y …` in percent of the view.
pub fn parse_corners(text: &str) -> Option<Vec<Vec2>> {
    let mut out = Vec::new();
    for pair in text.split(|c: char| c.is_whitespace() || c == ';').filter(|p| !p.is_empty()) {
        let (x, y) = pair.split_once(',')?;
        let (x, y): (f32, f32) = (x.trim().parse().ok()?, y.trim().parse().ok()?);
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        out.push(percent_to_ndc(x, y));
    }
    (3..=modelcraft_engine::sculpt::groups::MAX_REGION_POINTS).contains(&out.len()).then_some(out)
}

fn percent_to_ndc(x: f32, y: f32) -> Vec2 {
    Vec2::new(x / 50.0 - 1.0, 1.0 - y / 50.0)
}

fn close(app: &mut ModelApp, ctx: &egui::Context) {
    app.sculpt_groups.region = None;
    app.sculpt_groups.plane = None;
    if let Some(id) = app.sculpt_groups.return_focus.take() {
        ctx.memory_mut(|m| m.request_focus(id));
    }
}

/// Esc closes this module's dialogs. Returns true when it did.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    if (app.sculpt_groups.region.is_some() || app.sculpt_groups.plane.is_some()) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        close(app, ctx);
        return true;
    }
    false
}

fn action_label(a: RegionAction) -> String {
    match a {
        RegionAction::Mask => tl!("Mask"),
        RegionAction::Unmask => tl!("Clear the mask"),
        RegionAction::Hide => tl!("Hide"),
        RegionAction::Isolate => tl!("Keep only the inside"),
    }
}

pub fn dialogs(app: &mut ModelApp, ctx: &egui::Context) {
    if app.doc.sculpt.is_none() {
        app.sculpt_groups.region = None;
        app.sculpt_groups.plane = None;
        return;
    }
    region_dialog(app, ctx);
    plane_dialog(app, ctx);
}

fn region_dialog(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut d) = app.sculpt_groups.region.clone() else { return };
    let mut open = true;
    let mut apply = false;
    egui::Window::new(tl!("Region by Numbers")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.label(tl!("In percent of the active view; 0 0 is its top left corner."));
            ui.horizontal(|ui| {
                let first = ui.radio_value(&mut d.outline, false, tl!("Rectangle"));
                ui.radio_value(&mut d.outline, true, tl!("Outline"));
                if !d.focused {
                    d.focused = true;
                    first.request_focus();
                }
            });
            if d.outline {
                ui.label(tl!("Corners as x,y pairs:"));
                let r = ui.add(egui::TextEdit::singleline(&mut d.corners).desired_width(260.0));
                name(ui, &r, tl!("Corners of the outline"));
                if parse_corners(&d.corners).is_none() {
                    ui.label(RichText::new(tl!("Type at least three corners, like 30,30 70,30 50,70")).color(theme::TEXT_DIM).small());
                }
            } else {
                egui::Grid::new("region-rect").num_columns(4).show(ui, |ui| {
                    for (label, v) in [(tl!("Left"), &mut d.left), (tl!("Top"), &mut d.top), (tl!("Right"), &mut d.right), (tl!("Bottom"), &mut d.bottom)] {
                        ui.label(&label);
                        let r = ui.add(egui::DragValue::new(v).speed(0.5).range(0.0..=100.0).suffix(" %").max_decimals(1));
                        name(ui, &r, crate::tlf!("{side} of the region", side = label));
                        if label == tl!("Top") {
                            ui.end_row();
                        }
                    }
                });
            }
            let r = egui::ComboBox::from_id_salt("region-action").selected_text(action_label(d.action)).show_ui(ui, |ui| {
                for a in [RegionAction::Mask, RegionAction::Unmask, RegionAction::Hide, RegionAction::Isolate] {
                    ui.selectable_value(&mut d.action, a, action_label(a));
                }
            });
            name(ui, &r.response, tl!("Region action"));
            ui.checkbox(&mut app.sculpt_groups.front_only, tl!("Front faces only"));
            if ui.button(tl!("Apply")).clicked() {
                apply = true;
            }
        },
    );
    if apply {
        let points = if d.outline {
            parse_corners(&d.corners)
        } else {
            let (l, r, t, b) = (d.left.min(d.right), d.left.max(d.right), d.top.min(d.bottom), d.top.max(d.bottom));
            (r - l > 0.0 && b - t > 0.0).then(|| vec![percent_to_ndc(l, t), percent_to_ndc(r, t), percent_to_ndc(r, b), percent_to_ndc(l, b)])
        };
        match points {
            Some(points) => {
                let view = app.active_view;
                let rect = app.sculpt_groups.rects.get(view).copied().flatten().unwrap_or(Rect::from_min_size(Pos2::ZERO, egui::vec2(1.0, 1.0)));
                run_region(app, view, rect, points, d.action);
                close(app, ctx);
            }
            None => {
                app.status = tl!("The region needs a width and a height, or at least three corners");
                app.sculpt_groups.region = Some(d);
            }
        }
        return;
    }
    if open {
        app.sculpt_groups.region = Some(d);
    } else {
        close(app, ctx);
    }
}

/// Trim: a history step after the sculpt layer; sculpting carries on on the result.
fn trim(app: &mut ModelApp, point: Vec3, normal: Vec3, cap: bool) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    app.doc.sync_sculpt();
    let mesh = app.doc.eval.evaluate(&app.doc.scene, id).mesh;
    match modelcraft_engine::ops::trim::trim_plane(&mesh, point, normal, cap) {
        Err(e) => app.status = crate::tlf!("{action} failed: {error}", action = tl!("Trim by plane"), error = e),
        Ok(out) if out.face_count() == mesh.face_count() && out.vertex_count() == mesh.vertex_count() => app.status = tl!("The plane does not cut the mesh"),
        Ok(_) => {
            let _ = app.doc.execute(Command::EndSculpt);
            app.run(Command::AddStep { id, op: StepOp::TrimPlane { point, normal, cap } });
            app.begin_sculpt();
            app.status = crate::tlf!("Trim by plane: {count} faces", count = app.doc.eval.evaluate(&app.doc.scene, id).mesh.face_count());
        }
    }
}

fn plane_dialog(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut d) = app.sculpt_groups.plane else { return };
    let mut open = true;
    let mut run: Option<bool> = None;
    egui::Window::new(tl!("Clip or Trim by Plane")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.label(tl!("A plane across one axis of the object. The side the axis points to is affected."));
            ui.horizontal(|ui| {
                ui.label(tl!("Axis"));
                for (k, axis) in ["X", "Y", "Z"].iter().enumerate() {
                    let r = ui.radio_value(&mut d.axis, k, *axis);
                    name(ui, &r, crate::tlf!("{axis} axis of the plane", axis = axis));
                    if k == 0 && !d.focused {
                        d.focused = true;
                        r.request_focus();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Offset"));
                let r = ui.add(egui::DragValue::new(&mut d.offset).speed(0.01).max_decimals(3));
                name(ui, &r, tl!("Offset of the plane"));
            });
            ui.checkbox(&mut d.flip, tl!("Other side"));
            ui.checkbox(&mut d.cap, tl!("Close the cut (trim)"));
            ui.horizontal(|ui| {
                if ui.button(tl!("Clip to Plane")).on_hover_text(tl!("Move that side onto the plane; the mask protects")).clicked() {
                    run = Some(false);
                }
                if ui.button(tl!("Trim at Plane")).on_hover_text(tl!("Cut that side away as a history step")).clicked() {
                    run = Some(true);
                }
            });
        },
    );
    if let Some(trim_it) = run {
        let axis = [Vec3::X, Vec3::Y, Vec3::Z].get(d.axis).copied().unwrap_or(Vec3::Y);
        let (point, normal) = (axis * d.offset, if d.flip { -axis } else { axis });
        if trim_it {
            trim(app, point, normal, d.cap);
        } else if let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) {
            let before = app.doc.undo.len();
            if app.run(Command::SculptClip { id, point, normal }) && app.doc.undo.len() == before {
                app.status = tl!("Nothing lies on that side of the plane");
            }
        }
        close(app, ctx);
        return;
    }
    if open {
        app.sculpt_groups.plane = Some(d);
    } else {
        close(app, ctx);
    }
}
