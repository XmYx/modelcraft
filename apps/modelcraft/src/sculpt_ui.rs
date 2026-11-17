//! Sculpt panel sections that go beyond the brush itself: detail levels and shape layers.

use egui::RichText;
use modelcraft_engine::{Command, ShapeOp};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::theme;

fn name(ui: &egui::Ui, resp: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(label));
}

/// Detail levels and shape layers of the object being sculpted (nothing outside a session).
pub fn layers(app: &mut ModelApp, ui: &mut egui::Ui) {
    let Some(id) = app.doc.sculpt.as_ref().map(|s| s.id) else { return };
    let ctx = ui.ctx().clone();
    app.doc.sync_sculpt();
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
