//! UI and accessibility tests of the shared interface code: keyboard shortcuts and the object
//! organisation (tags, layers, pivots, user attributes, presets, instances).

use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable;

use crate::ui_tests::{harness, palette};

/// Something focused that is not a text field: the viewport, or any other widget.
fn focus(h: &egui_kittest::Harness<'static, crate::app::ModelApp>, id: egui::Id) {
    h.ctx.memory_mut(|m| m.request_focus(id));
}

#[test]
fn undo_and_select_all_work_while_the_viewport_or_another_widget_has_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    assert_eq!(h.state().doc.scene.len(), 1);
    // The viewport after a click: egui says it wants the keyboard, but it is not typing.
    focus(&h, egui::Id::unique(("viewport", 0)));
    h.run();
    assert!(h.ctx.egui_wants_keyboard_input() && !h.ctx.text_edit_focused());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 0, "Ctrl+Z undoes with the viewport focused");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1, "and redoes");
    h.state_mut().selection.clear();
    h.run();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run();
    assert_eq!(h.state().selection.len(), 1, "Ctrl+A selects with the viewport focused");
    // Any other focusable widget is the same: a panel header here.
    h.state_mut().selection.clear();
    focus(&h, egui::Id::unique(("panel-focus", 0)));
    h.run();
    assert!(h.ctx.egui_wants_keyboard_input() && !h.ctx.text_edit_focused());
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), 0, "Ctrl+Z undoes with a panel focused");
}

#[test]
fn a_single_letter_shortcut_does_nothing_while_a_text_field_has_focus() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    let before = h.state().tool;
    // Typing in the object search field: E (Rotate tool) is a letter there, not a command.
    h.get_by_label("Search objects").focus();
    h.run();
    assert!(h.ctx.text_edit_focused());
    // A real backend reports the key press, then the text it typed.
    h.key_press(Key::E);
    h.event(egui::Event::Text("e".into()));
    h.run();
    assert_eq!(h.state().tool, before, "the tool did not change while typing");
    assert!(h.state().objects.filter.contains('e'), "the letter went to the field: {:?}", h.state().objects.filter);
    // Ctrl+Z in a text field is the field's own undo, not the document's.
    let len = h.state().doc.scene.len();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.scene.len(), len, "the document's undo is not triggered from a text field");
    // Control: with the field released the same key switches the tool.
    h.ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique(("none", 0))));
    h.ctx.memory_mut(|m| {
        if let Some(f) = m.focused() {
            m.surrender_focus(f);
        }
    });
    h.run();
    assert!(!h.ctx.text_edit_focused());
    h.key_press(Key::E);
    h.run();
    assert_eq!(h.state().tool, crate::app::Tool::Rotate, "E switches to Rotate with nothing focused");
}
