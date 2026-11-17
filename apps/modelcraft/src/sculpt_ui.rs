//! Sculpt panel sections that go beyond the brush itself: detail levels and shape layers.

use egui::RichText;
use modelcraft_engine::{Command, ShapeOp};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::theme;

fn name(ui: &egui::Ui, resp: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(label));
}

/// Live detail size the app starts with, as a share of the brush radius.
const LIVE_DETAIL_SHARE: f32 = 0.15;
/// Cells along the longest side of the object for a new even remesh.
const REMESH_CELLS: f32 = 96.0;

/// The live-detail edge length for a stroke with this radius (None: off, or not possible on
/// this object right now; the status bar then says why).
pub fn live_detail_edge(app: &mut ModelApp, radius: f32) -> Option<f32> {
    let share = app.brush.live_detail?;
    let id = app.doc.sculpt.as_ref().map(|s| s.id)?;
    if let Some(why) = app.doc.live_detail_blocked(id) {
        app.status = crate::tlf!("Live detail is off for this stroke: {reason}", reason = crate::tl!(why));
        return None;
    }
    Some(share.clamp(0.02, 1.0) * radius)
}

pub fn toggle_live_detail(app: &mut ModelApp) {
    app.brush.live_detail = if app.brush.live_detail.is_some() { None } else { Some(LIVE_DETAIL_SHARE) };
    app.status = if app.brush.live_detail.is_some() { crate::tl!("Live detail on: strokes add triangles under the brush") } else { crate::tl!("Live detail off") };
}

/// Add an even-remesh step to the object being sculpted (or the selected mesh) and carry on
/// sculpting on the result.
pub fn even_remesh(app: &mut ModelApp) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id).or_else(|| app.active_mesh()) else {
        app.status = crate::tl!("Select a mesh first");
        return;
    };
    let sculpting = app.doc.sculpt.is_some();
    app.doc.sync_sculpt();
    let mesh = app.doc.eval.evaluate(&app.doc.scene, id).mesh;
    let (lo, hi) = mesh.bounds();
    let cell = ((hi - lo).max_element() / REMESH_CELLS).max(modelcraft_engine::ops::remesh::min_cell(&mesh));
    if let Err(e) = modelcraft_engine::ops::remesh::even_remesh(&mesh, cell) {
        app.status = crate::tlf!("{action} failed: {error}", action = Action::EvenRemesh.label(), error = e);
        return;
    }
    if sculpting {
        let _ = app.doc.execute(Command::EndSculpt);
    }
    app.run(Command::AddStep { id, op: modelcraft_engine::scene::StepOp::EvenRemesh { cell } });
    if sculpting {
        app.begin_sculpt();
    }
    app.status = crate::tlf!("Even remesh: {count} quads", count = app.doc.eval.evaluate(&app.doc.scene, id).mesh.face_count());
}

