//! kittest tests of the connect, thicken, reduce and boolean generators: Create menu entries from
//! the command palette (the keyboard), their controls by name, and live updates.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::ops::boolean::BoolOp;
use modelcraft_engine::scene::generator::Generator;
use modelcraft_engine::scene::{ObjectId, ObjectKind};

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

fn generator(h: &Harness<'static, ModelApp>) -> (ObjectId, Generator) {
    let id = h.state().selection[0];
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Generator { generator } => (id, generator.clone()),
        k => panic!("not a generator: {k:?}"),
    }
}

fn faces(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> usize {
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, id).mesh.face_count()
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn every_generator_is_in_the_palette_and_makes_something_to_look_at() {
    for (command, kind) in [("Add Connect", "connect"), ("Add Thicken", "thicken"), ("Add Reduce", "reduce"), ("Add Boolean", "boolean")] {
        let mut h = harness();
        h.run();
        palette(&mut h, command);
        let (id, g) = generator(&h);
        assert_eq!(g.kind(), kind, "{command}");
        assert!(faces(&mut h, id) > 0, "{kind} shows nothing");
        h.run();
        h.get_by_role_and_label(Role::TreeItem, g.type_name());
    }
}

#[test]
fn thickness_edits_through_accessibility_with_one_undo_step_and_the_direction_combo_works_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Plane");
    palette(&mut h, "Add Thicken");
    let (id, _) = generator(&h);
    assert_eq!(h.state().doc.scene.children(id).len(), 1, "the plane is inside the generator");
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Thickness").set_value_accesskit(0.5);
    h.run();
    h.run();
    assert!(matches!(generator(&h).1, Generator::Thicken { amount, .. } if (amount - 0.5).abs() < 1e-6));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    // Direction: Space opens the combo, Arrow Down moves to the first entry, Tab walks, Enter picks.
    h.get_by_role_and_label(Role::ComboBox, "Grow").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Both sides"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Outward"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(generator(&h).1, Generator::Thicken { direction: modelcraft_engine::ops::thicken::Direction::Outward, .. }));
    assert!(h.get_by_role_and_label(Role::ComboBox, "Grow").is_focused());
}

#[test]
fn a_boolean_around_two_selected_meshes_follows_them_and_its_operation_is_a_combo() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let cube = h.state().selection[0];
    palette(&mut h, "Add Sphere");
    let ball = h.state().selection[0];
    h.state_mut().selection = vec![cube, ball];
    palette(&mut h, "Add Boolean");
    let (id, g) = generator(&h);
    assert!(matches!(g, Generator::Boolean { op: BoolOp::Subtract }));
    assert!(h.state().doc.scene.is_cage_input(cube) && h.state().doc.scene.is_cage_input(ball), "both are shown through it");
    let carved = faces(&mut h, id);
    // Change to union through the combo, from the keyboard.
    h.get_by_role_and_label(Role::ComboBox, "Operation").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Union"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(generator(&h).1, Generator::Boolean { op: BoolOp::Union }));
    let joined = faces(&mut h, id);
    assert_ne!(joined, carved, "union and subtraction make different meshes");
    // Moving the sphere out changes the result: it is live.
    let before = joined;
    h.state_mut().run(modelcraft_engine::Command::SetTransform {
        id: ball,
        transform: modelcraft_engine::scene::Transform { translation: glam::Vec3::new(10.0, 0.0, 0.0), ..Default::default() },
    });
    assert_ne!(faces(&mut h, id), before);
    h.run();
    // The face count is shown in Properties (problems of a result are listed below it).
    h.get_by_label_contains("faces");
}
