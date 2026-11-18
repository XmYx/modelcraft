//! Keyboard and accessibility-tree tests of the UV Editor (egui_kittest). Every pointer action has
//! a keyboard or numeric route, and these tests take only those routes.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::scene::StepOp;

use crate::app::{ModelApp, SelectMode};
use crate::args::Cli;
use crate::uv_ui::{Heat, UvSelect, UvTool};

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

/// A document with one object and the UV Editor open on it, the canvas focused.
fn editor(kind: &str) -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, &format!("Add {kind}"));
    palette(&mut h, "UV Editor");
    h.run();
    h.run();
    assert!(h.state().uv.open, "the palette opens the UV Editor");
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    assert!(h.state().uv.had_focus, "the canvas takes keyboard focus");
    h
}

fn uv0(h: &mut Harness<'static, ModelApp>, id: modelcraft_engine::scene::ObjectId) -> glam::Vec2 {
    let app = h.state_mut();
    let scene = app.doc.scene.clone();
    app.doc.eval.steps_output(&scene, id, usize::MAX).mesh.uv(0)
}

fn steps(h: &Harness<'static, ModelApp>) -> Vec<StepOp> {
    let id = h.state().selection[0];
    h.state().doc.scene.get(id).unwrap().steps().iter().map(|s| s.op.clone()).collect()
}

#[test]
fn the_editor_opens_by_keyboard_and_publishes_its_canvas_and_controls() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::U);
    h.run();
    h.run();
    assert!(h.state().uv.open);
    // The canvas names its content; the toolbar buttons and the numeric fields have names.
    let canvas = h.get_by_label_contains("UV canvas, 6 faces in 6 islands");
    assert_eq!(canvas.accesskit_node().role(), Role::Canvas);
    for name in
        ["Pin", "Unpin", "Weld", "Mark Seam", "Clear Seam", "Unwrap UVs", "Frame", "Apply transform", "Flip U", "Flip V", "Checker background", "Show seams"]
    {
        h.get_by_label(name);
    }
    for name in ["Move U", "Move V", "Rotate (degrees)", "Scale U", "Scale V", "Nudge step", "Seam angle"] {
        h.get_by_role_and_label(Role::SpinButton, name);
    }
    for mode in ["UV Vertex", "UV Edge", "UV Face", "UV Island"] {
        h.get_by_label(mode);
    }
    // The measures are text, not only colour.
    h.get_by_label_contains("Angle error: mean");
    h.get_by_label_contains("Islands: 6");
    // Escape closes the editor when nothing is selected.
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().uv.open);
}

#[test]
fn selecting_and_nudging_with_the_keyboard_adds_undoable_steps() {
    let mut h = editor("Cube");
    h.key_press(Key::Num4);
    h.run();
    assert_eq!(h.state().uv.mode, UvSelect::Island);
    // Page Down picks the first island: one cube face, four corners.
    h.key_press(Key::PageDown);
    h.run();
    assert_eq!(h.state().uv.selection.len(), 4);
    h.get_by_label_contains("UV canvas, 6 faces in 6 islands, 4 corners selected");
    let before = h.state().doc.undo.len();
    h.key_press(Key::ArrowRight);
    h.run();
    let s = steps(&h);
    let StepOp::TransformUvs { corners, translate, .. } = &s[0] else { panic!("{s:?}") };
    assert_eq!(corners.len(), 4);
    assert!((translate[0] - 0.01).abs() < 1e-6 && translate[1] == 0.0, "{translate:?}");
    // Shift moves ten times as far, Down moves along v.
    h.key_press_modifiers(Modifiers::SHIFT, Key::ArrowDown);
    h.run();
    let s = steps(&h);
    let StepOp::TransformUvs { translate, .. } = &s[1] else { panic!("{s:?}") };
    assert!(translate[0] == 0.0 && (translate[1] - 0.1).abs() < 1e-6, "{translate:?}");
    // The layout really moved, and undo puts it back.
    let id = h.state().selection[0];
    let moved = uv0(&mut h, id);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 1, "one undo removes the last step; status: {}", h.state().status);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(steps(&h).is_empty(), "both steps are undone");
    assert_eq!(h.state().doc.undo.len(), before);
    // Redo brings one back (Cmd+Shift+Z from the canvas too).
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 1);
    let back = uv0(&mut h, id);
    assert!((moved - back).length() > 0.005, "{moved} {back}");
}

