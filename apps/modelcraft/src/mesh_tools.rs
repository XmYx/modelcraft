//! Patch grouping and mesh cleanup: actions for the menus and palette, and the Properties
//! section "Patches and Cleanup" with the mesh check (n-gons, triangles, open borders, faces with
//! no area, poles, shells). Every change is a build-history step (`modelcraft_ops::patches`).

use egui::RichText;
use modelcraft_engine::ops::patches::{self, Grouping, MeshCheck};
use modelcraft_engine::scene::{ObjectId, StepOp};

use crate::app::{ModelApp, SelectMode};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    GroupByAngle,
    GroupByShell,
    NewPatch,
    SelectPatch,
    SelectNgons,
    SelectTriangles,
    SelectBorders,
    CleanUp,
}

pub const ALL: &[Tool] =
    &[Tool::GroupByAngle, Tool::GroupByShell, Tool::NewPatch, Tool::SelectPatch, Tool::SelectNgons, Tool::SelectTriangles, Tool::SelectBorders, Tool::CleanUp];

/// Welding distance of Clean Up Mesh.
pub const CLEAN_DISTANCE: f32 = 1e-5;

impl Tool {
    pub fn label(self) -> String {
        match self {
            Tool::GroupByAngle => tl!("Group Patches by Angle"),
            Tool::GroupByShell => tl!("Group Patches by Shell"),
            Tool::NewPatch => tl!("New Patch from Selection"),
            Tool::SelectPatch => tl!("Select Patch"),
            Tool::SelectNgons => tl!("Select N-gons"),
            Tool::SelectTriangles => tl!("Select Triangles"),
            Tool::SelectBorders => tl!("Select Open Borders"),
            Tool::CleanUp => tl!("Clean Up Mesh"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Tool::GroupByAngle => "mesh.patches.by_angle",
            Tool::GroupByShell => "mesh.patches.by_shell",
            Tool::NewPatch => "mesh.patches.new",
            Tool::SelectPatch => "select.patch",
            Tool::SelectNgons => "select.ngons",
            Tool::SelectTriangles => "select.triangles",
            Tool::SelectBorders => "select.borders",
            Tool::CleanUp => "mesh.clean_up",
        }
    }
}

/// Settings kept between uses.
pub struct State {
    /// Crease angle of Group Patches by Angle, in degrees.
    pub angle: f32,
}

impl Default for State {
    fn default() -> Self {
        State { angle: 30.0 }
    }
}

fn mesh(app: &mut ModelApp) -> Option<(ObjectId, std::sync::Arc<modelcraft_engine::mesh::PolyMesh>)> {
    let id = app.edit_target()?;
    Some((id, app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh))
}

fn select(app: &mut ModelApp, mode: SelectMode, items: Vec<u32>, what: &str) {
    if app.mode != mode {
        app.set_mode(mode);
    }
    if app.mode != mode {
        return;
    }
    let n = items.len();
    app.components = items.into_iter().collect();
    app.status = if n == 0 { crate::tlf!("No {what} found", what = what) } else { crate::tlf!("{count} {what} selected", count = n, what = what) };
}

pub fn run(app: &mut ModelApp, t: Tool) {
    let Some((_, m)) = mesh(app) else {
        app.status = tl!("Select a mesh first");
        return;
    };
    match t {
        Tool::GroupByAngle => app.add_step(StepOp::GroupPatches { grouping: Grouping::Angle { degrees: app.mesh_tools.angle } }),
        Tool::GroupByShell => app.add_step(StepOp::GroupPatches { grouping: Grouping::Shells }),
        Tool::NewPatch => {
            let faces: Vec<u32> = app.components.iter().copied().collect();
            if app.mode != SelectMode::Face || faces.is_empty() {
                app.status = tl!("Select faces first (face mode)");
                return;
            }
            let next = patches::patches_in_use(&m).last().map_or(0, |p| p.saturating_add(1));
            app.add_step(StepOp::SetPatch { faces, patch: next });
        }
        Tool::SelectPatch => {
            if app.mode != SelectMode::Face || app.components.is_empty() {
                app.status = tl!("Select a face of the patch first (face mode)");
                return;
            }
            let wanted: std::collections::BTreeSet<u32> = app.components.iter().filter(|f| (**f as usize) < m.face_count()).map(|f| m.patch(*f)).collect();
            let faces = (0..m.face_count() as u32).filter(|f| wanted.contains(&m.patch(*f))).collect();
            select(app, SelectMode::Face, faces, &tl!("faces"));
        }
        Tool::SelectNgons => select(app, SelectMode::Face, patches::check(&m).ngons, &tl!("n-gons")),
        Tool::SelectTriangles => select(app, SelectMode::Face, patches::check(&m).triangles, &tl!("triangles")),
        Tool::SelectBorders => select(app, SelectMode::Edge, patches::check(&m).border_edges, &tl!("border edges")),
        Tool::CleanUp => {
            let c = patches::check(&m);
            app.add_step(StepOp::CleanUp { distance: CLEAN_DISTANCE });
            if c.is_clean() {
                app.status = tl!("Clean up added: nothing had no area; near vertices are welded");
            }
        }
    }
}

/// The Properties section for the edited mesh.
pub fn section(app: &mut ModelApp, ui: &mut egui::Ui) {
    let Some((_, m)) = mesh(app) else { return };
    egui::CollapsingHeader::new(tl!("Patches and Cleanup")).default_open(false).show(ui, |ui| {
        let check: MeshCheck = patches::check(&m);
        let used = patches::patches_in_use(&m).len();
        ui.label(crate::tlf!("{count} patches, {shells} shells", count = used, shells = check.shells));
        ui.horizontal(|ui| {
            ui.label(tl!("Crease angle"));
            let r = ui.add(egui::DragValue::new(&mut app.mesh_tools.angle).range(0.0..=180.0).suffix("°").speed(0.5));
            ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("Crease angle for grouping")));
        });
        ui.horizontal_wrapped(|ui| {
            for t in [Tool::GroupByAngle, Tool::GroupByShell, Tool::NewPatch, Tool::SelectPatch] {
                if ui.button(t.label()).clicked() {
                    run(app, t);
                }
            }
        });
        ui.separator();
        let rows = [
            (tl!("N-gons"), check.ngons.len(), Some(Tool::SelectNgons)),
            (tl!("Triangles"), check.triangles.len(), Some(Tool::SelectTriangles)),
            (tl!("Open border edges"), check.border_edges.len(), Some(Tool::SelectBorders)),
            (tl!("Faces with no area"), check.degenerate_faces.len(), None),
            (tl!("Poles (5+ edges)"), check.poles.len(), None),
        ];
        for (name, n, tool) in rows {
            ui.horizontal(|ui| {
                let text = format!("{name}: {n}");
                ui.label(if n == 0 { RichText::new(text).color(theme::TEXT_DIM) } else { RichText::new(text) });
                if let Some(t) = tool
                    && n > 0
                {
                    let r = ui.small_button(tl!("Select"));
                    ui.ctx().accesskit_node_builder(r.id, |b| b.set_label(t.label()));
                    if r.clicked() {
                        run(app, t);
                    }
                }
            });
        }
        if ui.button(Tool::CleanUp.label()).clicked() {
            run(app, Tool::CleanUp);
        }
    });
}
