//! UI tests for sculpting through the pointer and the mask tools. They live apart from
//! `ui_tests.rs` so that workstreams appending tests there do not collide with these.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

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