#[test]
fn select_all_rotate_and_scale_by_keys_and_clear_with_escape() {
    let mut h = editor("Cube");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    assert_eq!(h.state().uv.selection.len(), 24);
    h.key_press(Key::CloseBracket);
    h.run();
    h.key_press(Key::Period);
    h.run();
    let s = steps(&h);
    assert!(matches!(&s[0], StepOp::TransformUvs { rotate_deg, .. } if (*rotate_deg - 15.0).abs() < 1e-6));
    assert!(matches!(&s[1], StepOp::TransformUvs { scale, .. } if (scale[0] - 1.1).abs() < 1e-6));
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().uv.selection.is_empty());
    assert!(h.state().uv.open, "the first Escape only clears the selection");
    // Nothing selected: keys that edit say so and add no step.
    h.key_press(Key::ArrowLeft);
    h.run();
    assert_eq!(steps(&h).len(), 2);
    assert_eq!(h.state().status, "Select UVs first");
}

#[test]
fn numeric_fields_replace_dragging() {
    let mut h = editor("Cube");
    h.key_press(Key::Num4);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Move U").set_value_accesskit(0.25);
    h.get_by_role_and_label(Role::SpinButton, "Rotate (degrees)").set_value_accesskit(90.0);
    h.run();
    h.get_by_label("Apply transform").click();
    h.run();
    let s = steps(&h);
    assert_eq!(s.len(), 1);
    let StepOp::TransformUvs { translate, rotate_deg, scale, .. } = &s[0] else { panic!("{s:?}") };
    assert!((translate[0] - 0.25).abs() < 1e-6 && (*rotate_deg - 90.0).abs() < 1e-6 && *scale == [1.0, 1.0], "{translate:?} {rotate_deg} {scale:?}");
    // The fields reset after applying; flipping is a button too.
    h.get_by_label("Flip U").click();
    h.run();
    let s = steps(&h);
    assert!(matches!(&s[1], StepOp::TransformUvs { scale, .. } if *scale == [-1.0, 1.0]));
}

#[test]
fn modes_and_tools_have_keys_and_buttons() {
    let mut h = editor("Cube");
    for (key, mode) in [(Key::Num1, UvSelect::Vertex), (Key::Num2, UvSelect::Edge), (Key::Num3, UvSelect::Face), (Key::Num4, UvSelect::Island)] {
        h.key_press(key);
        h.run();
        assert_eq!(h.state().uv.mode, mode);
    }
    for (key, tool) in [(Key::Q, UvTool::Select), (Key::W, UvTool::Move), (Key::E, UvTool::Rotate), (Key::R, UvTool::Scale)] {
        h.key_press(key);
        h.run();
        assert_eq!(h.state().uv.tool, tool);
    }
    // The app's own 1..4 (object / vertex / edge / face) did not fire while the canvas had focus.
    assert_eq!(h.state().mode, SelectMode::Object);
    h.get_by_label("UV Face").click();
    h.run();
    assert_eq!(h.state().uv.mode, UvSelect::Face);
    h.get_by_label("Rotate tool").click();
    h.run();
    assert_eq!(h.state().uv.tool, UvTool::Rotate);
    // Page Down walks the elements and wraps; Page Up goes back. Clicking a button took the focus.
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    h.key_press(Key::Num3);
    h.run();
    for _ in 0..7 {
        h.key_press(Key::PageDown);
        h.run();
    }
    assert_eq!(h.state().uv.selection.len(), 4);
    h.key_press(Key::PageUp);
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::PageDown);
    h.run();
    assert_eq!(h.state().uv.selection.len(), 8, "Shift adds the next face");
}

