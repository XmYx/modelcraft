//! The Variants panel: the scene's variants as a list to switch between, and the overrides of the
//! applied one.
//!
//! A variant changes chosen numbers (position, rotation, scale, visibility, a material value, any
//! keyable attribute) of chosen objects and nothing else; the main scene is always one click
//! away and comes back exactly. To make a variant: add it, switch to it, set the values, then
//! override them (the buttons take the selected object's current values). Everything here is a
//! `variant` command, so it is undoable, saved and scriptable.

use egui::RichText;
use modelcraft_engine::scene::{ObjectId, attr};
use modelcraft_engine::{Command, VariantCommand};

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

/// Most override rows listed (the rest is reachable by selecting an object and clearing from there).
const MAX_ROWS: usize = 200;

#[derive(Default)]
pub struct State {
    /// The attribute "Override Attribute" takes.
    pub attribute: String,
    /// The name being typed for the applied variant.
    name: Option<(u32, String)>,
}

fn run(app: &mut ModelApp, c: VariantCommand) -> bool {
    app.run(Command::Variant(c))
}

/// The paths of `wanted` that the object has and that are not animated.
fn overridable(app: &ModelApp, id: ObjectId, wanted: &[&str]) -> Vec<String> {
    let Some(o) = app.doc.scene.get(id) else { return Vec::new() };
    let have: std::collections::BTreeSet<String> = attr::list(o).into_iter().map(|(p, _)| p).collect();
    wanted.iter().filter(|p| have.contains(**p) && !app.doc.scene.anim.has_curve(id, p)).map(|p| p.to_string()).collect()
}

