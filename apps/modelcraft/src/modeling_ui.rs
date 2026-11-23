//! Tool options for the polygon tools that take numbers: extrude (faces and border edges),
//! bevel (edges and vertices) and loop cut.
//!
//! Starting a tool adds its step with the last settings and opens an options window. Every change
//! in the window rewrites that step (`Command::SetStep`), so the viewport shows the result live.
//! **Apply** (or Enter) folds everything the window did into one undo step; **Cancel** (or Esc)
//! takes it all back. The same fields appear on the step in the Build History, so a step stays
//! editable afterwards.

use egui::Key;
use modelcraft_engine::Command;
use modelcraft_engine::ops::bevel::{BevelProfile, MAX_SEGMENTS};
use modelcraft_engine::ops::extrude::MAX_SUBDIVISIONS;
use modelcraft_engine::ops::loopcut::MAX_CUTS;
use modelcraft_engine::scene::{ObjectId, StepOp};

use crate::app::{ModelApp, SelectMode};
use crate::panels::{drag, set_name};

/// The step the open window edits.
pub struct Options {
    object: ObjectId,
    index: usize,
    step_id: u64,
    /// The undo mark before the window's first change.
    mark: usize,
    /// The undo mark after its latest change; if the history moved on without it, the window
    /// closes rather than guess.
    expected: usize,
    focus_pending: bool,
}

/// Settings kept between uses.
pub struct State {
    pub window: Option<Options>,
    pub cut: crate::cut_ui::State,
    pub soft: crate::soft_ui::State,
    pub bevel_segments: u32,
    pub bevel_profile: BevelProfile,
    pub bevel_tension: f32,
    pub extrude_rows: u32,
    pub extrude_caps: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            window: None,
            cut: Default::default(),
            soft: Default::default(),
            bevel_segments: 2,
            bevel_profile: BevelProfile::Round,
            bevel_tension: 0.5,
            extrude_rows: 1,
            extrude_caps: true,
        }
    }
}

impl State {
    pub fn is_open(&self) -> bool {
        self.window.is_some()
    }
}

/// Add `op` as a step and open its options window.
fn begin(app: &mut ModelApp, op: StepOp) {
    // A window that is still open is applied before the next tool starts.
    apply(app);
    let Some(id) = app.edit_target() else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let mark = app.doc.undo_mark();
    app.add_step(op);
    let steps = app.doc.scene.get(id).map(|o| o.steps().to_vec()).unwrap_or_default();
    let Some(last) = steps.last() else { return };
    if app.doc.undo_mark() == mark {
        return; // the command failed; the status says why
    }
    app.modeling.window = Some(Options { object: id, index: steps.len() - 1, step_id: last.id, mark, expected: app.doc.undo_mark(), focus_pending: true });
}

pub fn extrude(app: &mut ModelApp) {
    let sel: Vec<u32> = app.components.iter().copied().collect();
    if sel.is_empty() || app.mode == SelectMode::Object || app.mode == SelectMode::Vertex {
        app.status = tl!("Select faces (face mode) or border edges (edge mode) first");
        return;
    }
    let (rows, caps, distance) = (app.modeling.extrude_rows, app.modeling.extrude_caps, app.ops.extrude);
    let op = match app.mode {
        SelectMode::Edge => StepOp::ExtrudeEdges { edges: sel, distance: distance.abs().max(0.05), lift: 0.0 },
        _ => StepOp::Extrude { faces: sel, distance, offset: 0.0, subdivisions: rows, variation: 0.0, seed: 0, caps, individual: false },
    };
    begin(app, op);
}

pub fn bevel(app: &mut ModelApp) {
    let sel: Vec<u32> = app.components.iter().copied().collect();
    let width = app.ops.bevel;
    let op = match app.mode {
        SelectMode::Edge if !sel.is_empty() => {
            StepOp::Bevel { edges: sel, width, segments: app.modeling.bevel_segments, profile: app.modeling.bevel_profile, tension: app.modeling.bevel_tension }
        }
        SelectMode::Vertex if !sel.is_empty() => StepOp::BevelVertices { verts: sel, width },
        _ => {
            app.status = tl!("Bevel needs selected edges (edge mode) or vertices (vertex mode)");
            return;
        }
    };
    begin(app, op);
}

pub fn loop_cut(app: &mut ModelApp) {
    let (SelectMode::Edge, Some(&edge)) = (app.mode, app.components.iter().next()) else {
        app.status = tl!("Loop cut needs a selected edge (edge mode)");
        return;
    };
    let cuts = app.ops.cuts;
    begin(app, StepOp::LoopCut { edge, cuts, offset: 0.0, symmetric: false, profile: 0.0 });
}

