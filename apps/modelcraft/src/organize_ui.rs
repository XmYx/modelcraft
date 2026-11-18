//! Organising objects in the interface: the Organize section of Properties (layer, tags, pivot,
//! user attributes, attribute presets, instance), the Layers window and the Organize menu. All of
//! it goes through [`OrganizeCommand`], so it is scriptable, undoable and works on a whole
//! selection at once.

use egui::RichText;
use modelcraft_engine::scene::{DisplayMode, Object, ObjectId, ObjectKind, Tag, TagKind, UserValue};
use modelcraft_engine::{Command, OrganizeCommand as O};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::panels::{commit_edit, set_name, vec3_row};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Layers,
    NewLayer,
    Instance,
    MakeUnique,
    PivotToCenter,
    ToggleProtection,
}

pub const ALL: &[Op] = &[Op::Layers, Op::NewLayer, Op::Instance, Op::MakeUnique, Op::PivotToCenter, Op::ToggleProtection];

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::Layers => tl!("Layers…"),
            Op::NewLayer => tl!("New Layer from Selection"),
            Op::Instance => tl!("Create Instance"),
            Op::MakeUnique => tl!("Make Instance Unique"),
            Op::PivotToCenter => tl!("Pivot to Center"),
            Op::ToggleProtection => tl!("Protect or Unprotect"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::Layers => "organize.layers",
            Op::NewLayer => "organize.new_layer",
            Op::Instance => "organize.instance",
            Op::MakeUnique => "organize.make_unique",
            Op::PivotToCenter => "organize.pivot_to_center",
            Op::ToggleProtection => "organize.protect",
        }
    }
}

#[derive(Default)]
pub struct State {
    pub layers_open: bool,
    /// The name typed for a new layer.
    pub layer_name: String,
    /// The name typed for a new user attribute.
    pub attr_name: String,
    /// The name typed for a new preset.
    pub preset_name: String,
}

pub fn mode_label(m: DisplayMode) -> String {
    match m {
        DisplayMode::Auto => tl!("Follow the View"),
        DisplayMode::Smooth => tl!("Smooth Shaded"),
        DisplayMode::Flat => tl!("Flat Shaded"),
        DisplayMode::Wireframe => tl!("Wireframe"),
        DisplayMode::SeeThrough => tl!("See-Through"),
        DisplayMode::BoundingBox => tl!("Bounding Box"),
    }
}

fn tag_label(k: TagKind) -> String {
    match k {
        TagKind::Display => tl!("Display"),
        TagKind::Render => tl!("Render"),
        TagKind::Protection => tl!("Protection"),
        TagKind::LookAt => tl!("Look At"),
    }
}

/// The selected objects that exist, in selection order.
fn selected(app: &ModelApp) -> Vec<ObjectId> {
    app.selection.iter().copied().filter(|id| app.doc.scene.get(*id).is_some()).collect()
}

/// The objects an edit of `id` in Properties applies to: the whole selection when it holds `id`.
fn targets(app: &ModelApp, id: ObjectId) -> Vec<ObjectId> {
    let sel = selected(app);
    if sel.contains(&id) { sel } else { vec![id] }
}

fn org(app: &mut ModelApp, c: O) -> bool {
    app.run(Command::Organize(c))
}

pub fn run(app: &mut ModelApp, op: Op) {
    let sel = selected(app);
    if sel.is_empty() && !matches!(op, Op::Layers) {
        app.status = tl!("Select objects first");
        return;
    }
    match op {
        Op::Layers => app.organize.layers_open = !app.organize.layers_open,
        Op::NewLayer => {
            let name = match app.organize.layer_name.trim() {
                "" => crate::tlf!("Layer {n}", n = app.doc.scene.layers.len() + 1),
                n => n.to_string(),
            };
            if org(app, O::AddLayer { name, ids: sel }) {
                app.organize.layer_name.clear();
            }
        }
        Op::Instance => {
            org(app, O::Instance { ids: sel });
        }
        Op::MakeUnique => {
            let ids: Vec<ObjectId> =
                sel.into_iter().filter(|id| matches!(app.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Instance { .. }))).collect();
            if ids.is_empty() {
                app.status = tl!("Select an instance first");
            } else {
                org(app, O::MakeUnique { ids });
            }
        }
        Op::PivotToCenter => {
            org(app, O::PivotToCenter { ids: sel });
        }
        Op::ToggleProtection => {
            let all = sel.iter().all(|id| app.doc.scene.tag(*id, TagKind::Protection).is_some());
            if all {
                org(app, O::RemoveTag { ids: sel, kind: "protection".into() });
            } else {
                org(app, O::SetTag { ids: sel, tag: Tag::Protection { position: true, rotation: true, scale: true } });
            }
        }
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Organize"), |ui| {
        for op in ALL {
            let action = Action::Organize(*op);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                run(app, *op);
                ui.close();
            }
        }
        ui.separator();
        ui.menu_button(tl!("Display Mode"), |ui| {
            for m in [DisplayMode::Smooth, DisplayMode::Flat, DisplayMode::Wireframe, DisplayMode::SeeThrough, DisplayMode::BoundingBox] {
                let on = app.display_mode == m || (m == DisplayMode::Smooth && app.display_mode == DisplayMode::Auto);
                if ui.radio(on, mode_label(m)).clicked() {
                    app.display_mode = m;
                    ui.close();
                }
            }
        });
    });
}

