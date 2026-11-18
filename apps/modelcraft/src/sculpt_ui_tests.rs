//! UI tests for sculpting through the pointer and the mask tools. They live apart from
//! `ui_tests.rs` so that workstreams appending tests there do not collide with these.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use crate::app::ModelApp;
use crate::ui_tests::{harness, palette};

#[test]
fn mask_tools_by_keyboard_and_buttons() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    let count = |h: &Harness<'static, ModelApp>| h.state().doc.sculpt.as_ref().unwrap().mesh.mask.iter().filter(|m| **m > 0.0).count();
    h.key_press(Key::M);
    h.run();
    // Mask one patch from the engine, then grow it from the keyboard and blur it from a button.
    let id = h.state().selection[0];
    h.state_mut().run(modelcraft_engine::Command::SculptMask { id, op: modelcraft_engine::MaskOp::Patch { triangle: 0 } });
    h.run();
    let start = count(&h);
    assert!(start > 0);
    h.key_press_modifiers(Modifiers::COMMAND, Key::CloseBracket);
    h.run();
    let grown = count(&h);
    assert!(grown > start);
    assert_eq!(h.state().status, "Mask");
    h.key_press_modifiers(Modifiers::COMMAND, Key::OpenBracket);
    h.run();
    assert_eq!(count(&h), start);
    h.get_by_role_and_label(Role::Button, "Blur Mask").click();
    h.run();
    assert!(count(&h) > start);
    for name in ["Sharpen Mask", "Grow Mask", "Shrink Mask", "Mask Hollows", "Mask Ridges"] {
        h.get_by_role_and_label(Role::Button, name);
    }
    palette(&mut h, "Mask Ridges");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(count(&h) > start, "undo brings the blurred mask back");
}

/// Drag the primary button across the viewport, from `from` to `to` in screen points.
fn drag(h: &mut Harness<'static, ModelApp>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for k in 1..=12 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
        h.step();
    }
    h.event(button(to, false));
    h.step();
    h.step();
}

#[test]
fn a_pointer_drag_in_the_viewport_sculpts_in_every_stroke_mode() {
    use modelcraft_engine::sculpt::{BrushKind, StrokeMode};
    let modes = [StrokeMode::Spaced, StrokeMode::Dots, StrokeMode::Spray { scatter: 0.5 }, StrokeMode::Trail { leash: 0.5 }, StrokeMode::Drag];
    let cases = modes.iter().map(|m| (BrushKind::Draw, *m, false)).chain([
        (BrushKind::Pull, StrokeMode::Spaced, false),
        (BrushKind::SurfaceGrab, StrokeMode::Spaced, false),
        (BrushKind::Slide, StrokeMode::Trail { leash: 0.5 }, false),
        (BrushKind::Clay, StrokeMode::Dots, false),
        (BrushKind::Draw, StrokeMode::Spaced, true),
    ]);
    // One drag per fresh window: the pointer path is the same every time.
    for (kind, mode, live) in cases {
        let mut h = harness();
        h.run();
        palette(&mut h, "Add Quad sphere");
        h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
        h.run();
        if live {
            h.key_press_modifiers(Modifiers::SHIFT, Key::L);
        }
        h.state_mut().brush.kind = kind;
        h.state_mut().brush.stroke = mode;
        h.run();
        let rect = h.get_by_label_contains("Viewport, Perspective").rect();
        let (a, b) = (rect.center() - egui::vec2(30.0, 0.0), rect.center() + egui::vec2(30.0, 10.0));
        let before = h.state().doc.sculpt.as_ref().unwrap().mesh.positions.clone();
        let (undo, tris) = (h.state().doc.undo.len(), h.state().doc.sculpt.as_ref().unwrap().mesh.triangle_count());
        drag(&mut h, a, b);
        let mesh = &h.state().doc.sculpt.as_ref().unwrap().mesh;
        assert!(before.iter().zip(&mesh.positions).any(|(x, y)| x != y), "{kind:?} with {mode:?} moved nothing");
        assert!(mesh.positions.iter().all(|p| p.is_finite()));
        assert_eq!(mesh.triangle_count() > tris, live, "only live detail adds triangles");
        assert_eq!(h.state().doc.undo.len(), undo + 1, "{kind:?} with {mode:?}: one stroke, one undo step");
        assert!(h.state().stroke.is_none(), "the stroke ended with the button release");
        // Undo puts the surface back.
        h.run();
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        let mesh = &h.state().doc.sculpt.as_ref().unwrap().mesh;
        assert!(before.len() == mesh.positions.len() && before.iter().zip(&mesh.positions).all(|(x, y)| x.distance(*y) < 1e-6), "{kind:?} with {mode:?}: undo");
    }
}

