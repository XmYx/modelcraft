//! The Properties list of a spline's points: which curve type a segment has, whether it is
//! closed, the points to choose from, and numeric fields for the chosen point and its handles. It
//! is the keyboard and screen-reader path for everything the pen and the point drag do.

use std::sync::Arc;

use egui::RichText;
use glam::Vec3;
use modelcraft_engine::curves::{CurveKind, HandleMode, Knot, KnotMove, KnotRef, Side, Spline, SplineEdit};
use modelcraft_engine::scene::splines::SplineSource;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, SplineCommand};

use crate::app::ModelApp;
use crate::panels::{commit_edit, set_name, vec3_row};
use crate::spline_pen::spline_of;
use crate::theme;

/// Most points listed (a longer spline lists the first ones; the pointer reaches the rest).
const MAX_ROWS: usize = 48;

fn kind_label(k: CurveKind) -> String {
    match k {
        CurveKind::Linear => tl!("Polyline"),
        CurveKind::Bezier => tl!("Bezier"),
        CurveKind::BSpline => tl!("B-spline"),
    }
}

fn mode_label(m: HandleMode) -> String {
    match m {
        HandleMode::Free => tl!("Free"),
        HandleMode::Aligned => tl!("Aligned"),
        HandleMode::Mirrored => tl!("Mirrored"),
        HandleMode::Auto => tl!("Smooth"),
    }
}

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

fn run_edit(app: &mut ModelApp, id: ObjectId, edit: SplineEdit) -> bool {
    app.run(Command::Spline(SplineCommand::Edit { id, edit }))
}

/// A numeric edit with a live preview while a field is dragged and one undo step at the end.
fn commit(app: &mut ModelApp, resp: &egui::Response, id: ObjectId, spline: &Spline, edit: SplineEdit) {
    let mut after = spline.clone();
    let ok = after.apply(&edit).is_ok();
    commit_edit(
        app,
        resp,
        |o| {
            if ok && let ObjectKind::Spline { source, .. } = &mut o.kind {
                *source = SplineSource::Curve { spline: Arc::new(after) };
            }
        },
        || Command::Spline(SplineCommand::Edit { id, edit }),
        id,
    );
}

