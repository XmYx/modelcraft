//! The deformer stack in the app: blend shapes, lattices and clusters. Rig menu entries, the
//! Properties sections (shape weights, lattice control points by number) and the lattice cage
//! drawn in the viewport. Every action is a `rig` command.

use egui::RichText;
use modelcraft_engine::scene::point_deform::PointDeformer;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, RigCommand};

use crate::app::{ModelApp, SelectMode};
use crate::rig_ui::RigAction;
use crate::theme;

fn is_mesh(app: &ModelApp, id: ObjectId) -> bool {
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }))
}

/// The blend-shape deformer of `id`: itself, or the first one among a mesh's children.
fn shapes_of(app: &ModelApp, id: ObjectId) -> Option<(ObjectId, ObjectId)> {
    let scene = &app.doc.scene;
    let is_shapes = |o: ObjectId| matches!(scene.get(o).map(|o| &o.kind), Some(ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { .. } }));
    if is_shapes(id) {
        return scene.deformed_mesh(id).map(|m| (m, id));
    }
    scene.children(id).into_iter().find(|c| is_shapes(*c)).map(|c| (id, c))
}

pub fn act(app: &mut ModelApp, a: RigAction) {
    match a {
        RigAction::AddBlendShape | RigAction::AddInBetween => {
            // The sculpted copy first, the mesh that gets the shape last.
            let mesh = app.active().filter(|id| is_mesh(app, *id));
            let from = app.selection.iter().copied().find(|id| Some(*id) != mesh && is_mesh(app, *id));
            let (Some(mesh), Some(from)) = (mesh, from) else {
                app.status = tl!("Select the shaped copy, then the mesh that gets the shape");
                return;
            };
            let target = if a == RigAction::AddInBetween {
                let count = shapes_of(app, mesh).and_then(|(_, s)| match app.doc.scene.get(s).map(|o| &o.kind) {
                    Some(ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } }) => Some(targets.len()),
                    _ => None,
                });
                match count {
                    Some(n) if n > 0 => Some(n - 1),
                    _ => {
                        app.status = tl!("Add a blend shape first; an in-between belongs to one");
                        return;
                    }
                }
            } else {
                None
            };
            let at = if target.is_some() { 0.5 } else { 1.0 };
            if app.run(Command::Rig(RigCommand::AddShape { mesh, from, name: None, target, at })) {
                app.selection = vec![mesh];
                app.status = if target.is_some() {
                    tl!("In-between added at half weight of the last shape")
                } else {
                    tl!("Blend shape added; set its weight in Properties")
                };
            }
        }
        RigAction::AddLattice => {
            let Some(mesh) = app.active().filter(|id| is_mesh(app, *id)) else {
                app.status = tl!("Select a mesh to put a lattice around");
                return;
            };
            if app.run(Command::Rig(RigCommand::AddLattice { mesh, divisions: [2, 2, 2], linear: false })) {
                app.status = tl!("Lattice added; move its control points in Properties");
            }
        }
        RigAction::AddCluster => {
            let Some(mesh) = app.active().filter(|id| is_mesh(app, *id)) else {
                app.status = tl!("Select a mesh, and vertices in vertex mode, to make a cluster");
                return;
            };
            if app.mode != SelectMode::Vertex || app.components.is_empty() {
                app.status = tl!("Select a mesh, and vertices in vertex mode, to make a cluster");
                return;
            }
            let points: Vec<u32> = app.components.iter().copied().collect();
            if app.run(Command::Rig(RigCommand::AddCluster { mesh, points, weights: vec![], radius: None, center: None, name: None })) {
                app.mode = SelectMode::Object;
                app.status = tl!("Cluster added; move it and its points follow");
            }
        }
        RigAction::StackEarlier | RigAction::StackLater => {
            let Some(id) = app.active().filter(|id| app.doc.scene.in_stack(*id)) else {
                app.status = tl!("Select a deformer to reorder it");
                return;
            };
            app.run(Command::Rig(RigCommand::MoveInStack { id, delta: if a == RigAction::StackEarlier { -1 } else { 1 } }));
            app.selection = vec![id];
        }
        _ => {}
    }
}

/// A numeric field that previews through the attribute path while dragged and runs one command
/// at the end.
fn commit(app: &mut ModelApp, resp: &egui::Response, target: ObjectId, path: &str, value: f32, cmd: impl FnOnce() -> RigCommand) {
    if resp.changed() && resp.dragged() {
        if app.prop_snapshot.is_none() {
            app.prop_snapshot = Some(Box::new(app.doc.scene.clone()));
        }
        let _ = app.doc.scene.set_attr(target, path, value as f64);
        return;
    }
    if resp.drag_stopped() || resp.changed() {
        if let Some(s) = app.prop_snapshot.take() {
            app.doc.scene = *s;
        }
        app.run(Command::Rig(cmd()));
    }
}

