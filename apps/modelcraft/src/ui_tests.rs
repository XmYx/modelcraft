//! UI tests with egui_kittest, driven by the keyboard. Every `run()` also checks that each input
//! widget has an accessible name (house rule: accessibility).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use crate::app::{ModelApp, SelectMode, Workspace};
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

/// Open the command palette with Ctrl+K, type, press Enter.
fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

#[test]
fn command_palette_adds_a_cube_and_the_tree_shows_it() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    assert_eq!(h.state().doc.scene.len(), 1);
    assert!(h.state().palette.is_none(), "Enter closes the palette");
    let item = h.get_by_role_and_label(Role::TreeItem, "Cube");
    assert!(item.accesskit_node().is_selected().unwrap_or(false), "the new cube is selected in the tree");
    // The viewport publishes the object as a selected list item for screen readers.
    let vp = h.get_by_label_contains("Viewport, Perspective");
    assert!(vp.accesskit_node().label().is_some_and(|l| l.contains("Cube selected")));
    assert!(h.get_all_by_role(Role::ListItem).any(|n| n.accesskit_node().label().as_deref() == Some("Cube") && n.accesskit_node().is_selected() == Some(true)));
}

#[test]
fn keyboard_modeling_builds_history_and_undoes_it() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    h.key_press(Key::Num4); // face mode
    h.run();
    assert_eq!(h.state().mode, SelectMode::Face);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    assert_eq!(h.state().components.len(), 6);
    h.key_press_modifiers(Modifiers::SHIFT, Key::E); // extrude
    h.run();
    let id = h.state().selection[0];
    assert_eq!(h.state().doc.scene.get(id).unwrap().steps().len(), 1);
    // The history panel lists the step with an accessible name.
    h.get_by_label("Step 1: Extrude");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().steps().len(), 0);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.get(id).unwrap().steps().len(), 1);
}

#[test]
fn numeric_transform_replaces_the_gizmo_drag() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Torus");
    h.key_press_modifiers(Modifiers::COMMAND, Key::T);
    h.run();
    assert!(h.state().numeric.is_some());
    h.get_by_label("Move X").set_value_accesskit(2.5);
    h.run();
    h.get_by_label("Apply").click();
    h.run();
    let id = h.state().selection[0];
    assert!((h.state().doc.scene.get(id).unwrap().transform.translation.x - 2.5).abs() < 1e-5);
    // The properties panel takes typed values too: one undo step per edit.
    let before = h.state().doc.undo.len();
    h.get_by_label("Position Y").set_value_accesskit(1.25);
    h.run();
    assert!((h.state().doc.scene.get(id).unwrap().transform.translation.y - 1.25).abs() < 1e-5);
    assert_eq!(h.state().doc.undo.len(), before + 1);
}

#[test]
fn tree_moves_selection_with_arrow_keys() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Sphere");
    h.get_by_role_and_label(Role::TreeItem, "Cube").focus();
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    h.run();
    let sel = h.state().selection.clone();
    let name = h.state().doc.scene.get(sel[0]).unwrap().name.clone();
    assert_eq!(name, "Sphere");
    assert!(h.get_by_role_and_label(Role::TreeItem, "Sphere").is_focused());
}

#[test]
fn sculpt_workspace_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    assert_eq!(h.state().workspace, Workspace::Sculpt);
    assert!(h.state().doc.sculpt.is_some(), "entering the workspace starts a sculpt session");
    h.key_press(Key::C);
    h.run();
    assert_eq!(h.state().brush.kind, modelcraft_engine::sculpt::BrushKind::Clay);
    let r = h.state().brush_px;
    h.key_press(Key::CloseBracket);
    h.run();
    assert!(h.state().brush_px > r);
    h.key_press(Key::X);
    h.run();
    assert!(h.state().brush.symmetry.x);
    h.get_by_label("Mask All").click();
    h.run();
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.mask.iter().all(|m| *m == 1.0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.mask.iter().all(|m| *m == 0.0));
}

