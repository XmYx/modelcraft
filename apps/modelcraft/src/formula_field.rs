//! Formula fields: any keyable number in Properties can hold a formula instead of a value.
//!
//! Typing `=` and a formula into a number (`=sin(time) * 2`, `=@Lever.rotate.z / 2`) and
//! pressing Enter or leaving the field gives the attribute an expression constraint
//! (`rig op=set_expression`), so it is ordered with the other constraints, guarded against
//! cycles, capped in work, solved once per frame change, saved in the project and undone like
//! any command. The field then shows the formula as text; typing a plain number there removes
//! the formula and sets the value (one undo step), and an empty field removes it and keeps the
//! value.
//!
//! A formula that does not parse stays in the field with its error, marked by a frame and a `!`
//! (and in the field's accessible description), until it is fixed or Escape drops it. A formula
//! the solve had to skip (a cycle, a missing object) shows the solve's reason the same way.
//!
//! The fields draw inside the key-marker scope of the Properties panel (`key_marker::begin`),
//! which knows the object and its formulas; outside one they are plain numbers.

use std::ops::RangeInclusive;

use egui::{Id, Key, Response, RichText, Stroke, Ui};
use modelcraft_engine::animation::expr::Expr;
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{AnimCommand, Command, RigCommand};

use crate::app::ModelApp;
use crate::key_marker::{self, Press, Scope};
use crate::panels::set_name;
use crate::theme;

/// What was typed into a field and could not become a formula yet.
#[derive(Clone, Debug, PartialEq)]
struct Draft {
    text: String,
    error: String,
}

fn draft_id(scope: &Scope, path: &str) -> Id {
    Id::unique(("modelcraft-formula-draft", scope.id.0, path))
}

fn edit_id(scope: &Scope, path: &str) -> Id {
    Id::unique(("modelcraft-formula-edit", scope.id.0, path))
}

fn pending_id(scope: &Scope, path: &str) -> Id {
    Id::unique(("modelcraft-formula-pending", scope.id.0, path))
}

/// Numbers as egui reads them: whitespace is ignored and the minus may be the typographic one.
fn parse_number(text: &str) -> Option<f64> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).map(|c| if c == '\u{2212}' { '-' } else { c }).collect();
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// What committing typed text in a field means.
#[derive(Debug, PartialEq)]
enum Commit {
    Formula(String),
    Remove,
    Value(f64),
    Invalid(String),
}

fn commit_of(text: &str) -> Commit {
    let t = text.trim();
    if let Some(f) = t.strip_prefix('=') {
        return match Expr::parse(f) {
            Ok(_) => Commit::Formula(f.trim().to_string()),
            Err(e) => Commit::Invalid(e.to_string()),
        };
    }
    if t.is_empty() {
        return Commit::Remove;
    }
    match parse_number(t) {
        Some(v) => Commit::Value(v),
        None => Commit::Invalid(tl!("Type a number, or = and a formula")),
    }
}

/// A number for attribute `path`, named `name`: a drag value, or the formula's text when the
/// attribute has one. `width` sizes it like the field it replaces. A formula edit never reports
/// `changed`, so callers that preview drags leave it alone.
pub(crate) fn number(ui: &mut Ui, path: &str, name: &str, value: &mut f32, speed: f32, range: Option<RangeInclusive<f32>>, width: Option<f32>) -> Response {
    let drag = |ui: &mut Ui, value: &mut f32, parser: Option<&dyn Fn(&str) -> Option<f64>>| {
        let mut d = egui::DragValue::new(value).speed(speed).max_decimals(3);
        if let Some(r) = range.clone() {
            d = d.range(r);
        }
        if let Some(p) = parser {
            d = d.custom_parser(p);
        }
        let r = match width {
            Some(w) => ui.add_sized([w, 18.0], d),
            None => ui.add(d),
        };
        set_name(ui, &r, name);
        r
    };
    let Some(scope) = key_marker::scope(ui).filter(|s| s.valid.contains(path)) else { return drag(ui, value, None) };
    let draft: Option<Draft> = ui.data(|d| d.get_temp(draft_id(&scope, path)));
    let formula = scope.formulas.get(path).cloned();
    if formula.is_none() && draft.is_none() {
        // A plain number. egui parses on every keystroke, so `=` typed into it is caught by the
        // parser and kept until the field is left.
        let pending = pending_id(&scope, path);
        let ctx = ui.ctx().clone();
        let parser = move |s: &str| -> Option<f64> {
            let t = s.trim();
            if t.starts_with('=') {
                ctx.data_mut(|d| d.insert_temp(pending, t.to_string()));
                None
            } else {
                ctx.data_mut(|d| d.remove::<String>(pending));
                parse_number(t)
            }
        };
        let r = drag(ui, value, Some(&parser));
        let typed: Option<String> = ui.data(|d| d.get_temp(pending));
        if let Some(text) = typed
            && !r.has_focus()
        {
            ui.data_mut(|d| d.remove::<String>(pending));
            if !(r.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape))) {
                commit(ui, &scope, path, &text, false);
            }
        }
        return r;
    }
    let shown = match (&draft, &formula) {
        (Some(d), _) => d.text.clone(),
        (None, Some(f)) => format!("={}", f.expr),
        (None, None) => String::new(),
    };
    let error = draft.as_ref().map(|d| d.error.clone()).or_else(|| formula.as_ref().and_then(|f| f.problem.clone()));
    let eid = edit_id(&scope, path);
    let mut text: String = ui.data(|d| d.get_temp(eid)).unwrap_or_else(|| shown.clone());
    let mut r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            let edit = egui::TextEdit::singleline(&mut text).desired_width(width.map_or(120.0, |w| (w - 10.0).max(24.0)));
            let r = ui.add(edit);
            if error.is_some() {
                ui.painter().rect_stroke(r.rect.expand(1.0), 2.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Outside);
                ui.label(RichText::new("!").color(theme::ACCENT).strong());
            }
            r
        })
        .inner;
    let description = match &error {
        Some(e) => crate::tlf!("Formula error: {error}", error = e),
        None => crate::tlf!("Formula, now {value}", value = format!("{:.3}", *value)),
    };
    let label = name.to_string();
    let value_text = text.clone();
    let desc = description.clone();
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_label(label);
        n.set_value(value_text);
        n.set_description(desc);
    });
    r = r.on_hover_text(description);
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(eid, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(eid));
    }
    if r.lost_focus() {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            ui.data_mut(|d| d.remove::<Draft>(draft_id(&scope, path)));
        } else if text.trim() != shown.trim() || draft.is_some() {
            commit(ui, &scope, path, &text, formula.is_some());
        }
    }
    r.flags.remove(egui::response::Flags::CHANGED);
    r
}

