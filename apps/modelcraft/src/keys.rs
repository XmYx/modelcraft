//! Keyboard shortcuts through craft-keymap: the catalog comes from the action table, user
//! overrides live in the preferences, the suite's Keyboard Shortcuts dialog edits them, and one
//! dispatcher per frame runs them. Workspace-only keys use the `Model` / `Sculpt` / `Paint`
//! contexts.

use craft_keymap::{Active, Bindable, Binding, CommandRef, Keymap, Platform, Scope, Scopes};

use crate::actions::{self, Action};
use crate::app::{ModelApp, Workspace};

pub const CONTEXTS: [&str; 3] = ["Model", "Sculpt", "Paint"];

pub fn context(ws: Workspace) -> &'static str {
    match ws {
        Workspace::Model => "Model",
        Workspace::Sculpt => "Sculpt",
        Workspace::Paint => "Paint",
    }
}

pub fn catalog() -> Vec<Bindable> {
    actions::all()
        .into_iter()
        .map(|a| {
            let mut b = Bindable::new(a.id().as_str(), a.label(), Scope::app()).with_path(vec![a.menu().to_string(), a.label()]);
            if let Some(Ok(binding)) = a.default_binding().map(Binding::parse) {
                b = b.with_default(binding);
            }
            b
        })
        .collect()
}

pub fn keymap(overrides: craft_keymap::Overrides) -> Keymap {
    let scopes = CONTEXTS.iter().fold(Scopes::new(), |s, c| s.with_context(c));
    Keymap::new(catalog(), scopes, Platform::current()).with_overrides(overrides)
}

pub fn action_of(cmd: &CommandRef) -> Option<Action> {
    actions::all().into_iter().find(|a| a.id() == cmd.id)
}

/// The first binding of an action, as this platform writes it ("Ctrl+S", "⌘S").
pub fn shortcut_text(app: &ModelApp, a: Action) -> String {
    let bindings = app.keymap.bindings(&CommandRef::from(a.id().as_str()));
    // Prefer a binding that works in the current workspace.
    let ctx = context(app.workspace);
    let pick = bindings
        .iter()
        .find(|b| b.to_string().starts_with(&format!("{ctx}: ")))
        .or_else(|| bindings.iter().find(|b| !CONTEXTS.iter().any(|c| b.to_string().starts_with(&format!("{c}: ")))));
    pick.map(|b| {
        let t = b.trigger.display(Platform::current());
        t.split_once(": ").map(|(_, k)| k.to_string()).unwrap_or(t)
    })
    .unwrap_or_default()
}

/// Run this frame's shortcuts.
pub fn dispatch(app: &mut ModelApp, ctx: &egui::Context) -> Vec<Action> {
    // For egui any focused widget "wants the keyboard" (`egui_wants_keyboard_input`): a viewport
    // after a click, a canvas, a ruler, a button. Treating that as typing switched off Undo,
    // Select All and the tool keys whenever anything but empty space had focus. Only a focused
    // text field (a DragValue being edited is one) is typing.
    let typing = ctx.text_edit_focused();
    // The workspace contexts hold the single-letter tool and brush keys ("Model: E", "Sculpt: D").
    // craft-keymap lets a binding in an active context fire even while typing, so while a text
    // field has focus no context is active: those letters go to the field.
    let contexts = [context(app.workspace)];
    let active = Active { panel: None, contexts: if typing { &[] } else { &contexts }, typing };
    let keymap = &app.keymap;
    let dispatcher = &mut app.dispatcher;
    let hits = ctx.input_mut(|i| dispatcher.dispatch(i, keymap, &active));
    hits.iter().filter_map(|h| action_of(&h.command)).collect()
}

/// The suite's Keyboard Shortcuts dialog; saves overrides into the preferences on OK.
pub fn dialog(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(state) = &mut app.shortcuts_state else { return };
    let text = craft_keymap::ui::I18nText { i18n: &crate::i18n::I18N, lang: crate::i18n::current() };
    let outcome = craft_keymap::ui::ShortcutsDialog::new(&mut app.keymap, &text).show(ctx, state);
    if !state.is_open() {
        app.shortcuts_state = None;
    }
    if matches!(outcome, Some(craft_keymap::ui::Outcome::Ok)) {
        let o = app.keymap.overrides().clone();
        if let Some(store) = &mut app.prefs {
            store.update(|p| p.shortcuts = o);
        }
        app.status = tl!("Shortcuts saved");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_parses_and_has_no_conflict() {
        for a in actions::all() {
            if let Some(t) = a.default_binding() {
                assert!(Binding::parse(t).is_ok(), "{a:?}: {t}");
            }
        }
        // Every variant with a label is in the table (menus, palette and keymap read it).
        for a in
            [Action::SaveIncremental, Action::KeyboardShortcuts, Action::SelectLoop, Action::Bridge, Action::FillHole, Action::Mirror(2), Action::ToggleSnap]
        {
            assert!(actions::all().contains(&a), "{a:?} is missing from actions::all()");
        }
        let ids: std::collections::HashSet<String> = actions::all().into_iter().map(|a| a.id()).collect();
        assert_eq!(ids.len(), actions::all().len(), "action ids are unique");
        let km = keymap(Default::default());
        let model = Active { panel: None, contexts: &["Model"], typing: false };
        let sculpt = Active { panel: None, contexts: &["Sculpt"], typing: false };
        let run = |t: &str, a: &Active| match km.lookup(&craft_keymap::Chord::parse(t).unwrap(), false, a) {
            craft_keymap::Lookup::Run(c) => action_of(&c),
            _ => None,
        };
        assert_eq!(run("W", &model), Some(Action::ToolMove));
        assert_eq!(run("D", &sculpt), Some(Action::Brush(modelcraft_engine::sculpt::BrushKind::Draw)));
        assert_eq!(run("Cmd+S", &model), Some(Action::Save));
        assert_eq!(run("Cmd+Shift+Z", &sculpt), Some(Action::Redo));
    }
}