#[test]
fn undo_and_brush_keys_work_after_a_stroke_in_the_viewport() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    let at = h.get_by_label_contains("Viewport, Perspective").rect().center();
    let before = h.state().doc.sculpt.as_ref().unwrap().mesh.positions.clone();
    let undo = h.state().doc.undo.len();
    drag(&mut h, at, at + egui::vec2(40.0, 0.0));
    h.run();
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    // The viewport now has keyboard focus; shortcuts must still reach the app.
    assert!(h.get_by_label_contains("Viewport, Perspective").is_focused());
    h.key_press(Key::C);
    h.run();
    assert_eq!(h.state().brush.kind, modelcraft_engine::sculpt::BrushKind::Clay);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.undo.len(), undo, "Ctrl+Z undoes the stroke");
    assert_eq!(h.state().doc.sculpt.as_ref().unwrap().mesh.positions, before);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().doc.undo.len(), undo + 1, "and Ctrl+Shift+Z redoes it");
}

// ---------------------------------------------------------------- patches and visibility

fn sculpting(shape: &str) -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, shape);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run();
    h
}

fn hidden(h: &Harness<'static, ModelApp>) -> usize {
    h.state().doc.sculpt.as_ref().unwrap().mesh.hidden_triangles().iter().filter(|x| **x).count()
}

fn masked(h: &Harness<'static, ModelApp>) -> usize {
    h.state().doc.sculpt.as_ref().unwrap().mesh.mask.iter().filter(|m| **m > 0.5).count()
}

/// Drag the primary button along `path` (screen points), with `modifiers` held.
fn drag_path(h: &mut Harness<'static, ModelApp>, path: &[egui::Pos2], modifiers: Modifiers) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers };
    let (first, last) = (path[0], path[path.len() - 1]);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::PointerMoved(first));
    h.step();
    h.event(button(first, true));
    h.step();
    for pair in path.windows(2) {
        for k in 1..=6 {
            h.event(egui::Event::PointerMoved(pair[0].lerp(pair[1], k as f32 / 6.0)));
            h.step();
        }
    }
    h.event(button(last, false));
    h.step();
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.step();
}

fn clear_mask(h: &mut Harness<'static, ModelApp>) {
    let id = h.state().doc.sculpt.as_ref().unwrap().id;
    h.state_mut().run(modelcraft_engine::Command::SculptMask { id, op: modelcraft_engine::MaskOp::Clear });
    h.run();
}

/// Bring a control of the Sculpt panel into view (the panel scrolls) and click it.
fn press(h: &mut Harness<'static, ModelApp>, role: Role, label: &str) {
    h.get_by_role_and_label(role, label).scroll_to_me();
    h.run();
    h.get_by_role_and_label(role, label).click();
    h.run();
}

fn toggled(h: &Harness<'static, ModelApp>, role: Role, label: &str) -> bool {
    h.get_by_role_and_label(role, label).accesskit_node().toggled() == Some(egui::accesskit::Toggled::True)
}

#[test]
fn patches_and_visibility_by_keyboard_and_the_patch_list() {
    let mut h = sculpting("Add Cube");
    palette(&mut h, "Sculpt Patches by Angle");
    assert_eq!(h.state().status, "6 patches");
    assert!(h.state().show_patches, "making patches shows them");
    let tris = h.state().doc.sculpt.as_ref().unwrap().mesh.triangle_count();
    let per = tris / 6;
    // The patch list is the keyboard path to hiding one patch.
    let row = format!("Patch 1, {per} triangles");
    assert!(toggled(&h, Role::CheckBox, &row));
    h.get_by_role_and_label(Role::CheckBox, &row).scroll_to_me();
    h.run();
    h.get_by_role_and_label(Role::CheckBox, &row).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(hidden(&h), per);
    assert_eq!(h.state().status, "Hide patch");
    assert!(!toggled(&h, Role::CheckBox, &row));
    press(&mut h, Role::Button, "Invert Visibility");
    assert_eq!(hidden(&h), tris - per);
    press(&mut h, Role::CheckBox, &row);
    assert_eq!(hidden(&h), tris - per, "hiding the last patch that shows is refused");
    assert!(h.state().status.contains("hide everything"), "{}", h.state().status);
    // Alt+H shows everything; Shift+H and Shift+I act on the patch in the middle of the view
    // when the pointer is not over it.
    h.get_by_label_contains("Viewport, Perspective").focus();
    h.run();
    h.key_press_modifiers(Modifiers::ALT, Key::H);
    h.run();
    assert_eq!(hidden(&h), 0);
    h.key_press_modifiers(Modifiers::SHIFT, Key::H);
    h.run();
    assert_eq!(hidden(&h), per);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(hidden(&h), 0);
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    h.run();
    assert_eq!(hidden(&h), tris - per);
    palette(&mut h, "Show All");
    assert_eq!(hidden(&h), 0);
    // The other makers are there by name and say why when they cannot work.
    for name in ["Sculpt Patches by UV Island", "Sculpt Patches by Part", "Sculpt Patch from Mask", "Hide Masked", "Mask Occlusion", "Extract Shell from Mask"]
    {
        h.get_by_role_and_label(Role::Button, name);
    }
    press(&mut h, Role::Button, "Sculpt Patch from Mask");
    assert!(h.state().status.contains("nothing is masked"), "{}", h.state().status);
    palette(&mut h, "Sculpt Patches by Part");
    assert_eq!(h.state().status, "1 patches");
}

