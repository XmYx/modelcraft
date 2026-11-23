//! kittest tests of the falloff-layer list and the second set of field shapes: the list is
//! driven by the keyboard (focus a control, press a key) and read through the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::field::{Blend, SHAPES, Shape};
use modelcraft_engine::scene::{Field, ObjectId, ObjectKind};

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

/// A cube with a twist deformer holding a big sphere field and, after it, a smaller one that is
/// subtracted from it; the deformer is selected so its Properties show the layer list.
fn with_two_fields(h: &mut Harness<'static, ModelApp>) -> (ObjectId, ObjectId, ObjectId, ObjectId) {
    h.run();
    palette(h, "Add Cube");
    let cube = h.state().selection[0];
    palette(h, "Add Twist Deformer");
    let twist = h.state().selection[0];
    let mut ids = Vec::new();
    for (radius, blend) in [(3.0, Blend::Multiply), (2.0, Blend::Subtract)] {
        let f = h.state_mut().doc.execute(Command::AddField { kind: "sphere".into(), target: twist }).unwrap().created[0];
        let mut field = Field::default_of("sphere").unwrap();
        field.shape = Shape::Sphere { radius, inner: 0.0 };
        field.blend = blend;
        h.state_mut().doc.execute(Command::SetField { id: f, field }).unwrap();
        ids.push(f);
    }
    h.state_mut().select_only(twist);
    h.run();
    h.run();
    (cube, twist, ids[0], ids[1])
}

fn positions(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> Vec<glam::Vec3> {
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, id).mesh.positions().to_vec()
}

fn order(h: &Harness<'static, ModelApp>, twist: ObjectId) -> Vec<ObjectId> {
    h.state().doc.scene.children(twist)
}

fn field_of(h: &Harness<'static, ModelApp>, id: ObjectId) -> Field {
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Field { field } => field.clone(),
        k => panic!("not a field: {k:?}"),
    }
}

#[test]
fn the_layer_list_names_every_control_and_reorders_two_fields_from_the_keyboard() {
    let mut h = harness();
    let (cube, twist, big, small) = with_two_fields(&mut h);
    assert_eq!(order(&h, twist), vec![big, small]);
    h.get_by_label("Falloff layers (2)");
    // Two rows with the same kind of field still have different names (the second is renamed
    // by the scene to stay unique).
    let names: Vec<String> = [big, small].iter().map(|i| h.state().doc.scene.get(*i).unwrap().name.clone()).collect();
    assert_ne!(names[0], names[1]);
    for n in &names {
        h.get_by_role_and_label(Role::CheckBox, &format!("{n} switched on"));
        h.get_by_role_and_label(Role::ComboBox, &format!("{n} blend"));
        h.get_by_role_and_label(Role::SpinButton, &format!("{n} opacity"));
        h.get_by_role_and_label(Role::Button, &format!("Edit {n}"));
    }
    let before = positions(&mut h, cube);
    // The first row cannot move earlier; the second cannot move later.
    assert!(h.get_by_role_and_label(Role::Button, &format!("Earlier: {}", names[0])).accesskit_node().is_disabled());
    assert!(h.get_by_role_and_label(Role::Button, &format!("Later: {}", names[1])).accesskit_node().is_disabled());
    // Focus "Move later" on the first row and press Enter.
    h.get_by_role_and_label(Role::Button, &format!("Later: {}", names[0])).focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(order(&h, twist), vec![small, big], "the order of the fields changed");
    let after = positions(&mut h, cube);
    assert_ne!(before, after, "subtracting the big field from the small one is another weight");
    // And back, with undo.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(order(&h, twist), vec![big, small]);
    assert_eq!(positions(&mut h, cube), before);
}

#[test]
fn a_layer_switch_is_a_checkbox_that_the_keyboard_toggles_and_it_changes_the_result() {
    let mut h = harness();
    let (cube, _twist, big, small) = with_two_fields(&mut h);
    let with_both = positions(&mut h, cube);
    let name = h.state().doc.scene.get(small).unwrap().name.clone();
    h.get_by_role_and_label(Role::CheckBox, &format!("{name} switched on")).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(!field_of(&h, small).enabled);
    assert!(field_of(&h, big).enabled);
    let only_big = positions(&mut h, cube);
    assert_ne!(with_both, only_big, "a switched-off field no longer takes part");
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(field_of(&h, small).enabled);
    assert_eq!(positions(&mut h, cube), with_both);
}

#[test]
fn opacity_and_blend_edit_through_accessibility_and_each_is_one_undo_step() {
    let mut h = harness();
    let (cube, _twist, _big, small) = with_two_fields(&mut h);
    let name = h.state().doc.scene.get(small).unwrap().name.clone();
    let full = positions(&mut h, cube);
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, &format!("{name} opacity")).set_value_accesskit(0.0);
    h.run();
    h.run();
    assert_eq!(field_of(&h, small).opacity, 0.0);
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    assert_ne!(positions(&mut h, cube), full, "an opacity of nothing leaves the subtraction out");
    // The blend combo opens from the keyboard; the pick is a command too.
    h.get_by_role_and_label(Role::ComboBox, &format!("{name} blend")).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Tab);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_ne!(field_of(&h, small).blend, Blend::Subtract);
}

#[test]
fn every_shape_can_be_added_from_the_palette_and_shows_its_own_controls() {
    let cases: &[(&str, &[&str])] = &[
        ("radial", &["Sweep angle"]),
        ("cylinder", &["Radius", "Height"]),
        ("cone", &["Radius", "Height"]),
        ("capsule", &["Radius", "Length"]),
        ("torus", &["Ring radius", "Tube radius"]),
        ("random", &["Cells per unit", "Seed"]),
    ];
    for (kind, controls) in cases {
        assert!(SHAPES.contains(kind));
        let mut h = harness();
        h.run();
        palette(&mut h, "Add Cube");
        palette(&mut h, "Add Twist Deformer");
        let label = match *kind {
            "radial" => "Radial",
            "cylinder" => "Cylinder",
            "cone" => "Cone",
            "capsule" => "Capsule",
            "torus" => "Torus",
            _ => "Random",
        };
        palette(&mut h, &format!("Add {label} Falloff Field"));
        h.run();
        let id = h.state().selection[0];
        assert_eq!(field_of(&h, id).kind(), *kind);
        for c in *controls {
            h.get_by_role_and_label(Role::SpinButton, c);
        }
        h.get_by_role_and_label(Role::SpinButton, "Opacity");
        h.get_by_role_and_label(Role::CheckBox, "Field enabled");
    }
}

#[test]
fn a_formula_field_previews_while_typing_and_commits_when_the_box_loses_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Twist Deformer");
    palette(&mut h, "Add Formula Falloff Field");
    h.run();
    let id = h.state().selection[0];
    assert!(matches!(field_of(&h, id).shape, Shape::Formula { .. }));
    let undo = h.state().doc.undo.len();
    let edit = h.get_by_role_and_label(Role::TextInput, "Formula");
    edit.focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.event(egui::Event::Text("y + 0.5".to_string()));
    h.run();
    h.key_press(Key::Tab);
    h.run();
    h.run();
    assert!(matches!(&field_of(&h, id).shape, Shape::Formula { expr } if expr == "y + 0.5"), "{:?}", field_of(&h, id).shape);
    assert_eq!(h.state().doc.undo.len(), undo + 1, "one undo step for the whole edit");
}
