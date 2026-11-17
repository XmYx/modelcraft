//! Sculpt panel sections that go beyond the brush itself: detail levels and shape layers.

use egui::RichText;
use modelcraft_engine::sculpt::StrokeMode;
use modelcraft_engine::sculpt::preset::{self, BrushPreset};
use modelcraft_engine::{Command, ShapeOp};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::theme;

fn name(ui: &egui::Ui, resp: &egui::Response, label: String) {
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(label));
}

pub fn alpha_label(a: modelcraft_engine::sculpt::Alpha) -> String {
    use modelcraft_engine::sculpt::Alpha;
    match a {
        Alpha::Dots => crate::tl!("Dots"),
        Alpha::Scales => crate::tl!("Scales"),
        Alpha::Cracks => crate::tl!("Cracks"),
        Alpha::Weave => crate::tl!("Weave"),
        Alpha::Pores => crate::tl!("Pores"),
        Alpha::Star => crate::tl!("Star"),
        Alpha::Stripes => crate::tl!("Stripes"),
        Alpha::Hex => crate::tl!("Honeycomb"),
    }
}

fn stroke_label(m: StrokeMode) -> String {
    match m {
        StrokeMode::Spaced => crate::tl!("Spaced"),
        StrokeMode::Dots => crate::tl!("Dots"),
        StrokeMode::Spray { .. } => crate::tl!("Spray"),
        StrokeMode::Trail { .. } => crate::tl!("Trail"),
        StrokeMode::Drag => crate::tl!("Drag out"),
    }
}

/// Brush options beyond falloff and stamp: hard core, stroke mode and spacing, stamp turn, and
/// the preset list with its save and remove buttons.
pub fn brush_options(app: &mut ModelApp, ui: &mut egui::Ui) {
    egui::Grid::new("brush-more").num_columns(2).show(ui, |ui| {
        ui.label(crate::tl!("Hardness"));
        let r = ui
            .add(egui::Slider::new(&mut app.brush.hardness, 0.0..=1.0))
            .on_hover_text(crate::tl!("How much of the radius keeps full strength before the falloff begins"));
        name(ui, &r, crate::tl!("Brush hardness"));
        ui.end_row();
        ui.label(crate::tl!("Stroke"));
        let r = egui::ComboBox::from_id_salt("stroke-mode").selected_text(stroke_label(app.brush.stroke)).show_ui(ui, |ui| {
            for m in [StrokeMode::Spaced, StrokeMode::Dots, StrokeMode::Spray { scatter: 1.0 }, StrokeMode::Trail { leash: 1.5 }, StrokeMode::Drag] {
                let on = std::mem::discriminant(&app.brush.stroke) == std::mem::discriminant(&m);
                if ui.selectable_label(on, stroke_label(m)).clicked() {
                    app.brush.stroke = m;
                }
            }
        });
        name(ui, &r.response, crate::tl!("Stroke"));
        ui.end_row();
        match &mut app.brush.stroke {
            StrokeMode::Spray { scatter } => {
                ui.label(crate::tl!("Scatter"));
                let r = ui.add(egui::Slider::new(scatter, 0.0..=5.0));
                name(ui, &r, crate::tl!("Spray scatter"));
                ui.end_row();
            }
            StrokeMode::Trail { leash } => {
                ui.label(crate::tl!("Leash"));
                let r = ui.add(egui::Slider::new(leash, 0.0..=10.0));
                name(ui, &r, crate::tl!("Trail leash"));
                ui.end_row();
            }
            _ => {}
        }
        if !matches!(app.brush.stroke, StrokeMode::Drag | StrokeMode::Dots) {
            ui.label(crate::tl!("Spacing"));
            let r = ui.add(egui::Slider::new(&mut app.brush.spacing, 0.02..=2.0));
            name(ui, &r, crate::tl!("Dab spacing"));
            ui.end_row();
        }
        if let modelcraft_engine::sculpt::Stamp::Image { turn, .. } = &mut app.brush.stamp {
            ui.label(crate::tl!("Stamp turn"));
            let mut deg = turn.to_degrees();
            let r = ui.add(egui::Slider::new(&mut deg, -180.0..=180.0).suffix("°"));
            name(ui, &r, crate::tl!("Stamp turn"));
            *turn = deg.to_radians();
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.label(RichText::new(crate::tl!("Presets")).strong());
    let builtin = preset::builtin();
    let mut pick: Option<BrushPreset> = None;
    let mut remove: Option<usize> = None;
    ui.horizontal_wrapped(|ui| {
        for p in &builtin {
            let r = ui.button(crate::tl!(p.name.as_str()));
            name(ui, &r, crate::tlf!("{name}, brush preset", name = crate::tl!(p.name.as_str())));
            if r.clicked() {
                pick = Some(p.clone());
            }
        }
        for (i, p) in app.user_presets.iter().enumerate() {
            let r = ui.button(&p.name).on_hover_text(crate::tl!("Your preset. Right-click or press Delete to remove it."));
            name(ui, &r, crate::tlf!("{name}, your brush preset", name = p.name));
            if r.clicked() {
                pick = Some(p.clone());
            }
            if r.secondary_clicked() || (r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Delete))) {
                remove = Some(i);
            }
        }
    });
    if let Some(p) = pick {
        app.brush = p.apply(&app.brush);
        app.status = crate::tlf!("{name} preset", name = p.name);
    }
    if let Some(i) = remove {
        let gone = app.user_presets.remove(i);
        app.store_prefs();
        app.status = crate::tlf!("Removed the preset {name}", name = gone.name);
    }
    ui.horizontal(|ui| {
        let key = ui.scope_id().with("preset-name");
        let mut text = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_default();
        let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(110.0).hint_text(crate::tl!("Preset name")));
        name(ui, &r, crate::tl!("Preset name"));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let save = ui.button(crate::tl!("Save Preset")).on_hover_text(crate::tl!("Keep the current brush under this name"));
        if (save.clicked() || enter) && !text.trim().is_empty() {
            let p = BrushPreset::new(&text, &app.brush);
            let label = p.name.clone();
            app.status = if preset::upsert(&mut app.user_presets, p) {
                app.store_prefs();
                text.clear();
                crate::tlf!("Saved the preset {name}", name = label)
            } else {
                crate::tl!("The preset list is full")
            };
        }
        ui.data_mut(|d| d.insert_temp(key, text));
    });
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
    app.status =
        if app.brush.live_detail.is_some() { crate::tl!("Live detail on: strokes add triangles under the brush") } else { crate::tl!("Live detail off") };
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
            let r = ui
                .add(egui::DragValue::new(&mut pct).speed(0.5).range(2.0..=100.0).suffix(" %").max_decimals(0))
                .on_hover_text(crate::tl!("Longest edge left under the brush, as a share of the brush radius"));
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
