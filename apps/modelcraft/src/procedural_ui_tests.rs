//! kittest tests of the procedural stream's Properties sections and menu entries, driven by the
//! command palette (the keyboard) and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::ops::primitives::Primitive;
use modelcraft_engine::scene::{ObjectKind, Source};

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

fn primitive(h: &Harness<'static, ModelApp>) -> Primitive {
    let id = h.state().selection[0];
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Mesh { source: Source::Primitive { primitive }, .. } => *primitive,
        other => panic!("not a primitive: {other:?}"),
    }
}

#[test]
fn every_new_primitive_is_in_the_palette_and_names_its_controls() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("Add Disc", "disc", &["Radius", "Hole radius", "Sides", "Rings"]),
        ("Add Polygon", "polygon", &["Radius", "Sides"]),
        ("Add Tube", "tube", &["Radius", "Inner radius", "Height", "Sides", "Rings"]),
        ("Add Capsule", "capsule", &["Radius", "Height", "Sides", "Rings"]),
        ("Add Pyramid", "pyramid", &["Width", "Depth", "Height"]),
        ("Add Tetrahedron", "tetrahedron", &["Radius"]),
        ("Add Octahedron", "octahedron", &["Radius"]),
        ("Add Dodecahedron", "dodecahedron", &["Radius"]),
        ("Add Icosahedron", "icosahedron", &["Radius"]),
        ("Add Landscape", "landscape", &["Width", "Depth", "Height", "Segments", "Seed", "Feature scale", "Octaves"]),
    ];
    for (command, kind, controls) in cases {
        let mut h = harness();
        h.run();
        palette(&mut h, command);
        assert_eq!(primitive(&h).kind(), *kind, "{command}");
        h.run();
        for c in *controls {
            h.get_by_role_and_label(Role::SpinButton, c);
        }
    }
}

#[test]
fn a_landscape_seed_edits_through_accessibility_with_one_undo_step() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Landscape");
    let id = h.state().selection[0];
    let before_mesh = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, id).mesh
    };
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Seed").set_value_accesskit(42.0);
    h.run();
    h.run();
    assert!(matches!(primitive(&h), Primitive::Landscape { seed: 42, .. }));
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    let after_mesh = {
        let s = h.state_mut();
        s.doc.eval.evaluate(&s.doc.scene, id).mesh
    };
    assert_ne!(before_mesh.positions(), after_mesh.positions(), "another seed, another terrain");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(matches!(primitive(&h), Primitive::Landscape { seed: 1, .. }));
}