/// Turn what was typed into a queued command, or a draft with its error.
fn commit(ui: &Ui, scope: &Scope, path: &str, text: &str, has_formula: bool) {
    let did = draft_id(scope, path);
    match commit_of(text) {
        Commit::Formula(f) => {
            ui.data_mut(|d| d.remove::<Draft>(did));
            scope.press(Press::Formula(path.to_string(), f));
        }
        Commit::Remove => {
            ui.data_mut(|d| d.remove::<Draft>(did));
            if has_formula {
                scope.press(Press::Formula(path.to_string(), String::new()));
            }
        }
        Commit::Value(v) => {
            ui.data_mut(|d| d.remove::<Draft>(did));
            scope.press(Press::Value(path.to_string(), v));
        }
        Commit::Invalid(error) => {
            ui.data_mut(|d| d.insert_temp(did, Draft { text: text.trim().to_string(), error }));
        }
    }
}

/// Give `path` of `id` the formula `expr`, or remove its formula when `expr` is empty.
pub fn set(app: &mut ModelApp, id: ObjectId, path: &str, expr: &str) -> bool {
    let label = crate::anim_ui::attr_label(path);
    let ok = app.run(Command::Rig(RigCommand::SetExpression { target: id, path: path.to_string(), expr: expr.to_string() }));
    if ok {
        app.status = if expr.is_empty() {
            crate::tlf!("Formula removed from {attribute}", attribute = label)
        } else {
            crate::tlf!("{attribute} follows a formula", attribute = label)
        };
    }
    ok
}

/// Remove the formula on `path` (if any) and set the attribute to `value`, as one undo step.
pub fn replace_with_value(app: &mut ModelApp, id: ObjectId, path: &str, value: f64) -> bool {
    let set_value = Command::Anim(AnimCommand::SetAttr { id, path: path.to_string(), value });
    if app.doc.scene.constraints.formula(id, path).is_none() {
        return app.run(set_value);
    }
    let remove = Command::Rig(RigCommand::SetExpression { target: id, path: path.to_string(), expr: String::new() });
    let ok = app.run(Command::Batch { label: Some(tl!("Replace formula with a number")), commands: vec![remove, set_value] });
    if ok {
        app.status = crate::tlf!("Formula removed from {attribute}", attribute = crate::anim_ui::attr_label(path));
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_becomes_a_formula_a_number_or_an_error() {
        assert_eq!(commit_of("=sin(time) * 2"), Commit::Formula("sin(time) * 2".into()));
        assert_eq!(commit_of("  = frame "), Commit::Formula("frame".into()));
        assert_eq!(commit_of("2.5"), Commit::Value(2.5));
        assert_eq!(commit_of("\u{2212}1 000"), Commit::Value(-1000.0));
        assert_eq!(commit_of(""), Commit::Remove);
        assert!(matches!(commit_of("=1 +"), Commit::Invalid(_)));
        assert!(matches!(commit_of("="), Commit::Invalid(_)));
        assert!(matches!(commit_of("abc"), Commit::Invalid(_)));
        assert!(matches!(commit_of("inf"), Commit::Invalid(_)), "a value must be finite");
    }
}
