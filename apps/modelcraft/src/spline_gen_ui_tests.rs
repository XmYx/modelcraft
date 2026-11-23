//! kittest tests of the spline generators: the palette and Spline menu, the Properties pages by
//! the accessibility tree and the keyboard, and that a point moved in a child changes the surface.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::curves::{KnotMove, KnotRef, SplineEdit};
use modelcraft_engine::ops::spline_gen::Axis;
use modelcraft_engine::scene::spline_gen::SplineGen;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, SplineCommand};

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

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

fn faces(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> usize {
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, id).mesh.face_count()
}

fn generator(h: &Harness<'static, ModelApp>, id: ObjectId) -> SplineGen {
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::SplineGenerator { generator } => generator.clone(),
        other => panic!("not a spline generator: {other:?}"),
    }
}

#[test]
fn every_generator_is_in_the_palette_and_shows_a_surface_at_once() {
    for (text, name, min_faces) in [("Add Extrude", "Extrude", 20), ("Add Revolve", "Revolve", 100), ("Add Loft", "Loft", 100), ("Add Sweep", "Sweep", 500)] {
        let mut h = harness();
        h.run();
        palette(&mut h, text);
        h.run();
        let id = h.state().selection[0];
        assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::SplineGenerator { .. }), "{text}");
        h.get_by_role_and_label(Role::TreeItem, name);
        assert!(faces(&mut h, id) >= min_faces, "{text}: {} faces", faces(&mut h, id));
        // The splines are inside it, as ordinary children.
        let kids = h.state().doc.scene.spline_children(id);
        assert!(!kids.is_empty(), "{text}");
        h.get_by_label_contains("faces");
    }
}

#[test]
fn a_selected_spline_moves_inside_a_new_extrude_and_follows_when_a_point_moves() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let rect = h.state().selection[0];
    palette(&mut h, "Add Extrude");
    let id = h.state().selection[0];
    assert_eq!(h.state().doc.scene.get(rect).unwrap().parent, Some(id), "the rectangle is inside the extrude");
    assert_eq!(faces(&mut h, id), 4 + 2 * 2, "four sides and two caps of two triangles");
    let top = |h: &mut Harness<'static, ModelApp>| {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, id).mesh.bounds().1.x
    };
    assert!((top(&mut h) - 1.0).abs() < 1e-4);
    // Move a corner of the rectangle outwards (as the pen or a point drag would): the surface follows.
    h.state_mut().run(Command::Spline(SplineCommand::Edit {
        id: rect,
        edit: SplineEdit::Translate { knots: vec![KnotRef { segment: 0, knot: 1 }], by: Vec3::new(2.0, 0.0, 0.0) },
    }));
    assert!((top(&mut h) - 3.0).abs() < 1e-4, "{}", top(&mut h));
    // And a second spline inside makes a hole.
    h.state_mut().selection = vec![rect];
    palette(&mut h, "Add Circle Spline");
    let circle = h.state().selection[0];
    h.state_mut().run(Command::Reparent { id: circle, parent: Some(id) });
    h.state_mut().run(Command::SetTransform { id: circle, transform: modelcraft_engine::scene::Transform { scale: Vec3::splat(0.1), ..Default::default() } });
    assert!(faces(&mut h, id) > 20, "caps with a hole");
}

#[test]
fn extrude_distance_edits_through_accessibility_and_the_axis_combo_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Extrude");
    let id = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Axis");
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Distance").set_value_accesskit(3.0);
    h.run();
    h.run();
    assert!(matches!(generator(&h, id), SplineGen::Extrude(o) if (o.distance - 3.0).abs() < 1e-4));
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert!(matches!(generator(&h, id), SplineGen::Extrude(o) if (o.distance - 1.0).abs() < 1e-4));
    // Axis: Space, Arrow Down (the first entry), Tab to the next, Enter.
    h.get_by_role_and_label(Role::ComboBox, "Axis").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Automatic"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("X axis"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(generator(&h, id), SplineGen::Extrude(o) if o.axis == Axis::X));
    assert!(h.get_by_role_and_label(Role::ComboBox, "Axis").is_focused());
    // The caps are check boxes: focus one and press Space.
    h.get_by_role_and_label(Role::CheckBox, "Cap at the end").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(matches!(generator(&h, id), SplineGen::Extrude(o) if !o.cap_end && o.cap_start), "{:?}", generator(&h, id));
}

#[test]
fn the_other_generators_have_their_own_fields() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Revolve");
    let id = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Angle").set_value_accesskit(180.0);
    h.run();
    h.run();
    assert!(matches!(generator(&h, id), SplineGen::Revolve(o) if (o.angle - 180.0).abs() < 1e-4));
    let capped = faces(&mut h, id);
    h.get_by_role_and_label(Role::CheckBox, "Cap the ends").click();
    h.run();
    h.run();
    assert!(faces(&mut h, id) < capped, "no caps, fewer faces");
    palette(&mut h, "Add Sweep");
    let sweep = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Twist").set_value_accesskit(90.0);
    h.run();
    h.run();
    assert!(matches!(generator(&h, sweep), SplineGen::Sweep(o) if (o.twist - 90.0).abs() < 1e-4));
    palette(&mut h, "Add Loft");
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Steps between curves");
    h.get_by_role_and_label(Role::CheckBox, "Smooth through the curves");
}

#[test]
fn a_generator_without_enough_splines_says_so_and_shows_nothing() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    palette(&mut h, "Add Sweep");
    let id = h.state().selection[0];
    h.run();
    assert_eq!(faces(&mut h, id), 0);
    assert!(h.query_all_by_label_contains("needs two splines inside it").count() >= 1);
    // Nothing to draw: the viewport still has a node for it, and no error is raised.
    h.run();
}

#[test]
fn a_generator_exports_its_mesh_and_can_be_the_input_of_another_generator() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Extrude");
    let id = h.state().selection[0];
    let items = h.state_mut().doc.export_items();
    let item = items.iter().find(|i| i.name == "Extrude").expect("the generator is exported");
    assert_eq!(item.mesh.as_ref().map(|m| m.face_count()), Some(faces(&mut h, id)));
    assert!(items.iter().all(|i| i.name != "Star"), "the curve is not");
    // An array around it joins copies of the extruded mesh.
    let one = faces(&mut h, id);
    palette(&mut h, "Add Linear Array");
    let arr = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(arr).unwrap().kind, ObjectKind::Array { .. }));
    assert_eq!(faces(&mut h, arr), one * 4);
    // Moving a point of the curve re-evaluates through both.
    let star = h.state().doc.scene.spline_children(id)[0];
    h.state_mut().run(Command::Spline(SplineCommand::Edit {
        id: star,
        edit: SplineEdit::MoveKnots { moves: vec![KnotMove { at: KnotRef { segment: 0, knot: 0 }, pos: Vec3::new(0.0, 3.0, 0.0) }] },
    }));
    let s = h.state_mut();
    assert!(s.doc.eval.evaluate(&s.doc.scene, arr).mesh.bounds().1.y > 2.9);
}