#[test]
fn four_views_and_focus_navigation() {
    let mut h = harness();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert!(h.state().quad);
    for v in ["Perspective", "Front", "Right", "Top"] {
        h.get_by_label_contains(&format!("Viewport, {v}"));
    }
    // F6 moves focus between the panels.
    h.key_press(Key::F6);
    h.run();
    h.run();
    assert!(h.root().query_all(egui_kittest::kittest::by()).any(|n| n.is_focused()));
}

#[test]
fn new_scene_asks_before_discarding_changes() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    assert!(h.state().doc.dirty);
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run();
    assert!(h.state().pending.is_some(), "unsaved changes open the prompt");
    assert_eq!(h.state().doc.scene.len(), 1);
    // Esc cancels and keeps the work.
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().pending.is_none());
    assert_eq!(h.state().doc.scene.len(), 1);
    // Don't Save discards.
    h.key_press_modifiers(Modifiers::COMMAND, Key::N);
    h.run();
    h.get_by_label("Don't Save").click();
    h.run();
    assert!(h.state().doc.scene.is_empty());
}

#[test]
fn undo_history_panel_jumps_back_and_forth() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Sphere");
    palette(&mut h, "Add Torus");
    assert_eq!(h.state().doc.scene.len(), 3);
    h.get_by_label("History step 1: Add cube").click();
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1);
    h.get_by_label("Undone step 3: Add torus").click();
    h.run();
    assert_eq!(h.state().doc.scene.len(), 3);
}

#[test]
fn keyboard_selection_tools_and_new_mesh_steps() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Torus");
    let id = h.state().selection[0];
    h.key_press(Key::Num3); // edge mode
    h.run();
    h.state_mut().components = [0].into_iter().collect();
    h.key_press_modifiers(Modifiers::ALT, Key::L);
    h.run();
    let n = h.state().components.len();
    assert!(n == 32 || n == 16, "a torus loop, got {n}");
    // Face mode: grow, then bridge two faces, then mirror.
    h.key_press(Key::Num4);
    h.run();
    h.state_mut().components = [0].into_iter().collect();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Equals);
    h.run();
    assert_eq!(h.state().components.len(), 9);
    h.key_press_modifiers(Modifiers::SHIFT, Key::X);
    h.run();
    let steps = h.state().doc.scene.get(id).unwrap().steps().to_vec();
    assert!(matches!(steps.last().map(|s| &s.op), Some(modelcraft_engine::scene::StepOp::Mirror { axis: 0, .. })));
    // Snapping toggles from the keyboard.
    h.key_press_modifiers(Modifiers::SHIFT, Key::S);
    h.run();
    assert!(h.state().snap);
}

#[test]
fn shortcuts_dialog_opens_from_the_palette() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Keyboard Shortcuts");
    h.run();
    assert!(h.state().shortcuts_state.is_some());
}

#[test]
fn incremental_save_names() {
    use crate::app::next_increment;
    use std::path::Path;
    let d = std::env::temp_dir().join(format!("mc-incr-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    assert_eq!(next_increment(&d.join("scene.modelcraft")), d.join("scene_v002.modelcraft"));
    assert_eq!(next_increment(&d.join("scene_v009.modelcraft")), d.join("scene_v010.modelcraft"));
    std::fs::write(d.join("a_v002.modelcraft"), b"x").unwrap();
    assert_eq!(next_increment(&d.join("a.modelcraft")), d.join("a_v003.modelcraft"));
    let _ = std::fs::remove_dir_all(&d);
    let _ = Path::new("");
}

#[test]
fn detail_levels_and_shape_layers_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    let id = h.state().selection[0];
    let base = h.state().doc.sculpt.as_ref().unwrap().mesh.vertex_count();
    h.get_by_label_contains("Level 0,");
    h.key_press(Key::PageUp);
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().level, 1);
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.vertex_count() > base * 3);
    h.get_by_label_contains("Level 1,");
    assert_eq!(h.state().status, "Detail level up", "the status bar (a live region) announces the change");
    h.key_press(Key::PageDown);
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().level, 0);
    h.get_by_label("1 finer levels kept");
    // Shape layers: add from the palette, set the strength by value, select, remove.
    palette(&mut h, "Add Shape Layer");
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().shapes.len(), 1);
    let radio = h.get_by_role_and_label(Role::RadioButton, "Shape 1, shape layer");
    assert_eq!(radio.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "the new layer records strokes");
    let before = h.state().doc.undo.len();
    h.get_by_label("Strength of Shape 1").set_value_accesskit(0.25);
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().shapes[0].strength, 0.25);
    assert_eq!(h.state().doc.undo.len(), before + 1, "one undo step per edit");
    h.get_by_role_and_label(Role::RadioButton, "Base sculpt").click();
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().active_shape, None);
    // Stepping down is refused while shape layers exist, and says why.
    h.key_press(Key::PageUp);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().level, 1);
    assert!(h.state().status.contains("merge or remove the shape layers"), "{}", h.state().status);
    h.get_by_label("Remove Shape 1").click();
    h.run();
    assert!(h.state().doc.sculpt_layer(id).unwrap().shapes.is_empty());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.sculpt_layer(id).unwrap().shapes.len(), 1);
    assert!(h.state().doc.sculpt.is_some(), "undo keeps the sculpt session");
}