#[test]
fn rectangle_and_lasso_tools_mask_and_hide_by_dragging() {
    let mut h = sculpting("Add Quad sphere");
    let c = h.get_by_label_contains("Viewport, Perspective").rect().center();
    let before = h.state().doc.sculpt.as_ref().unwrap().mesh.positions.clone();
    h.get_by_label_contains("Viewport, Perspective").focus();
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::R);
    h.run();
    assert_eq!(h.state().sculpt_groups.tool, crate::sculpt_groups_ui::RegionTool::RectMask);
    assert!(toggled(&h, Role::RadioButton, "Rectangle Mask"));
    let undo = h.state().doc.undo.len();
    drag_path(&mut h, &[c - egui::vec2(30.0, 30.0), c + egui::vec2(30.0, 30.0)], Modifiers::NONE);
    let first = masked(&h);
    assert!(first > 0);
    assert_eq!(h.state().doc.sculpt.as_ref().unwrap().mesh.positions, before, "a region tool does not sculpt");
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    assert_eq!(h.state().status, "Mask region");
    // With Ctrl a rectangle clears what is inside it.
    drag_path(&mut h, &[c - egui::vec2(30.0, 30.0), c + egui::vec2(0.0, 30.0)], Modifiers::COMMAND);
    assert!(masked(&h) < first && masked(&h) > 0);
    // Front faces only masks fewer vertices than through the whole object.
    clear_mask(&mut h);
    press(&mut h, Role::CheckBox, "Front faces only");
    drag_path(&mut h, &[c - egui::vec2(30.0, 30.0), c + egui::vec2(30.0, 30.0)], Modifiers::NONE);
    assert!(masked(&h) > 0 && masked(&h) < first);
    // A lasso around a triangle hides what is inside; a click without a drag does nothing.
    press(&mut h, Role::RadioButton, "Lasso Hide");
    assert_eq!(h.state().sculpt_groups.tool, crate::sculpt_groups_ui::RegionTool::LassoHide);
    let undo = h.state().doc.undo.len();
    drag_path(&mut h, &[c, c + egui::vec2(1.0, 0.0)], Modifiers::NONE);
    assert_eq!((hidden(&h), h.state().doc.undo.len()), (0, undo));
    drag_path(&mut h, &[c + egui::vec2(-40.0, -30.0), c + egui::vec2(40.0, -30.0), c + egui::vec2(0.0, 40.0)], Modifiers::NONE);
    assert!(hidden(&h) > 0);
    assert_eq!(h.state().status, "Hide region");
    h.get_by_label_contains("Viewport, Perspective").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(hidden(&h), 0);
    // Back to the brush: the drag sculpts again.
    h.key_press_modifiers(Modifiers::SHIFT, Key::B);
    h.run();
    assert_eq!(h.state().sculpt_groups.tool, crate::sculpt_groups_ui::RegionTool::Brush);
    clear_mask(&mut h);
    drag_path(&mut h, &[c - egui::vec2(20.0, 0.0), c + egui::vec2(20.0, 0.0)], Modifiers::NONE);
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.positions != before);
}

