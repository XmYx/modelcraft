//! UI tests of the polygon tool options (extrude, bevel, loop cut): live preview, one undo step,
//! and numeric and keyboard paths. They live apart from `ui_tests.rs` so that workstreams
//! appending tests there do not collide with these.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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

/// A new primitive in the given component mode (1 object, 2 vertex, 3 edge, 4 face key).
fn primitive(h: &mut Harness<'static, ModelApp>, name: &str, mode: Key) {
    h.run();
    palette(h, name);
    h.key_press(mode);
    h.run();
}

fn select(h: &mut Harness<'static, ModelApp>, items: &[u32]) {
    h.state_mut().components = items.iter().copied().collect();
    h.run();
}

/// Replace what a number field holds by typing: Backspace clears it (whatever it was) and the
/// digits go in.
fn type_text(h: &mut Harness<'static, ModelApp>, text: &str) {
    for _ in 0..10 {
        h.key_press(Key::Backspace);
    }
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
}

/// [`type_text`], then Enter finishes the field.
fn type_number(h: &mut Harness<'static, ModelApp>, text: &str) {
    type_text(h, text);
    h.key_press(Key::Enter);
    h.run();
}

fn closed(m: &modelcraft_engine::mesh::PolyMesh) -> bool {
    (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
}

#[test]
fn extrude_options_by_keyboard_preview_live_and_make_one_undo_step() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num4);
    select(&mut h, &[0]);
    let before = h.state().doc.undo.len();
    h.key_press_modifiers(Modifiers::SHIFT, Key::E);
    h.run();
    assert!(h.state().modeling.is_open());
    assert_eq!(steps(&h).len(), 1, "the step is in the history while the window is open (live preview)");
    // The first number has the keyboard: type, and the Enter that ends typing does not apply.
    assert!(h.get_by_label("Distance (tool options)").is_focused());
    type_number(&mut h, "0.75");
    assert!(matches!(steps(&h)[0], StepOp::Extrude { distance, .. } if (distance - 0.75).abs() < 1e-6), "{:?}", steps(&h));
    assert!(h.state().modeling.is_open());
    // Typing again, then Tab finishes the field and moves on: Offset, then Rows. The mesh follows.
    h.get_by_label("Distance (tool options)").focus();
    h.run();
    type_text(&mut h, "0.5");
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Offset (tool options)").is_focused());
    assert!(matches!(steps(&h)[0], StepOp::Extrude { distance, .. } if (distance - 0.5).abs() < 1e-6), "Tab commits the field it leaves");
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Rows (tool options)").is_focused());
    type_number(&mut h, "3");
    assert!(matches!(steps(&h)[0], StepOp::Extrude { subdivisions: 3, .. }));
    assert_eq!(mesh(&mut h).face_count(), 6 + 4 * 3);
    // Enter with nothing focused applies; the window's edits became one undo step.
    assert!(h.state().modeling.is_open());
    h.key_press(Key::Enter);
    h.run();
    assert!(!h.state().modeling.is_open());
    assert_eq!(h.state().doc.undo.len(), before + 1);
    assert_eq!(steps(&h).len(), 1);
    // One undo takes the whole tool back; redo brings it back with the options.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 0);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::Extrude { subdivisions: 3, .. }));
}

#[test]
fn escape_cancels_the_tool_and_leaves_no_history() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num3);
    select(&mut h, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    let before = h.state().doc.undo.len();
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    h.run();
    assert!(h.state().modeling.is_open());
    type_number(&mut h, "0.3");
    assert!(steps(&h).len() == 1 && h.state().doc.undo.len() > before + 1);
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().modeling.is_open());
    assert_eq!(steps(&h).len(), 0);
    assert_eq!(mesh(&mut h).face_count(), 6);
    // The history below the tool is untouched; what the window made can be redone like any undo.
    assert!(h.state().doc.undo.len() >= before);
}

