//! kittest tests of the per-view headers and the statistics overlay: each header changes only its
//! own view, the menus work from the keyboard, names are unique across views, and the overlay
//! is a live region that changes only when its numbers change.

use egui::accesskit::{Live, Role};
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::scene::DisplayMode;
use modelcraft_render::ViewPreset;

use crate::app::ModelApp;
use crate::args::Cli;

fn harness(quad: bool) -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli { quad, ..Cli::default() });
    let id = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    app.select_only(id);
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    h
}

fn open_menu(h: &mut Harness<'static, ModelApp>, name: &str) {
    h.get_by_label(name).click();
    h.run();
    h.run();
}

#[test]
fn each_header_changes_only_its_own_view() {
    let mut h = harness(true);
    let before: Vec<_> = h.state().views.iter().map(|v| (v.preset, v.options)).collect();
    open_menu(&mut h, "Display menu of the Right view");
    h.get_by_label("Wireframe").click();
    h.run();
    h.run();
    let app = h.state();
    assert_eq!(app.views[2].options.display, Some(DisplayMode::Wireframe));
    for i in [0, 1, 3] {
        assert_eq!((app.views[i].preset, app.views[i].options), before[i], "view {i} is untouched");
    }
    assert_eq!(app.display_mode, DisplayMode::Auto, "the app-wide mode is untouched");
    assert_eq!(crate::view_header::display_of(app, 2), DisplayMode::Wireframe);
    assert_eq!(crate::view_header::display_of(app, 0), DisplayMode::Auto);
    // The view menu changes the camera of its own view.
    open_menu(&mut h, "View menu of the Perspective view");
    h.get_by_role_and_label(Role::RadioButton, "Top").click();
    h.run();
    h.run();
    let app = h.state();
    assert_eq!(app.views[0].preset, ViewPreset::Top);
    assert_eq!((app.views[1].preset, app.views[2].preset), (ViewPreset::Front, ViewPreset::Right));
}

#[test]
fn the_header_menus_are_named_by_their_view_and_never_repeat() {
    let h = harness(true);
    let mut names = Vec::new();
    for view in ["Perspective", "Front", "Right", "Top"] {
        for menu in ["View", "Display", "Options", "Camera"] {
            let name = format!("{menu} menu of the {view} view");
            h.get_by_label(&name);
            names.push(name);
        }
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 16);
}

#[test]
fn statistics_and_grid_options_work_from_the_keyboard() {
    let mut h = harness(false);
    assert!(!h.state().views[0].options.stats);
    h.get_by_label("Options menu of the Perspective view").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    h.get_by_label("Statistics").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(h.state().views[0].options.stats, "Space turns the statistics on");
    // Grid off for this view only.
    assert!(crate::view_header::grid_of(h.state(), 0));
    h.get_by_label("Grid").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert!(!crate::view_header::grid_of(h.state(), 0));
    assert!(h.state().show_grid, "the app-wide grid switch is untouched");
    // Reset View Options.
    h.get_by_label("Reset View Options").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().views[0].options, crate::view_header::ViewOptions::default());
    h.key_press_modifiers(Modifiers::NONE, Key::Escape);
}

fn stats_text(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root()
        .query_all(egui_kittest::kittest::by())
        .find(|n| n.accesskit_node().value().is_some_and(|l| l.contains("points")))
        .and_then(|n| n.accesskit_node().value())
}

#[test]
fn the_overlay_lists_the_numbers_and_the_grid_spacing_and_announces_politely() {
    let mut h = harness(false);
    assert!(stats_text(&h).is_none(), "off by default");
    h.state_mut().views[0].options.stats = true;
    h.run();
    h.run();
    let text = stats_text(&h).expect("overlay");
    for part in ["Cube", "8 points", "6 polygons", "About ", "Grid spacing 0.5 m"] {
        assert!(text.contains(part), "{part:?} in {text:?}");
    }
    let node = h.root().query_all(egui_kittest::kittest::by()).find(|n| n.accesskit_node().value().is_some_and(|l| l.contains("points"))).unwrap();
    assert_eq!(node.accesskit_node().live(), Live::Polite);
    // Identical frames give identical text, so nothing is announced; a change gives new text.
    h.run();
    assert_eq!(stats_text(&h), Some(text.clone()));
    h.state_mut().doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }).unwrap();
    let id = h.state().doc.scene.objects().last().unwrap().id;
    h.state_mut().select_only(id);
    h.run();
    h.run();
    assert_ne!(stats_text(&h), Some(text));
    // Units change the readout.
    h.state_mut().unit = crate::prefs_ui::Unit::Cm;
    h.run();
    h.run();
    assert!(stats_text(&h).unwrap().contains("Grid spacing 50 cm"), "{:?}", stats_text(&h));
    // The grid off takes the readout away.
    h.state_mut().views[0].options.grid = Some(false);
    h.run();
    h.run();
    assert!(!stats_text(&h).unwrap().contains("Grid spacing"));
}

#[test]
fn the_overlay_belongs_to_one_view() {
    let mut h = harness(true);
    h.state_mut().views[1].options.stats = true;
    h.run();
    h.run();
    let n = h.root().query_all(egui_kittest::kittest::by()).filter(|n| n.accesskit_node().value().is_some_and(|l| l.contains("points"))).count();
    assert_eq!(n, 1);
}

#[test]
fn camera_reset_and_display_default_work() {
    let mut h = harness(false);
    h.state_mut().views[0].camera.orbit(1.0, 0.3);
    let moved = h.state().views[0].camera;
    open_menu(&mut h, "Camera menu of the Perspective view");
    h.get_by_label("Reset Camera").click();
    h.run();
    assert_ne!(h.state().views[0].camera.view(), moved.view());
    h.state_mut().views[0].options.display = Some(DisplayMode::Flat);
    open_menu(&mut h, "Display menu of the Perspective view");
    h.get_by_label("Follow the Scene Setting").click();
    h.run();
    assert_eq!(h.state().views[0].options.display, None);
}