#[test]
fn seams_pins_and_unwrap_go_through_the_history() {
    let mut h = editor("Plane");
    // A welded grid: pick an edge by keyboard and mark it as a seam.
    h.key_press(Key::Num2);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    assert!(!h.state().uv.selection.is_empty());
    h.key_press(Key::M);
    h.run();
    let s = steps(&h);
    assert!(matches!(&s[0], StepOp::Seams { edges } if edges.len() == 1), "{s:?}");
    // The same edge again: nothing new, and the status says so.
    h.key_press(Key::M);
    h.run();
    assert_eq!(steps(&h).len(), 1);
    assert_eq!(h.state().status, "Those edges are already seams");
    // Shift+M clears it again, into the same Seams step.
    h.key_press_modifiers(Modifiers::SHIFT, Key::M);
    h.run();
    let s = steps(&h);
    assert_eq!(s.len(), 1);
    assert!(matches!(&s[0], StepOp::Seams { edges } if edges.is_empty()));
    // Mark it again, pin a vertex, and unwrap: the step carries the seam and the pin.
    h.key_press(Key::M);
    h.run();
    h.key_press(Key::Num1);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    h.key_press(Key::P);
    h.run();
    assert!(!h.state().uv.pinned.is_empty());
    h.get_by_label_contains("Pinned: ");
    h.key_press(Key::U);
    h.run();
    let s = steps(&h);
    let StepOp::Unwrap { seams, pins, auto_cut, .. } = s.last().unwrap() else { panic!("{s:?}") };
    assert_eq!(seams.len(), 1);
    assert!(!pins.is_empty() && *auto_cut);
    // Unpinning is Shift+P.
    h.key_press(Key::PageDown);
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::P);
    h.run();
}

