//! The Properties section of a clone generator: its layout, counts, offsets, seed and target.
//! Every field has an accessible name; layout and axis are combo boxes, so they work from the
//! keyboard like the rest of the panel.

use egui::RichText;
use modelcraft_engine::Command;
use modelcraft_engine::clone::{Axis, KINDS, Layout, MAX_CLONES};
use modelcraft_engine::scene::{Cloner, ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name, vec3_row};
use crate::theme;

/// The label of a layout (our own names, `docs/feature-names.md`).
pub fn layout_label(kind: &str) -> String {
    match kind {
        "linear" => tl!("Linear"),
        "radial" => tl!("Radial"),
        "grid" => tl!("Grid"),
        "scatter" => tl!("Scatter"),
        "surface" => tl!("Surface"),
        "vertices" => tl!("Vertices"),
        _ => kind.to_string(),
    }
}

fn axis_label(a: Axis) -> String {
    match a {
        Axis::X => tl!("X axis"),
        Axis::Y => tl!("Y axis"),
        Axis::Z => tl!("Z axis"),
    }
}

/// An integer field with an accessible name.
fn count(ui: &mut egui::Ui, v: &mut u32, name: String) -> egui::Response {
    ui.label(name.clone());
    let r = ui.add(egui::DragValue::new(v).speed(0.2).range(1..=MAX_CLONES as u32));
    set_name(ui, &r, name);
    r
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, cloner: Cloner, target: Option<ObjectId>) {
    let mut q = cloner;
    let mut t = target;
    let mut drags: Vec<egui::Response> = Vec::new();
    // Direct changes (combos, checkboxes) commit at once; drags commit when released.
    let mut discrete = false;

    ui.horizontal(|ui| {
        ui.label(tl!("Layout"));
        let before = q.kind();
        let r = egui::ComboBox::from_id_salt(("clone-layout", id.0)).selected_text(layout_label(before)).show_ui(ui, |ui| {
            for k in KINDS {
                if ui.selectable_label(before == *k, layout_label(k)).clicked() && before != *k {
                    q.layout = Layout::default_of(k).unwrap_or(q.layout);
                    discrete = true;
                }
            }
        });
        set_name(ui, &r.response, tl!("Layout"));
    });

    match &mut q.layout {
        Layout::Linear { count: n, offset, rotation, scale } => {
            ui.horizontal(|ui| drags.push(count(ui, n, tl!("Clone count"))));
            drags.extend(vec3_row(ui, &tl!("Offset"), offset, 0.01));
            drags.extend(vec3_row(ui, &tl!("Turn per clone"), rotation, 0.5));
            drags.extend(vec3_row(ui, &tl!("Grow per clone"), scale, 0.01));
        }
        Layout::Radial { count: n, radius, axis, start, end, align } => {
            ui.horizontal(|ui| drags.push(count(ui, n, tl!("Clone count"))));
            ui.horizontal(|ui| drags.push(drag(ui, radius, 0.01, 0.0..=100_000.0, tl!("Radius"))));
            ui.horizontal(|ui| {
                drags.push(drag(ui, start, 0.5, -36_000.0..=36_000.0, tl!("Start angle")));
                drags.push(drag(ui, end, 0.5, -36_000.0..=36_000.0, tl!("End angle")));
            });
            ui.horizontal(|ui| {
                ui.label(tl!("Axis"));
                let r = egui::ComboBox::from_id_salt(("clone-axis", id.0)).selected_text(axis_label(*axis)).show_ui(ui, |ui| {
                    for a in [Axis::X, Axis::Y, Axis::Z] {
                        if ui.selectable_label(*axis == a, axis_label(a)).clicked() && *axis != a {
                            *axis = a;
                            discrete = true;
                        }
                    }
                });
                set_name(ui, &r.response, tl!("Axis"));
            });
            let r = ui.checkbox(align, tl!("Face outwards"));
            discrete |= r.changed();
        }
        Layout::Grid { count: n, spacing } => {
            ui.horizontal(|ui| {
                for (axis, v) in ["X", "Y", "Z"].iter().zip(n.iter_mut()) {
                    drags.push(count(ui, v, crate::tlf!("Count {axis}", axis = axis)));
                }
            });
            drags.extend(vec3_row(ui, &tl!("Spacing"), spacing, 0.01));
        }
        Layout::Scatter { count: n, extent } => {
            ui.horizontal(|ui| drags.push(count(ui, n, tl!("Clone count"))));
            drags.extend(vec3_row(ui, &tl!("Box size"), extent, 0.01));
        }
        Layout::Surface { count: n, align } => {
            ui.horizontal(|ui| drags.push(count(ui, n, tl!("Clone count"))));
            let r = ui.checkbox(align, tl!("Stand on surface"));
            discrete |= r.changed();
        }
        Layout::Vertices { align } => {
            let r = ui.checkbox(align, tl!("Follow vertex normal"));
            discrete |= r.changed();
        }
    }

    if q.layout.is_random() {
        ui.horizontal(|ui| {
            ui.label(tl!("Seed"));
            let r = ui.add(egui::DragValue::new(&mut q.seed).speed(0.2));
            set_name(ui, &r, tl!("Seed"));
            drags.push(r);
        });
    }

    if q.layout.needs_target() {
        let candidates: Vec<(ObjectId, String)> = app
            .doc
            .scene
            .objects()
            .iter()
            .filter(|o| o.id != id && matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. }) && !app.doc.scene.subtree(id).contains(&o.id))
            .map(|o| (o.id, o.name.clone()))
            .collect();
        let shown = t.and_then(|x| candidates.iter().find(|(i, _)| *i == x)).map_or_else(|| tl!("None"), |(_, n)| n.clone());
        ui.horizontal(|ui| {
            ui.label(tl!("Target object"));
            let r = egui::ComboBox::from_id_salt(("clone-target", id.0)).selected_text(shown).show_ui(ui, |ui| {
                if ui.selectable_label(t.is_none(), tl!("None")).clicked() && t.is_some() {
                    t = None;
                    discrete = true;
                }
                for (cid, name) in &candidates {
                    if ui.selectable_label(t == Some(*cid), name).clicked() && t != Some(*cid) {
                        t = Some(*cid);
                        discrete = true;
                    }
                }
            });
            set_name(ui, &r.response, tl!("Target object"));
        });
    }

    // Commit: a combo or checkbox change at once, number fields as one undo step per edit.
    let changed = q != cloner || t != target;
    if changed && discrete {
        app.run(Command::SetCloner { id, cloner: q, target: t });
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Cloner { cloner: q, target: t }, || Command::SetCloner { id, cloner: q, target: t }, id);
    }

    // What came out.
    let e = app.doc.eval.evaluate(&app.doc.scene, id);
    if let Some(inst) = &e.instances {
        ui.label(crate::tlf!("{count} clones of one mesh", count = inst.clones.len()));
    }
    for err in &e.errors {
        ui.label(RichText::new(&err.message).color(theme::TEXT_DIM).small());
    }
    ui.label(
        RichText::new(crate::tlf!("Up to {max} clones. The source object is the child inside this generator.", max = MAX_CLONES))
            .color(theme::TEXT_DIM)
            .small(),
    );
}
