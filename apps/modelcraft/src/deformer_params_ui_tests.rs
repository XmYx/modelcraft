//! kittest tests of the second set of deformers: menu entries from the palette, their controls
//! by name, the box ("Reach") and the shrink-wrap target picked from the keyboard.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::ops::deform::{Deformer, LimitMode};
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

fn deformer(h: &Harness<'static, ModelApp>) -> (Deformer, modelcraft_engine::ops::deform::Limits, Option<ObjectId>) {
    let id = h.state().selection[0];
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Deformer { deformer, limits, target } => (*deformer, *limits, *target),
        k => panic!("not a deformer: {k:?}"),
    }
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

#[test]
fn every_new_deformer_is_in_the_palette_and_names_its_controls() {
    let cases: &[(&str, &[&str])] = &[
        ("Add Bulge Deformer", &["Amount"]),
        ("Add Squash Deformer", &["Amount"]),
        ("Add Smooth Deformer", &["Passes", "Strength"]),
        ("Add Delta Smooth Deformer", &["Passes", "Strength"]),
        ("Add Displace Deformer", &["Amplitude", "Feature scale", "Octaves", "Seed"]),
        ("Add Shrink Wrap Deformer", &["Offset"]),
    ];
    for (command, controls) in cases {
        let mut h = harness();
        h.run();
        palette(&mut h, "Add Sphere");
        palette(&mut h, command);
        h.run();
        for c in *controls {
            h.get_by_role_and_label(Role::SpinButton, c);
        }
    }
}

#[test]
fn a_bulge_amount_edits_through_accessibility_with_one_undo_step_and_moves_the_mesh() {
    let mut h = harness();
    h.run();
    // A sphere has points at every height; a cylinder with one ring only has its two ends, which
    // a bulge leaves alone.
    palette(&mut h, "Add Sphere");
    let mesh = h.state().selection[0];
    palette(&mut h, "Add Bulge Deformer");
    let before = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, mesh).mesh.positions().to_vec()
    };
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Amount").set_value_accesskit(1.5);
    h.run();
    h.run();
    assert!(matches!(deformer(&h).0, Deformer::Bulge { amount } if (amount - 1.5).abs() < 1e-6));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    let after = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, mesh).mesh.positions().to_vec()
    };
    assert!(before != after, "a stronger bulge moves the points");
}

#[test]
fn the_reach_section_changes_the_mode_and_the_box_from_the_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cylinder");
    palette(&mut h, "Add Twist Deformer");
    h.run();
    h.get_by_label("Reach").click();
    h.run();
    h.run();
    // Fit to parent is on, so no box size; switching it off shows the three numbers.
    assert!(h.query_by_label("Box size X").is_none());
    h.get_by_role_and_label(Role::CheckBox, "Fit to parent").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(!deformer(&h).1.fit_to_parent);
    h.get_by_role_and_label(Role::SpinButton, "Box size X");
    // The mode combo: Space opens it, Arrow Down moves, Tab walks to Unlimited, Enter picks it.
    h.get_by_role_and_label(Role::ComboBox, "Mode").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Limited"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Unlimited"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(deformer(&h).1.mode, LimitMode::Unlimited);
    assert!(h.get_by_role_and_label(Role::ComboBox, "Mode").is_focused());
}

#[test]
fn a_shrink_wrap_target_is_picked_from_the_keyboard_and_the_mesh_lands_on_it() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let cube = h.state().selection[0];
    palette(&mut h, "Add Sphere");
    let ball = h.state().selection[0];
    palette(&mut h, "Add Shrink Wrap Deformer");
    h.run();
    h.get_by_label("Choose the object to wrap onto.");
    let e = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, ball)
    };
    assert!(e.errors.iter().any(|e| e.message.contains("target")));
    // Open the target combo and pick the cube.
    h.get_by_role_and_label(Role::ComboBox, "Target object").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("None"));
    for _ in 0..8 {
        h.key_press(Key::Tab);
        h.run();
        if focused_label(&h).as_deref() == Some("Cube") {
            break;
        }
    }
    assert_eq!(focused_label(&h).as_deref(), Some("Cube"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(deformer(&h).2, Some(cube));
    let m = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, ball).mesh
    };
    // The default cube is 2 wide: the sphere's points lie on its surface.
    assert!(m.positions().iter().all(|p| (p.abs().max_element() - 1.0).abs() < 1e-3), "on the cube");
}
