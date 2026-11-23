//! kittest tests of light and camera objects: the command palette, the attribute pages, picking
//! and looking through a camera, all driven through the keyboard and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::stage::LightKind;
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

fn light(h: &Harness<'static, ModelApp>, id: ObjectId) -> modelcraft_engine::scene::stage::LightData {
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Light { light } => light.clone(),
        k => panic!("not a light: {k:?}"),
    }
}

#[test]
fn the_palette_adds_every_kind_of_light_and_a_camera() {
    let mut h = harness();
    h.run();
    for (text, kind) in [
        ("Add Point Light", LightKind::Point),
        ("Add Spot Light", LightKind::Spot),
        ("Add Area Light", LightKind::Area),
        ("Add Sun Light", LightKind::Sun),
        ("Add Dome Light", LightKind::Dome),
    ] {
        palette(&mut h, text);
        let id = h.state().selection[0];
        assert_eq!(light(&h, id).kind, kind, "{text}");
    }
    palette(&mut h, "Add Camera");
    let cam = h.state().selection[0];
    assert!(h.state().doc.scene.camera_data(cam).is_some());
    assert_eq!(h.state().doc.scene.lights().len(), 5);
    // The tree lists them by name, and the page for the camera shows its lens.
    h.run();
    h.get_by_role_and_label(Role::TreeItem, "Camera");
    h.get_by_role_and_label(Role::SpinButton, "Focal length (mm)");
}

#[test]
fn a_spot_lights_page_edits_with_one_undo_step_each_and_clamps() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Spot Light");
    let id = h.state().selection[0];
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Cone angle (degrees)").set_value_accesskit(60.0);
    h.run();
    h.run();
    assert_eq!(light(&h, id).spread, 60.0);
    assert_eq!(h.state().doc.undo.len(), before + 1);
    // A value past the end of the range is held at it.
    h.get_by_role_and_label(Role::SpinButton, "Intensity (W/sr)").set_value_accesskit(1e12);
    h.run();
    h.run();
    assert_eq!(light(&h, id).intensity, modelcraft_engine::scene::stage::MAX_INTENSITY);
    h.get_by_role_and_label(Role::SpinButton, "Edge softness");
    assert!(h.query_by_label("Width").is_none(), "only an area light has a width");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(light(&h, id).intensity, 300.0);
    assert_eq!(light(&h, id).spread, 60.0);
}

#[test]
fn the_kind_of_a_light_changes_from_the_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Point Light");
    let id = h.state().selection[0];
    let combo = h.get_by_role_and_label(Role::ComboBox, "Kind");
    combo.focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    // The list opens on its first entry; walk to Sun and pick it.
    for _ in 0..3 {
        h.key_press(Key::Tab);
        h.run();
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(light(&h, id).kind, LightKind::Sun);
    h.get_by_role_and_label(Role::SpinButton, "Apparent size (degrees)");
}

#[test]
fn looking_through_a_camera_follows_its_lens_and_moving_the_view_moves_the_camera() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Camera");
    let cam = h.state().selection[0];
    palette(&mut h, "Look Through Selected Camera");
    h.run();
    assert_eq!(h.state().stage.looking_through(h.state().active_view), Some(cam));
    let wide = h.state().views[h.state().active_view].camera.fov_y;
    // A longer lens narrows the view.
    h.get_by_role_and_label(Role::SpinButton, "Focal length (mm)").set_value_accesskit(200.0);
    h.run();
    h.run();
    let narrow = h.state().views[h.state().active_view].camera.fov_y;
    assert!(narrow < wide * 0.5, "{narrow} vs {wide}");
    // The view sits where the camera does.
    let eye = h.state().views[h.state().active_view].camera.eye();
    let at = h.state().doc.scene.camera_view(cam).unwrap().0.eye;
    assert!((eye - at).length() < 1e-3, "{eye:?} {at:?}");
    // Arrow keys on the focused viewport orbit it: the camera follows, as one undo step each.
    let view = h.get_all_by_role(Role::Canvas).next().unwrap();
    view.focus();
    h.run();
    let before_t = h.state().doc.scene.get(cam).unwrap().transform;
    let undo = h.state().doc.undo.len();
    h.key_press(Key::ArrowLeft);
    h.run();
    h.run();
    let after_t = h.state().doc.scene.get(cam).unwrap().transform;
    assert_ne!(before_t.translation, after_t.translation, "the camera moved with the view");
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    // Stopping goes back to the view's own camera; the camera stays where it is.
    palette(&mut h, "Stop Looking Through Camera");
    assert_eq!(h.state().stage.looking_through(h.state().active_view), None);
    assert_eq!(h.state().doc.scene.get(cam).unwrap().transform, after_t);
    // The camera can be made the render camera from the keyboard.
    palette(&mut h, "Render Through Selected Camera");
    assert_eq!(h.state().doc.scene.render_camera(), Some(cam));
    h.run();
    assert!(h.get_by_role_and_label(Role::Button, "Render Through This Camera").accesskit_node().is_disabled());
}

#[test]
fn no_camera_selected_says_so() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Look Through Selected Camera");
    assert!(h.state().status.contains("Select a camera"), "{}", h.state().status);
    palette(&mut h, "Render Through Selected Camera");
    assert!(h.state().status.contains("Select a camera"), "{}", h.state().status);
}

#[test]
fn clicking_a_lights_drawing_selects_it_and_a_camera_from_view_sits_at_the_view() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Point Light");
    let id = h.state().selection[0];
    h.state_mut().selection.clear();
    h.run();
    // Pick directly: a click on one of the segments of the drawing finds the light.
    let app = h.state();
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
    let seg = crate::stage_ui::outline(&app.doc.scene, id)[0];
    let cam = app.views[0].camera;
    let at = crate::viewport::to_screen(&cam, rect, (seg.0 + seg.1) * 0.5).unwrap();
    assert_eq!(crate::stage_ui::pick(app, 0, rect, at), Some(id));
    assert_eq!(crate::stage_ui::pick(app, 0, rect, at + egui::vec2(60.0, 60.0)), None);
    palette(&mut h, "Add Camera From View");
    let cam_id = h.state().selection[0];
    let eye = h.state().views[h.state().active_view].camera.eye();
    let placed = h.state().doc.scene.camera_view(cam_id).unwrap().0;
    assert!((placed.eye - eye).length() < 1e-4);
    let want = -h.state().views[h.state().active_view].camera.back();
    assert!((placed.forward - want).length() < 1e-4, "{:?} vs {want:?}", placed.forward);
}
