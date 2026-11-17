//! Constraints in the app: the constraint entries of the Rig menu, the Constraints section of
//! the Properties panel (weights, switches, solver, formulas, driven keys) and the per-frame
//! solve that keeps constrained objects following while something is dragged.
//!
//! Selection order names the roles, as in the tree: what is followed first, what follows last
//! (the active object). Every action is a `rig` command, so it is undoable, journaled and
//! available to scripts.

use egui::RichText;
use modelcraft_engine::scene::anim::TRANSFORM_PATHS;
use modelcraft_engine::scene::constraint::{Constraint, ConstraintKind, IkSolver};
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, RigCommand};

use crate::app::ModelApp;
use crate::rig_ui::RigAction;
use crate::theme;

fn kind_of(a: RigAction) -> Option<&'static str> {
    Some(match a {
        RigAction::ConstrainPosition => "point",
        RigAction::ConstrainRotation => "orient",
        RigAction::ConstrainScale => "scale",
        RigAction::ConstrainAsChild => "parent",
        RigAction::ConstrainLookAt => "aim",
        _ => return None,
    })
}

/// The IK constraints that end at `id` or turn it.
fn ik_of(app: &ModelApp, id: ObjectId) -> Option<u32> {
    let scene = &app.doc.scene;
    scene
        .constraints
        .list()
        .iter()
        .find(|c| matches!(c.kind, ConstraintKind::Ik { .. }) && (c.target == id || scene.constraint_drives(c).contains(&id)))
        .map(|c| c.id)
}

pub fn act(app: &mut ModelApp, a: RigAction) {
    if let Some(kind) = kind_of(a) {
        let Some(target) = app.active() else {
            app.status = tl!("Select what to follow, then the object to constrain");
            return;
        };
        let sources: Vec<ObjectId> = app.selection.iter().copied().filter(|id| *id != target && app.doc.scene.get(*id).is_some()).collect();
        if sources.is_empty() {
            app.status = tl!("Select what to follow, then the object to constrain");
            return;
        }
        let n = sources.len();
        let cmd = RigCommand::AddConstraint {
            kind: kind.into(),
            target,
            sources,
            weights: vec![],
            maintain_offset: true,
            aim: "y".into(),
            up: "z".into(),
            up_object: None,
            name: None,
        };
        if app.run(Command::Rig(cmd)) && app.doc.scene.constraints.problems().is_empty() {
            app.status = crate::tlf!("Constrained to {count} objects; set weights in Properties", count = n);
        }
        return;
    }
    match a {
        RigAction::AddIk => {
            let is_joint = |id: &ObjectId| matches!(app.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Joint { .. }));
            let Some(end) = app.active().filter(is_joint) else {
                app.status = tl!("Select the end joint of the chain (after a goal object, if you have one)");
                return;
            };
            // Another selected object that is not a joint is the goal; without one a goal is made.
            let goal = app.selection.iter().copied().find(|id| *id != end && !is_joint(id) && app.doc.scene.get(*id).is_some());
            if app.run(Command::Rig(RigCommand::AddIk { end, goal, pole: None, bones: 2, solver: IkSolver::Auto, name: None })) {
                app.status = tl!("IK added: move the goal and the two joints above the end joint follow");
            }
        }
        RigAction::MatchFk | RigAction::MatchIk => {
            let Some(id) = app.selection.iter().rev().find_map(|o| ik_of(app, *o)) else {
                app.status = tl!("Select a joint of an IK chain first");
                return;
            };
            let to = if a == RigAction::MatchFk { "fk" } else { "ik" };
            if app.run(Command::Rig(RigCommand::MatchIk { id, to: to.into() })) {
                app.status = if a == RigAction::MatchFk {
                    tl!("The joints keep the pose; IK is blended out")
                } else {
                    tl!("The goal is on the chain; IK is blended in")
                };
            }
        }
        RigAction::BakeConstraints => {
            let objects: Vec<ObjectId> = app.selection.clone();
            if objects.is_empty() {
                app.status = tl!("Select the constrained objects to bake");
                return;
            }
            app.run(Command::Rig(RigCommand::BakeConstraints { objects, start: None, end: None, step: 1.0, keep: false }));
        }
        RigAction::RemoveConstraints => {
            let ids: Vec<u32> = app.doc.scene.constraints.list().iter().filter(|c| app.selection.contains(&c.target)).map(|c| c.id).collect();
            if ids.is_empty() {
                app.status = tl!("The selected objects have no constraints");
                return;
            }
            let n = ids.len();
            // One command each (each is its own undo step, like deleting them one by one).
            if ids.into_iter().all(|id| app.run(Command::Rig(RigCommand::RemoveConstraint { id }))) {
                app.status = crate::tlf!("{count} constraints removed", count = n);
            }
        }
        _ => {}
    }
}