/// The Organize section of Properties for the active object `id`.
pub fn properties(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some(obj) = app.doc.scene.get(id).cloned() else { return };
    egui::CollapsingHeader::new(tl!("Organization")).default_open(false).show(ui, |ui| {
        layer_row(app, ui, id, &obj);
        tags(app, ui, id, &obj);
        pivot(app, ui, id, &obj);
        user_attributes(app, ui, id, &obj);
        presets(app, ui, id);
        if let ObjectKind::Instance { of } = obj.kind {
            let master = app.doc.scene.get(of).map(|o| o.name.clone()).unwrap_or_default();
            ui.separator();
            ui.label(RichText::new(crate::tlf!("Instance of {name}", name = master)).color(theme::TEXT_DIM));
            ui.horizontal(|ui| {
                if ui.button(tl!("Select Master")).clicked() {
                    app.select_only(of);
                }
                if ui.button(tl!("Make Unique")).clicked() {
                    org(app, O::MakeUnique { ids: targets(app, id) });
                }
            });
        }
    });
}

fn layer_row(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, obj: &Object) {
    let layers: Vec<(u32, String)> = app.doc.scene.layers.iter().map(|l| (l.id, l.name.clone())).collect();
    let current = obj.layer.and_then(|l| layers.iter().find(|(i, _)| *i == l)).map(|(_, n)| n.clone());
    ui.horizontal(|ui| {
        ui.label(tl!("Layer"));
        let shown = current.clone().unwrap_or_else(|| tl!("No Layer"));
        let r = ui.menu_button(shown.clone(), |ui| {
            if ui.button(tl!("No Layer")).clicked() {
                org(app, O::AssignLayer { ids: targets(app, id), layer: None });
                ui.close();
            }
            for (lid, name) in &layers {
                if ui.button(name).clicked() {
                    org(app, O::AssignLayer { ids: targets(app, id), layer: Some(*lid) });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(tl!("New Layer from Selection")).clicked() {
                run(app, Op::NewLayer);
                ui.close();
            }
        });
        set_name(ui, &r.response, crate::tlf!("Layer: {name}", name = shown));
    });
}

fn tags(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, obj: &Object) {
    ui.separator();
    ui.label(RichText::new(tl!("Tags")).color(theme::TEXT_DIM));
    for tag in &obj.tags {
        ui.horizontal_wrapped(|ui| {
            match tag {
                Tag::Display { mode } => {
                    ui.label(tag_label(TagKind::Display));
                    let r = ui.menu_button(mode_label(*mode), |ui| {
                        for m in DisplayMode::ALL {
                            if ui.radio(m == *mode, mode_label(m)).clicked() {
                                org(app, O::SetTag { ids: targets(app, id), tag: Tag::Display { mode: m } });
                                ui.close();
                            }
                        }
                    });
                    set_name(ui, &r.response, tl!("Display tag mode"));
                }
                Tag::Render { visible } => {
                    let mut v = *visible;
                    if ui.checkbox(&mut v, tl!("Renders")).changed() {
                        org(app, O::SetTag { ids: targets(app, id), tag: Tag::Render { visible: v } });
                    }
                }
                Tag::Protection { position, rotation, scale } => {
                    ui.label(tag_label(TagKind::Protection));
                    for (name, value, set) in [(tl!("Position"), *position, 0), (tl!("Rotation"), *rotation, 1), (tl!("Scale"), *scale, 2)] {
                        let mut v = value;
                        let r = ui.checkbox(&mut v, name.clone());
                        set_name(ui, &r, crate::tlf!("Protect {channel}", channel = name.to_lowercase()));
                        if r.changed() {
                            let mut p = [*position, *rotation, *scale];
                            p[set] = v;
                            org(app, O::SetTag { ids: targets(app, id), tag: Tag::Protection { position: p[0], rotation: p[1], scale: p[2] } });
                        }
                    }
                }
                Tag::LookAt { target, .. } => {
                    let name = app.doc.scene.get(*target).map(|o| o.name.clone()).unwrap_or_default();
                    ui.label(crate::tlf!("Looks at {name}", name = name));
                }
            }
            let kind = tag.kind();
            let r = ui.small_button("×");
            set_name(ui, &r, crate::tlf!("Remove {tag} Tag", tag = tag_label(kind)));
            if r.clicked() {
                org(app, O::RemoveTag { ids: targets(app, id), kind: kind.id().into() });
            }
        });
    }
    let r = ui.menu_button(tl!("Add Tag"), |ui| {
        for kind in TagKind::ALL {
            if obj.tags.iter().any(|t| t.kind() == kind) {
                continue;
            }
            let label = crate::tlf!("{tag} Tag", tag = tag_label(kind));
            if ui.button(label).clicked() {
                let tag = match Tag::default_of(kind) {
                    Some(t) => Some(t),
                    // Look at: the other selected object is the target.
                    None => selected(app).into_iter().rev().find(|o| *o != id).map(|target| Tag::LookAt { target, constraint: 0 }),
                };
                match tag {
                    Some(tag) => {
                        org(app, O::SetTag { ids: vec![id], tag });
                    }
                    None => app.status = tl!("Select the object to look at as well"),
                }
                ui.close();
            }
        }
    });
    set_name(ui, &r.response, tl!("Add Tag"));
}

fn pivot(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, obj: &Object) {
    ui.separator();
    let mut p = obj.pivot;
    let rows = vec3_row(ui, &tl!("Pivot"), &mut p, 0.01);
    for r in &rows {
        if r.changed() || r.drag_stopped() {
            // Moving the pivot keeps the object where it is, in the preview too.
            commit_edit(
                app,
                r,
                |o| {
                    o.set_pivot(p, true);
                },
                || Command::Organize(O::SetPivot { ids: vec![id], pivot: p.to_array(), keep_in_place: true }),
                id,
            );
            break;
        }
    }
    ui.horizontal(|ui| {
        if ui.button(tl!("Pivot to Center")).clicked() {
            org(app, O::PivotToCenter { ids: targets(app, id) });
        }
        if ui.add_enabled(obj.pivot != glam::Vec3::ZERO, egui::Button::new(tl!("Reset Pivot"))).clicked() {
            org(app, O::SetPivot { ids: targets(app, id), pivot: [0.0; 3], keep_in_place: true });
        }
    });
}

fn user_attributes(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, obj: &Object) {
    ui.separator();
    ui.label(RichText::new(tl!("User Attributes")).color(theme::TEXT_DIM));
    for (name, value) in &obj.user {
        let set = |app: &mut ModelApp, v: UserValue| {
            org(app, O::SetUserAttr { ids: targets(app, id), name: name.clone(), value: v });
        };
        ui.horizontal(|ui| {
            ui.label(name);
            match value {
                UserValue::Number(n) => {
                    let mut v = *n as f32;
                    let r = ui.add(egui::DragValue::new(&mut v).speed(0.01).max_decimals(4));
                    set_name(ui, &r, name.clone());
                    let nm = name.clone();
                    commit_edit(
                        app,
                        &r,
                        |o| {
                            o.user.insert(nm, UserValue::Number(v as f64));
                        },
                        || Command::Organize(O::SetUserAttr { ids: vec![id], name: name.clone(), value: UserValue::Number(v as f64) }),
                        id,
                    );
                }
                UserValue::Switch(b) => {
                    let mut v = *b;
                    let r = ui.checkbox(&mut v, "");
                    set_name(ui, &r, name.clone());
                    if r.changed() {
                        set(app, UserValue::Switch(v));
                    }
                }
                UserValue::Text(t) => {
                    let mut v = t.clone();
                    let r = ui.add(egui::TextEdit::singleline(&mut v).desired_width(120.0));
                    set_name(ui, &r, name.clone());
                    if r.lost_focus() && v != *t {
                        set(app, UserValue::Text(v));
                    }
                }
                UserValue::Colour(c) | UserValue::Vector(c) => {
                    let colour = matches!(value, UserValue::Colour(_));
                    let mut v = glam::Vec3::from_array(*c);
                    let rows = vec3_row(ui, name, &mut v, 0.01);
                    for r in &rows {
                        if r.changed() || r.drag_stopped() {
                            let new = if colour { UserValue::Colour(v.to_array()) } else { UserValue::Vector(v.to_array()) };
                            let nm = name.clone();
                            let preview = new.clone();
                            commit_edit(
                                app,
                                r,
                                |o| {
                                    o.user.insert(nm, preview);
                                },
                                || Command::Organize(O::SetUserAttr { ids: vec![id], name: name.clone(), value: new }),
                                id,
                            );
                            break;
                        }
                    }
                }
            }
            let r = ui.small_button("×");
            set_name(ui, &r, crate::tlf!("Remove attribute {name}", name = name));
            if r.clicked() {
                org(app, O::RemoveUserAttr { ids: targets(app, id), name: name.clone() });
            }
        });
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.organize.attr_name).hint_text(tl!("Attribute name")).desired_width(110.0));
        set_name(ui, &r, tl!("New attribute name"));
        let menu = ui.menu_button(tl!("Add Attribute"), |ui| {
            for (label, v) in [
                (tl!("Number"), UserValue::Number(0.0)),
                (tl!("Switch"), UserValue::Switch(false)),
                (tl!("Text"), UserValue::Text(String::new())),
                (tl!("Colour"), UserValue::Colour([1.0; 3])),
                (tl!("Vector"), UserValue::Vector([0.0; 3])),
            ] {
                if ui.button(label).clicked() {
                    let typed = app.organize.attr_name.trim().to_string();
                    let name = if typed.is_empty() { crate::tlf!("Attribute {n}", n = obj.user.len() + 1) } else { typed };
                    if org(app, O::SetUserAttr { ids: targets(app, id), name, value: v }) {
                        app.organize.attr_name.clear();
                    }
                    ui.close();
                }
            }
        });
        set_name(ui, &menu.response, tl!("Add Attribute"));
    });
}

