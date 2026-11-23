//! The falloff-layer list in a deformer's or clone modifier's Properties: every field inside it
//! as a row with an enable switch, a blend mode, an opacity and buttons that move it earlier or
//! later in the stack. Each row is built from controls that work from the keyboard (checkbox,
//! combo box, spin button, buttons), and every name carries the field's own name so no two rows
//! sound the same. Selecting a row's name selects the field, whose full settings are in its own
//! Properties section.

use egui::RichText;
use modelcraft_engine::field::{BLENDS, Blend, Field};
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, RigCommand};

use crate::app::ModelApp;
use crate::field_ui::blend_label;
use crate::panels::{commit_edit, set_name};
use crate::theme;

/// The fields directly inside `owner`, in stack order.
pub fn layers(app: &ModelApp, owner: ObjectId) -> Vec<(ObjectId, String, Field)> {
    let scene = &app.doc.scene;
    scene
        .children(owner)
        .into_iter()
        .filter_map(|c| scene.get(c))
        .filter_map(|o| match &o.kind {
            ObjectKind::Field { field } => Some((o.id, o.name.clone(), field.clone())),
            _ => None,
        })
        .collect()
}

/// Move a field one place earlier (`-1`) or later (`1`) among its siblings.
pub fn move_layer(app: &mut ModelApp, field: ObjectId, delta: i32) {
    app.run(Command::Rig(RigCommand::MoveInStack { id: field, delta }));
}

pub fn list(app: &mut ModelApp, ui: &mut egui::Ui, owner: ObjectId) {
    let rows = layers(app, owner);
    if rows.is_empty() {
        return;
    }
    let count = rows.len();
    egui::CollapsingHeader::new(crate::tlf!("Falloff layers ({count})", count = count)).id_salt(("field-layers", owner.0)).default_open(true).show(ui, |ui| {
        for (i, (fid, name, field)) in rows.iter().enumerate() {
            ui.push_id(("field-layer", fid.0), |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut q = field.clone();
                    let mut discrete = false;

                    let r = ui.checkbox(&mut q.enabled, name.clone());
                    set_name(ui, &r, crate::tlf!("{name} switched on", name = name));
                    discrete |= r.changed();

                    let mut picked = false;
                    let combo = egui::ComboBox::from_id_salt(("field-layer-blend", fid.0)).selected_text(blend_label(q.blend.name())).show_ui(ui, |ui| {
                        for b in BLENDS {
                            let Some(blend) = Blend::from_name(b) else { continue };
                            if ui.selectable_label(q.blend == blend, blend_label(b)).clicked() && q.blend != blend {
                                q.blend = blend;
                                picked = true;
                            }
                        }
                    });
                    set_name(ui, &combo.response, crate::tlf!("{name} blend", name = name));
                    if picked {
                        combo.response.request_focus();
                        discrete = true;
                    }

                    ui.label(tl!("Opacity"));
                    let op = ui.add(egui::DragValue::new(&mut q.opacity).speed(0.01).range(0.0..=1.0).max_decimals(2));
                    set_name(ui, &op, crate::tlf!("{name} opacity", name = name));

                    let up = ui.add_enabled(i > 0, egui::Button::new(tl!("Earlier")).small());
                    set_name(ui, &up, crate::tlf!("Earlier: {name}", name = name));
                    let down = ui.add_enabled(i + 1 < count, egui::Button::new(tl!("Later")).small());
                    set_name(ui, &down, crate::tlf!("Later: {name}", name = name));
                    if up.clicked() {
                        move_layer(app, *fid, -1);
                    } else if down.clicked() {
                        move_layer(app, *fid, 1);
                    }

                    let select = ui.add(egui::Button::new(tl!("Edit")).small());
                    set_name(ui, &select, crate::tlf!("Edit {name}", name = name));
                    if select.clicked() {
                        app.select_only(*fid);
                    }

                    if q != *field {
                        if discrete {
                            app.run(Command::SetField { id: *fid, field: q });
                        } else {
                            let fq = q.clone();
                            let fid = *fid;
                            commit_edit(
                                app,
                                &op,
                                |o| o.kind = ObjectKind::Field { field: fq.clone() },
                                || Command::SetField { id: fid, field: fq.clone() },
                                fid,
                            );
                        }
                    }
                });
            });
        }
        ui.label(
            RichText::new(tl!("Fields combine from the top: each is blended with the ones above it, by its opacity. Switch one off to leave it out."))
                .color(theme::TEXT_DIM)
                .small(),
        );
    });
}