/// Keep constrained objects following while a gizmo or a field is dragged (those previews edit
/// the scene directly; commands solve by themselves).
pub fn tick(app: &mut ModelApp) {
    if !app.doc.scene.constraints.is_empty() {
        app.doc.scene.solve_constraints();
    }
}

fn kind_label(k: &ConstraintKind) -> String {
    match k {
        ConstraintKind::Point { .. } => tl!("Position"),
        ConstraintKind::Orient { .. } => tl!("Rotation"),
        ConstraintKind::Scale { .. } => tl!("Scale"),
        ConstraintKind::Parent { .. } => tl!("As child"),
        ConstraintKind::Aim { .. } => tl!("Look at"),
        ConstraintKind::Ik { .. } => tl!("IK"),
        ConstraintKind::Driven { .. } => tl!("Driven key"),
        ConstraintKind::Expression { .. } => tl!("Formula"),
    }
}

fn solver_label(s: IkSolver) -> String {
    match s {
        IkSolver::Auto => tl!("Automatic"),
        IkSolver::TwoBone => tl!("Two bones"),
        IkSolver::Fabrik => tl!("Reaching"),
        IkSolver::Ccd => tl!("Joint by joint"),
    }
}

fn name_of(app: &ModelApp, id: ObjectId) -> String {
    app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_else(|| tl!("missing"))
}

/// What a constraint follows, in words.
fn follows(app: &ModelApp, c: &Constraint) -> String {
    match &c.kind {
        ConstraintKind::Ik { goal, pole, bones, .. } => match pole {
            Some(p) => crate::tlf!("{bones} bones reach {goal}, bending towards {pole}", bones = bones, goal = name_of(app, *goal), pole = name_of(app, *p)),
            None => crate::tlf!("{bones} bones reach {goal}", bones = bones, goal = name_of(app, *goal)),
        },
        ConstraintKind::Driven { path, driver, driver_path, curve } => {
            crate::tlf!(
                "{path} follows {driver} {driver_path} ({count} keys)",
                path = path,
                driver = name_of(app, *driver),
                driver_path = driver_path,
                count = curve.len()
            )
        }
        ConstraintKind::Expression { path, .. } => crate::tlf!("Drives {path}", path = path),
        k => crate::tlf!("Follows {names}", names = k.sources().iter().map(|s| name_of(app, s.object)).collect::<Vec<_>>().join(", ")),
    }
}

/// A numeric field that previews while dragged and runs one command at the end.
fn commit(app: &mut ModelApp, resp: &egui::Response, preview: impl FnOnce(&mut ModelApp), cmd: impl FnOnce() -> RigCommand) {
    if resp.changed() && resp.dragged() {
        if app.prop_snapshot.is_none() {
            app.prop_snapshot = Some(Box::new(app.doc.scene.clone()));
        }
        preview(app);
        return;
    }
    if resp.drag_stopped() || resp.changed() {
        if let Some(s) = app.prop_snapshot.take() {
            app.doc.scene = *s;
        }
        app.run(Command::Rig(cmd()));
    }
}

#[derive(Clone, Default)]
struct Draft {
    path: usize,
    expr: String,
    driver_path: usize,
}

