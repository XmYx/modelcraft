//! Keyboard and accessibility-tree tests of blend shapes, lattices and clusters (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::scene::{ObjectId, StepOp};
use modelcraft_engine::{AnimCommand, Command};

use crate::app::{ModelApp, SelectMode};
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn set(h: &mut Harness<'static, ModelApp>, field: &str, v: f64) {
    h.get_by_role_and_label(Role::SpinButton, field).set_value_accesskit(v);
    h.run();
}

fn top(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> f32 {
    let scene = h.state().doc.scene.clone();
    let e = h.state_mut().doc.eval.evaluate(&scene, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    e.mesh.positions().iter().map(|p| p.y).fold(f32::MIN, f32::max)
}

#[test]
fn blend_shapes_from_a_copy_with_a_weight_field_and_a_key() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let face = h.state().selection[0];
    palette(&mut h, "Add Cube");
    let copy = h.state().selection[0];
    // Shape the copy: its top vertices go up by one.
    let scene = h.state().doc.scene.clone();
    let rest = h.state_mut().doc.eval.evaluate(&scene, copy).mesh;
    let verts: Vec<u32> = rest.positions().iter().enumerate().filter(|(_, p)| p.y > 0.0).map(|(i, _)| i as u32).collect();
    let high = rest.positions().iter().map(|p| p.y).fold(f32::MIN, f32::max);
    assert!(
        h.state_mut()
            .run(Command::AddStep { id: copy, op: StepOp::MoveVertices { verts, translate: Vec3::Y, rotate: glam::Quat::IDENTITY, scale: Vec3::ONE } })
    );
    // Without the two meshes selected the action says what it needs.
    h.state_mut().select_only(face);
    palette(&mut h, "Add Blend Shape from Mesh");
    assert_eq!(h.state().status, "Select the shaped copy, then the mesh that gets the shape");
    palette(&mut h, "Add In-Between Shape from Mesh");
    assert_eq!(h.state().status, "Select the shaped copy, then the mesh that gets the shape");
    h.state_mut().selection = vec![copy, face];
    palette(&mut h, "Add In-Between Shape from Mesh");
    assert_eq!(h.state().status, "Add a blend shape first; an in-between belongs to one");
    palette(&mut h, "Add Blend Shape from Mesh");
    assert!(h.state().status.contains("Blend shape added"), "{}", h.state().status);
    assert_eq!(h.state().selection, vec![face]);
    // The mesh's Properties show the shape as a named field; the tree shows the deformer.
    h.get_by_role_and_label(Role::TreeItem, "Blend shapes");
    h.get_by_label("Blend Shapes (1)");
    assert!((top(&mut h, face) - high).abs() < 1e-5);
    set(&mut h, "Shape Cube.1", 0.5);
    assert!((top(&mut h, face) - high - 0.5).abs() < 1e-5);
    // Key it through the attribute path and scrub.
    let shapes = h.state().doc.scene.children(face)[0];
    for (frame, value) in [(0.0, 0.0), (20.0, 1.0)] {
        assert!(h.state_mut().run(Command::Anim(AnimCommand::SetKey { id: shapes, path: "target.0.weight".into(), frame: Some(frame), value: Some(value) })));
    }
    h.state_mut().doc.set_frame(20.0);
    h.run();
    assert!((top(&mut h, face) - high - 1.0).abs() < 1e-4);
    assert_eq!(h.get_by_role_and_label(Role::SpinButton, "Shape Cube.1").accesskit_node().numeric_value(), Some(1.0));
    // An in-between from the same copy at half weight, then remove the shape with its button.
    h.state_mut().selection = vec![copy, face];
    palette(&mut h, "Add In-Between Shape from Mesh");
    h.get_by_label("1 in-betweens");
    h.get_by_role_and_label(Role::Button, "Remove Cube.1").click();
    h.run();
    h.get_by_label("Blend Shapes (0)");
    assert!((top(&mut h, face) - high).abs() < 1e-5);
}

#[test]
fn lattice_points_by_number_clusters_from_vertices_and_stack_order() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let mesh = h.state().selection[0];
    let high = top(&mut h, mesh);
    palette(&mut h, "Add Lattice");
    let lattice = h.state().selection[0];
    h.get_by_role_and_label(Role::TreeItem, "Lattice");
    h.get_by_label("2 × 2 × 2 cells, 27 control points");
    assert_eq!(crate::deform_ui::lattice_lines(h.state()).len(), 54, "the cage: 18 edges along each axis");
    // Pick a top corner control point (index 26 = x 2, y 2, z 2) and lift it by typing.
    set(&mut h, "Control point", 26.0);
    assert_eq!(h.get_by_role_and_label(Role::SpinButton, "Control point Y").accesskit_node().numeric_value(), Some(0.5));
    set(&mut h, "Control point Y", 1.5);
    assert!(top(&mut h, mesh) > high + 0.05, "the top bulges");
    let bulged = top(&mut h, mesh);
    // Cell by cell, by Space on the checkbox.
    h.get_by_role_and_label(Role::CheckBox, "Cell by cell").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(h.state().doc.scene.attr(lattice, "linear"), Some(1.0));
    assert!((top(&mut h, mesh) - bulged).abs() < 0.1 && top(&mut h, mesh) > high + 0.05);
    h.get_by_role_and_label(Role::Button, "Reset Lattice").click();
    h.run();
    assert!((top(&mut h, mesh) - high).abs() < 1e-5);
    // A cluster needs vertices.
    h.state_mut().select_only(mesh);
    palette(&mut h, "Add Cluster from Vertices");
    assert_eq!(h.state().status, "Select a mesh, and vertices in vertex mode, to make a cluster");
    let scene = h.state().doc.scene.clone();
    let rest = h.state_mut().doc.eval.evaluate(&scene, mesh).mesh;
    h.state_mut().mode = SelectMode::Vertex;
    h.state_mut().components = rest.positions().iter().enumerate().filter(|(_, p)| p.y > 0.0).map(|(i, _)| i as u32).collect();
    palette(&mut h, "Add Cluster from Vertices");
    let cluster = h.state().selection[0];
    assert_ne!(cluster, mesh);
    h.get_by_label("4 points follow this object. Move, rotate or scale it.");
    let y = h.state().doc.scene.get(cluster).map(|o| o.transform.translation.y).unwrap_or(0.0);
    set(&mut h, "Position Y", (y + 2.0) as f64);
    assert!((top(&mut h, mesh) - high - 2.0).abs() < 1e-4, "the cluster's points follow it");
    // Reorder: the cluster moves before the lattice, and back.
    assert_eq!(h.state().doc.scene.children(mesh), vec![lattice, cluster]);
    palette(&mut h, "Move Deformer Earlier");
    assert_eq!(h.state().doc.scene.children(mesh), vec![cluster, lattice]);
    assert_eq!(h.state().selection, vec![cluster]);
    palette(&mut h, "Move Deformer Later");
    assert_eq!(h.state().doc.scene.children(mesh), vec![lattice, cluster]);
    h.state_mut().select_only(mesh);
    palette(&mut h, "Move Deformer Earlier");
    assert_eq!(h.state().status, "Select a deformer to reorder it");
    assert_eq!(h.get_by_label("Select a deformer to reorder it").accesskit_node().live(), egui::accesskit::Live::Polite);
}