#[test]
fn live_detail_and_even_remesh_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    let id = h.state().selection[0];
    // Live detail: a checkbox with a keyboard toggle; the size is a number field.
    let boxed = h.get_by_role_and_label(Role::CheckBox, "Live Detail");
    assert_eq!(boxed.accesskit_node().toggled(), Some(egui::accesskit::Toggled::False));
    h.key_press_modifiers(Modifiers::SHIFT, Key::L);
    h.run();
    assert!(h.state().brush.live_detail.is_some());
    assert_eq!(h.get_by_role_and_label(Role::CheckBox, "Live Detail").accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
    assert!(h.state().status.starts_with("Live detail on"));
    h.get_by_role_and_label(Role::SpinButton, "Detail size").set_value_accesskit(40.0);
    h.run();
    assert!((h.state().brush.live_detail.unwrap() - 0.4).abs() < 1e-5);
    // Changing brush keeps it on.
    h.key_press(Key::C);
    h.run();
    assert!(h.state().brush.live_detail.is_some());
    // With a finer level kept, live detail explains why it stays off.
    h.key_press(Key::PageUp);
    h.run();
    h.get_by_label_contains("live detail works on the base detail level only");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    // Even remesh adds a history step and sculpting carries on.
    let before = h.state().doc.sculpt.as_ref().unwrap().mesh.triangle_count();
    palette(&mut h, "Even Remesh");
    let steps = h.state().doc.scene.get(id).unwrap().steps().to_vec();
    assert!(steps.iter().any(|s| matches!(s.op, modelcraft_engine::scene::StepOp::EvenRemesh { .. })));
    assert!(h.state().doc.sculpt.is_some(), "a new sculpt layer sits on the remeshed surface");
    assert!(h.state().status.starts_with("Even remesh:"), "{}", h.state().status);
    assert_ne!(h.state().doc.sculpt.as_ref().unwrap().mesh.triangle_count(), before);
}

