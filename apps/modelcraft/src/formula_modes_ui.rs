//! The formula text of the formula deformer and the formula clone-modifier mode in Properties.
//!
//! The field keeps what is typed while it has the focus. Enter or leaving it sets the formula
//! when it parses and reads only the names the mode knows; otherwise the text stays with its
//! error shown under it (and in the field's accessible description) until it is fixed or Escape
//! drops it. An error the evaluation reports (too much work) shows the same way.

use std::hash::Hash;

use egui::{Id, Key, RichText, Stroke, Ui};
use modelcraft_engine::animation::expr::Expr;

use crate::theme;

#[derive(Clone, Debug, PartialEq)]
struct Draft {
    text: String,
    error: Option<String>,
}

/// Why `text` cannot be a formula of a mode that reads `names`, if it cannot.
pub(crate) fn check(text: &str, names: &[&str]) -> Option<String> {
    match Expr::parse(text) {
        Err(e) => Some(e.to_string()),
        Ok(e) => e
            .variables()
            .iter()
            .find(|v| !names.contains(&v.as_str()))
            .map(|v| crate::tlf!("{name} is not one of: {names}", name = v, names = names.join(", "))),
    }
}

/// A labelled formula field. Returns the new formula when one was committed.
pub(crate) fn formula_text(
    ui: &mut Ui,
    salt: impl Hash + std::fmt::Debug,
    label: &str,
    current: &str,
    problem: Option<String>,
    hint: &str,
    names: &[&str],
) -> Option<String> {
    let id = Id::unique(("modelcraft-formula-mode", salt));
    let mut draft: Draft = ui.data(|d| d.get_temp(id)).unwrap_or(Draft { text: current.to_string(), error: None });
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(label);
        let r = ui.add(egui::TextEdit::singleline(&mut draft.text).hint_text(hint).desired_width(200.0));
        let error = draft.error.clone().or_else(|| problem.clone());
        if error.is_some() {
            ui.painter().rect_stroke(r.rect.expand(1.0), 2.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Outside);
            ui.label(RichText::new("!").color(theme::ACCENT).strong());
        }
        let description = match &error {
            Some(e) => crate::tlf!("Formula error: {error}", error = e),
            None => crate::tlf!("Names: {names}", names = hint),
        };
        let (name, value) = (label.to_string(), draft.text.clone());
        ui.ctx().accesskit_node_builder(r.id, |n| {
            n.set_label(name);
            n.set_value(value);
            n.set_description(description);
        });
        if r.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                draft = Draft { text: current.to_string(), error: None };
            } else if draft.text.trim() != current.trim() {
                match check(draft.text.trim(), names) {
                    None => {
                        out = Some(draft.text.trim().to_string());
                        draft.error = None;
                    }
                    Some(e) => draft.error = Some(e),
                }
            }
        }
    });
    if let Some(e) = draft.error.clone().or(problem) {
        ui.label(RichText::new(crate::tlf!("Formula error: {error}", error = e)).color(theme::ACCENT).small());
    }
    // Keep the draft only while it differs from the formula (being typed, or waiting to be fixed).
    if out.is_some() || (draft.error.is_none() && draft.text == current) {
        ui.data_mut(|d| d.remove::<Draft>(id));
    } else {
        ui.data_mut(|d| d.insert_temp(id, draft));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_formula_must_parse_and_read_only_known_names() {
        let names = ["x", "y", "time"];
        assert_eq!(check("sin(x) * time", &names), None);
        assert!(check("x +", &names).is_some());
        assert!(check("frame", &names).is_some_and(|e| e.contains("frame")));
        assert!(check("@3.translate.x", &names).is_some(), "a mode formula reads no objects");
    }
}