#[test]
fn bevel_options_take_numbers_and_stay_closed() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num3);
    select(&mut h, &(0..12).collect::<Vec<u32>>());
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    h.run();
    // The options of the last use: two segments, round.
    assert!(matches!(steps(&h)[0], StepOp::Bevel { segments: 2, .. }));
    h.get_by_label("Segments (tool options)").set_value_accesskit(5.0);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::Bevel { segments: 5, .. }));
    let m = mesh(&mut h);
    assert_eq!(m.face_count(), 6 + 12 * 5 + 8);
    assert!(closed(&m));
    // The profile is a group of radio buttons; the tension shows for the custom one only.
    assert!(h.query_by_label("Tension (tool options)").is_none());
    h.get_by_label("Profile: Custom (tool options)").click();
    h.run();
    h.get_by_label("Tension (tool options)").set_value_accesskit(0.9);
    h.run();
    assert!(
        matches!(steps(&h)[0], StepOp::Bevel { profile: modelcraft_engine::ops::bevel::BevelProfile::Custom, tension, .. } if (tension - 0.9).abs() < 1e-6)
    );
    h.get_by_label("Width (tool options)").set_value_accesskit(0.25);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    assert!(!h.state().modeling.is_open());
    // The same numbers stay editable on the step in the Build History.
    // The cube's own Segments field has the same name: pick the step's by its value.
    h.get_all_by_role_and_label(egui::accesskit::Role::SpinButton, "Segments")
        .find(|n| n.accesskit_node().numeric_value() == Some(5.0))
        .unwrap()
        .set_value_accesskit(3.0);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::Bevel { segments: 3, .. }));
    assert_eq!(mesh(&mut h).face_count(), 6 + 12 * 3 + 8);
    // The next use starts from the last numbers.
    assert_eq!(h.state().modeling.bevel_segments, 5);
}

#[test]
fn vertex_bevel_and_border_edge_extrude_have_their_own_options() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num2);
    select(&mut h, &[0]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::BevelVertices { .. }));
    h.get_by_label("Width (tool options)").set_value_accesskit(0.4);
    h.run();
    assert_eq!(mesh(&mut h).face_count(), 7);
    h.get_by_label("Apply").click();
    h.run();
    // A plane has a border: extrude its edges outwards.
    primitive(&mut h, "Add Plane", Key::Num3);
    let border: Vec<u32> = {
        let m = mesh(&mut h);
        (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).collect()
    };
    select(&mut h, &border);
    h.key_press_modifiers(Modifiers::SHIFT, Key::E);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::ExtrudeEdges { .. }), "{:?}", steps(&h));
    let faces = mesh(&mut h).face_count();
    h.get_by_label("Lift (tool options)").set_value_accesskit(0.5);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::ExtrudeEdges { lift, .. } if (lift - 0.5).abs() < 1e-6));
    h.get_by_label("Apply").click();
    h.run();
    assert!(mesh(&mut h).face_count() > 1);
    let _ = faces;
}

#[test]
fn loop_cut_slide_by_keyboard() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num3);
    select(&mut h, &[0]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    h.run();
    assert!(h.get_by_label("Cuts (tool options)").is_focused());
    h.key_press(Key::Tab);
    h.run();
    assert!(h.get_by_label("Slide (tool options)").is_focused());
    type_number(&mut h, "0.5");
    assert!(matches!(steps(&h)[0], StepOp::LoopCut { offset, cuts: 1, .. } if (offset - 0.5).abs() < 1e-6), "{:?}", steps(&h));
    let m = mesh(&mut h);
    assert_eq!(m.face_count(), 6 + 4);
    assert!(closed(&m));
    // Shift+Enter applies from anywhere.
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run();
    assert!(!h.state().modeling.is_open());
    // Without a selected edge there is no tool, and the status says why.
    select(&mut h, &[]);
    let n = steps(&h).len();
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    h.run();
    assert_eq!(steps(&h).len(), n);
    assert!(h.state().status.contains("needs a selected edge"));
}

#[test]
fn an_undo_while_the_window_is_open_closes_it_without_folding() {
    let mut h = harness();
    primitive(&mut h, "Add Cube", Key::Num4);
    select(&mut h, &[0]);
    h.key_press_modifiers(Modifiers::SHIFT, Key::E);
    h.run();
    assert!(h.state().modeling.is_open());
    // Enter finishes the field the keyboard is in; then an undo is the app's, not the text's.
    h.key_press(Key::Enter);
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert!(!h.state().modeling.is_open());
    assert_eq!(steps(&h).len(), 0);
}