/// The Properties sections for the active object: its blend shapes (on the mesh or on the
/// deformer), a lattice's control points, a cluster's summary.
pub fn properties(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    crate::rig_tools::properties(app, ui);
    if let Some((mesh, shapes)) = shapes_of(app, id) {
        let targets = match app.doc.scene.get(shapes).map(|o| o.kind.clone()) {
            Some(ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } }) => targets,
            _ => Vec::new(),
        };
        egui::CollapsingHeader::new(crate::tlf!("Blend Shapes ({count})", count = targets.len())).id_salt("blend-shapes").default_open(true).show(ui, |ui| {
            for (i, t) in targets.iter().enumerate() {
                ui.push_id(("shape", i), |ui| {
                    ui.horizontal(|ui| {
                        let mut w = t.weight;
                        let r = crate::panels::drag(ui, &mut w, 0.01, -1.0..=2.0, crate::tlf!("Shape {name}", name = t.name));
                        commit(app, &r, shapes, &format!("target.{i}.weight"), w, || RigCommand::SetShape { mesh, target: i, weight: Some(w), name: None });
                        if t.frames.len() > 1 {
                            ui.label(RichText::new(crate::tlf!("{count} in-betweens", count = t.frames.len() - 1)).color(theme::TEXT_DIM).small());
                        }
                        if ui.button(crate::tlf!("Remove {name}", name = t.name)).clicked() {
                            app.run(Command::Rig(RigCommand::RemoveShape { mesh, target: i, frame: None }));
                        }
                    });
                });
            }
            if targets.is_empty() {
                ui.label(
                    RichText::new(tl!("No shapes left. Select a shaped copy, then the mesh, and add a blend shape from the Rig menu."))
                        .color(theme::TEXT_DIM)
                        .small(),
                );
            }
        });
    }
    let Some(ObjectKind::PointDeformer { deformer }) = app.doc.scene.get(id).map(|o| o.kind.clone()) else { return };
    match deformer {
        PointDeformer::Lattice { divisions, linear, points } => {
            egui::CollapsingHeader::new(tl!("Lattice")).id_salt("lattice").default_open(true).show(ui, |ui| {
                ui.label(
                    RichText::new(crate::tlf!(
                        "{x} × {y} × {z} cells, {count} control points",
                        x = divisions[0],
                        y = divisions[1],
                        z = divisions[2],
                        count = points.len()
                    ))
                    .color(theme::TEXT_DIM)
                    .small(),
                );
                let mut cells = linear;
                if ui
                    .checkbox(&mut cells, tl!("Cell by cell"))
                    .on_hover_text(tl!("Each control point acts on the cells next to it and the surface passes through it"))
                    .changed()
                {
                    app.run(Command::Rig(RigCommand::EditLattice { id, points: vec![], translate: None, to: None, reset: false, linear: Some(cells) }));
                }
                // One control point at a time, by index and by number.
                let key = egui::Id::unique(("lattice-point", id.0));
                let mut index: u32 = ui.data_mut(|d| d.get_temp(key)).unwrap_or(0).min(points.len().saturating_sub(1) as u32);
                ui.horizontal(|ui| {
                    ui.label(tl!("Control point"));
                    let r = ui.add(egui::DragValue::new(&mut index).range(0..=points.len().saturating_sub(1) as u32));
                    ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("Control point")));
                });
                ui.data_mut(|d| d.insert_temp(key, index));
                let now = points.get(index as usize).copied().unwrap_or_default();
                ui.horizontal(|ui| {
                    for (a, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        let mut v = now.to_array()[a];
                        let r = crate::panels::drag(ui, &mut v, 0.01, -100.0..=100.0, crate::tlf!("Control point {axis}", axis = axis));
                        let mut to = now.to_array();
                        to[a] = v;
                        let path = format!("point.{index}.{}", axis.to_lowercase());
                        commit(app, &r, id, &path, v, || RigCommand::EditLattice {
                            id,
                            points: vec![index],
                            translate: None,
                            to: Some(to),
                            reset: false,
                            linear: None,
                        });
                    }
                });
                if ui.button(tl!("Reset Lattice")).clicked() {
                    app.run(Command::Rig(RigCommand::EditLattice { id, points: vec![], translate: None, to: None, reset: true, linear: None }));
                }
                ui.label(
                    RichText::new(tl!("The lattice is one unit wide in its own space; its position and scale place it around the mesh."))
                        .color(theme::TEXT_DIM)
                        .small(),
                );
            });
        }
        PointDeformer::Cluster { points, .. } => {
            egui::CollapsingHeader::new(tl!("Cluster")).id_salt("cluster").default_open(true).show(ui, |ui| {
                ui.label(crate::tlf!("{count} points follow this object. Move, rotate or scale it.", count = points.len()));
            });
        }
        PointDeformer::BlendShapes { .. } => {}
    }
}

/// The cages of visible lattices as world-space line segments; the selected one is brighter.
pub fn lattice_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let scene = &app.doc.scene;
    let mut out = Vec::new();
    for o in scene.objects().iter().filter(|o| scene.visible(o.id)) {
        let ObjectKind::PointDeformer { deformer: PointDeformer::Lattice { divisions, points, .. } } = &o.kind else { continue };
        let [nx, ny, nz] = modelcraft_engine::animation::deform::lattice_counts(*divisions);
        if points.len() != nx * ny * nz || out.len() > 60_000 {
            continue;
        }
        let world = scene.world_matrix(o.id);
        let color = if app.selection.contains(&o.id) { [1.0, 0.65, 0.3, 1.0] } else { [0.78, 0.55, 0.9, 0.8] };
        let at = |i: usize, j: usize, k: usize| points.get(i + nx * (j + ny * k)).map(|p| world.transform_point3(*p).to_array());
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let Some(here) = at(i, j, k) else { continue };
                    for next in [(i + 1 < nx).then(|| at(i + 1, j, k)), (j + 1 < ny).then(|| at(i, j + 1, k)), (k + 1 < nz).then(|| at(i, j, k + 1))]
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        out.push((here, next, color));
                    }
                }
            }
        }
    }
    out
}
