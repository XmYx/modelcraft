//! kittest tests of falloff fields, driven by the command palette and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::field::{Blend, Curve, Shape};
use modelcraft_engine::scene::{Field, ObjectKind};

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

fn selected_field(h: &Harness<'static, ModelApp>) -> Field {
    let id = h.state().selection[0];
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Field { field } => field.clone(),
        k => panic!("not a field: {k:?}"),
    }
}

/// A cube with a twist deformer, then a sphere falloff field inside it, all by the palette.
fn with_field(h: &mut Harness<'static, ModelApp>) {
    h.run();
    palette(h, "Add Cube");
    palette(h, "Add Twist Deformer");
    palette(h, "Add Sphere Falloff Field");
    h.run();
}

#[test]
fn the_palette_puts_a_field_inside_the_selected_deformer() {
    let mut h = harness();
    with_field(&mut h);
    let id = h.state().selection[0];
    let o = h.state().doc.scene.get(id).unwrap().clone();
    assert!(matches!(o.kind, ObjectKind::Field { .. }));
    let parent = h.state().doc.scene.get(o.parent.unwrap()).unwrap();
    assert!(matches!(parent.kind, ObjectKind::Deformer { .. }));
    h.get_by_role_and_label(Role::TreeItem, "Sphere field");
    // Adding another from the field adds it to the same deformer.
    palette(&mut h, "Add Noise Falloff Field");
    let n = h.state().selection[0];
    assert_eq!(h.state().doc.scene.get(n).unwrap().parent, Some(parent_id(&h, id)));
    assert_eq!(h.state().doc.scene.field_stack(parent_id(&h, id)).len(), 2);
}

fn parent_id(h: &Harness<'static, ModelApp>, id: modelcraft_engine::scene::ObjectId) -> modelcraft_engine::scene::ObjectId {
    h.state().doc.scene.get(id).unwrap().parent.unwrap()
}

#[test]
fn a_field_without_a_deformer_says_what_to_select() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Box Falloff Field");
    assert_eq!(h.state().doc.scene.len(), 1, "nothing was added");
    assert!(h.state().status.contains("Select a deformer"), "{}", h.state().status);
}

#[test]
fn shape_fields_edit_through_the_accessibility_tree_with_one_undo_step_each() {
    let mut h = harness();
    with_field(&mut h);
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Radius").set_value_accesskit(1.8);
    h.run();
    h.run();
    assert_eq!(selected_field(&h).shape, Shape::Sphere { radius: 1.8, inner: 0.0 });
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.get_by_role_and_label(Role::SpinButton, "Full strength inside").set_value_accesskit(0.5);
    h.run();
    h.run();
    assert_eq!(selected_field(&h).shape, Shape::Sphere { radius: 1.8, inner: 0.5 });
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(selected_field(&h).shape, Shape::Sphere { radius: 1.8, inner: 0.0 });
}

/// Focus a control and press a key, the way a keyboard user does.
fn press_on(h: &mut Harness<'static, ModelApp>, role: Role, name: &str, key: Key) {
    h.get_by_role_and_label(role, name).focus();
    h.run();
    h.key_press(key);
    h.run();
    h.run();
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

/// Pick an option of a combo from the keyboard: focus it, Space opens it, Arrow Down enters the
/// list (first option), Tab walks on to `option`, Enter picks it; focus ends on the combo again.
fn pick_by_keyboard(h: &mut Harness<'static, ModelApp>, combo: &str, options: &[&str], option: &str) {
    press_on(h, Role::ComboBox, combo, Key::Space);
    assert!(h.get_by_role_and_label(Role::ComboBox, combo).is_focused(), "focus stays on the {combo} combo when it opens");
    h.key_press(Key::ArrowDown);
    h.run();
    let at = options.iter().position(|o| *o == option).expect("option in list");
    assert_eq!(focused_label(h).as_deref(), Some(options[0]), "Arrow Down enters the list");
    for next in &options[1..=at] {
        h.key_press(Key::Tab);
        h.run();
        assert_eq!(focused_label(h).as_deref(), Some(*next));
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(h.get_by_role_and_label(Role::ComboBox, combo).is_focused(), "focus returns to the {combo} combo, now on {:?}", focused_label(h));
}

#[test]
fn the_shape_blend_and_curve_pickers_work_from_the_keyboard() {
    let mut h = harness();
    with_field(&mut h);
    pick_by_keyboard(&mut h, "Shape", &["Sphere", "Box", "Linear", "Noise"], "Box");
    assert!(matches!(selected_field(&h).shape, Shape::Box { .. }));
    h.get_by_role_and_label(Role::SpinButton, "Box size X");
    pick_by_keyboard(&mut h, "Curve", &["Straight", "Smooth", "Slow start"], "Slow start");
    assert_eq!(selected_field(&h).remap.curve, Curve::preset("ease_in").unwrap());
    pick_by_keyboard(&mut h, "Combine with earlier fields", &["Multiply", "Add", "Subtract", "Smallest"], "Smallest");
    assert_eq!(selected_field(&h).blend, Blend::Min);
    // The Invert checkbox toggles with Space, and keeps focus.
    press_on(&mut h, Role::CheckBox, "Invert", Key::Space);
    assert!(selected_field(&h).remap.invert);
    assert!(h.get_by_role_and_label(Role::CheckBox, "Invert").is_focused());
    press_on(&mut h, Role::CheckBox, "Invert", Key::Space);
    assert!(!selected_field(&h).remap.invert);
}

#[test]
fn curve_points_can_be_added_edited_and_removed_from_the_keyboard() {
    let mut h = harness();
    with_field(&mut h);
    h.get_by_label("Curve points").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(selected_field(&h).remap.curve.points.len(), 2);
    press_on(&mut h, Role::Button, "Add point", Key::Enter);
    assert_eq!(selected_field(&h).remap.curve.points.len(), 3);
    h.get_by_role_and_label(Role::SpinButton, "Point 2 output").set_value_accesskit(0.2);
    h.run();
    h.run();
    assert!((selected_field(&h).remap.curve.points[1][1] - 0.2).abs() < 1e-4);
    // The remove button's visible text is "Remove", and its name starts with it.
    h.get_by_role_and_label(Role::Button, "Remove point 2");
    h.get_by_role_and_label(Role::Button, "Remove point 2").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert_eq!(selected_field(&h).remap.curve.points.len(), 2);
    assert!(h.query_by_label("Remove point 3").is_none());
    // At two points the remove buttons are disabled: a curve needs two.
    assert!(h.get_by_role_and_label(Role::Button, "Remove point 1").accesskit_node().is_disabled());
}

#[test]
fn the_field_changes_what_the_deformer_does() {
    let mut h = harness();
    with_field(&mut h);
    let cube = h.state().doc.scene.objects().iter().find(|o| o.name == "Cube").unwrap().id;
    let with = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, cube).mesh.positions().to_vec()
    };
    // Hiding the field (the tree's eye) makes the twist act everywhere.
    let fid = h.state().selection[0];
    h.state_mut().run(modelcraft_engine::Command::SetVisible { id: fid, visible: false });
    h.run();
    let without = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, cube).mesh.positions().to_vec()
    };
    assert_ne!(with, without);
}