/// Live detail, detail levels and shape layers of the object being sculpted (nothing outside
/// a session).
pub fn layers(app: &mut ModelApp, ui: &mut egui::Ui) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    let ctx = ui.ctx().clone();
    app.doc.sync_sculpt();
    ui.add_space(8.0);
    ui.label(RichText::new(crate::tl!("Surface detail")).strong());
    let mut on = app.brush.live_detail.is_some();
    let tip = format!("{} ({})", crate::tl!("Strokes split the triangles under the brush down to the detail size"), Action::LiveDetail.shortcut_text(app));
    if ui.checkbox(&mut on, Action::LiveDetail.label()).on_hover_text(tip).changed() {
        app.act(&ctx, Action::LiveDetail);
    }
    if let Some(share) = &mut app.brush.live_detail {
        ui.horizontal(|ui| {
            ui.label(crate::tl!("Detail size"));
            let mut pct = *share * 100.0;
            let r = ui.add(egui::DragValue::new(&mut pct).speed(0.5).range(2.0..=100.0).suffix(" %").max_decimals(0)).on_hover_text(crate::tl!("Longest edge left under the brush, as a share of the brush radius"));
            name(ui, &r, crate::tl!("Detail size"));
            if r.changed() {
                *share = pct / 100.0;
            }
        });
        if let Some(why) = app.doc.live_detail_blocked(id) {
            ui.label(RichText::new(crate::tl!(why)).color(theme::TEXT_DIM).small());
        }
    }
    if ui.button(Action::EvenRemesh.label()).on_hover_text(crate::tl!("Rebuild the surface as even quads (closed meshes; keeps the volume)")).clicked() {
        app.act(&ctx, Action::EvenRemesh);
    }
    let Some(layer) = app.doc.sculpt_layer(id).cloned() else { return };
    let triangles = app.doc.sculpt.as_ref().map(|s| s.mesh.triangle_count()).unwrap_or(0);
    ui.add_space(8.0);
    ui.label(RichText::new(crate::tl!("Detail levels")).strong());
    ui.label(crate::tlf!("Level {level}, {triangles} triangles", level = layer.level, triangles = triangles));
    if !layer.finer.is_empty() {
        ui.label(RichText::new(crate::tlf!("{count} finer levels kept", count = layer.finer.len())).color(theme::TEXT_DIM).small());
    }
    ui.horizontal(|ui| {
        for a in [Action::DetailDown, Action::DetailUp] {
            let tip = format!("{} ({})", a.label(), a.shortcut_text(app));
            if ui.button(a.label()).on_hover_text(tip).clicked() {
                app.act(&ctx, a);
            }
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new(crate::tl!("Shape layers")).strong());
    if ui.button(Action::ShapeAdd.label()).clicked() {
        app.act(&ctx, Action::ShapeAdd);
    }
    let mut op: Option<ShapeOp> = None;
    if !layer.shapes.is_empty() {
        let r = ui.radio(layer.active_shape.is_none(), crate::tl!("Base sculpt")).on_hover_text(crate::tl!("Strokes change the sculpt itself"));
        if r.clicked() {
            op = Some(ShapeOp::Select { index: None });
        }
    }
    for (i, sh) in layer.shapes.iter().enumerate() {
        let i = i as u32;
        ui.horizontal(|ui| {
            let r = ui.radio(layer.active_shape == Some(i), &sh.name).on_hover_text(crate::tl!("Record strokes into this shape layer"));
            name(ui, &r, crate::tlf!("{name}, shape layer", name = sh.name));
            if r.clicked() {
                op = Some(ShapeOp::Select { index: Some(i) });
            }
            // The value commits once, when the drag or the typing ends: one undo step per edit.
            let key = ui.scope_id().with(("shape-strength", id.0, i));
            let mut v = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(sh.strength);
            let r = ui.add(egui::DragValue::new(&mut v).speed(0.01).range(-1.0..=1.0).max_decimals(2));
            name(ui, &r, crate::tlf!("Strength of {name}", name = sh.name));
            if r.changed() && r.dragged() {
                ui.data_mut(|d| d.insert_temp(key, v));
            } else if r.drag_stopped() || r.changed() {
                ui.data_mut(|d| d.remove::<f32>(key));
                if v != sh.strength {
                    op = Some(ShapeOp::Strength { index: i, strength: v });
                }
            }
            let r = ui.small_button(crate::tl!("Merge")).on_hover_text(crate::tl!("Keep the shape and drop the layer"));
            name(ui, &r, crate::tlf!("Merge {name}", name = sh.name));
            if r.clicked() {
                op = Some(ShapeOp::Merge { index: i });
            }
            let r = ui.small_button(crate::tl!("Remove")).on_hover_text(crate::tl!("Remove the layer and its shape"));
            name(ui, &r, crate::tlf!("Remove {name}", name = sh.name));
            if r.clicked() {
                op = Some(ShapeOp::Remove { index: i });
            }
        });
    }
    if let Some(op) = op {
        app.run(Command::SculptShape { id, op });
    }
}