#[test]
fn brush_presets_stroke_modes_and_new_brushes_by_keyboard() {
    use modelcraft_engine::sculpt::{BrushKind, Stamp, StrokeMode};
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    // A new brush from its key, and one from the palette.
    h.key_press(Key::H);
    h.run();
    assert_eq!(h.state().brush.kind, BrushKind::Pull);
    palette(&mut h, "Surface Grab Brush");
    assert_eq!(h.state().brush.kind, BrushKind::SurfaceGrab);
    // A built-in preset sets the brush, its curve and its stroke mode; the radius stays.
    let radius = h.state().brush.radius;
    h.get_by_label("Hard edge, brush preset").click();
    h.run();
    let b = h.state().brush.clone();
    assert_eq!((b.kind, b.radius), (BrushKind::Gouge, radius));
    assert!(matches!(b.stroke, StrokeMode::Trail { .. }));
    assert_eq!(h.state().status, "Hard edge preset");
    h.get_by_label("Trail leash");
    // Hardness is a named slider; the stroke mode is a named combo box.
    h.get_by_label("Brush hardness").set_value_accesskit(0.6);
    h.run();
    assert!((h.state().brush.hardness - 0.6).abs() < 1e-5);
    h.get_by_role_and_label(Role::ComboBox, "Stroke").click();
    h.run();
    h.get_by_label("Drag out").click();
    h.run();
    assert_eq!(h.state().brush.stroke, StrokeMode::Drag);
    // A picture stamp shows its turn control.
    h.get_by_role_and_label(Role::ComboBox, "Stamp").click();
    h.run();
    h.run();
    h.get_by_label("Scales").click();
    h.run();
    assert!(matches!(h.state().brush.stamp, Stamp::Image { .. }));
    h.get_by_role_and_label(Role::Slider, "Stamp turn");
    // Save the brush as a preset by typing a name and pressing Enter; remove it with Delete.
    let field = h.get_by_role_and_label(Role::TextInput, "Preset name");
    field.focus();
    h.run();
    h.event(egui::Event::Text("Rock chips".to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().user_presets.len(), 1);
    assert_eq!(h.state().status, "Saved the preset Rock chips");
    assert_eq!(h.state().user_presets[0].brush.stroke, StrokeMode::Drag);
    h.key_press(Key::D);
    h.run();
    h.get_by_label("Rock chips, your brush preset").click();
    h.run();
    assert_eq!(h.state().brush.kind, BrushKind::Gouge);
    h.get_by_label("Rock chips, your brush preset").focus();
    h.run();
    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().user_presets.is_empty());
    assert_eq!(h.state().status, "Removed the preset Rock chips");
}

#[test]
fn undo_history_reaches_back_into_the_last_session() {
    let d = std::env::temp_dir().join(format!("mc-ui-history-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let file = d.join("kept.modelcraft");
    {
        let mut h = harness();
        h.run();
        palette(&mut h, "Add Cube");
        palette(&mut h, "Add Sphere");
        h.state_mut().doc.save_as(&file).unwrap();
    }
    let mut h = harness();
    h.run();
    h.state_mut().open_path(file.clone());
    h.run();
    assert!(h.state().status.contains("Restored 2 undo steps"), "{}", h.state().status);
    // The history panel lists the earlier session's steps, and the keyboard undoes them.
    assert!(h.query_by_label("History step 2: Add sphere").is_some());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn agents_drive_the_window_over_the_control_channel() {
    use std::io::{BufRead, BufReader, Write};
    let mut h = harness();
    h.run();
    let token = "feedfacefeedfacefeedfacefeedfacefeedface";
    h.state_mut().agents.token = Some(token.into());
    h.state_mut().agents.port = Some(0);
    // Off by default; the Edit menu item turns it on.
    assert!(!h.state().agents.is_on());
    h.get_by_label("Edit").click();
    h.run();
    h.get_by_label("Allow Agents to Control ModelCraft").click();
    h.run();
    assert!(h.state().agents.is_on(), "{}", h.state().status);
    assert!(h.state().status.contains("on port"), "the status bar says where: {}", h.state().status);
    let port = h.state().agents.listening.as_ref().unwrap().port;
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    writeln!(s, r#"{{"id":1,"method":"engine.execute","params":{{"cmd":"add_primitive","kind":"cylinder"}},"token":"{token}"}}"#).unwrap();
    // The window answers between frames.
    let mut reader = BufReader::new(s.try_clone().unwrap());
    let mut line = String::new();
    let waiter = std::thread::spawn(move || {
        reader.read_line(&mut line).unwrap();
        line
    });
    for _ in 0..200 {
        h.run();
        if waiter.is_finished() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let reply = waiter.join().unwrap();
    assert!(reply.contains("\"ok\":true"), "{reply}");
    assert_eq!(h.state().doc.scene.len(), 1);
    assert!(h.state().status.contains("An agent changed the scene"), "announced in the status bar: {}", h.state().status);
    h.get_by_role_and_label(Role::TreeItem, "Cylinder");
    crate::agents::stop(h.state_mut());
    assert!(!h.state().agents.is_on());
}
