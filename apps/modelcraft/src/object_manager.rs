//! The Scene panel as an object manager: the object tree with a search field, drag-and-drop
//! reparenting (and Move Under in the row menu for the keyboard), visibility, lock and template
//! toggles per row, selection sets, and the commands that act on many objects at once (copy and
//! paste attributes, lock, template). Edits of many objects are one undo step (`Command::Batch`).

use egui::{Color32, RichText};
use modelcraft_engine::Command;
use modelcraft_engine::scene::{Material, ObjectId, Transform};

use crate::actions::Action;
use crate::app::ModelApp;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    CopyAttributes,
    PasteAttributes,
    ToggleLock,
    ToggleTemplate,
    SaveSelectionSet,
}

pub const ALL: &[Op] = &[Op::CopyAttributes, Op::PasteAttributes, Op::ToggleLock, Op::ToggleTemplate, Op::SaveSelectionSet];

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::CopyAttributes => tl!("Copy Attributes"),
            Op::PasteAttributes => tl!("Paste Attributes"),
            Op::ToggleLock => tl!("Lock or Unlock"),
            Op::ToggleTemplate => tl!("Template On or Off"),
            Op::SaveSelectionSet => tl!("Save Selection as Set"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::CopyAttributes => "edit.copy_attributes",
            Op::PasteAttributes => "edit.paste_attributes",
            Op::ToggleLock => "edit.lock",
            Op::ToggleTemplate => "edit.template",
            Op::SaveSelectionSet => "select.save_set",
        }
    }
}

/// Attributes copied from one object, pasted onto others.
#[derive(Clone, Debug)]
pub struct Copied {
    pub transform: Transform,
    pub material: Material,
    pub visible: bool,
}

#[derive(Default)]
pub struct State {
    /// The tree's search text.
    pub filter: String,
    pub copied: Option<Copied>,
    /// The name typed for a new selection set.
    pub set_name: String,
}

/// The selected objects that exist, in selection order.
fn selected(app: &ModelApp) -> Vec<ObjectId> {
    app.selection.iter().copied().filter(|id| app.doc.scene.get(*id).is_some()).collect()
}

pub fn run(app: &mut ModelApp, op: Op) {
    let sel = selected(app);
    match op {
        Op::CopyAttributes => {
            let Some(o) = app.active().and_then(|id| app.doc.scene.get(id)) else {
                app.status = tl!("Select an object to copy from");
                return;
            };
            app.objects.copied = Some(Copied { transform: o.transform, material: o.material.clone(), visible: o.visible });
            app.status = crate::tlf!("Copied the attributes of {name}", name = o.name);
        }
        Op::PasteAttributes => {
            let Some(c) = app.objects.copied.clone() else {
                app.status = tl!("Copy attributes first");
                return;
            };
            if sel.is_empty() {
                app.status = tl!("Select objects to paste onto");
                return;
            }
            let mut commands = Vec::new();
            for id in &sel {
                commands.push(Command::SetTransform { id: *id, transform: c.transform });
                commands.push(Command::SetMaterial { id: *id, material: c.material.clone() });
                commands.push(Command::SetVisible { id: *id, visible: c.visible });
            }
            let label = format!("Paste attributes to {} objects", sel.len());
            app.run(Command::Batch { label: Some(label), commands });
        }
        Op::ToggleLock | Op::ToggleTemplate => {
            if sel.is_empty() {
                app.status = tl!("Select objects first");
                return;
            }
            let all = |f: fn(&modelcraft_engine::scene::Object) -> bool| sel.iter().all(|id| app.doc.scene.get(*id).is_some_and(f));
            if op == Op::ToggleLock {
                let locked = !all(|o| o.locked);
                app.run(Command::SetLocked { ids: sel, locked });
            } else {
                let template = !all(|o| o.template);
                app.run(Command::SetTemplate { ids: sel, template });
            }
        }
        Op::SaveSelectionSet => {
            let name = if app.objects.set_name.trim().is_empty() {
                crate::tlf!("Set {n}", n = app.doc.scene.sets.len() + 1)
            } else {
                app.objects.set_name.trim().to_string()
            };
            if app.run(Command::AddSelectionSet { name, ids: sel }) {
                app.objects.set_name.clear();
            }
        }
    }
}