/// The Constraints section of the Properties panel for the active object.
pub fn properties(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let list: Vec<Constraint> = app.doc.scene.constraints.of(id).cloned().collect();
    let header = if list.is_empty() { tl!("Constraints") } else { crate::tlf!("Constraints ({count})", count = list.len()) };
    egui::CollapsingHeader::new(header).id_salt("constraints").default_open(true).show(ui, |ui| {
        if list.is_empty() {
            ui.label(
                RichText::new(tl!("None. Select what to follow, then this object, and pick a constraint from the Rig menu.")).color(theme::TEXT_DIM).small(),
            );
        }
        for c in &list {
            let cid = c.id;
            ui.push_id(("constraint", cid), |ui| {
                ui.horizontal(|ui| {
                    let mut on = c.enabled;
                    let r = ui.checkbox(&mut on, crate::tlf!("{name} ({kind})", name = c.name, kind = kind_label(&c.kind)));
                    if r.changed() {
                        app.run(Command::Rig(RigCommand::SetConstraint {
                            id: cid,
                            weight: None,
                            enabled: Some(on),
                            name: None,
                            source_weights: None,
                            solver: None,
                            iterations: None,
                            pole: None,
                        }));
                    }
                    if ui.button(crate::tlf!("Remove {name}", name = c.name)).clicked() {
                        app.run(Command::Rig(RigCommand::RemoveConstraint { id: cid }));
                    }
                });
                ui.label(RichText::new(follows(app, c)).color(theme::TEXT_DIM).small());
                let weighted = !matches!(c.kind, ConstraintKind::Driven { .. } | ConstraintKind::Expression { .. });
                if weighted {
                    ui.horizontal(|ui| {
                        let mut w = c.weight;
                        let name = if matches!(c.kind, ConstraintKind::Ik { .. }) {
                            crate::tlf!("{name} IK blend", name = c.name)
                        } else {
                            crate::tlf!("{name} weight", name = c.name)
                        };
                        let r = crate::panels::drag(ui, &mut w, 0.01, 0.0..=1.0, name);
                        commit(
                            app,
                            &r,
                            |app| {
                                if let Some(slot) = app.doc.scene.constraints.get_mut(cid) {
                                    slot.weight = w;
                                }
                            },
                            || RigCommand::SetConstraint {
                                id: cid,
                                weight: Some(w),
                                enabled: None,
                                name: None,
                                source_weights: None,
                                solver: None,
                                iterations: None,
                                pole: None,
                            },
                        );
                    });
                }
                // One weight per source when there are several to balance.
                if c.kind.sources().len() > 1 {
                    let weights: Vec<f32> = c.kind.sources().iter().map(|s| s.weight).collect();
                    for (i, s) in c.kind.sources().iter().enumerate() {
                        ui.horizontal(|ui| {
                            let mut w = s.weight;
                            let r = crate::panels::drag(
                                ui,
                                &mut w,
                                0.01,
                                0.0..=100.0,
                                crate::tlf!("{name} share of {source}", name = c.name, source = name_of(app, s.object)),
                            );
                            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                let mut ws = weights.clone();
                                if let Some(slot) = ws.get_mut(i) {
                                    *slot = w;
                                }
                                app.run(Command::Rig(RigCommand::SetConstraint {
                                    id: cid,
                                    weight: None,
                                    enabled: None,
                                    name: None,
                                    source_weights: Some(ws),
                                    solver: None,
                                    iterations: None,
                                    pole: None,
                                }));
                            }
                        });
                    }
                }
                match &c.kind {
                    ConstraintKind::Ik { solver, .. } => {
                        ui.horizontal(|ui| {
                            let label = ui.label(tl!("Solver"));
                            let mut pick = *solver;
                            egui::ComboBox::from_id_salt("solver")
                                .selected_text(solver_label(pick))
                                .show_ui(ui, |ui| {
                                    for s in IkSolver::ALL {
                                        ui.selectable_value(&mut pick, s, solver_label(s));
                                    }
                                })
                                .response
                                .labelled_by(label.id);
                            if pick != *solver {
                                app.run(Command::Rig(RigCommand::SetConstraint {
                                    id: cid,
                                    weight: None,
                                    enabled: None,
                                    name: None,
                                    source_weights: None,
                                    solver: Some(pick),
                                    iterations: None,
                                    pole: None,
                                }));
                            }
                        });
                    }
                    ConstraintKind::Expression { path, expr } => {
                        ui.horizontal(|ui| {
                            let key = ui.scope_id().with("formula");
                            let mut text = ui.data_mut(|d| d.get_temp::<String>(key)).unwrap_or_else(|| expr.clone());
                            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(180.0));
                            let label = crate::tlf!("Formula for {path}", path = path);
                            ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(label));
                            if r.lost_focus() && text != *expr {
                                ui.data_mut(|d| d.remove::<String>(key));
                                app.run(Command::Rig(RigCommand::SetExpression { target: id, path: path.clone(), expr: text }));
                            } else if r.has_focus() || text != *expr {
                                ui.data_mut(|d| d.insert_temp(key, text));
                            }
                        });
                    }
                    _ => {}
                }
                for p in app.doc.scene.constraints.problems().iter().filter(|p| p.constraint == cid) {
                    ui.label(RichText::new(crate::tlf!("Skipped: {reason}", reason = p.message)).color(theme::ACCENT));
                }
                ui.separator();
            });
        }
        add_rows(app, ui, id);
    });
}

