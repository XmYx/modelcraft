//! UI tests of the cutting tools (slice, knife, connect, polygon pen): each by keyboard through
//! its by-numbers window or its command, and the pointer tools by a drag or clicks. They live
//! apart from `ui_tests.rs` so that workstreams appending tests there do not collide with these.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::ops::cut::PenPoint;
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

fn closed(m: &modelcraft_engine::mesh::PolyMesh) -> bool {
    (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
}

fn cube(h: &mut Harness<'static, ModelApp>) {
    h.run();
    palette(h, "Add Cube");
}

fn undo(h: &mut Harness<'static, ModelApp>) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
}

#[test]
fn slice_by_plane_by_keyboard_and_undo() {
    let mut h = harness();
    cube(&mut h);
    h.key_press_modifiers(Modifiers::SHIFT, Key::C);
    h.run();
    assert!(h.state().modeling.cut.is_open());
    // The window opens on the first number; the plane starts through the middle, facing up.
    assert!(h.get_by_label("Point X").is_focused());
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run();
    assert!(!h.state().modeling.cut.is_open());
    assert!(matches!(steps(&h)[0], StepOp::SlicePlane { .. }));
    let m = mesh(&mut h);
    assert_eq!(m.face_count(), 10);
    assert!(closed(&m));
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
    assert_eq!(mesh(&mut h).face_count(), 6);
}

#[test]
fn slice_numbers_move_the_plane_and_a_miss_adds_nothing() {
    let mut h = harness();
    cube(&mut h);
    h.key_press_modifiers(Modifiers::SHIFT, Key::C);
    h.run();
    h.get_by_label("Point Y").set_value_accesskit(0.4);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    let StepOp::SlicePlane { point, .. } = &steps(&h)[0] else { panic!("no slice") };
    assert!((point.y - 0.4).abs() < 1e-6);
    // A plane that crosses nothing: the status says so, no step is added, the window stays.
    h.key_press_modifiers(Modifiers::SHIFT, Key::C);
    h.run();
    h.get_by_label("Point Y").set_value_accesskit(5.0);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(steps(&h).len(), 1);
    assert!(h.state().status.contains("crosses no face"), "{}", h.state().status);
    assert!(h.state().modeling.cut.is_open());
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().modeling.cut.is_open());
}

#[test]
fn knife_by_numbers_cuts_across_faces_by_keyboard() {
    let mut h = harness();
    cube(&mut h);
    h.key_press_modifiers(Modifiers::ALT, Key::C);
    h.run();
    assert!(h.state().modeling.cut.is_open());
    assert!(h.get_by_label("From X").is_focused());
    // Across the whole front of the cube, looking at it: the faces turned towards us are cut.
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run();
    let StepOp::CutLine { front_only, .. } = steps(&h)[0] else { panic!("no cut: {:?}", steps(&h)) };
    assert!(front_only);
    let m = mesh(&mut h);
    assert!(closed(&m));
    assert_eq!(m.face_count(), 6 + 3, "the front and the two sides it meets, seen from the front");
    // One undo takes the cut back.
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
    // Stopping the line half-way cuts only the face that is reached; through-cuts include the back.
    h.key_press_modifiers(Modifiers::ALT, Key::C);
    h.run();
    h.get_by_label("To X").set_value_accesskit(0.0);
    h.run();
    h.get_by_label("Visible faces only").click();
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run();
    assert_eq!(mesh(&mut h).face_count(), 7);
    assert!(h.state().modeling.cut.through, "the choice is kept for the pointer knife");
}

#[test]
fn connect_vertices_by_keyboard() {
    let mut h = harness();
    cube(&mut h);
    h.key_press(Key::Num2); // vertex mode
    h.run();
    // Two opposite corners of one face.
    let vs: Vec<u32> = mesh(&mut h).face_vertices(0).collect();
    h.state_mut().components = [vs[0], vs[2]].into_iter().collect();
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::J);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::Connect { .. }), "{:?}", steps(&h));
    assert_eq!(mesh(&mut h).face_count(), 7);
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
    // Corners that are neighbours, or a single vertex, have nothing to join.
    h.state_mut().components = [vs[0], vs[1]].into_iter().collect();
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::J);
    h.run();
    assert_eq!(steps(&h).len(), 0);
    assert!(h.state().status.contains("not neighbours") || h.state().status.contains("two"), "{}", h.state().status);
}

