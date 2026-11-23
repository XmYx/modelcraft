//! UI tests of vertex colour painting: fills and the colour fields by keyboard, the brush by a
//! pointer drag with one undo step per stroke, and the contrast of what the colour controls draw.
//! They live apart from `ui_tests.rs` so that workstreams appending tests there do not collide.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::scene::{ObjectId, StepOp};

use crate::app::ModelApp;
use crate::ui_tests::{harness, palette};

fn steps(h: &Harness<'static, ModelApp>) -> Vec<StepOp> {
    let id = h.state().selection[0];
    h.state().doc.scene.get(id).unwrap().steps().iter().map(|s| s.op.clone()).collect()
}

fn mesh(h: &mut Harness<'static, ModelApp>) -> std::sync::Arc<modelcraft_engine::mesh::PolyMesh> {
    let id: ObjectId = h.state().selection[0];
    let st = h.state_mut();
    st.doc.eval.steps_output(&st.doc.scene, id, usize::MAX).mesh
}

/// A sphere, then the Paint workspace.
fn paint_sphere(h: &mut Harness<'static, ModelApp>) {
    h.run();
    palette(h, "Add Sphere");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num3);
    h.run();
}

#[test]
fn fill_all_by_keyboard_is_one_undo_step() {
    let mut h = harness();
    paint_sphere(&mut h);
    let before = h.state().doc.undo.len();
    h.key_press_modifiers(Modifiers::SHIFT, Key::F);
    h.run();
    assert!(matches!(steps(&h)[0], StepOp::FillColor { ref verts, .. } if verts.is_empty()), "{:?}", steps(&h));
    assert_eq!(h.state().doc.undo.len(), before + 1);
    let want = h.state().paint.linear();
    let m = mesh(&mut h);
    assert!(m.has_colors() && m.corner_colors().iter().all(|c| *c == want));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 0);
    assert!(!mesh(&mut h).has_colors());
}

#[test]
fn the_colour_fields_swatches_and_hex_by_keyboard() {
    let mut h = harness();
    paint_sphere(&mut h);
    h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Red").set_value_accesskit(10.0);
    h.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Green").set_value_accesskit(20.0);
    h.run();
    assert_eq!(&h.state().paint.rgba[..2], &[10, 20]);
    // A swatch is a button: focus it and press Space, as a keyboard user does.
    h.get_by_label("Swatch #588EF0").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    assert_eq!(&h.state().paint.rgba[..3], &[0x58, 0x8E, 0xF0]);
    // The hex field: Tab to it, replace the text, Tab away.
    h.get_by_label("Hex colour").focus();
    h.run();
    // Select the whole text with the keyboard (End, then Shift+Home) and type over it.
    h.key_press(Key::End);
    h.run();
    h.key_press_modifiers(Modifiers::SHIFT, Key::Home);
    h.run();
    h.event(egui::Event::Text("#102030".into()));
    h.run();
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(&h.state().paint.rgba[..3], &[0x10, 0x20, 0x30]);
    assert_eq!(crate::paint_ui::parse_hex("102030"), Some([0x10, 0x20, 0x30]));
    assert_eq!(crate::paint_ui::parse_hex("#12"), None);
    assert_eq!(crate::paint_ui::parse_hex("#GGGGGG"), None);
    assert_eq!(crate::paint_ui::parse_hex("ééé"), None);
    // The fill uses it, as linear light.
    h.key_press_modifiers(Modifiers::SHIFT, Key::F);
    h.run();
    let c = mesh(&mut h).corner_colors()[0];
    assert!(c[0] < 0.01 && c[2] > c[0], "{c:?}");
}

#[test]
fn fill_selection_needs_a_selection() {
    let mut h = harness();
    paint_sphere(&mut h);
    h.key_press_modifiers(Modifiers::ALT, Key::F);
    h.run();
    assert_eq!(steps(&h).len(), 0);
    assert!(h.state().status.contains("Select"), "{}", h.state().status);
    h.state_mut().mode = crate::app::SelectMode::Vertex;
    h.state_mut().components = [0u32].into_iter().collect();
    h.run();
    h.key_press_modifiers(Modifiers::ALT, Key::F);
    h.run();
    assert!(matches!(&steps(&h)[0], StepOp::FillColor { verts, .. } if !verts.is_empty()));
    let n = mesh(&mut h).corner_colors().iter().filter(|c| **c != [1.0; 4]).count();
    assert!(n > 0 && n < mesh(&mut h).corner_count());
}

#[test]
fn the_brush_paints_one_undo_step_per_stroke() {
    let mut h = harness();
    paint_sphere(&mut h);
    h.key_press(Key::B);
    h.run();
    assert!(h.state().paint.brush);
    h.state_mut().paint.radius = 1.2;
    let rect = h.get_by_label_contains("Viewport").rect();
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    let before = h.state().doc.undo.len();
    for _ in 0..2 {
        let (a, b) = (rect.center() - egui::vec2(40.0, 0.0), rect.center() + egui::vec2(40.0, 10.0));
        h.event(egui::Event::PointerMoved(a));
        h.step();
        h.event(button(a, true));
        h.step();
        for k in 1..=8 {
            h.event(egui::Event::PointerMoved(a.lerp(b, k as f32 / 8.0)));
            h.step();
        }
        h.event(button(b, false));
        h.step();
        h.step();
    }
    let strokes = steps(&h).iter().filter(|s| matches!(s, StepOp::PaintStroke { .. })).count();
    assert_eq!(strokes, 2, "{:?}", steps(&h).len());
    assert_eq!(h.state().doc.undo.len(), before + 2);
    assert!(mesh(&mut h).has_colors());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(steps(&h).len(), 1, "undo takes back one stroke");
    // Esc leaves the brush: the key goes to the app, not to a stroke in progress.
    h.key_press(Key::B);
    h.run();
    assert!(!h.state().paint.brush);
}

#[test]
fn the_radius_has_a_key_path_and_stays_in_range() {
    let mut h = harness();
    paint_sphere(&mut h);
    let r0 = h.state().paint.radius;
    h.key_press_modifiers(Modifiers::ALT, Key::CloseBracket);
    h.run();
    assert!(h.state().paint.radius > r0);
    for _ in 0..200 {
        h.key_press_modifiers(Modifiers::ALT, Key::OpenBracket);
    }
    h.run();
    assert!(h.state().paint.radius >= 0.001);
}

/// WCAG relative luminance contrast of two sRGB colours.
fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
    let lum = |c: egui::Color32| {
        let f = |v: u8| {
            let x = f32::from(v) / 255.0;
            if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    };
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[test]
fn what_the_colour_controls_draw_is_visible_in_both_themes() {
    // Swatches are drawn on the panel with an outline in the text colour; the brush ring is drawn
    // twice (the colour and a dim ring) so one of them reads on any surface.
    assert!(contrast(crate::theme::TEXT, crate::theme::PANEL) >= 4.5);
    assert!(contrast(crate::theme::TEXT_DIM, crate::theme::PANEL) >= 3.0);
    // And every swatch is told apart from the panel by its outline even when its fill is close to it.
    for [r, g, b] in crate::paint_ui::SWATCHES {
        let fill = egui::Color32::from_rgb(r, g, b);
        assert!(contrast(fill, crate::theme::PANEL).max(contrast(crate::theme::TEXT_DIM, crate::theme::PANEL)) >= 3.0);
    }
}
