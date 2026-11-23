//! kittest tests of the Scene Nodes window: objects as nodes, parenting by keyboard links, the
//! node browser adding primitives, removing, and undo through the document.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::scene::ObjectId;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::node_editor::Part;

fn harness() -> (Harness<'static, ModelApp>, ObjectId, ObjectId) {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let cube = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: Some("Box".into()), parent: None }).unwrap().created[0];
    let ball = app.doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Ball".into()), parent: None }).unwrap().created[0];
    let h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    (h, cube, ball)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
}

fn focus(h: &mut Harness<'static, ModelApp>, part: Part) {
    h.state_mut().scene_nodes.editor.focus(part);
    h.run();
    h.run();
}

fn press(h: &mut Harness<'static, ModelApp>, k: Key) {
    h.key_press(k);
    h.run();
    h.run();
}

#[test]
fn objects_are_nodes_and_a_keyboard_link_parents_one_under_another() {
    let (mut h, cube, ball) = harness();
    h.run();
    palette(&mut h, "Scene Nodes");
    assert!(h.state().scene_nodes.open);
    assert!(h.get_by_role_and_label(Role::Canvas, "Node graph, 2 nodes and 0 links").accesskit_node().is_focused(), "opening puts the focus on the canvas");
    h.get_by_role_and_label(Role::Group, "Box, Mesh node");
    h.get_by_role_and_label(Role::Button, "Parent input of Ball, not linked");
    let undo = h.state().doc.undo.len();
    // Box's Children output to Ball's Parent input: Ball goes under Box, as one undo step.
    focus(&mut h, Part::Output(cube.0, 0));
    press(&mut h, Key::Enter);
    focus(&mut h, Part::Input(ball.0, 0));
    press(&mut h, Key::Enter);
    assert_eq!(h.state().doc.scene.get(ball).unwrap().parent, Some(cube));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    h.get_by_role_and_label(Role::Button, "Parent input of Ball, linked from Box");
    // The other way round would put Box inside its own child: refused.
    focus(&mut h, Part::Output(ball.0, 0));
    press(&mut h, Key::Enter);
    focus(&mut h, Part::Input(cube.0, 0));
    press(&mut h, Key::Enter);
    assert_eq!(h.state().doc.scene.get(cube).unwrap().parent, None);
    assert!(h.state().scene_nodes.editor.announcement.starts_with("Not linked"));
    // Delete on the input unparents; undo puts it back.
    focus(&mut h, Part::Input(ball.0, 0));
    press(&mut h, Key::Delete);
    assert_eq!(h.state().doc.scene.get(ball).unwrap().parent, None);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.get(ball).unwrap().parent, Some(cube));
}

#[test]
fn the_browser_adds_a_primitive_and_delete_removes_a_node() {
    let (mut h, _cube, ball) = harness();
    h.run();
    palette(&mut h, "Scene Nodes");
    h.key_press_modifiers(Modifiers::SHIFT, Key::A);
    h.run();
    h.run();
    h.event(egui::Event::Text("torus".into()));
    h.run();
    press(&mut h, Key::Enter);
    let n = h.state().doc.scene.objects().len();
    assert_eq!(n, 3, "a torus was added");
    assert!(h.state().doc.scene.objects().iter().any(|o| o.name.to_lowercase().contains("torus")));
    // Remove the ball from its node with Delete.
    focus(&mut h, Part::Node(ball.0));
    press(&mut h, Key::Delete);
    assert!(h.state().doc.scene.get(ball).is_none());
    // Esc on the canvas closes the window.
    press(&mut h, Key::Escape);
    assert!(!h.state().scene_nodes.open);
}
