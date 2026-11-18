//! The Preferences window (Edit ▸ Preferences…, Ctrl+,): interface language, navigation, units,
//! display, snapping steps, brush size and the shelf. Changes apply at once and are saved with the
//! other preferences; Esc or Close returns focus to the window behind.

use egui::RichText;

use crate::actions::{self, Action};
use crate::app::{ModelApp, NavPreset};
use crate::theme;

/// The length unit lengths are shown and typed in. Scene units are metres (glTF's convention);
/// this changes only how numbers read, never the stored values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    #[default]
    M,
    Cm,
    Mm,
    In,
    Ft,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::M, Unit::Cm, Unit::Mm, Unit::In, Unit::Ft];
    /// Shown value per metre.
    pub fn per_metre(self) -> f32 {
        match self {
            Unit::M => 1.0,
            Unit::Cm => 100.0,
            Unit::Mm => 1000.0,
            Unit::In => 1.0 / 0.0254,
            Unit::Ft => 1.0 / 0.3048,
        }
    }
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::M => " m",
            Unit::Cm => " cm",
            Unit::Mm => " mm",
            Unit::In => " in",
            Unit::Ft => " ft",
        }
    }
    pub fn label(self) -> String {
        match self {
            Unit::M => tl!("Metres"),
            Unit::Cm => tl!("Centimetres"),
            Unit::Mm => tl!("Millimetres"),
            Unit::In => tl!("Inches"),
            Unit::Ft => tl!("Feet"),
        }
    }
}

#[derive(Default)]
pub struct PrefsWindow {
    pub open: bool,
    focus: bool,
    /// Filter of the shelf list.
    filter: String,
}

pub fn open(app: &mut ModelApp) {
    app.prefs_window = PrefsWindow { open: true, focus: true, filter: String::new() };
}

fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized([150.0, 18.0], egui::Label::new(RichText::new(label).color(theme::TEXT_DIM)));
        add(ui);
    });
}