fn value_text(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

pub fn panel(app: &mut ModelApp, ui: &mut egui::Ui) {
    let active = app.doc.scene.variants.active;
    let list: Vec<(u32, String)> = app.doc.scene.variants.list.iter().map(|v| (v.id, v.name.clone())).collect();
    ui.label(RichText::new(tl!("Switch between versions of the scene. The main scene always comes back exactly.")).color(theme::TEXT_DIM).small());
    if ui.radio(active.is_none(), tl!("Main scene")).clicked() && active.is_some() && run(app, VariantCommand::Switch { id: None }) {
        app.status = tl!("Main scene applied");
    }
    for (id, name) in &list {
        let r = ui.radio(active == Some(*id), name.clone());
        if r.clicked() && active != Some(*id) && run(app, VariantCommand::Switch { id: Some(*id) }) {
            app.status = crate::tlf!("Variant {name} applied", name = name);
        }
    }
    ui.horizontal(|ui| {
        if ui.button(tl!("Add Variant")).clicked() {
            run(app, VariantCommand::Add { name: None });
        }
        let r = ui.add_enabled(active.is_some(), egui::Button::new(tl!("Remove Variant")));
        if r.clicked()
            && let Some(a) = active
        {
            run(app, VariantCommand::Remove { id: a });
        }
    });
    let Some(a) = active else {
        ui.label(RichText::new(tl!("Switch to a variant to override attributes in it.")).color(theme::TEXT_DIM));
        return;
    };
    ui.separator();
    // Rename.
    let current = list.iter().find(|(i, _)| *i == a).map(|(_, n)| n.clone()).unwrap_or_default();
    if !matches!(&app.variants.name, Some((i, _)) if *i == a) {
        app.variants.name = Some((a, current.clone()));
    }
    let mut draft = app.variants.name.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
    let mut commit = false;
    ui.horizontal(|ui| {
        ui.label(tl!("Name"));
        let r = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(140.0));
        set_name(ui, &r, tl!("Variant name"));
        commit = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    });
    app.variants.name = Some((a, draft.clone()));
    if commit && draft.trim() != current && run(app, VariantCommand::Rename { id: a, name: draft }) {
        app.variants.name = None;
    }
    ui.label(RichText::new(crate::tlf!("Overrides in {name}", name = current)).strong());
    if let Some(id) = app.active() {
        let who = app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
        ui.label(RichText::new(crate::tlf!("Selected: {name}", name = who)).color(theme::TEXT_DIM));
        let groups: [(String, Vec<&str>); 5] = [
            (tl!("Override Position"), vec!["translate.x", "translate.y", "translate.z"]),
            (tl!("Override Rotation"), vec!["rotate.x", "rotate.y", "rotate.z"]),
            (tl!("Override Scale"), vec!["scale.x", "scale.y", "scale.z"]),
            (tl!("Override Visibility"), vec!["visible"]),
            (
                tl!("Override Material"),
                vec!["material.base_color.0", "material.base_color.1", "material.base_color.2", "material.metallic", "material.roughness"],
            ),
        ];
        ui.horizontal_wrapped(|ui| {
            for (label, paths) in &groups {
                let paths = overridable(app, id, paths);
                if ui.add_enabled(!paths.is_empty(), egui::Button::new(label.clone())).clicked() {
                    run(app, VariantCommand::Override { object: id, paths });
                }
            }
        });
        let attrs: Vec<String> = app.doc.scene.get(id).map(|o| attr::list(o).into_iter().map(|(p, _)| p).collect()).unwrap_or_default();
        if !attrs.contains(&app.variants.attribute) {
            app.variants.attribute = attrs.first().cloned().unwrap_or_default();
        }
        ui.horizontal(|ui| {
            let combo =
                egui::ComboBox::from_id_salt("variant-attribute").selected_text(crate::anim_ui::attr_label(&app.variants.attribute)).show_ui(ui, |ui| {
                    for p in &attrs {
                        ui.selectable_value(&mut app.variants.attribute, p.clone(), crate::anim_ui::attr_label(p));
                    }
                });
            set_name(ui, &combo.response, tl!("Attribute to override"));
            if ui.button(tl!("Override Attribute")).clicked() {
                let paths = overridable(app, id, &[app.variants.attribute.clone().as_str()]);
                if paths.is_empty() {
                    app.status = tl!("That attribute is animated or missing, so a variant cannot override it");
                } else {
                    run(app, VariantCommand::Override { object: id, paths });
                }
            }
        });
    } else {
        ui.label(RichText::new(tl!("Select an object to override its values.")).color(theme::TEXT_DIM));
    }
    // What the variant overrides now.
    let rows: Vec<(ObjectId, String, f64)> = app
        .doc
        .scene
        .variants
        .get(a)
        .map(|v| v.overrides.iter().take(MAX_ROWS).map(|o| (o.object, o.path.clone(), app.doc.scene.attr(o.object, &o.path).unwrap_or(o.value))).collect())
        .unwrap_or_default();
    let total = app.doc.scene.variants.get(a).map_or(0, |v| v.overrides.len());
    if rows.is_empty() {
        ui.label(RichText::new(tl!("No overrides yet.")).color(theme::TEXT_DIM));
    }
    let mut clear: Option<(ObjectId, String)> = None;
    for (object, path, value) in &rows {
        let who = app.doc.scene.get(*object).map(|o| o.name.clone()).unwrap_or_else(|| tl!("(deleted)"));
        ui.horizontal(|ui| {
            let text = crate::tlf!("{object}: {attribute} = {value}", object = who, attribute = crate::anim_ui::attr_label(path), value = value_text(*value));
            ui.label(text.clone());
            let r = ui.small_button(tl!("Clear"));
            set_name(ui, &r, crate::tlf!("Clear override {object}: {attribute}", object = who, attribute = crate::anim_ui::attr_label(path)));
            if r.clicked() {
                clear = Some((*object, path.clone()));
            }
        });
    }
    if total > rows.len() {
        ui.label(RichText::new(crate::tlf!("and {count} more", count = total - rows.len())).color(theme::TEXT_DIM));
    }
    if let Some((object, path)) = clear {
        run(app, VariantCommand::ClearOverride { object, paths: vec![path] });
    }
}
