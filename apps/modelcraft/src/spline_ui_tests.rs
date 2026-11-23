//! kittest tests of the spline objects: the palette and the Spline menu, the Properties fields
//! (by the accessibility tree and by the keyboard), drawing and picking the curve.

use egui::accesskit::Role;
use egui::{Key, Modifiers, pos2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::curves::{SplinePrimitive, Tessellation};
use modelcraft_engine::scene::ObjectKind;
use modelcraft_engine::scene::splines::SplineSource;

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

fn primitive(h: &Harness<'static, ModelApp>, id: modelcraft_engine::scene::ObjectId) -> SplinePrimitive {
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Spline { source: SplineSource::Primitive { primitive }, .. } => *primitive,
        other => panic!("not a primitive spline: {other:?}"),
    }
}

#[test]
fn the_palette_adds_every_kind_of_spline_and_the_tree_shows_it() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Spline { .. }));
    h.run();
    h.get_by_role_and_label(Role::TreeItem, "Circle");
    h.get_by_role_and_label(Role::SpinButton, "Radius X");
    h.get_by_role_and_label(Role::ComboBox, "Plane");
    for (text, name) in [
        ("Add Rectangle Spline", "Rectangle"),
        ("Add Star Spline", "Star"),
        ("Add N-Side Spline", "N-Side"),
        ("Add Arc Spline", "Arc"),
        ("Add Helix Spline", "Helix"),
        ("Add Empty Spline", "Spline"),
    ] {
        palette(&mut h, text);
        h.run();
        h.get_by_role_and_label(Role::TreeItem, name);
    }
    assert_eq!(h.state().doc.scene.len(), 7);
    // The viewport publishes a node for each.
    h.run();
}

#[test]
fn a_radius_edits_through_accessibility_as_one_undo_step() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let id = h.state().selection[0];
    h.run();
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Radius X").set_value_accesskit(4.0);
    h.run();
    h.run();
    assert!(matches!(primitive(&h, id), SplinePrimitive::Circle { radius_x, .. } if (radius_x - 4.0).abs() < 1e-4));
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert!(matches!(primitive(&h, id), SplinePrimitive::Circle { radius_x, .. } if (radius_x - 1.0).abs() < 1e-4));
}

#[test]
fn the_plane_combo_works_by_keyboard_and_returns_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Star Spline");
    let id = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Plane").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("XY plane"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("YZ plane"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(primitive(&h, id), SplinePrimitive::Star { plane: modelcraft_engine::curves::Plane::Yz, .. }));
    assert!(h.get_by_role_and_label(Role::ComboBox, "Plane").is_focused());
}

#[test]
fn the_intermediate_points_mode_changes_by_keyboard_and_its_number_by_accessibility() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let id = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Tolerance");
    h.get_by_role_and_label(Role::ComboBox, "Intermediate points").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Fixed steps"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let tess = |h: &Harness<'static, ModelApp>| match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Spline { tess, .. } => *tess,
        _ => panic!("not a spline"),
    };
    assert!(matches!(tess(&h), Tessellation::Fixed { .. }));
    h.get_by_role_and_label(Role::SpinButton, "Steps").set_value_accesskit(3.0);
    h.run();
    h.run();
    assert_eq!(tess(&h), Tessellation::Fixed { steps: 3 });
    let points = {
        let s = h.state_mut();
        s.doc.eval.spline(&s.doc.scene, id).unwrap().point_count()
    };
    assert_eq!(points, 12, "four pieces of three steps");
    h.get_by_label_contains("12 points, length");
}

#[test]
fn make_editable_turns_a_primitive_into_points() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Make Spline Editable");
    assert!(h.state().status.contains("Select a spline"), "{}", h.state().status);
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    palette(&mut h, "Make Spline Editable");
    h.run();
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Spline { source: SplineSource::Curve { .. }, .. }));
    h.get_by_label("1 segments, 4 points");
    assert!(h.query_by_role_and_label(Role::SpinButton, "Width").is_none(), "no primitive numbers any more");
}

#[test]
fn curves_are_drawn_and_picked_where_they_are_and_hidden_ones_are_not() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let id = h.state().selection[0];
    h.run();
    let lines = crate::spline_ui::curve_lines(h.state_mut());
    assert!(lines.len() >= 8, "{} segments", lines.len());
    assert!(lines.iter().all(|(a, b, _)| a.iter().chain(b).all(|v| v.is_finite())));
    // A click on the curve picks it; a click far from it does not.
    let rect = egui::Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(900.0, 700.0));
    let cam = h.state().views[0].camera;
    let on_curve = crate::viewport::to_screen(&cam, rect, glam::Vec3::new(1.0, 0.0, 0.0)).unwrap();
    assert_eq!(crate::spline_ui::pick_curve(h.state_mut(), 0, rect, on_curve), Some(id));
    assert_eq!(crate::spline_ui::pick_curve(h.state_mut(), 0, rect, on_curve + egui::vec2(60.0, 60.0)), None);
    let inside = crate::viewport::to_screen(&cam, rect, glam::Vec3::ZERO).unwrap();
    assert_eq!(crate::spline_ui::pick_curve(h.state_mut(), 0, rect, inside), None, "the inside of a curve is not the curve");
    // Frame Selection sees the curve's box.
    let (lo, hi) = crate::spline_ui::world_bounds(h.state_mut(), id).unwrap();
    assert!((hi.x - 1.0).abs() < 0.01 && (lo.x + 1.0).abs() < 0.01);
    // Hidden: not drawn, not picked.
    h.state_mut().run(modelcraft_engine::Command::SetVisible { id, visible: false });
    assert!(crate::spline_ui::curve_lines(h.state_mut()).is_empty());
    assert_eq!(crate::spline_ui::pick_curve(h.state_mut(), 0, rect, on_curve), None);
}

#[test]
fn a_selected_curve_is_drawn_in_the_selection_colour() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let selected = crate::spline_ui::curve_lines(h.state_mut())[0].2;
    h.state_mut().selection.clear();
    let plain = crate::spline_ui::curve_lines(h.state_mut())[0].2;
    assert_ne!(selected, plain);
}
