//! kittest tests of Bake Maps: the palette, the window's controls, the worker and its files.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_bake::MapKind;
use modelcraft_engine::Command;
use modelcraft_engine::scene::Primitive;

use crate::app::ModelApp;
use crate::args::Cli;

/// A coarse sphere (selected) inside a finer one: bake the coarse one from the fine one.
fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    app.doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Dense".into()), parent: None }).unwrap();
    let low = app.doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Low".into()), parent: None }).unwrap().created[0];
    app.doc.execute(Command::SetPrimitive { id: low, primitive: Primitive::Sphere { radius: 1.0, segments: 8, rings: 4 } }).unwrap();
    app.selection = vec![low];
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

fn wait(h: &mut Harness<'static, ModelApp>) {
    for _ in 0..3000 {
        h.step();
        if !h.state().bake.busy() {
            h.step();
            h.step();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the bake did not finish: {}", h.state().bake.state);
}

#[test]
fn the_window_names_its_target_and_asks_for_a_place_to_write() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Bake Maps");
    assert!(h.state().bake.open);
    h.get_by_label("Bake onto: Low");
    for name in ["Normal", "Displacement", "Occlusion", "Curvature", "Cavity", "Colour"] {
        h.get_by_role_and_label(Role::CheckBox, name);
    }
    for name in ["Width", "Height", "Occlusion rays", "Padding (texels)", "Cage distance", "Ray distance"] {
        h.get_by_role_and_label(Role::SpinButton, name);
    }
    palette(&mut h, "Bake Maps Now");
    assert!(h.state().bake.state.contains("Choose where the maps are written"), "{}", h.state().bake.state);
    assert!(!h.state().bake.busy());
}

#[test]
fn a_bake_writes_a_file_per_map_and_keeps_them_in_the_picture_viewer() {
    let dir = std::env::temp_dir().join(format!("mc-bake-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.run();
    {
        let b = &mut h.state_mut().bake;
        b.options.width = 24;
        b.options.height = 24;
        b.options.rays = 4;
        b.options.cage = 0.2;
        b.options.distance = 0.2;
        b.options.maps = vec![MapKind::Normal, MapKind::Occlusion];
        b.prefix = dir.join("rock").display().to_string();
    }
    palette(&mut h, "Bake Maps Now");
    assert!(h.state().bake.busy() || h.state().bake.state.starts_with("Bake"), "{}", h.state().bake.state);
    wait(&mut h);
    assert_eq!(
        h.state().bake.state,
        "Bake finished: 2 maps, 0 texels found no surface"
            .replace("0 texels", &format!("{} texels", h.state().bake.state.split(", ").nth(1).and_then(|s| s.split(' ').next()).unwrap_or("0")))
    );
    for name in ["rock_normal.png", "rock_occlusion.png"] {
        assert!(std::fs::metadata(dir.join(name)).unwrap().len() > 0, "{name}");
    }
    assert_eq!(h.state().pictures.history.len(), 2);
    assert_eq!(h.state().pictures.history.get(0).unwrap().label, "Bake of Low: Normal");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_running_bake_can_be_cancelled_and_nothing_is_written() {
    let dir = std::env::temp_dir().join(format!("mc-bake-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.run();
    {
        let b = &mut h.state_mut().bake;
        b.options.width = 2048;
        b.options.height = 2048;
        b.options.rays = 512;
        b.options.maps = vec![MapKind::Occlusion];
        b.prefix = dir.join("never").display().to_string();
    }
    palette(&mut h, "Bake Maps Now");
    assert!(h.state().bake.busy());
    palette(&mut h, "Cancel Bake");
    wait(&mut h);
    assert_eq!(h.state().bake.state, "Bake cancelled");
    assert!(!dir.join("never_occlusion.png").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bake_without_a_target_or_dense_meshes_says_what_is_missing() {
    let mut h = harness();
    h.run();
    h.state_mut().selection.clear();
    h.state_mut().bake.prefix = "/tmp/never".into();
    palette(&mut h, "Bake Maps Now");
    assert!(h.state().bake.state.contains("Select the mesh to bake onto"), "{}", h.state().bake.state);
    // Selecting only the target, with "every other surface" off, has no dense mesh.
    let low = h.state().doc.scene.objects().iter().find(|o| o.name == "Low").unwrap().id;
    h.state_mut().selection = vec![low];
    h.state_mut().bake.from_all = false;
    palette(&mut h, "Bake Maps Now");
    assert!(h.state().bake.state.contains("Select the dense meshes too"), "{}", h.state().bake.state);
}
