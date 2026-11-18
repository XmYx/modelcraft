//! kittest tests of the array and symmetry generators and the shear deformer, driven by the
//! command palette, the keyboard and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::ops::array::Array;
use modelcraft_engine::scene::ObjectKind;

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

fn faces(h: &mut Harness<'static, ModelApp>) -> usize {
    let id = h.state().selection[0];
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, id).mesh.face_count()
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn the_palette_adds_an_array_around_a_cube_and_it_shows_joined_geometry() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Linear Array");
    assert_eq!(h.state().doc.scene.len(), 2, "the cube moved inside the array");
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Array { .. }));
    assert_eq!(faces(&mut h), 24, "four cubes");
    h.run();
    h.get_by_role_and_label(Role::TreeItem, "Array");
    h.get_by_label("24 faces");
    // The cube is shown through the array, not on its own.
    let cube = h.state().doc.scene.objects().iter().find(|o| o.name == "Cube").unwrap().id;
    assert!(h.state().doc.scene.is_cage_input(cube));
}

#[test]
fn array_copies_edit_through_accessibility_with_one_undo_step_and_the_layout_combo_works_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Linear Array");
    assert_eq!(faces(&mut h), 24);
    let before = h.state().doc.undo.len();
    // The accessibility action sets the value, as a screen reader does; the layout combo below is
    // the keyboard-driven part.
    h.get_by_role_and_label(Role::SpinButton, "Copies").set_value_accesskit(6.0);
    h.run();
    h.run();
    assert_eq!(faces(&mut h), 36, "six cubes");
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(faces(&mut h), 24);
    // Layout combo: focus, Space, Arrow Down, Tab to Radial, Enter; focus returns to the combo.
    h.get_by_role_and_label(Role::ComboBox, "Layout").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Linear"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Radial"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Array { array: Array::Radial { .. } }));
    assert_eq!(faces(&mut h), 48, "eight cubes");
    assert!(h.get_by_role_and_label(Role::ComboBox, "Layout").is_focused());
    h.get_by_role_and_label(Role::SpinButton, "Radius");
}

#[test]
fn symmetry_mirrors_its_input_and_its_axis_is_a_combo() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    // Move the cube off the mirror plane first, so the mirror makes a second cube.
    let cube = h.state().selection[0];
    h.state_mut().run(modelcraft_engine::Command::SetTransform {
        id: cube,
        transform: modelcraft_engine::scene::Transform { translation: glam::Vec3::new(2.0, 2.0, 2.0), ..Default::default() },
    });
    palette(&mut h, "Add Symmetry");
    let id = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Symmetry { axis: 0, .. }));
    assert_eq!(faces(&mut h), 12);
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Mirror across");
    h.get_by_role_and_label(Role::SpinButton, "Join distance");
    // Pick the Z axis from the keyboard (Space, Arrow Down, Tab, Tab, Enter): the mirror now
    // lies behind the cube, not beside it.
    let bounds = |h: &mut Harness<'static, ModelApp>| {
        let s = h.state_mut();
        let m = s.doc.eval.evaluate(&s.doc.scene, id).mesh;
        let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for p in m.positions() {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    };
    let (lo, hi) = bounds(&mut h);
    assert!(hi.x - lo.x > 2.0 * (hi.y - lo.y), "mirrored across X: wide, one cube tall: {lo} {hi}");
    h.get_by_role_and_label(Role::ComboBox, "Mirror across").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    let first = focused_label(&h);
    for _ in 0..2 {
        h.key_press(Key::Tab);
        h.run();
    }
    assert_ne!(focused_label(&h), first, "Tab walks down the list");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Symmetry { axis: 2, .. }), "the third entry is the Z axis");
    let (lo, hi) = bounds(&mut h);
    assert!(hi.z - lo.z > 2.0 * (hi.x - lo.x), "now mirrored across Z: {lo} {hi}");
    assert!(h.get_by_role_and_label(Role::ComboBox, "Mirror across").is_focused());
    // A generator around a generator chains: an array of the mirrored pair.
    palette(&mut h, "Add Linear Array");
    assert_eq!(faces(&mut h), 48);
}

#[test]
fn the_shear_deformer_is_in_the_palette_and_moves_the_top() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let cube = h.state().selection[0];
    let before = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, cube).mesh.positions().to_vec()
    };
    palette(&mut h, "Add Shear Deformer");
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Slide X");
    let after = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, cube).mesh.positions().to_vec()
    };
    assert_ne!(before, after);
    // The bottom stays, the top slides by 0.5 per unit of height.
    let lo = before.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let hi = before.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    for (a, b) in before.iter().zip(&after) {
        assert!((b.x - a.x - 0.5 * (a.y - lo)).abs() < 1e-4 && b.y == a.y && b.z == a.z, "{a:?} -> {b:?}");
    }
    assert!(hi > lo);
}
