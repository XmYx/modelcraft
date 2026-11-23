//! kittest tests of the render settings, the frame queue and the Picture Viewer, driven by the
//! command palette, the keyboard and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::stage::{OutputFormat, Pass, RenderConfig};
use modelcraft_engine::{Command, StageCommand};
use modelcraft_render::output::Picture;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::render_view::Device;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    app.render_view.device = Device::Cpu;
    app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap();
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

fn picture(label: &str, grey: u8) -> Picture {
    Picture { label: label.into(), size: [4, 4], pixels: (0..16).flat_map(|_| [grey, grey, grey, 255]).collect(), samples: 8, seconds: 0.5 }
}

fn wait_for_queue(h: &mut Harness<'static, ModelApp>) {
    for _ in 0..3000 {
        h.step();
        if !h.state().render_ui.busy() {
            h.step();
            h.step();
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the frame render did not finish: {}", h.state().render_ui.state);
}

fn small_config(dir: &std::path::Path, last: i32) -> RenderConfig {
    RenderConfig {
        width: 32,
        height: 24,
        samples: 2,
        frame_start: 1,
        frame_end: last,
        output: dir.join("shot.png").display().to_string(),
        ..Default::default()
    }
}

fn set_config(h: &mut Harness<'static, ModelApp>, config: RenderConfig) {
    h.state_mut().doc.execute(Command::Stage(StageCommand::SetRenderSettings { config })).unwrap();
}

#[test]
fn the_settings_window_edits_the_project_one_undo_step_at_a_time() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Render Settings");
    assert!(h.state().render_ui.open);
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Width").set_value_accesskit(640.0);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.render.width, 640);
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    h.get_by_role_and_label(Role::SpinButton, "Last frame").set_value_accesskit(5.0);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.render.frames().len(), 6, "frames 0 to 5");
    h.get_by_label("6 frames, each 640 by 540 pixels");
    // Hostile values are held in range.
    h.get_by_role_and_label(Role::SpinButton, "Width").set_value_accesskit(1e9);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.render.width, 8192);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.render.width, 640);
}

#[test]
fn categories_are_reachable_by_keyboard_and_passes_need_exr() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Render Settings");
    h.run();
    let passes = h.get_by_role_and_label(Role::Button, "Passes");
    passes.focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let depth = h.get_by_role_and_label(Role::CheckBox, "Depth");
    assert!(depth.accesskit_node().is_disabled(), "a PNG holds the picture only");
    set_config(&mut h, RenderConfig { format: OutputFormat::Exr, ..Default::default() });
    h.run();
    h.run();
    let depth = h.get_by_role_and_label(Role::CheckBox, "Depth");
    assert!(!depth.accesskit_node().is_disabled());
    depth.focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.render.passes, vec![Pass::Depth]);
    assert!(h.get_by_role_and_label(Role::CheckBox, "Depth").is_focused());
    // Esc with focus in the window closes it.
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().render_ui.open);
}

#[test]
fn a_frame_range_renders_numbered_files_into_the_viewers_history() {
    let dir = std::env::temp_dir().join(format!("mc-queue-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.run();
    set_config(&mut h, small_config(&dir, 2));
    palette(&mut h, "Render Frames to Files");
    assert!(h.state().render_ui.busy());
    wait_for_queue(&mut h);
    assert_eq!(h.state().render_ui.state, "Frame render finished: 2 files");
    for name in ["shot_0001.png", "shot_0002.png"] {
        assert!(std::fs::metadata(dir.join(name)).unwrap().len() > 0, "{name}");
    }
    assert_eq!(h.state().pictures.history.len(), 2);
    assert_eq!(h.state().pictures.history.get(0).unwrap().size, [32, 24]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_queue_without_a_place_to_write_says_so_and_a_running_one_can_be_cancelled() {
    let dir = std::env::temp_dir().join(format!("mc-queue-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.run();
    palette(&mut h, "Render Frames to Files");
    assert!(!h.state().render_ui.busy());
    assert!(h.state().render_ui.state.contains("Choose where the frames are written"), "{}", h.state().render_ui.state);
    assert!(h.state().render_ui.open, "the window opens on the page that asks");
    // A long queue of heavy frames; cancel it from the palette.
    set_config(&mut h, RenderConfig { samples: 4096, width: 256, height: 256, ..small_config(&dir, 1000) });
    palette(&mut h, "Render Frames to Files");
    assert!(h.state().render_ui.busy());
    palette(&mut h, "Cancel Frame Render");
    wait_for_queue(&mut h);
    assert!(h.state().render_ui.state.starts_with("Frame render cancelled after"), "{}", h.state().render_ui.state);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_picture_viewer_lists_compares_and_describes_pictures() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Picture Viewer");
    assert!(h.state().pictures.open);
    h.get_by_label("No pictures yet. Finished renders appear here.");
    h.state_mut().pictures.add(picture("Dark", 40));
    h.state_mut().pictures.add(picture("Light", 200));
    h.run();
    h.run();
    h.get_by_label("Dark: 4 by 4, 8 samples");
    h.get_by_label("Light: 4 by 4, 8 samples");
    assert_eq!(h.state().pictures.selected, 1, "the newest is selected");
    // The histogram says in words where the pixels are.
    h.get_by_label("Histogram: 0 percent of pixels dark, 0 percent middle, 100 percent light");
    // Pick the first from the keyboard.
    let first = h.get_by_label("Dark: 4 by 4, 8 samples");
    first.focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().pictures.selected, 0);
    h.get_by_label("Histogram: 100 percent of pixels dark, 0 percent middle, 0 percent light");
    // Compare with the other: the numbers are announced.
    h.state_mut().pictures.other = Some(1);
    h.state_mut().pictures.mode = crate::picture_viewer::Mode::Difference;
    h.run();
    h.run();
    h.get_by_label("Mean difference 62.75 percent, largest 62.7 percent, 100.0 percent of pixels differ");
    h.get_by_role_and_label(Role::SpinButton, "Difference gain");
    // Pictures of different sizes are not compared.
    h.state_mut().pictures.add(Picture { size: [2, 2], pixels: vec![0; 16], ..picture("Small", 0) });
    h.state_mut().pictures.other = Some(0);
    h.run();
    h.run();
    h.get_by_label("The pictures differ in size, so they cannot be compared.");
    // Esc closes the window.
    h.get_by_role_and_label(Role::Button, "Clear History").focus();
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().pictures.open);
}

#[test]
fn a_finished_render_view_picture_is_kept_in_the_history() {
    let mut h = harness();
    h.state_mut().render_view.width = 64;
    h.state_mut().render_view.height = 48;
    h.state_mut().render_view.samples = 4;
    h.run();
    h.key_press(Key::F9);
    for _ in 0..1500 {
        h.step();
        if !h.state().render_view.rendering() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.step();
    h.step();
    assert_eq!(h.state().pictures.history.len(), 1);
    let p = h.state().pictures.history.get(0).unwrap();
    assert_eq!((p.size, p.samples, p.label.as_str()), ([64, 48], 4, "Render 1"));
}
