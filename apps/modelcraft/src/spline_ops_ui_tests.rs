//! kittest tests of the operations on a spline: reverse, chamfer, fillet, convert to polygon and
//! to mesh, from the palette and from the Properties buttons by keyboard.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::curves::{CurveKind, KnotRef, Spline};
use modelcraft_engine::scene::ObjectId;

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn spline(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> Spline {
    let s = h.state_mut();
    (*s.doc.eval.spline(&s.doc.scene, id).unwrap().spline).clone()
}

#[test]
fn reverse_and_convert_to_polygon_work_from_the_palette_and_undo() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Arc Spline");
    let id = h.state().selection[0];
    let first = spline(&mut h, id).segments[0].knots[0].pos;
    let before = h.state().doc.undo.len();
    palette(&mut h, "Reverse Spline");
    assert_eq!(h.state().doc.undo.len(), before + 1);
    let s = spline(&mut h, id);
    assert!(s.segments[0].knots.last().unwrap().pos.distance(first) < 1e-5, "the curve runs the other way");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(spline(&mut h, id).segments[0].knots[0].pos.distance(first) < 1e-5);
    palette(&mut h, "Convert Spline to Polygon");
    let s = spline(&mut h, id);
    assert_eq!(s.segments[0].kind, CurveKind::Linear);
    assert!(s.knot_count() > 4);
    // Nothing selected: a message, not a change.
    h.state_mut().selection.clear();
    let n = h.state().doc.undo.len();
    palette(&mut h, "Reverse Spline");
    assert_eq!(h.state().doc.undo.len(), n);
    assert!(h.state().status.contains("Select a spline"), "{}", h.state().status);
}

#[test]
fn chamfer_and_fillet_cut_the_selected_corners_with_the_corner_size_field() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    palette(&mut h, "Make Spline Editable");
    h.run();
    // No point chosen: asked to choose.
    palette(&mut h, "Chamfer Points");
    assert!(h.state().status.contains("Select the points"), "{}", h.state().status);
    // Choose two corners by keyboard from the list.
    h.get_by_label("Point 1").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    crate::spline_pen::select_knot(h.state_mut(), KnotRef { segment: 0, knot: 2 }, true);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Corner size").set_value_accesskit(0.25);
    h.run();
    assert!((h.state().spline.corner - 0.25).abs() < 1e-5);
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::Button, "Chamfer Points").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().doc.undo.len(), before + 1);
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 6, "two corners became two points each");
    assert!(h.state().spline.selected.is_empty());
    // Fillet the first corner of the result: it becomes a Bezier curve with an arc.
    crate::spline_pen::select_knot(h.state_mut(), KnotRef { segment: 0, knot: 5 }, false);
    h.run();
    palette(&mut h, "Fillet Points");
    let s = spline(&mut h, id);
    assert_eq!((s.segments[0].kind, s.segments[0].knots.len()), (CurveKind::Bezier, 7));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(spline(&mut h, id).segments[0].kind, CurveKind::Linear);
}

#[test]
fn convert_to_mesh_makes_a_flat_mesh_object_and_open_curves_say_no() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Star Spline");
    palette(&mut h, "Convert Spline to Mesh");
    let m = h.state().selection[0];
    let e = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, m).mesh
    };
    assert_eq!(e.face_count(), 8, "a five-pointed star fills with eight triangles");
    h.run();
    h.get_by_role_and_label(Role::TreeItem, "Star Mesh");
    palette(&mut h, "Add Arc Spline");
    palette(&mut h, "Convert Spline to Mesh");
    assert!(h.state().status.contains("failed"), "{}", h.state().status);
}