#[test]
fn region_and_plane_by_numbers_replace_the_drags() {
    let mut h = sculpting("Add Quad sphere");
    h.get_by_label_contains("Viewport, Perspective").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::R);
    h.run();
    h.run();
    assert!(h.state().sculpt_groups.region.is_some());
    assert!(h.get_by_role_and_label(Role::RadioButton, "Rectangle").is_focused(), "the dialog takes the focus");
    h.get_by_label("Left of the region").set_value_accesskit(0.0);
    h.get_by_label("Right of the region").set_value_accesskit(50.0);
    h.get_by_label("Top of the region").set_value_accesskit(0.0);
    h.get_by_label("Bottom of the region").set_value_accesskit(100.0);
    h.run();
    press(&mut h, Role::Button, "Apply");
    h.run();
    assert!(h.state().sculpt_groups.region.is_none());
    let half = masked(&h);
    let all = h.state().doc.sculpt.as_ref().unwrap().mesh.vertex_count();
    assert!(half > all / 4 && half < all * 3 / 4, "{half} of {all}");
    assert!(h.get_by_label_contains("Viewport, Perspective").is_focused(), "and gives it back");
    // Esc closes without doing anything.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::R);
    h.run();
    assert!(h.state().sculpt_groups.region.is_some());
    h.key_press(Key::Escape);
    h.run();
    assert!(h.state().sculpt_groups.region.is_none());
    assert_eq!(masked(&h), half);
    // An outline typed as corners, hiding instead of masking.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::R);
    h.run();
    press(&mut h, Role::RadioButton, "Outline");
    h.get_by_label("Corners of the outline");
    h.state_mut().sculpt_groups.region.as_mut().unwrap().action = modelcraft_engine::RegionAction::Hide;
    h.run();
    press(&mut h, Role::Button, "Apply");
    assert!(hidden(&h) > 0);
    palette(&mut h, "Show All");
    clear_mask(&mut h);
    // The plane: clip moves the top onto it, trim cuts it away as a history step.
    h.get_by_label_contains("Viewport, Perspective").focus();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    h.run();
    h.run();
    assert!(h.get_by_role_and_label(Role::RadioButton, "X axis of the plane").is_focused());
    h.get_by_label("Offset of the plane").set_value_accesskit(0.2);
    h.run();
    press(&mut h, Role::Button, "Clip to Plane");
    assert!(h.state().sculpt_groups.plane.is_none());
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.positions.iter().all(|p| p.y <= 0.2 + 1e-4));
    assert_eq!(h.state().status, "Clip to plane");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    h.run();
    press(&mut h, Role::RadioButton, "X axis of the plane");
    press(&mut h, Role::Button, "Trim at Plane");
    let id = h.state().doc.sculpt.as_ref().expect("still sculpting after the trim").id;
    let steps = h.state().doc.scene.get(id).unwrap().steps();
    assert!(steps.iter().any(|s| matches!(s.op, modelcraft_engine::scene::StepOp::TrimPlane { .. })));
    assert!(h.state().doc.sculpt.as_ref().unwrap().mesh.positions.iter().all(|p| p.x <= 1e-4));
    assert!(h.state().status.starts_with("Trim by plane:"), "{}", h.state().status);
}

#[test]
fn a_shell_from_the_mask_is_a_new_object_and_occlusion_masks() {
    let mut h = sculpting("Add Quad sphere");
    let objects = h.state().doc.scene.len();
    press(&mut h, Role::Button, "Extract Shell from Mask");
    assert!(h.state().status.contains("nothing is masked"), "{}", h.state().status);
    let id = h.state().doc.sculpt.as_ref().unwrap().id;
    h.state_mut().run(modelcraft_engine::Command::SculptMask { id, op: modelcraft_engine::MaskOp::Patch { triangle: 0 } });
    h.run();
    h.get_by_label("Shell thickness").set_value_accesskit(0.1);
    h.run();
    press(&mut h, Role::Button, "Extract Shell from Mask");
    assert_eq!(h.state().doc.scene.len(), objects + 1);
    assert_eq!(h.state().doc.sculpt.as_ref().map(|s| s.id), Some(id), "sculpting stays on the original");
    assert_eq!(h.state().selection, vec![id]);
    assert_eq!(h.state().status, "Extract shell");
    // A convex ball sees the whole sky: the occlusion mask clears what was masked.
    palette(&mut h, "Mask Occlusion");
    assert_eq!(masked(&h), 0);
    assert_eq!(crate::sculpt_groups_ui::parse_corners("10,10 90,10; 50,90").map(|p| p.len()), Some(3));
    assert!(crate::sculpt_groups_ui::parse_corners("10,10 90,10").is_none());
    assert!(crate::sculpt_groups_ui::parse_corners("10,10 x,2 3,4").is_none());
}

#[test]
fn the_panels_keep_their_width_from_frame_to_frame() {
    // The panel header used to ask for a little more than the panel had, so every frame both
    // side panels grew and the viewport shrank until nothing was left of it.
    for (workspace, tool) in [(Key::Num1, ""), (Key::Num2, ""), (Key::Num2, "Lasso Hide Tool"), (Key::Num3, "")] {
        let mut h = harness();
        h.run();
        palette(&mut h, "Add Cube");
        h.key_press_modifiers(Modifiers::COMMAND, workspace);
        h.run();
        if !tool.is_empty() {
            palette(&mut h, "Sculpt Patches by Angle");
            palette(&mut h, tool);
        }
        let before = h.get_by_label_contains("Viewport, Perspective").rect();
        for _ in 0..30 {
            h.step();
        }
        assert_eq!(h.get_by_label_contains("Viewport, Perspective").rect(), before, "workspace {workspace:?}");
    }
}