/// The Points section of a spline object's Properties.
pub fn section(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some(spline) = spline_of(app, id) else { return };
    egui::CollapsingHeader::new(tl!("Points")).id_salt(("spline-points", id.0)).default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            let on = app.spline.pen_on();
            if ui.selectable_label(on, tl!("Pen Tool")).clicked() {
                crate::spline_pen::toggle_pen(app);
            }
        });
        if app.spline.pen_on() {
            ui.label(RichText::new(tl!("Click in the viewport to add points; drag to pull out a handle.")).color(theme::TEXT_DIM).small());
        }
        if spline.segments.is_empty() {
            ui.label(RichText::new(tl!("No points yet. Use the Pen Tool, or Add Point.")).color(theme::TEXT_DIM).small());
            if ui.button(tl!("Add Point")).clicked() {
                run_edit(app, id, SplineEdit::Append { segment: 0, knot: Knot::new(Vec3::ZERO), kind: CurveKind::Bezier });
            }
            return;
        }
        let si = app.spline.segment.min(spline.segments.len() - 1);
        if spline.segments.len() > 1 {
            let labels: Vec<String> = (0..spline.segments.len()).map(|i| crate::tlf!("Curve {number}", number = i + 1)).collect();
            if let Some(i) = pick(ui, ("spline-segment", id.0), tl!("Curve"), si, &labels) {
                app.spline.segment = i;
                app.spline.selected.clear();
                return;
            }
        }
        let seg = &spline.segments[si];
        let kinds = [CurveKind::Linear, CurveKind::Bezier, CurveKind::BSpline];
        let labels: Vec<String> = kinds.iter().map(|k| kind_label(*k)).collect();
        if let Some(i) = pick(ui, ("spline-kind", id.0), tl!("Curve type"), kinds.iter().position(|k| *k == seg.kind).unwrap_or(1), &labels) {
            run_edit(app, id, SplineEdit::SetKind { segment: si, kind: kinds[i] });
            return;
        }
        let mut closed = seg.closed;
        let r = ui.checkbox(&mut closed, tl!("Closed"));
        if r.changed() {
            run_edit(app, id, SplineEdit::SetClosed { segment: si, closed });
            return;
        }
        // The points.
        ui.label(crate::tlf!("{count} points", count = seg.knots.len()));
        egui::ScrollArea::vertical().id_salt(("spline-rows", id.0)).max_height(130.0).show(ui, |ui| {
            for ki in 0..seg.knots.len().min(MAX_ROWS) {
                let at = KnotRef { segment: si, knot: ki };
                let selected = app.spline.selected.contains(&at);
                if ui.selectable_label(selected, crate::tlf!("Point {number}", number = ki + 1)).clicked() {
                    let add = ui.input(|i| i.modifiers.shift);
                    crate::spline_pen::select_knot(app, at, add);
                }
            }
        });
        if seg.knots.len() > MAX_ROWS {
            ui.label(RichText::new(crate::tlf!("The first {count} points are listed.", count = MAX_ROWS)).color(theme::TEXT_DIM).small());
        }
        // The chosen point.
        let chosen = app.spline.selected.iter().copied().find(|k| k.segment == si && k.knot < seg.knots.len());
        if let Some(at) = chosen {
            let knot = seg.knots[at.knot];
            let (hin, hout) = seg.effective_handles().get(at.knot).copied().unwrap_or_default();
            let mut pos = knot.pos;
            let rs = vec3_row(ui, &tl!("Point position"), &mut pos, 0.01);
            if let Some(r) = rs.iter().find(|r| r.changed() || r.drag_stopped()) {
                commit(app, r, id, &spline, SplineEdit::MoveKnots { moves: vec![KnotMove { at, pos }] });
            }
            if seg.kind == CurveKind::Bezier {
                let modes = [HandleMode::Free, HandleMode::Aligned, HandleMode::Mirrored, HandleMode::Auto];
                let labels: Vec<String> = modes.iter().map(|m| mode_label(*m)).collect();
                if let Some(i) = pick(ui, ("spline-mode", id.0), tl!("Handles"), modes.iter().position(|m| *m == knot.mode).unwrap_or(0), &labels) {
                    run_edit(app, id, SplineEdit::SetMode { at, mode: modes[i] });
                    return;
                }
                for (side, label, mut v) in [(Side::In, tl!("Handle in"), hin), (Side::Out, tl!("Handle out"), hout)] {
                    let rs = vec3_row(ui, &label, &mut v, 0.01);
                    if let Some(r) = rs.iter().find(|r| r.changed() || r.drag_stopped()) {
                        commit(app, r, id, &spline, SplineEdit::SetHandle { at, side, offset: v });
                    }
                }
            }
        }
        // Buttons: every pen action without the pointer.
        ui.horizontal_wrapped(|ui| {
            if ui.button(tl!("Add Point")).clicked() {
                let last = seg.knots.last().map_or(Vec3::ZERO, |k| k.pos);
                let step = if seg.knots.len() >= 2 { last - seg.knots[seg.knots.len() - 2].pos } else { Vec3::X };
                let (target, new_segment) = if seg.closed { (spline.segments.len(), true) } else { (si, false) };
                let knot = Knot::new(last + if step.length() > 1e-6 { step } else { Vec3::X });
                if run_edit(app, id, SplineEdit::Append { segment: target, knot, kind: seg.kind }) {
                    app.spline.segment = target;
                    app.spline.selected = vec![KnotRef { segment: target, knot: if new_segment { 0 } else { seg.knots.len() } }];
                }
            }
            if ui.button(tl!("Insert Point")).clicked()
                && let Some(at) = chosen
            {
                // Between the chosen point and the next (or the one before, at the end of an open curve).
                let last = seg.knots.len() - 1;
                let u = if !seg.closed && at.knot == last { at.knot as f32 - 0.5 } else { at.knot as f32 + 0.5 };
                crate::spline_pen::insert_point(app, id, si, u.max(0.0));
            }
            if ui.button(tl!("Delete Points")).clicked() && !app.spline.selected.is_empty() {
                crate::spline_pen::delete_points(app);
            }
        });
    });
}