fn path_combo(ui: &mut egui::Ui, salt: &str, name: String, pick: &mut usize) {
    let label = ui.label(name);
    egui::ComboBox::from_id_salt(salt)
        .selected_text(TRANSFORM_PATHS.get(*pick).copied().unwrap_or("translate.x"))
        .show_ui(ui, |ui| {
            for (i, p) in TRANSFORM_PATHS.iter().enumerate() {
                ui.selectable_value(pick, i, *p);
            }
        })
        .response
        .labelled_by(label.id);
}

/// New formula and driven key for the active object.
fn add_rows(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let key = egui::Id::unique("constraint-draft");
    let mut draft: Draft = ui.data_mut(|d| d.get_temp(key)).unwrap_or_default();
    let path = TRANSFORM_PATHS.get(draft.path).copied().unwrap_or("translate.x").to_string();
    ui.horizontal(|ui| {
        path_combo(ui, "draft-path", tl!("Attribute"), &mut draft.path);
    });
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut draft.expr).hint_text(tl!("sin(time) * 2")).desired_width(150.0));
        ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("New formula")));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button(tl!("Set Formula")).clicked() || enter)
            && !draft.expr.trim().is_empty()
            && app.run(Command::Rig(RigCommand::SetExpression { target: id, path: path.clone(), expr: draft.expr.clone() }))
        {
            draft.expr.clear();
        }
    });
    // The driver is the other selected object (selected first).
    let driver = app.selection.iter().copied().find(|d| *d != id && app.doc.scene.get(*d).is_some());
    ui.horizontal(|ui| {
        path_combo(ui, "draft-driver", tl!("Driver attribute"), &mut draft.driver_path);
        let b = ui.add_enabled(driver.is_some(), egui::Button::new(tl!("Set Driven Key")));
        let b = b.on_disabled_hover_text(tl!("Select the driving object first, then this one"));
        if b.clicked()
            && let Some(driver) = driver
        {
            let driver_path = TRANSFORM_PATHS.get(draft.driver_path).copied().unwrap_or("translate.x").to_string();
            if app.run(Command::Rig(RigCommand::SetDrivenKey { driver, driver_path, target: id, path: path.clone(), driver_value: None, value: None })) {
                app.status = tl!("Driven key set from the current values; pose both and set another");
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(key, draft));
}

/// A line from every IK chain's bending joint to its pole, and from its end joint to its goal
/// (visible when the goal is out of reach or the IK is blended out).
pub fn guide_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let scene = &app.doc.scene;
    let at = |id: ObjectId| scene.world_matrix(id).transform_point3(glam::Vec3::ZERO);
    let mut out = Vec::new();
    for c in scene.constraints.list().iter().filter(|c| c.enabled).take(4096) {
        let ConstraintKind::Ik { goal, pole, bones, .. } = &c.kind else { continue };
        if scene.get(*goal).is_none() || !scene.visible(c.target) {
            continue;
        }
        let color = [0.95, 0.85, 0.35, 0.9];
        out.push((at(c.target).to_array(), at(*goal).to_array(), color));
        if let Some(p) = pole.filter(|p| scene.get(*p).is_some()) {
            let chain = scene.ik_chain(c.target, *bones);
            if let Some(mid) = chain.get(chain.len() / 2) {
                out.push((at(*mid).to_array(), at(p).to_array(), color));
            }
        }
    }
    out
}