/// The commands that apply one object's edit to every selected object: the change from `before`
/// to `after` (a move, a rotation, a scale factor), so each keeps its own placement.
pub fn transform_batch(app: &ModelApp, active: ObjectId, before: Transform, after: Transform) -> Command {
    let others: Vec<ObjectId> = selected(app).into_iter().filter(|id| *id != active).collect();
    if others.is_empty() {
        return Command::SetTransform { id: active, transform: after };
    }
    let dt = after.translation - before.translation;
    let dr = after.rotation * before.rotation.inverse();
    let ds = (after.scale / before.scale.max(glam::Vec3::splat(1e-6))).max(glam::Vec3::splat(1e-6));
    let mut commands = vec![Command::SetTransform { id: active, transform: after }];
    for id in &others {
        let Some(o) = app.doc.scene.get(*id) else { continue };
        if o.locked {
            continue;
        }
        let mut t = o.transform;
        t.translation += dt;
        t.rotation = (dr * t.rotation).normalize();
        t.scale *= ds;
        commands.push(Command::SetTransform { id: *id, transform: t.sanitized() });
    }
    Command::Batch { label: Some(format!("Transform {} objects", commands.len())), commands }
}

/// The material edit on every selected object.
pub fn material_batch(app: &ModelApp, active: ObjectId, material: Material) -> Command {
    let ids: Vec<ObjectId> = selected(app).into_iter().filter(|id| app.doc.scene.get(*id).is_some_and(|o| !o.locked) || *id == active).collect();
    if ids.len() <= 1 {
        return Command::SetMaterial { id: active, material };
    }
    let commands: Vec<Command> = ids.iter().map(|id| Command::SetMaterial { id: *id, material: material.clone() }).collect();
    Command::Batch { label: Some(format!("Material on {} objects", commands.len())), commands }
}

fn toggle(ui: &mut egui::Ui, on: bool, name: String, draw: impl FnOnce(&egui::Painter, egui::Rect, bool, Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, &name));
    let col = if resp.hovered() || resp.has_focus() { Color32::WHITE } else { theme::TEXT_DIM };
    draw(ui.painter(), rect, on, col);
    resp
}

fn eye(p: &egui::Painter, r: egui::Rect, on: bool, col: Color32) {
    if on {
        p.circle_filled(r.center(), 3.5, col);
    } else {
        p.circle_stroke(r.center(), 3.5, egui::Stroke::new(1.0, col));
    }
}

/// A small padlock: a shackle over a body, filled when locked.
fn padlock(p: &egui::Painter, r: egui::Rect, on: bool, col: Color32) {
    let c = r.center();
    let body = egui::Rect::from_center_size(c + egui::vec2(0.0, 2.0), egui::vec2(8.0, 6.0));
    if on {
        p.rect_filled(body, 1.0, col);
    } else {
        p.rect_stroke(body, 1.0, egui::Stroke::new(1.0, col), egui::StrokeKind::Inside);
    }
    let top = body.top();
    let pts = vec![egui::pos2(c.x - 2.5, top), egui::pos2(c.x - 2.5, top - 3.0), egui::pos2(c.x + 2.5, top - 3.0), egui::pos2(c.x + 2.5, top)];
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.0, col)));
}

/// A dashed square: drawn for reference only.
fn template_mark(p: &egui::Painter, r: egui::Rect, on: bool, col: Color32) {
    let s = r.shrink(3.0);
    if on {
        for (a, b) in [(s.left_top(), s.right_top()), (s.right_top(), s.right_bottom()), (s.right_bottom(), s.left_bottom()), (s.left_bottom(), s.left_top())] {
            p.extend(egui::Shape::dashed_line(&[a, b], egui::Stroke::new(1.0, col), 2.0, 2.0));
        }
    } else {
        p.circle_filled(s.center(), 1.0, col);
    }
}

/// Rows shown for a search: matches and their ancestors (so a match keeps its place).
fn shown_rows(app: &ModelApp, order: &[(ObjectId, usize)]) -> std::collections::HashSet<ObjectId> {
    let needle = app.objects.filter.trim().to_lowercase();
    let scene = &app.doc.scene;
    let mut keep = std::collections::HashSet::new();
    for (id, _) in order {
        let Some(o) = scene.get(*id) else { continue };
        if needle.is_empty() || o.name.to_lowercase().contains(&needle) || tl!(o.kind.type_name()).to_lowercase().contains(&needle) {
            let mut cur = Some(*id);
            let mut guard = 0;
            while let Some(c) = cur {
                if !keep.insert(c) || guard > 128 {
                    break;
                }
                guard += 1;
                cur = scene.get(c).and_then(|o| o.parent);
            }
        }
    }
    keep
}

