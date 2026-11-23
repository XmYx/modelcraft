//! kittest tests of the Variants panel, driven by the keyboard and the accessibility tree, and a
//! render of two variants compared.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{AnimCommand, Command};

use crate::app::{ModelApp, PanelId};
use crate::args::Cli;

fn harness() -> (Harness<'static, ModelApp>, ObjectId) {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let id = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    app.select_only(id);
    assert!(!app.layouts.shown(app.workspace, PanelId::Variants), "off by default, so saved layouts are untouched");
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    palette(&mut h, "Variants Panel");
    (h, id)
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

fn press(h: &mut Harness<'static, ModelApp>, role: Role, name: &str, key: Key) {
    h.get_by_role_and_label(role, name).focus();
    h.run();
    h.key_press(key);
    h.run();
    h.run();
}

#[test]
fn a_variant_is_made_switched_overridden_and_left_by_keyboard() {
    let (mut h, id) = harness();
    h.get_by_role_and_label(Role::RadioButton, "Main scene");
    press(&mut h, Role::Button, "Add Variant", Key::Enter);
    assert_eq!(h.state().doc.scene.variants.list.len(), 1);
    press(&mut h, Role::RadioButton, "Variant 1", Key::Space);
    let v = h.state().doc.scene.variants.list[0].id;
    assert_eq!(h.state().doc.scene.variants.active, Some(v));
    assert!(h.state().status.contains("Variant 1 applied"), "{}", h.state().status);
    // Override the position, then move the object: the move belongs to the variant.
    press(&mut h, Role::Button, "Override Position", Key::Enter);
    assert_eq!(h.state().doc.scene.variants.get(v).unwrap().overrides.len(), 3);
    h.state_mut().doc.execute(Command::Anim(AnimCommand::SetAttr { id, path: "translate.x".into(), value: 4.0 })).unwrap();
    h.run();
    h.run();
    h.get_by_label("Cube: Position X = 4");
    press(&mut h, Role::RadioButton, "Main scene", Key::Space);
    assert_eq!(h.state().doc.scene.attr(id, "translate.x"), Some(0.0), "the main scene is back");
    assert_eq!(h.state().doc.scene.variants.active, None);
    // Undo brings the variant and its edit back, redo leaves again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!((h.state().doc.scene.variants.active, h.state().doc.scene.attr(id, "translate.x")), (Some(v), Some(4.0)));
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    h.run();
    assert_eq!((h.state().doc.scene.variants.active, h.state().doc.scene.attr(id, "translate.x")), (None, Some(0.0)));
}

#[test]
fn clearing_an_override_and_removing_a_variant_work_from_the_keyboard() {
    let (mut h, id) = harness();
    press(&mut h, Role::Button, "Add Variant", Key::Enter);
    press(&mut h, Role::RadioButton, "Variant 1", Key::Space);
    press(&mut h, Role::Button, "Override Visibility", Key::Enter);
    h.get_by_label("Cube: Visible = 1");
    press(&mut h, Role::Button, "Clear override Cube: Visible", Key::Enter);
    assert!(h.state().doc.scene.variants.list[0].overrides.is_empty());
    h.get_by_label("No overrides yet.");
    press(&mut h, Role::Button, "Remove Variant", Key::Enter);
    assert!(h.state().doc.scene.variants.list.is_empty());
    assert_eq!(h.state().doc.scene.variants.active, None);
    assert_eq!(h.state().doc.scene.attr(id, "visible"), Some(1.0));
    // With the main scene applied there is nothing to override.
    h.get_by_label("Switch to a variant to override attributes in it.");
}

#[test]
fn an_animated_attribute_is_refused_with_a_message() {
    let (mut h, id) = harness();
    h.state_mut().doc.execute(Command::Anim(AnimCommand::SetKey { id, path: "translate.y".into(), frame: Some(0.0), value: Some(1.0) })).unwrap();
    press(&mut h, Role::Button, "Add Variant", Key::Enter);
    press(&mut h, Role::RadioButton, "Variant 1", Key::Space);
    press(&mut h, Role::Button, "Override Position", Key::Enter);
    let v = h.state().doc.scene.variants.list[0].id;
    let paths: Vec<_> = h.state().doc.scene.variants.get(v).unwrap().overrides.iter().map(|o| o.path.clone()).collect();
    assert_eq!(paths, ["translate.x", "translate.z"], "the animated channel is left out");
}

#[test]
fn the_name_is_typed_and_applied_with_enter() {
    let (mut h, _) = harness();
    press(&mut h, Role::Button, "Add Variant", Key::Enter);
    press(&mut h, Role::RadioButton, "Variant 1", Key::Space);
    h.get_by_label("Variant name").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text("Night".to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.variants.list[0].name, "Night");
    h.get_by_role_and_label(Role::RadioButton, "Night");
}

/// Render the scene as the Render View and the CLI do (CPU, tiny) and return the image.
fn render(d: &mut modelcraft_engine::Document) -> Vec<[f32; 3]> {
    use modelcraft_render::pathtrace::{RenderSettings, Renderer};
    use modelcraft_render::trace::{ExportOptions, export_scene, scene_bounds};
    let mut cam = modelcraft_render::Camera::default();
    if let Some((lo, hi)) = scene_bounds(&d.scene, &mut d.eval) {
        cam.frame(lo, hi);
    }
    let scene = export_scene(&d.scene, &mut d.eval, &cam, &ExportOptions::default()).unwrap();
    let settings = RenderSettings { width: 32, height: 24, seed: 3, ..RenderSettings::default() };
    let mut r = Renderer::new(std::sync::Arc::new(scene), settings).unwrap();
    r.render(4, &std::sync::atomic::AtomicBool::new(false));
    r.film().mean()
}

#[test]
fn two_variants_render_differently_and_the_main_scene_renders_the_same_after_them() {
    use modelcraft_engine::VariantCommand as V;
    let mut d = modelcraft_engine::Document::new();
    let id = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    let main = render(&mut d);
    for name in ["Red", "Hidden"] {
        d.execute(Command::Variant(V::Add { name: Some(name.into()) })).unwrap();
    }
    let (red, hidden) = (d.scene.variants.list[0].id, d.scene.variants.list[1].id);
    d.execute(Command::Variant(V::Switch { id: Some(red) })).unwrap();
    d.execute(Command::Variant(V::Override { object: id, paths: vec!["material.base_color.0".into(), "material.base_color.1".into()] })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetAttr { id, path: "material.base_color.0".into(), value: 1.0 })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetAttr { id, path: "material.base_color.1".into(), value: 0.0 })).unwrap();
    let red_image = render(&mut d);
    d.execute(Command::Variant(V::Switch { id: Some(hidden) })).unwrap();
    d.execute(Command::Variant(V::Override { object: id, paths: vec!["visible".into()] })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetAttr { id, path: "visible".into(), value: 0.0 })).unwrap();
    let hidden_image = render(&mut d);
    d.execute(Command::Variant(V::Switch { id: None })).unwrap();
    let again = render(&mut d);
    let mse = |a: &[[f32; 3]], b: &[[f32; 3]]| {
        a.iter().zip(b).map(|(p, q)| (0..3).map(|i| f64::from(p[i] - q[i]).powi(2)).sum::<f64>()).sum::<f64>() / a.len() as f64
    };
    assert!(mse(&main, &red_image) > 1e-4, "the red variant looks different from main");
    assert!(mse(&main, &hidden_image) > 1e-4, "the hidden variant looks different from main");
    assert!(mse(&red_image, &hidden_image) > 1e-4, "and from each other");
    assert_eq!(main, again, "after switching around, the main scene renders identically (same seed)");
}