fn title(op: &StepOp) -> String {
    match op {
        StepOp::Extrude { .. } => tl!("Extrude Options"),
        StepOp::ExtrudeEdges { .. } => tl!("Extrude Edges Options"),
        StepOp::Bevel { .. } => tl!("Bevel Options"),
        StepOp::BevelVertices { .. } => tl!("Bevel Vertices Options"),
        StepOp::LoopCut { .. } => tl!("Loop Cut Options"),
        _ => tl!("Options"),
    }
}

/// Keep what the window is doing as the next use's settings.
fn remember(app: &mut ModelApp, op: &StepOp) {
    match op {
        StepOp::Extrude { distance, subdivisions, caps, .. } => {
            app.ops.extrude = *distance;
            app.modeling.extrude_rows = *subdivisions;
            app.modeling.extrude_caps = *caps;
        }
        StepOp::ExtrudeEdges { distance, .. } => app.ops.extrude = *distance,
        StepOp::Bevel { width, segments, profile, tension, .. } => {
            app.ops.bevel = *width;
            app.modeling.bevel_segments = *segments;
            app.modeling.bevel_profile = *profile;
            app.modeling.bevel_tension = *tension;
        }
        StepOp::BevelVertices { width, .. } => app.ops.bevel = *width,
        StepOp::LoopCut { cuts, .. } => app.ops.cuts = *cuts,
        _ => {}
    }
}

/// Which place a set of fields is drawn in: the names get a suffix in the options window so
/// that they differ from the fields of the same step in the Build History.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Window,
    History,
}

impl Place {
    fn name(self, label: &str) -> String {
        match self {
            Place::Window => crate::tlf!("{label} (tool options)", label = label),
            Place::History => label.to_string(),
        }
    }
}

fn num(ui: &mut egui::Ui, place: Place, v: &mut f32, speed: f32, range: std::ops::RangeInclusive<f32>, label: String) -> egui::Response {
    let r = drag(ui, v, speed, range, label.clone());
    set_name(ui, &r, place.name(&label));
    r
}

fn int(ui: &mut egui::Ui, place: Place, v: &mut u32, range: std::ops::RangeInclusive<u32>, label: String) -> egui::Response {
    ui.label(label.clone());
    let r = ui.add(egui::DragValue::new(v).range(range));
    set_name(ui, &r, place.name(&label));
    r
}

fn check(ui: &mut egui::Ui, place: Place, v: &mut bool, label: String) -> egui::Response {
    let r = ui.checkbox(v, label.clone());
    set_name(ui, &r, place.name(&label));
    r
}

/// The fields of a step this module edits, for the Build History. Returns the response to
/// commit from: the one that changed, else the last one drawn. `None` for other steps.
pub fn fields(ui: &mut egui::Ui, op: &mut StepOp) -> Option<egui::Response> {
    let rs = draw_fields(ui, op, Place::History);
    let pick = rs.iter().position(|r| r.changed() || r.drag_stopped()).or(rs.len().checked_sub(1))?;
    rs.into_iter().nth(pick)
}

fn draw_fields(ui: &mut egui::Ui, op: &mut StepOp, place: Place) -> Vec<egui::Response> {
    let mut rs: Vec<egui::Response> = Vec::new();
    match op {
        StepOp::Extrude { distance, offset, subdivisions, variation, seed, caps, individual, .. } => {
            rs.push(num(ui, place, distance, 0.01, -100.0..=100.0, tl!("Distance")));
            rs.push(num(ui, place, offset, 0.01, -100.0..=100.0, tl!("Offset")));
            rs.push(int(ui, place, subdivisions, 1..=MAX_SUBDIVISIONS, tl!("Rows")));
            rs.push(num(ui, place, variation, 0.01, 0.0..=1.0, tl!("Variation")));
            rs.push(int(ui, place, seed, 0..=u32::MAX, tl!("Variation seed")));
            rs.push(check(ui, place, caps, tl!("Caps")));
            rs.push(check(ui, place, individual, tl!("Each face on its own")));
        }
        StepOp::ExtrudeEdges { distance, lift, .. } => {
            rs.push(num(ui, place, distance, 0.01, -100.0..=100.0, tl!("Distance")));
            rs.push(num(ui, place, lift, 0.01, -100.0..=100.0, tl!("Lift")));
        }
        StepOp::Bevel { width, segments, profile, tension, .. } => {
            rs.push(num(ui, place, width, 0.005, 0.0..=10.0, tl!("Width")));
            rs.push(int(ui, place, segments, 1..=MAX_SEGMENTS, tl!("Segments")));
            for (p, name) in [(BevelProfile::Flat, tl!("Flat")), (BevelProfile::Round, tl!("Round")), (BevelProfile::Custom, tl!("Custom"))] {
                let r = ui.radio_value(profile, p, name.clone());
                set_name(ui, &r, place.name(&crate::tlf!("Profile: {shape}", shape = name)));
                rs.push(r);
            }
            if *profile == BevelProfile::Custom {
                rs.push(num(ui, place, tension, 0.01, 0.0..=1.0, tl!("Tension")));
            }
        }
        StepOp::BevelVertices { width, .. } => rs.push(num(ui, place, width, 0.005, 0.0..=10.0, tl!("Width"))),
        StepOp::MoveVertices { soft, symmetry, .. } => {
            if let Some(s) = soft {
                rs.push(num(ui, place, &mut s.radius, 0.01, 0.001..=1e5, tl!("Soft radius")));
            }
            if let Some(sym) = symmetry {
                ui.label(crate::tlf!("Symmetry: {axis}", axis = [tl!("X"), tl!("Y"), tl!("Z")].get(usize::from(sym.axis.min(2))).cloned().unwrap_or_default()));
            }
        }
        StepOp::LoopCut { cuts, offset, symmetric, profile, .. } => {
            rs.push(int(ui, place, cuts, 1..=MAX_CUTS, tl!("Cuts")));
            rs.push(num(ui, place, offset, 0.01, -1.0..=1.0, tl!("Slide")));
            rs.push(check(ui, place, symmetric, tl!("Symmetric")));
            rs.push(num(ui, place, profile, 0.01, -1.0..=1.0, tl!("Spacing curve")));
        }
        _ => {}
    }
    rs
}