fn presets(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    ui.separator();
    ui.label(RichText::new(tl!("Attribute Presets")).color(theme::TEXT_DIM));
    let list: Vec<String> = app.doc.scene.presets.iter().map(|p| p.name.clone()).collect();
    for (i, name) in list.iter().enumerate() {
        ui.horizontal(|ui| {
            let r = ui.button(crate::tlf!("Apply {name}", name = name));
            set_name(ui, &r, crate::tlf!("Apply Preset: {name}", name = name));
            if r.clicked() {
                org(app, O::ApplyPreset { index: i, ids: targets(app, id) });
            }
            let r = ui.small_button("×");
            set_name(ui, &r, crate::tlf!("Remove Preset: {name}", name = name));
            if r.clicked() {
                org(app, O::RemovePreset { index: i });
            }
        });
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.organize.preset_name).hint_text(tl!("Preset name")).desired_width(110.0));
        set_name(ui, &r, tl!("New preset name"));
        if ui.button(tl!("Save Preset")).clicked() {
            let name = match app.organize.preset_name.trim() {
                "" => crate::tlf!("Preset {n}", n = list.len() + 1),
                n => n.to_string(),
            };
            if org(app, O::SavePreset { from: id, name, parts: None }) {
                app.organize.preset_name.clear();
            }
        }
    });
}