fn named(ui: &egui::Ui, r: &egui::Response, name: impl Into<String>) {
    let name = name.into();
    ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(name));
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.prefs_window.open {
        return;
    }
    let mut open = true;
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Window::new(tl!("Preferences")).id(egui::Id::unique("preferences")).open(&mut open).default_width(460.0).collapsible(false).show(ctx, |ui| {
        egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
            egui::CollapsingHeader::new(tl!("Interface")).default_open(true).show(ui, |ui| {
                row(ui, &tl!("Language"), |ui| {
                    let i18n = &*crate::i18n::I18N;
                    let current = app.language.clone();
                    let shown = match &current {
                        None => tl!("Follow the system"),
                        Some(c) => i18n.from_code(c).map(|l| i18n.name(l).to_string()).unwrap_or_else(|| c.clone()),
                    };
                    let r = egui::ComboBox::from_id_salt("pref-language").selected_text(shown).show_ui(ui, |ui| {
                        let mut pick = current.clone();
                        ui.selectable_value(&mut pick, None, tl!("Follow the system"));
                        for l in i18n.languages() {
                            ui.selectable_value(&mut pick, Some(i18n.code(l).to_string()), i18n.name(l));
                        }
                        pick
                    });
                    named(ui, &r.response, tl!("Language"));
                    if app.prefs_window.focus {
                        app.prefs_window.focus = false;
                        r.response.request_focus();
                    }
                    if let Some(pick) = r.inner
                        && pick != current
                    {
                        crate::i18n::set_language(pick.as_deref());
                        app.language = pick;
                    }
                    if i18n.languages().len() <= 1 {
                        ui.label(RichText::new(tl!("No translations in this build")).color(theme::TEXT_DIM).small());
                    }
                });
                row(ui, &tl!("Navigation"), |ui| {
                    let r = egui::ComboBox::from_id_salt("pref-nav").selected_text(app.nav.label()).show_ui(ui, |ui| {
                        for n in [NavPreset::AltMouse, NavPreset::MiddleMouse, NavPreset::RightMouse] {
                            ui.selectable_value(&mut app.nav, n, n.label());
                        }
                    });
                    named(ui, &r.response, tl!("Navigation"));
                });
            });
            egui::CollapsingHeader::new(tl!("Units")).default_open(false).show(ui, |ui| {
                row(ui, &tl!("Lengths in"), |ui| {
                    let r = egui::ComboBox::from_id_salt("pref-unit").selected_text(app.unit.label()).show_ui(ui, |ui| {
                        for u in Unit::ALL {
                            ui.selectable_value(&mut app.unit, u, u.label());
                        }
                    });
                    named(ui, &r.response, tl!("Length unit"));
                });
                ui.label(RichText::new(tl!("Scenes are stored in metres; the unit changes how lengths read and are typed.")).color(theme::TEXT_DIM).small());
            });
            egui::CollapsingHeader::new(tl!("Display")).default_open(false).show(ui, |ui| {
                ui.checkbox(&mut app.show_grid, tl!("Show the ground grid"));
                ui.checkbox(&mut app.show_axes, tl!("Show the axis triad"));
                ui.checkbox(&mut app.show_wire, tl!("Wireframe on Selection"));
                ui.checkbox(&mut app.show_patches, tl!("Show Patches"));
            });
            egui::CollapsingHeader::new(tl!("Snapping and brushes")).default_open(false).show(ui, |ui| {
                let (mut mv, mut rot, mut sc) = app.snap_steps;
                let u = app.unit;
                let mut mv_shown = mv * u.per_metre();
                row(ui, &tl!("Move step"), |ui| {
                    let r = ui.add(egui::DragValue::new(&mut mv_shown).range(0.0001..=100_000.0).speed(0.01).suffix(u.suffix()));
                    named(ui, &r, tl!("Move snapping step"));
                });
                mv = mv_shown / u.per_metre();
                row(ui, &tl!("Rotate step"), |ui| {
                    let r = ui.add(egui::DragValue::new(&mut rot).range(0.1..=180.0).speed(0.5).suffix("°"));
                    named(ui, &r, tl!("Rotate snapping step"));
                });
                row(ui, &tl!("Scale step"), |ui| {
                    let r = ui.add(egui::DragValue::new(&mut sc).range(0.001..=10.0).speed(0.01));
                    named(ui, &r, tl!("Scale snapping step"));
                });
                app.snap_steps = (mv, rot, sc);
                row(ui, &tl!("Brush size"), |ui| {
                    let r = ui.add(egui::Slider::new(&mut app.brush_px, 4.0..=400.0).suffix(" px"));
                    named(ui, &r, tl!("Brush size"));
                });
            });
            egui::CollapsingHeader::new(tl!("Quick Bar")).default_open(false).show(ui, |ui| {
                ui.label(RichText::new(tl!("Tick the commands that get a button on the Quick Bar.")).color(theme::TEXT_DIM).small());
                let f = ui.add(egui::TextEdit::singleline(&mut app.prefs_window.filter).hint_text(tl!("Filter commands")));
                named(ui, &f, tl!("Filter commands"));
                let filter = app.prefs_window.filter.to_lowercase();
                egui::ScrollArea::vertical().id_salt("shelf-list").max_height(180.0).show(ui, |ui| {
                    for a in actions::all().into_iter().filter(|a| filter.is_empty() || a.label().to_lowercase().contains(&filter)) {
                        let mut on = crate::shelf::contains(app, a);
                        if ui.checkbox(&mut on, a.label()).changed() {
                            crate::shelf::toggle(app, a);
                        }
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let k = ui.button(Action::KeyboardShortcuts.label());
                if k.clicked() {
                    app.shortcuts_state = Some(craft_keymap::ui::DialogState::new());
                }
                if ui.button(tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    });
    if !open || close {
        app.prefs_window.open = false;
        app.status = tl!("Preferences saved");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_convert_both_ways() {
        for u in Unit::ALL {
            let shown = 1.25 * u.per_metre();
            assert!((shown / u.per_metre() - 1.25).abs() < 1e-5);
        }
        assert!((Unit::In.per_metre() - 39.370_08).abs() < 1e-3);
        assert_eq!(Unit::Mm.per_metre(), 1000.0);
    }
}