#[test]
fn polygon_by_numbers_by_keyboard() {
    let mut h = harness();
    cube(&mut h);
    h.key_press_modifiers(Modifiers::SHIFT, Key::P);
    h.run();
    assert!(h.get_by_label("Points").is_focused());
    h.event(egui::Event::Text("3 3 0; 4 3 0; 4 4 0".into()));
    h.run();
    // Tab leaves the text for the Apply button; Enter presses it.
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Apply").is_focused());
    h.key_press(Key::Enter);
    h.run();
    let StepOp::AddPolygon { points, .. } = &steps(&h)[0] else { panic!("no polygon: {:?}", steps(&h)) };
    assert_eq!(points.len(), 3);
    assert!(matches!(points[0], PenPoint::Position { .. }));
    assert_eq!(mesh(&mut h).face_count(), 7);
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
    // A line that is not a point is reported on the spot.
    h.key_press_modifiers(Modifiers::SHIFT, Key::P);
    h.run();
    h.event(egui::Event::Text("1 2".into()));
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    assert_eq!(steps(&h).len(), 0);
    assert!(h.state().status.contains("Line 1"), "{}", h.state().status);
}

#[test]
fn point_text_is_parsed_strictly() {
    use crate::cut_ui::parse_points;
    assert_eq!(parse_points("v 3\n0 1 2").unwrap().len(), 2);
    assert_eq!(parse_points("V 3; 1,2,3").unwrap().len(), 2);
    assert!(parse_points("v x").is_err());
    assert!(parse_points("1 2").is_err());
    assert!(parse_points("1 2 nan").is_err());
    assert!(parse_points(&"0 0 0\n".repeat(1000)).is_err());
    assert!(parse_points("").unwrap().is_empty());
}

/// Drag the primary button across the viewport.
fn drag(h: &mut Harness<'static, ModelApp>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for k in 1..=10 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 10.0)));
        h.step();
    }
    h.event(button(to, false));
    h.step();
    h.step();
}

fn click(h: &mut Harness<'static, ModelApp>, at: egui::Pos2) {
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(egui::Event::PointerMoved(at));
    h.step();
    h.event(button(true));
    h.step();
    h.event(button(false));
    h.step();
    h.step();
}

#[test]
fn the_knife_tool_cuts_along_a_dragged_line_and_undoes() {
    let mut h = harness();
    cube(&mut h);
    h.key_press(Key::C);
    h.run();
    assert!(h.state().modeling.cut.tool.is_on());
    let rect = h.get_by_label_contains("Viewport, Perspective").rect();
    // Wide enough to reach past both sides of the cube, so every face it crosses is cut.
    let half = rect.width() * 0.4;
    let (a, b) = (rect.center() - egui::vec2(half, 0.0), rect.center() + egui::vec2(half, 0.0));
    let before = h.state().doc.undo.len();
    drag(&mut h, a, b);
    assert!(matches!(steps(&h).first(), Some(StepOp::CutLine { .. })), "{:?}", steps(&h));
    assert!(mesh(&mut h).face_count() > 6);
    assert!(closed(&mesh(&mut h)));
    assert_eq!(h.state().doc.undo.len(), before + 1);
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
    // Esc stops the tool.
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().modeling.cut.tool.is_on());
}

#[test]
fn the_pen_tool_clicks_points_and_enter_makes_the_polygon() {
    let mut h = harness();
    cube(&mut h);
    h.key_press(Key::P);
    h.run();
    assert!(h.state().modeling.cut.tool.is_on());
    let rect = h.get_by_label_contains("Viewport, Perspective").rect();
    let c = rect.center();
    // Three points well away from the cube: each lands on the plane facing the camera.
    for d in [egui::vec2(220.0, -120.0), egui::vec2(300.0, -120.0), egui::vec2(260.0, -40.0)] {
        click(&mut h, c + d);
    }
    // Focus is on the viewport after a click; Enter closes the polygon.
    h.key_press(Key::Enter);
    h.run();
    assert!(matches!(steps(&h).first(), Some(StepOp::AddPolygon { points, .. }) if points.len() == 3), "{:?}", steps(&h));
    assert_eq!(mesh(&mut h).face_count(), 7);
    undo(&mut h);
    assert_eq!(steps(&h).len(), 0);
}

#[test]
fn no_two_commands_share_a_default_key() {
    // A shared key silently disables one of the commands (a clash with Key Transform and with Pack
    // UV Islands was caught this way).
    let mut seen: std::collections::HashMap<&'static str, String> = std::collections::HashMap::new();
    for a in crate::actions::all() {
        if let Some(key) = a.default_binding()
            && let Some(other) = seen.insert(key, a.id())
        {
            panic!("{key} is bound to both {other} and {}", a.id());
        }
    }
}