pub fn tree(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let order = app.doc.scene.tree_order();
    if order.is_empty() {
        ui.label(RichText::new(tl!("The scene is empty. Add a shape from the Create menu or the command palette.")).color(theme::TEXT_DIM));
        return;
    }
    ui.horizontal(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.objects.filter).hint_text(tl!("Search objects")).desired_width(ui.available_width() - 24.0));
        ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("Search objects")));
        if !app.objects.filter.is_empty() && ui.small_button("×").on_hover_text(tl!("Clear the search")).clicked() {
            app.objects.filter.clear();
        }
    });
    let keep = shown_rows(app, &order);
    if keep.is_empty() {
        ui.label(RichText::new(tl!("No object matches the search.")).color(theme::TEXT_DIM));
    }
    let tree_id = ui.make_persistent_id("scene-tree");
    let visible_rows: Vec<(ObjectId, usize)> = order.iter().copied().filter(|(id, _)| keep.contains(id)).collect();
    let mut move_to: Option<(usize, i32)> = None;
    let mut toggle_vis: Option<(ObjectId, bool)> = None;
    let mut toggle_lock: Option<(ObjectId, bool)> = None;
    let mut toggle_template: Option<(ObjectId, bool)> = None;
    let mut reparent: Option<(ObjectId, Option<ObjectId>)> = None;
    for (row, (id, depth)) in visible_rows.iter().enumerate() {
        let Some(o) = app.doc.scene.get(*id).cloned() else { continue };
        let selected = app.selection.contains(id);
        let name = o.name.clone();
        ui.horizontal(|ui| {
            ui.add_space(*depth as f32 * 14.0);
            if toggle(ui, o.visible, crate::tlf!("Visible: {name}", name = name), eye).clicked() {
                toggle_vis = Some((*id, !o.visible));
            }
            if toggle(ui, o.locked, crate::tlf!("Locked: {name}", name = name), padlock).clicked() {
                toggle_lock = Some((*id, !o.locked));
            }
            if toggle(ui, o.template, crate::tlf!("Template: {name}", name = name), template_mark).clicked() {
                toggle_template = Some((*id, !o.template));
            }
            crate::panels::kind_badge(ui, &o);
            let mut text = RichText::new(&name);
            if !o.visible || o.template {
                text = text.color(theme::TEXT_DIM);
            }
            if o.template {
                text = text.italics();
            }
            let r = ui.add(egui::Button::selectable(selected, text).frame_when_inactive(false).sense(egui::Sense::click_and_drag()));
            ui.ctx().accesskit_node_builder(r.id, |n| {
                n.set_role(egui::accesskit::Role::TreeItem);
                n.set_label(name.clone());
                n.set_level(depth + 1);
                n.set_selected(selected);
            });
            // Drag a row onto another to put it under that one.
            r.dnd_set_drag_payload(*id);
            if let Some(dragged) = r.dnd_release_payload::<ObjectId>()
                && *dragged != *id
            {
                reparent = Some((*dragged, Some(*id)));
            }
            if r.dnd_hover_payload::<ObjectId>().is_some_and(|d| *d != *id) {
                ui.painter().rect_stroke(r.rect.expand(1.0), 2.0, egui::Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Outside);
            }
            if r.clicked() {
                let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                if add {
                    if let Some(p) = app.selection.iter().position(|s| s == id) {
                        app.selection.remove(p);
                    } else {
                        app.selection.push(*id);
                    }
                } else {
                    app.select_only(*id);
                }
            }
            if r.double_clicked() {
                app.rename = Some((*id, name.clone()));
            }
            if app.tree_focus == Some(*id) {
                r.request_focus();
                app.tree_focus = None;
            }
            if r.has_focus() {
                // Take the arrows so egui's own focus navigation does not move focus elsewhere.
                let (up, down) =
                    ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)));
                if up {
                    move_to = Some((row, -1));
                } else if down {
                    move_to = Some((row, 1));
                }
            }
            r.context_menu(|ui| {
                if ui.button(Action::Rename.label()).clicked() {
                    app.rename = Some((*id, name.clone()));
                    ui.close();
                }
                ui.menu_button(tl!("Move Under"), |ui| {
                    if ui.button(tl!("Top Level")).clicked() {
                        reparent = Some((*id, None));
                        ui.close();
                    }
                    for (other, _) in &order {
                        if other == id {
                            continue;
                        }
                        let n = app.doc.scene.get(*other).map(|o| o.name.clone()).unwrap_or_default();
                        if ui.button(n).clicked() {
                            reparent = Some((*id, Some(*other)));
                            ui.close();
                        }
                    }
                });
                if ui.button(if o.locked { tl!("Unlock") } else { tl!("Lock") }).clicked() {
                    toggle_lock = Some((*id, !o.locked));
                    ui.close();
                }
                if ui.button(if o.template { tl!("Template Off") } else { tl!("Template On") }).clicked() {
                    toggle_template = Some((*id, !o.template));
                    ui.close();
                }
                if ui.button(Action::Duplicate.label()).clicked() {
                    app.selection = vec![*id];
                    app.act(&ctx, Action::Duplicate);
                    ui.close();
                }
                if ui.button(Action::Delete.label()).clicked() {
                    app.selection = vec![*id];
                    app.act(&ctx, Action::Delete);
                    ui.close();
                }
            });
            ui.memory_mut(|m| m.data.insert_temp(tree_id.with(row), r.id));
        });
    }
    // Dropping below the rows puts the object at the top level.
    let (rest, drop) = ui.allocate_at_least(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    if let Some(dragged) = drop.dnd_release_payload::<ObjectId>() {
        reparent = Some((*dragged, None));
    } else if drop.dnd_hover_payload::<ObjectId>().is_some() {
        ui.painter().text(rest.left_center(), egui::Align2::LEFT_CENTER, tl!("Drop here for the top level"), egui::FontId::proportional(11.0), theme::ACCENT);
    }
    if let Some((row, dir)) = move_to {
        let next = (row as i32 + dir).clamp(0, visible_rows.len() as i32 - 1) as usize;
        if let Some((id, _)) = visible_rows.get(next) {
            app.select_only(*id);
            app.tree_focus = Some(*id);
            if let Some(wid) = ui.memory(|m| m.data.get_temp::<egui::Id>(tree_id.with(next))) {
                ui.memory_mut(|m| m.request_focus(wid));
            }
        }
    }
    if let Some((id, visible)) = toggle_vis {
        app.run(Command::SetVisible { id, visible });
    }
    if let Some((id, locked)) = toggle_lock {
        app.run(Command::SetLocked { ids: vec![id], locked });
    }
    if let Some((id, template)) = toggle_template {
        app.run(Command::SetTemplate { ids: vec![id], template });
    }
    if let Some((id, parent)) = reparent {
        app.run(Command::Reparent { id, parent });
    }
    selection_sets(app, ui);
}