/// The Layers window: every layer with its Show, Render and Lock switches.
pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.organize.layers_open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Layers")).id(egui::Id::unique("layers-window")).open(&mut open).default_width(340.0).show(ctx, |ui| {
        let layers = app.doc.scene.layers.clone();
        if layers.is_empty() {
            ui.label(RichText::new(tl!("No layers yet. Select objects and add one.")).color(theme::TEXT_DIM));
        }
        for l in &layers {
            ui.horizontal(|ui| {
                let c = l.color;
                let swatch = egui::Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.0, swatch);
                let mut name = l.name.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
                set_name(ui, &r, crate::tlf!("Layer name: {name}", name = l.name));
                if r.lost_focus() && name.trim() != l.name && !name.trim().is_empty() {
                    org(app, O::EditLayer { id: l.id, name: Some(name), color: None, visible: None, render: None, locked: None });
                }
                for (label, on, which) in [(tl!("Show"), l.visible, 0), (tl!("Render"), l.render, 1), (tl!("Lock"), l.locked, 2)] {
                    let mut v = on;
                    let r = ui.checkbox(&mut v, label.clone());
                    set_name(ui, &r, crate::tlf!("{what}: {name}", what = label, name = l.name));
                    if r.changed() {
                        let (visible, render, locked) = match which {
                            0 => (Some(v), None, None),
                            1 => (None, Some(v), None),
                            _ => (None, None, Some(v)),
                        };
                        org(app, O::EditLayer { id: l.id, name: None, color: None, visible, render, locked });
                    }
                }
                let r = ui.small_button(tl!("Select"));
                set_name(ui, &r, crate::tlf!("Select objects on {name}", name = l.name));
                if r.clicked() {
                    let ids: Vec<ObjectId> = app.doc.scene.objects().iter().filter(|o| o.layer == Some(l.id)).map(|o| o.id).collect();
                    app.selection = ids;
                    app.components.clear();
                }
                let r = ui.small_button("×");
                set_name(ui, &r, crate::tlf!("Delete layer {name}", name = l.name));
                if r.clicked() {
                    org(app, O::RemoveLayer { id: l.id });
                }
            });
        }
        ui.separator();
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.organize.layer_name).hint_text(tl!("Layer name")).desired_width(130.0));
            set_name(ui, &r, tl!("New layer name"));
            if ui.button(tl!("New Layer from Selection")).clicked() {
                run(app, Op::NewLayer);
            }
        });
    });
    if !open {
        app.organize.layers_open = false;
    }
}
