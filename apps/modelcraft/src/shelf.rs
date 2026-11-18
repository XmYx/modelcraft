//! The Quick Bar (internally the shelf): a bar of the user's own buttons under the tool options, one per chosen action
//! (any menu or palette command). Edited in Preferences ▸ Quick Bar (a checkbox per action) or with
//! the right-click menu of a shelf button; saved in the preferences by action id.

use crate::actions::{self, Action};
use crate::app::ModelApp;
use crate::theme;

/// Most buttons on the shelf.
pub const MAX: usize = 40;

/// The action with this stable id.
pub fn action(id: &str) -> Option<Action> {
    actions::all().into_iter().find(|a| a.id() == id)
}

pub fn contains(app: &ModelApp, a: Action) -> bool {
    let id = a.id();
    app.shelf.contains(&id)
}

pub fn toggle(app: &mut ModelApp, a: Action) {
    let id = a.id();
    if let Some(i) = app.shelf.iter().position(|s| *s == id) {
        app.shelf.remove(i);
        app.status = crate::tlf!("{action} removed from the Quick Bar", action = a.label());
    } else if app.shelf.len() < MAX {
        app.shelf.push(id);
        app.status = crate::tlf!("{action} added to the Quick Bar", action = a.label());
    } else {
        app.status = crate::tlf!("The Quick Bar holds {count} buttons at most", count = MAX);
    }
}

/// Keep only ids of actions that exist, once each, at most [`MAX`].
pub fn cleaned(ids: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in ids {
        if action(&id).is_some() && !out.contains(&id) && out.len() < MAX {
            out.push(id);
        }
    }
    out
}

/// The bar; drawn only when the shelf has buttons.
pub fn bar(app: &mut ModelApp, ui: &mut egui::Ui) {
    if app.shelf.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    egui::Panel::top("shelf").frame(egui::Frame::NONE.fill(theme::PANEL_DEEP).inner_margin(egui::Margin::symmetric(6, 2))).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(tl!("Quick Bar")).color(theme::TEXT_DIM).small());
            for a in app.shelf.clone().iter().filter_map(|id| action(id)) {
                let r = ui.button(a.label()).on_hover_text(a.shortcut_text(app));
                // Told apart from the same command elsewhere; the name starts with the visible label.
                let name = crate::tlf!("{action}, on the Quick Bar", action = a.label());
                ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(name));
                if r.clicked() {
                    app.act(&ctx, a);
                }
                r.context_menu(|ui| {
                    if ui.button(tl!("Remove from the Quick Bar")).clicked() {
                        toggle(app, a);
                        ui.close();
                    }
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_junk_is_dropped() {
        for a in actions::all() {
            assert_eq!(action(&a.id()), Some(a), "{}", a.id());
        }
        let ids = vec!["mesh.extrude".to_string(), "nope".into(), "mesh.extrude".into(), "file.save".into()];
        assert_eq!(cleaned(ids), vec!["mesh.extrude".to_string(), "file.save".into()]);
    }
}