fn selection_sets(app: &mut ModelApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new(tl!("Selection Sets")).default_open(!app.doc.scene.sets.is_empty()).show(ui, |ui| {
        let sets = app.doc.scene.sets.clone();
        let mut remove = None;
        for (i, s) in sets.iter().enumerate() {
            ui.horizontal(|ui| {
                let r = ui.button(crate::tlf!("{name} ({count})", name = s.name, count = s.ids.len()));
                ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(crate::tlf!("Select set {name}", name = s.name)));
                if r.clicked() {
                    app.selection = s.ids.iter().copied().filter(|id| app.doc.scene.get(*id).is_some()).collect();
                    app.status = crate::tlf!("Selected the set {name}", name = s.name);
                }
                let x = ui.small_button("×");
                ui.ctx().accesskit_node_builder(x.id, |n| n.set_label(crate::tlf!("Remove set {name}", name = s.name)));
                if x.clicked() {
                    remove = Some(i);
                }
            });
        }
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.objects.set_name).hint_text(tl!("Set name")).desired_width(120.0));
            ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("Set name")));
            if ui.button(Op::SaveSelectionSet.label()).clicked() {
                run(app, Op::SaveSelectionSet);
            }
        });
        if let Some(index) = remove {
            app.run(Command::RemoveSelectionSet { index });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Quat, Vec3};

    #[test]
    fn multi_edit_applies_the_change_not_the_value() {
        let ctx = egui::Context::default();
        let mut app = ModelApp::with_context(&ctx, crate::args::Cli::default());
        let mut ids = Vec::new();
        for (k, x) in [("cube", 0.0), ("sphere", 3.0)] {
            let id = app.doc.execute(Command::AddPrimitive { kind: k.into(), name: None, parent: None }).unwrap().created[0];
            let t = Transform { translation: Vec3::new(x, 0.0, 0.0), ..Default::default() };
            app.doc.execute(Command::SetTransform { id, transform: t }).unwrap();
            ids.push(id);
        }
        app.selection = ids.clone();
        let before = app.doc.scene.get(ids[1]).unwrap().transform;
        let mut after = before;
        after.translation.y += 2.0;
        after.rotation = Quat::from_rotation_y(0.5);
        after.scale = Vec3::splat(2.0);
        let c = transform_batch(&app, ids[1], before, after);
        app.doc.execute(c).unwrap();
        let a = app.doc.scene.get(ids[0]).unwrap().transform;
        assert!((a.translation - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-5, "{:?}", a.translation);
        assert!((a.scale - Vec3::splat(2.0)).length() < 1e-5);
        assert!(a.rotation.angle_between(Quat::from_rotation_y(0.5)) < 1e-4);
        assert_eq!(app.doc.undo.labels().last().map(String::as_str), Some("Transform 2 objects"));
    }
}
