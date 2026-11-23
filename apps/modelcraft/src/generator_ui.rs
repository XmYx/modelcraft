//! Properties sections of the array and symmetry generators. Every control has an accessible name
//! and the pickers are combo boxes, so they work from the keyboard.

use egui::RichText;
use modelcraft_engine::Command;
use modelcraft_engine::ops::array::{Array, KINDS, MAX_COPIES};
use modelcraft_engine::scene::{ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::panels::{KeyPaths, commit_edit, drag_k, set_name, vec3_row_k};
use crate::theme;

pub fn array_label(kind: &str) -> String {
    match kind {
        "linear" => tl!("Linear"),
        "radial" => tl!("Radial"),
        _ => kind.to_string(),
    }
}

pub fn axis_label(a: u32) -> String {
    match a {
        0 => tl!("X axis"),
        1 => tl!("Y axis"),
        _ => tl!("Z axis"),
    }
}

/// A combo box over `options` (label per index); returns the index picked this frame. Focus goes
/// back to the combo after a pick (its popup closes).
pub(crate) fn pick(ui: &mut egui::Ui, salt: impl std::hash::Hash + std::fmt::Debug, name: String, current: usize, options: &[String]) -> Option<usize> {
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

pub fn array_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, array: Array) {
    let mut q = array;
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    let kinds: Vec<String> = KINDS.iter().map(|k| array_label(k)).collect();
    let cur = KINDS.iter().position(|k| *k == q.kind()).unwrap_or(0);
    if let Some(i) = pick(ui, ("array-layout", id.0), tl!("Layout"), cur, &kinds)
        && let Some(a) = KINDS.get(i).and_then(|k| Array::default_of(k))
    {
        q = a;
        discrete = true;
    }
    match &mut q {
        Array::Linear { count, offset, rotation, scale } => {
            ui.horizontal(|ui| {
                crate::key_marker::marker(ui, "array.count", &tl!("Copies"));
                ui.label(tl!("Copies"));
                let r = ui.add(egui::DragValue::new(count).speed(0.2).range(1..=MAX_COPIES));
                set_name(ui, &r, tl!("Copies"));
                drags.push(r);
            });
            drags.extend(vec3_row_k(ui, &tl!("Offset"), KeyPaths::Index("array.offset"), offset, 0.01));
            drags.extend(vec3_row_k(ui, &tl!("Turn per copy"), KeyPaths::Index("array.rotation"), rotation, 0.5));
            drags.extend(vec3_row_k(ui, &tl!("Grow per copy"), KeyPaths::Index("array.scale"), scale, 0.01));
        }
        Array::Radial { count, radius, axis, angle, align } => {
            ui.horizontal(|ui| {
                crate::key_marker::marker(ui, "array.count", &tl!("Copies"));
                ui.label(tl!("Copies"));
                let r = ui.add(egui::DragValue::new(count).speed(0.2).range(1..=MAX_COPIES));
                set_name(ui, &r, tl!("Copies"));
                drags.push(r);
            });
            ui.horizontal(|ui| drags.push(drag_k(ui, "array.radius", radius, 0.01, 0.0..=100_000.0, tl!("Radius"))));
            ui.horizontal(|ui| drags.push(drag_k(ui, "array.angle", angle, 0.5, -36_000.0..=36_000.0, tl!("Angle"))));
            let axes: Vec<String> = (0..3).map(axis_label).collect();
            if let Some(i) = pick(ui, ("array-axis", id.0), tl!("Axis"), *axis as usize, &axes) {
                *axis = i as u32;
                discrete = true;
            }
            let r = ui.checkbox(align, tl!("Face outwards"));
            discrete |= r.changed();
        }
    }
    if q != array && discrete {
        app.run(Command::SetArray { id, array: q });
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Array { array: q }, || Command::SetArray { id, array: q }, id);
    }
    problems(app, ui, id);
    ui.label(
        RichText::new(tl!("Joins real copies of the first mesh inside into one mesh. For many copies that stay light, use Clones."))
            .color(theme::TEXT_DIM)
            .small(),
    );
}

pub fn symmetry_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, axis: u32, weld: f32) {
    let (mut a, mut w) = (axis, weld);
    let axes: Vec<String> = (0..3).map(axis_label).collect();
    let mut discrete = false;
    if let Some(i) = pick(ui, ("symmetry-axis", id.0), tl!("Mirror across"), axis as usize, &axes) {
        a = i as u32;
        discrete = true;
    }
    let mut drags = Vec::new();
    ui.horizontal(|ui| drags.push(drag_k(ui, "weld", &mut w, 0.001, 0.0..=10.0, tl!("Join distance"))));
    if discrete {
        app.run(Command::SetSymmetry { id, axis: a, weld: w });
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Symmetry { axis: a, weld: w }, || Command::SetSymmetry { id, axis: a, weld: w }, id);
    }
    problems(app, ui, id);
    ui.label(RichText::new(tl!("Shows the first mesh inside mirrored across a plane through its origin, joined at the seam.")).color(theme::TEXT_DIM).small());
}

fn problems(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let e = app.doc.eval.evaluate(&app.doc.scene, id);
    ui.label(crate::tlf!("{count} faces", count = e.mesh.face_count()));
    for err in &e.errors {
        ui.label(RichText::new(&err.message).color(theme::TEXT_DIM).small());
    }
}
