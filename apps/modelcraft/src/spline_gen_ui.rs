//! Properties sections of the spline generators (extrude, revolve, loft, sweep). Every control has
//! an accessible name, the pickers are combo boxes that work from the keyboard, and the numbers
//! commit as one undo step.

use egui::RichText;
use modelcraft_engine::ops::spline_gen::Axis;
use modelcraft_engine::scene::spline_gen::SplineGen;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, SplineCommand};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name};
use crate::theme;

pub fn kind_label(kind: &str) -> String {
    match kind {
        "extrude" => tl!("Extrude"),
        "revolve" => tl!("Revolve"),
        "loft" => tl!("Loft"),
        "sweep" => tl!("Sweep"),
        _ => kind.to_string(),
    }
}

fn axis_label(a: Axis) -> String {
    match a {
        Axis::Auto => tl!("Automatic"),
        Axis::X => tl!("X axis"),
        Axis::Y => tl!("Y axis"),
        Axis::Z => tl!("Z axis"),
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

fn axis_field(ui: &mut egui::Ui, id: ObjectId, axis: &mut Axis, allow_auto: bool) -> bool {
    let axes: Vec<Axis> = Axis::ALL.iter().copied().filter(|a| allow_auto || *a != Axis::Auto).collect();
    let labels: Vec<String> = axes.iter().map(|a| axis_label(*a)).collect();
    match pick(ui, ("spline-gen-axis", id.0), tl!("Axis"), axes.iter().position(|a| a == axis).unwrap_or(0), &labels) {
        Some(i) => {
            *axis = axes.get(i).copied().unwrap_or(*axis);
            true
        }
        None => false,
    }
}

fn count(ui: &mut egui::Ui, v: &mut u32, range: std::ops::RangeInclusive<u32>, name: String) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(v).speed(0.2).range(range));
        set_name(ui, &r, name);
        r
    })
    .inner
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, generator: &SplineGen) {
    use modelcraft_engine::ops::spline_gen::MAX_STEPS;
    let mut q = generator.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    match &mut q {
        SplineGen::Extrude(o) => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut o.distance, 0.01, -1.0e6..=1.0e6, tl!("Distance"))));
            discrete |= axis_field(ui, id, &mut o.axis, true);
            drags.push(count(ui, &mut o.steps, 1..=MAX_STEPS, tl!("Steps")));
            discrete |= ui.checkbox(&mut o.cap_start, tl!("Cap at the start")).changed();
            discrete |= ui.checkbox(&mut o.cap_end, tl!("Cap at the end")).changed();
        }
        SplineGen::Revolve(o) => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut o.angle, 0.5, -360.0..=360.0, tl!("Angle"))));
            drags.push(count(ui, &mut o.steps, 1..=MAX_STEPS, tl!("Steps")));
            discrete |= axis_field(ui, id, &mut o.axis, false);
            discrete |= ui.checkbox(&mut o.caps, tl!("Cap the ends")).changed();
        }
        SplineGen::Loft(o) => {
            drags.push(count(ui, &mut o.steps, 1..=MAX_STEPS, tl!("Steps between curves")));
            drags.push(count(ui, &mut o.points, 0..=4096, tl!("Points per curve")));
            discrete |= ui.checkbox(&mut o.smooth, tl!("Smooth through the curves")).changed();
            discrete |= ui.checkbox(&mut o.caps, tl!("Cap the ends")).changed();
        }
        SplineGen::Sweep(o) => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut o.end_scale, 0.01, 0.001..=1000.0, tl!("Scale at the end"))));
            ui.horizontal(|ui| drags.push(drag(ui, &mut o.twist, 0.5, -36_000.0..=36_000.0, tl!("Twist"))));
            ui.horizontal(|ui| drags.push(drag(ui, &mut o.roll, 0.5, -36_000.0..=36_000.0, tl!("Roll at the start"))));
            discrete |= ui.checkbox(&mut o.caps, tl!("Cap the ends")).changed();
        }
    }
    if q != *generator && discrete {
        app.run(Command::Spline(SplineCommand::SetGenerator { id, generator: q }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        let preview = q.clone();
        commit_edit(
            app,
            r,
            |o| o.kind = ObjectKind::SplineGenerator { generator: preview },
            || Command::Spline(SplineCommand::SetGenerator { id, generator: q }),
            id,
        );
    }
    problems(app, ui, id);
    let hint = match generator {
        SplineGen::Extrude(_) => tl!("Pushes the splines inside it along an axis. Splines inside other splines make holes."),
        SplineGen::Revolve(_) => tl!("Turns the splines inside it about an axis through the origin."),
        SplineGen::Loft(_) => tl!("Blends through the splines inside it, in order."),
        SplineGen::Sweep(_) => tl!("Moves the first spline inside it (the profile) along the second (the path)."),
    };
    ui.label(RichText::new(hint).color(theme::TEXT_DIM).small());
}

fn problems(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let e = app.doc.eval.evaluate(&app.doc.scene, id);
    ui.label(crate::tlf!("{count} faces", count = e.mesh.face_count()));
    for err in &e.errors {
        ui.label(RichText::new(&err.message).color(theme::TEXT_DIM).small());
    }
}
