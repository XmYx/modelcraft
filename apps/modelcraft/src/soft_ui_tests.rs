//! UI tests of soft selection and symmetric component editing: the settings by keyboard, the
//! numeric transform with them, and the colours of the weights. They live apart from
//! `ui_tests.rs` so that workstreams appending tests there do not collide with these.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::scene::{ObjectId, StepOp};

use crate::app::ModelApp;
use crate::ui_tests::{harness, palette};

fn steps(h: &Harness<'static, ModelApp>) -> Vec<StepOp> {
    let id = h.state().selection[0];
    h.state().doc.scene.get(id).unwrap().steps().iter().map(|s| s.op.clone()).collect()
}

fn mesh(h: &mut Harness<'static, ModelApp>) -> std::sync::Arc<modelcraft_engine::mesh::PolyMesh> {
    let id: ObjectId = h.state().selection[0];
    let st = h.state_mut();
    st.doc.eval.steps_output(&st.doc.scene, id, usize::MAX).mesh
}

/// A subdivided plane (so there are neighbours to pull) in vertex mode.
fn plane(h: &mut Harness<'static, ModelApp>) {
    h.run();
    palette(h, "Add Plane");
    h.key_press(Key::Num2);
    h.run();
}

/// The vertex of the mesh nearest to a point.
fn vertex_at(m: &modelcraft_engine::mesh::PolyMesh, p: glam::Vec3) -> u32 {
    (0..m.vertex_count() as u32).min_by(|a, b| m.position(*a).distance(p).total_cmp(&m.position(*b).distance(p))).unwrap()
}

#[test]
fn soft_selection_and_its_radius_by_keyboard() {
    let mut h = harness();
    plane(&mut h);
    assert!(!h.state().modeling.soft.enabled);
    h.key_press(Key::O);
    h.run();
    assert!(h.state().modeling.soft.enabled);
    let r0 = h.state().modeling.soft.radius;
    h.key_press_modifiers(Modifiers::ALT, Key::CloseBracket);
    h.run();
    assert!(h.state().modeling.soft.radius > r0);
    h.key_press_modifiers(Modifiers::ALT, Key::OpenBracket);
    h.key_press_modifiers(Modifiers::ALT, Key::OpenBracket);
    h.run();
    assert!(h.state().modeling.soft.radius < r0);
    // Symmetry cycles off, X, Y, Z, off.
    let sym = |h: &Harness<'static, ModelApp>| h.state().modeling.soft.symmetry;
    assert_eq!(sym(&h), None);
    for want in [Some(0), Some(1), Some(2), None] {
        h.key_press_modifiers(Modifiers::SHIFT, Key::Y);
        h.run();
        assert_eq!(sym(&h), want);
    }
}

#[test]
fn the_properties_fields_set_radius_falloff_and_symmetry() {
    let mut h = harness();
    plane(&mut h);
    h.run();
    // The section is a collapsing header: open it from its name.
    h.get_by_label("Soft Selection and Symmetry").click();
    h.run();
    h.get_by_label("Soft selection").click();
    h.run();
    assert!(h.state().modeling.soft.enabled);
    h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Soft radius").set_value_accesskit(1.5);
    h.run();
    assert!((h.state().modeling.soft.radius - 1.5).abs() < 1e-6);
    h.get_by_label("Falloff: Linear").click();
    h.run();
    assert_eq!(h.state().modeling.soft.curve, "linear");
    h.get_by_label("Symmetry: X").click();
    h.run();
    assert_eq!(h.state().modeling.soft.symmetry, Some(0));
}

#[test]
fn a_numeric_move_follows_the_soft_selection_in_one_undo_step() {
    let mut h = harness();
    plane(&mut h);
    let m = mesh(&mut h);
    let v = vertex_at(&m, glam::Vec3::ZERO);
    let neighbour = m.vertex_neighbors(v).next().unwrap();
    h.state_mut().components = [v].into_iter().collect();
    h.state_mut().modeling.soft.enabled = true;
    h.state_mut().modeling.soft.radius = 2.0;
    h.state_mut().modeling.soft.curve = "linear";
    h.run();
    let before = h.state().doc.undo.len();
    h.key_press_modifiers(Modifiers::COMMAND, Key::T);
    h.run();
    h.get_by_label("Move Y").set_value_accesskit(1.0);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(h.state().doc.undo.len(), before + 1);
    let StepOp::MoveVertices { soft, symmetry, .. } = &steps(&h)[0] else { panic!("no move: {:?}", steps(&h)) };
    assert!(soft.is_some() && symmetry.is_none());
    let m2 = mesh(&mut h);
    assert!((m2.position(v).y - 1.0).abs() < 1e-5);
    let d = m.position(neighbour).distance(m.position(v));
    let want = (1.0 - d / 2.0).clamp(0.0, 1.0);
    assert!(m2.position(neighbour).y > 0.05 && (m2.position(neighbour).y - want).abs() < 0.2, "{} vs {want}", m2.position(neighbour).y);
    // Undo restores every neighbour.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 0);
}

#[test]
fn a_numeric_move_with_symmetry_moves_the_mirror_vertex_too() {
    let mut h = harness();
    plane(&mut h);
    let m = mesh(&mut h);
    let a = vertex_at(&m, glam::Vec3::new(1.0, 0.0, 0.5));
    let b = vertex_at(&m, glam::Vec3::new(-1.0, 0.0, 0.5));
    assert_ne!(a, b);
    h.state_mut().components = [a].into_iter().collect();
    h.state_mut().modeling.soft.symmetry = Some(0);
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::T);
    h.run();
    h.get_by_label("Move Y").set_value_accesskit(0.7);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    let m2 = mesh(&mut h);
    assert!((m2.position(a).y - 0.7).abs() < 1e-5);
    assert!((m2.position(b).y - 0.7).abs() < 1e-5, "the mirror vertex moved with it");
    assert!((m2.position(a).x + m2.position(b).x).abs() < 1e-5);
    // The step records the symmetry.
    assert!(matches!(&steps(&h)[0], StepOp::MoveVertices { symmetry: Some(s), .. } if s.axis == 0));
}

#[test]
fn weight_colours_run_from_blue_to_red() {
    let (far, near) = (crate::soft_ui::weight_color(0.0), crate::soft_ui::weight_color(1.0));
    assert!(far.b() > far.r() && near.r() > near.b());
    // Out-of-range weights are clamped, not wrapped.
    assert_eq!(crate::soft_ui::weight_color(7.0), near);
    assert_eq!(crate::soft_ui::weight_color(-3.0), far);
    // The weights drawn are the ones the step will use.
    let mut h = harness();
    plane(&mut h);
    let m = mesh(&mut h);
    let v = vertex_at(&m, glam::Vec3::ZERO);
    h.state_mut().components = [v].into_iter().collect();
    h.state_mut().modeling.soft.enabled = true;
    h.state_mut().modeling.soft.radius = 10.0;
    h.run();
    let w = crate::soft_ui::weights(h.state_mut()).unwrap();
    assert_eq!(w[v as usize], 1.0);
    assert!(w.iter().filter(|x| **x > 0.0).count() > 1);
}