#[test]
fn welding_joins_the_two_sides_of_a_uv_seam() {
    let mut h = harness();
    h.run();
    // Two triangles that share an edge but not its UVs: a seam inside a connected mesh.
    let dir = std::env::temp_dir().join(format!("mc-uv-ui-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let obj = dir.join("seam.obj");
    std::fs::write(
        &obj,
        "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nvt 0 0\nvt 1 0\nvt 0 1\nvt 0.2 0.2\nvt 0.9 0.2\nvt 0.2 0.9\nvt 1 1\nf 1/1 2/2 3/3\nf 2/5 4/7 3/6\n",
    )
    .unwrap();
    assert!(h.state_mut().run(Command::Import { path: obj.display().to_string(), up: Default::default(), scale: None }));
    palette(&mut h, "UV Editor");
    h.run();
    h.run();
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    h.key_press(Key::J);
    h.run();
    let s = steps(&h);
    assert!(matches!(s.last(), Some(StepOp::StitchUvs { edges }) if !edges.is_empty()), "{s:?}");
    assert!(h.state().uv.selection.is_empty(), "the selection is cleared after a weld");
}

#[test]
fn heat_map_and_checker_are_display_options_with_numbers_beside_them() {
    let mut h = editor("Cube");
    assert!(h.state().uv.checker);
    h.get_by_label("Checker background").click();
    h.run();
    assert!(!h.state().uv.checker);
    h.state_mut().uv.heat = Heat::Angle;
    h.run();
    h.run();
    // The legend explains the colours in words.
    h.get_by_label_contains("0°");
    h.get_by_label_contains("Stretch: mean");
    h.get_by_label_contains("Mirrored triangles: 0");
    // Colours are monotonic and cover the range; non-finite input is safe.
    let c: Vec<_> = (0..=10).map(|i| crate::uv_ui::heat_color(i as f32 / 10.0)).collect();
    assert_ne!(c[0], c[10]);
    assert_eq!(crate::uv_ui::heat_color(f32::NAN), crate::uv_ui::heat_color(0.0));
    for hm in Heat::ALL {
        let m = modelcraft_engine::uv::TriangleMetric { face: 0, angle_error_deg: 5.0, stretch: 1.5, area_ratio: 2.0, flipped: false };
        let v = crate::uv_ui::heat_value(hm, &m);
        assert!((0.0..=1.0).contains(&v), "{hm:?}: {v}");
    }
}

#[test]
fn the_editor_follows_the_selection_and_survives_a_mesh_without_uvs() {
    let mut h = harness();
    h.run();
    palette(&mut h, "UV Editor");
    h.run();
    h.get_by_label_contains("Select a mesh object");
    palette(&mut h, "Add Cube");
    h.run();
    h.run();
    h.get_by_label_contains("UV canvas, 6 faces");
    // A mesh that has no UVs offers to unwrap.
    let dir = std::env::temp_dir().join(format!("mc-uv-ui2-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let obj = dir.join("bare.obj");
    std::fs::write(&obj, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
    assert!(h.state_mut().run(Command::Import { path: obj.display().to_string(), up: Default::default(), scale: None }));
    h.run();
    h.run();
    h.get_by_label_contains("no UVs yet");
    h.get_by_label("Unwrap UVs").click();
    h.run();
    h.run();
    assert!(matches!(steps(&h).last(), Some(StepOp::Unwrap { .. })));
    h.get_by_label_contains("UV canvas, 1 faces");
}

fn canvas_rect(h: &Harness<'static, ModelApp>) -> egui::Rect {
    h.get_by_label_contains("UV canvas").rect()
}

#[test]
fn picking_and_box_selection_follow_the_selection_mode() {
    let mut h = editor("Plane");
    let rect = canvas_rect(&h);
    let app = h.state_mut();
    let (_, mesh) = crate::uv_ui::subject(app).unwrap();
    let d = crate::uv_ui::derived(app, &mesh);
    let at = |app: &ModelApp, c: u32| crate::uv_ui::to_screen(&app.uv, rect, mesh.uv(c));
    // Vertex mode: the corner under the pointer, with every corner that shares its UV vertex.
    let inner = (0..mesh.corner_count() as u32).find(|c| mesh.origin(*c) == 12).unwrap();
    let hit = crate::uv_ui::pick(app, rect, &d, at(app, inner) + egui::vec2(2.0, 1.0)).unwrap();
    assert_eq!(hit.len(), 4, "an inner vertex of the grid has four corners");
    assert!(crate::uv_ui::pick(app, rect, &d, rect.left_top()).is_none(), "empty space picks nothing");
    // Face mode: the face under the pointer, four corners; island mode: the whole plane.
    app.uv.mode = UvSelect::Face;
    let centre = (0..4).map(|k| mesh.uv(mesh.face_half_edge(0) + k)).fold(glam::Vec2::ZERO, |a, b| a + b) / 4.0;
    let p = crate::uv_ui::to_screen(&app.uv, rect, centre);
    assert_eq!(crate::uv_ui::pick(app, rect, &d, p).unwrap().len(), 4);
    app.uv.mode = UvSelect::Island;
    assert_eq!(crate::uv_ui::pick(app, rect, &d, p).unwrap().len(), mesh.corner_count());
    // Edge mode picks near an edge's middle and takes both ends.
    app.uv.mode = UvSelect::Edge;
    let h0 = mesh.face_half_edge(0);
    let mid = 0.5 * (mesh.uv(h0) + mesh.uv(mesh.next(h0)));
    assert!(crate::uv_ui::pick(app, rect, &d, crate::uv_ui::to_screen(&app.uv, rect, mid)).unwrap().len() >= 2);
    // Box selection in vertex mode: everything in a box around the whole layout, nothing in an empty one.
    app.uv.mode = UvSelect::Vertex;
    let all = egui::Rect::from_min_max(rect.left_top(), rect.right_bottom());
    assert_eq!(crate::uv_ui::in_box(app, rect, &d, all).len(), mesh.corner_count());
    let none = egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, 2.0));
    assert!(crate::uv_ui::in_box(app, rect, &d, none).is_empty());
}

#[test]
fn the_pointer_drags_the_selection_and_clicks_select() {
    let mut h = editor("Cube");
    // Select an island by the keyboard, then drag it with the Move tool.
    h.key_press(Key::Num4);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    let rect = canvas_rect(&h);
    let (zoom, origin) = (h.state().uv.zoom, h.state().uv.origin);
    let c = {
        let app = h.state_mut();
        let (_, m) = crate::uv_ui::subject(app).unwrap();
        let cs: Vec<_> = app.uv.selection.iter().map(|c| m.uv(*c)).collect();
        cs.iter().fold(glam::Vec2::ZERO, |a, b| a + *b) / cs.len() as f32
    };
    let start = egui::pos2(rect.left() + (c.x - origin.x) * zoom, rect.top() + (c.y - origin.y) * zoom);
    let end = start + egui::vec2(40.0, 0.0);
    h.event(egui::Event::PointerMoved(start));
    h.run();
    h.event(egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.run();
    for k in 1..=4 {
        h.event(egui::Event::PointerMoved(start + egui::vec2(10.0 * k as f32, 0.0)));
        h.run();
    }
    h.event(egui::Event::PointerButton { pos: end, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run();
    let s = steps(&h);
    let StepOp::TransformUvs { translate, corners, .. } = s.last().expect("a transform step") else { panic!("{s:?}") };
    assert_eq!(corners.len(), 4);
    assert!((translate[0] - 40.0 / zoom).abs() < 1.5 / zoom, "moved {} for 40 px at {zoom} px per UV", translate[0]);
    assert!(translate[1].abs() < 1e-4);
    // A click on empty canvas clears the selection.
    let empty = rect.left_top() + egui::vec2(3.0, 3.0);
    h.event(egui::Event::PointerMoved(empty));
    h.run();
    h.event(egui::Event::PointerButton { pos: empty, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: empty, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run();
    h.run();
    assert!(h.state().uv.selection.is_empty());
}

#[test]
fn seams_show_in_the_viewport_overlay() {
    let mut h = editor("Plane");
    h.key_press(Key::Num2);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    h.key_press(Key::PageDown);
    h.run();
    h.key_press(Key::M);
    h.run();
    let lines = crate::uv_ui::seam_lines(h.state_mut());
    assert_eq!(lines.len(), 2, "one seam is one line of two vertices");
    assert!(lines.iter().all(|v| v.color[0] > 0.8 && v.color[1] < 0.4), "seams are red");
}

#[test]
fn projecting_packing_and_exporting_are_buttons_keys_and_menu_entries() {
    let mut h = editor("Plane");
    for name in ["Pack UV Islands", "Project UVs: Planar", "Project UVs: Box", "Project UVs: Cylindrical", "Project UVs: Spherical", "Export UV Layout…"] {
        h.get_by_label(name);
    }
    h.get_by_label("Project UVs: Box").click();
    h.run();
    let s = steps(&h);
    assert!(matches!(s.last(), Some(StepOp::ProjectUvs { projection: modelcraft_engine::scene::Projection::Box, faces, .. }) if faces.is_empty()), "{s:?}");
    // K packs from the canvas; the global shortcut and the palette reach the same command.
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    h.key_press(Key::K);
    h.run();
    assert!(matches!(steps(&h).last(), Some(StepOp::PackUvs { .. })));
    palette(&mut h, "Project UVs: Spherical");
    assert!(matches!(steps(&h).last(), Some(StepOp::ProjectUvs { projection: modelcraft_engine::scene::Projection::Spherical { .. }, .. })));
    // The mesh really has a new layout, inside the unit square.
    let id = h.state().selection[0];
    let app = h.state_mut();
    let scene = app.doc.scene.clone();
    let m = app.doc.eval.steps_output(&scene, id, usize::MAX).mesh;
    assert!(m.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-4 && u.max_element() <= 1.0 + 1e-4));
}

#[test]
fn a_mesh_without_uvs_gets_them_from_a_projection() {
    let mut h = harness();
    h.run();
    let dir = std::env::temp_dir().join(format!("mc-uv-ui3-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let obj = dir.join("bare.obj");
    std::fs::write(&obj, "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 4 3\n").unwrap();
    assert!(h.state_mut().run(Command::Import { path: obj.display().to_string(), up: Default::default(), scale: None }));
    palette(&mut h, "UV Editor");
    h.run();
    h.run();
    h.get_by_label_contains("no UVs yet");
    palette(&mut h, "Project UVs: Planar");
    h.run();
    h.run();
    h.get_by_label_contains("UV canvas, 1 faces");
}

#[test]
fn the_angle_based_solver_is_a_menu_entry_and_a_choice_in_the_toolbar() {
    let mut h = editor("Sphere");
    h.get_by_role_and_label(Role::ComboBox, "Unwrap solver");
    palette(&mut h, "Unwrap UVs (Angle-Based)");
    let s = steps(&h);
    assert!(matches!(s.last(), Some(StepOp::Unwrap { method: modelcraft_engine::scene::UnwrapMethod::Abf, .. })), "{s:?}");
    let id = h.state().selection[0];
    let app = h.state_mut();
    let scene = app.doc.scene.clone();
    let out = app.doc.eval.steps_output(&scene, id, usize::MAX);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert!(out.mesh.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-4 && u.max_element() <= 1.0 + 1e-4));
    // The toolbar choice decides what the plain Unwrap button and shortcut do.
    h.state_mut().uv.unwrap_method = modelcraft_engine::scene::UnwrapMethod::Abf;
    h.get_by_label_contains("UV canvas").focus();
    h.run();
    h.key_press(Key::U);
    h.run();
    let s = steps(&h);
    assert!(matches!(s.last(), Some(StepOp::Unwrap { method: modelcraft_engine::scene::UnwrapMethod::Abf, .. })), "{s:?}");
    assert_eq!(s.iter().filter(|o| matches!(o, StepOp::Unwrap { .. })).count(), 2);
}