/// Accept the window's work as one undo step.
pub fn apply(app: &mut ModelApp) -> bool {
    let Some(w) = app.modeling.window.take() else { return false };
    if app.doc.undo_mark() == w.expected && app.doc.undo_mark() > w.mark + 1 {
        let label = app.doc.scene.get(w.object).and_then(|o| o.steps().get(w.index)).map(|s| tl!(s.op.name())).unwrap_or_default();
        app.doc.fold_undo_since(w.mark, &label, false);
    }
    true
}

/// Take back everything the window did.
pub fn cancel(app: &mut ModelApp) -> bool {
    let Some(w) = app.modeling.window.take() else { return false };
    if app.doc.undo_mark() == w.expected {
        for _ in 0..app.doc.undo_mark().saturating_sub(w.mark) {
            app.run(Command::Undo);
        }
        app.components.clear();
        app.status = tl!("Cancelled");
    }
    true
}

/// Escape while the window is open cancels the tool.
pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    app.modeling.is_open() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) && cancel(app)
}

/// Draw the options window.
pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(w) = &app.modeling.window else { return };
    let (object, index, step_id, expected) = (w.object, w.index, w.step_id, w.expected);
    // The step it edits must still be there, and nothing else may have changed the history.
    let step = app.doc.scene.get(object).and_then(|o| o.steps().get(index).cloned()).filter(|s| s.id == step_id);
    let Some(step) = step.filter(|_| app.doc.undo_mark() == expected) else {
        app.modeling.window = None;
        return;
    };
    let mut op = step.op.clone();
    let error = app.doc.eval.evaluate(&app.doc.scene, object).errors.into_iter().find(|e| e.step == index).map(|e| e.message);
    let (mut do_apply, mut do_cancel, mut changed) = (false, false, false);
    let mut focus_pending = app.modeling.window.as_ref().is_some_and(|w| w.focus_pending);
    egui::Window::new(title(&op)).collapsible(false).resizable(false).anchor(egui::Align2::RIGHT_TOP, [-12.0, 60.0]).show(ctx, |ui| {
        let rs = ui.vertical(|ui| draw_fields(ui, &mut op, Place::Window)).inner;
        changed = rs.iter().any(|r| r.changed() || r.drag_stopped());
        if focus_pending {
            // Put the keyboard on the first number so typing starts at once.
            if let Some(first) = rs.first() {
                first.request_focus();
            }
            focus_pending = false;
        }
        let committed = rs.iter().any(|r| r.lost_focus());
        if let Some(e) = &error {
            ui.colored_label(egui::Color32::from_rgb(240, 180, 60), e);
        }
        ui.horizontal(|ui| {
            do_apply |= ui.button(tl!("Apply")).clicked();
            do_cancel |= ui.button(tl!("Cancel")).clicked();
        });
        // Enter applies, unless it was the Enter that finished typing a number or a focused
        // control (a checkbox, a button) takes it. Shift+Enter always applies.
        let nothing_focused = ui.ctx().memory(|m| m.focused()).is_none();
        if ui.input(|i| i.key_pressed(Key::Enter) && (i.modifiers.shift || (nothing_focused && !committed))) {
            do_apply = true;
        }
    });
    if let Some(w) = app.modeling.window.as_mut() {
        w.focus_pending = focus_pending;
    }
    if (changed || op != step.op) && app.run(Command::SetStep { id: object, index, op: op.clone() }) {
        remember(app, &op);
        let now = app.doc.undo_mark();
        if let Some(w) = app.modeling.window.as_mut() {
            w.expected = now;
        }
    }
    if do_cancel {
        cancel(app);
    } else if do_apply {
        apply(app);
        app.status = tl!("Applied");
    }
}
