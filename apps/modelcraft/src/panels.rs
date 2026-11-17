//! Menus, toolbar, tool options, status bar, the dockable panels (scene tree, properties with the
//! build history, brushes, paint) and the dialogs (command palette, numeric transform, rename,
//! recovery, about).

use egui::{Color32, RichText};
use glam::Vec3;
use modelcraft_engine::Command;
use modelcraft_engine::ops::deform::Deformer;
use modelcraft_engine::ops::primitives::Primitive;
use modelcraft_engine::scene::{Object, ObjectId, ObjectKind, Source, StepOp};
use modelcraft_engine::sculpt::{BrushKind, Falloff, Stamp};

use crate::actions::{self, Action, DEFORMERS, PRIMITIVES};
use crate::app::{Dock, ModelApp, NavPreset, PanelId, SelectMode, Tool, Workspace};
use crate::theme;

fn set_name(ui: &egui::Ui, resp: &egui::Response, name: impl Into<String>) {
    let name = name.into();
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(name));
}

fn action_button(app: &mut ModelApp, ui: &mut egui::Ui, a: Action) {
    let ctx = ui.ctx().clone();
    let b = egui::Button::new(a.label()).shortcut_text(a.shortcut_text(app));
    if ui.add(b).clicked() {
        app.act(&ctx, a);
        ui.close();
    }
}

pub fn menu_bar(app: &mut ModelApp, ui: &mut egui::Ui) {
    egui::Panel::top("menu").show(ui, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button(tl!("File"), |ui| {
                for a in [Action::New, Action::Open] {
                    action_button(app, ui, a);
                }
                ui.menu_button(tl!("Open Recent"), |ui| {
                    if app.recent.is_empty() {
                        ui.label(RichText::new(tl!("No recent files")).color(theme::TEXT_DIM));
                    }
                    for r in app.recent.clone() {
                        if ui.button(&r).clicked() {
                            app.guard(crate::app::Pending::OpenPath(r.into()));
                            ui.close();
                        }
                    }
                });
                for a in [Action::Save, Action::SaveAs, Action::SaveIncremental, Action::Import] {
                    action_button(app, ui, a);
                }
                ui.menu_button(tl!("Export"), |ui| {
                    for a in [Action::ExportGlb, Action::ExportObj, Action::ExportStl] {
                        action_button(app, ui, a);
                    }
                });
                ui.separator();
                if ui.button(tl!("Quit")).clicked() {
                    app.guard(crate::app::Pending::Quit);
                }
            });
            ui.menu_button(tl!("Edit"), |ui| {
                for a in [
                    Action::Undo,
                    Action::Redo,
                    Action::Delete,
                    Action::Duplicate,
                    Action::Rename,
                    Action::TransformDialog,
                    Action::Palette,
                    Action::KeyboardShortcuts,
                ] {
                    action_button(app, ui, a);
                }
            });
            ui.menu_button(tl!("Select"), |ui| {
                for a in [Action::SelectAll, Action::SelectNone, Action::SelectLoop, Action::SelectRing, Action::GrowSelection, Action::ShrinkSelection] {
                    action_button(app, ui, a);
                }
            });
            ui.menu_button(tl!("Create"), |ui| {
                for p in PRIMITIVES {
                    action_button(app, ui, Action::AddPrimitive(p));
                }
                ui.separator();
                action_button(app, ui, Action::AddCage);
                action_button(app, ui, Action::AddGroup);
                ui.separator();
                for d in DEFORMERS {
                    action_button(app, ui, Action::AddDeformer(d));
                }
            });
            ui.menu_button(tl!("Mesh"), |ui| {
                for a in [
                    Action::Extrude,
                    Action::Inset,
                    Action::Bevel,
                    Action::LoopCut,
                    Action::Merge,
                    Action::Bridge,
                    Action::FillHole,
                    Action::Mirror(0),
                    Action::Mirror(1),
                    Action::Mirror(2),
                    Action::Subdivide,
                    Action::Triangulate,
                    Action::Flip,
                    Action::Collapse,
                ] {
                    action_button(app, ui, a);
                }
            });
            ui.menu_button(tl!("Sculpt"), |ui| {
                for k in modelcraft_engine::sculpt::brush::KINDS {
                    action_button(app, ui, Action::Brush(*k));
                }
                ui.separator();
                for a in [Action::Symmetry, Action::MaskClear, Action::MaskInvert, Action::MaskFill, Action::BrushBigger, Action::BrushSmaller] {
                    action_button(app, ui, a);
                }
                ui.separator();
                for a in [Action::DetailUp, Action::DetailDown, Action::ShapeAdd, Action::LiveDetail, Action::EvenRemesh] {
                    action_button(app, ui, a);
                }
            });
            ui.menu_button(tl!("View"), |ui| {
                for ws in [Workspace::Model, Workspace::Sculpt, Workspace::Paint] {
                    action_button(app, ui, Action::Workspace(ws));
                }
                ui.separator();
                for a in [
                    Action::ToggleSnap,
                    Action::ToggleQuad,
                    Action::Frame,
                    Action::ViewPerspective,
                    Action::ViewFront,
                    Action::ViewRight,
                    Action::ViewTop,
                    Action::NextPanel,
                    Action::RenderView,
                ] {
                    action_button(app, ui, a);
                }
                ui.checkbox(&mut app.show_wire, tl!("Wireframe on Selection"));
                ui.checkbox(&mut app.show_patches, tl!("Show Patches"));
                ui.menu_button(tl!("Navigation"), |ui| {
                    for n in [NavPreset::AltMouse, NavPreset::MiddleMouse, NavPreset::RightMouse] {
                        ui.radio_value(&mut app.nav, n, n.label());
                    }
                });
                ui.menu_button(tl!("Panels"), |ui| {
                    for p in PanelId::ALL {
                        let mut shown = app.layout.dock(app.workspace, p) != Dock::Hidden;
                        if ui.checkbox(&mut shown, p.label()).changed() {
                            app.layout.set(app.workspace, p, if shown { if p == PanelId::Tree { Dock::Left } else { Dock::Right } } else { Dock::Hidden });
                        }
                    }
                });
            });
            ui.menu_button(tl!("Help"), |ui| {
                action_button(app, ui, Action::Palette);
                action_button(app, ui, Action::KeyboardShortcuts);
                action_button(app, ui, Action::About);
            });
        });
    });
}

fn tab(ui: &mut egui::Ui, on: bool, label: String, tip: String) -> bool {
    let r = ui.add(egui::Button::selectable(on, label)).on_hover_text(tip);
    r.clicked()
}

pub fn toolbar(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::Panel::top("toolbar").frame(egui::Frame::NONE.fill(theme::PANEL_DEEP).inner_margin(egui::Margin::symmetric(6, 3))).show(ui, |ui| {
        ui.horizontal(|ui| {
            for ws in [Workspace::Model, Workspace::Sculpt, Workspace::Paint] {
                let a = Action::Workspace(ws);
                if tab(ui, app.workspace == ws, ws.label(), format!("{} ({})", a.label(), a.shortcut_text(app))) {
                    app.act(&ctx, a);
                }
            }
            ui.separator();
            if app.workspace == Workspace::Model {
                for (m, a, l) in [
                    (SelectMode::Object, Action::ModeObject, tl!("Object")),
                    (SelectMode::Vertex, Action::ModeVertex, tl!("Vertex")),
                    (SelectMode::Edge, Action::ModeEdge, tl!("Edge")),
                    (SelectMode::Face, Action::ModeFace, tl!("Face")),
                ] {
                    if tab(ui, app.mode == m, l, format!("{} ({})", a.label(), a.shortcut_text(app))) {
                        app.act(&ctx, a);
                    }
                }
                ui.separator();
                for (t, a, l) in [
                    (Tool::Select, Action::ToolSelect, tl!("Select")),
                    (Tool::Move, Action::ToolMove, tl!("Move")),
                    (Tool::Rotate, Action::ToolRotate, tl!("Rotate")),
                    (Tool::Scale, Action::ToolScale, tl!("Scale")),
                ] {
                    if tab(ui, app.tool == t, l, format!("{} ({})", a.label(), a.shortcut_text(app))) {
                        app.act(&ctx, a);
                    }
                }
                ui.separator();
                ui.menu_button(tl!("Add"), |ui| {
                    for p in PRIMITIVES {
                        action_button(app, ui, Action::AddPrimitive(p));
                    }
                    ui.separator();
                    action_button(app, ui, Action::AddCage);
                    for d in DEFORMERS {
                        action_button(app, ui, Action::AddDeformer(d));
                    }
                });
            }
            if app.workspace == Workspace::Model {
                let tip = format!("{} ({})", Action::ToggleSnap.label(), Action::ToggleSnap.shortcut_text(app));
                if tab(ui, app.snap, tl!("Snap"), tip) {
                    app.act(&ctx, Action::ToggleSnap);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if tab(ui, app.quad, tl!("Four Views"), Action::ToggleQuad.label()) {
                    app.quad = !app.quad;
                }
                let nav = egui::ComboBox::from_id_salt("nav")
                    .selected_text(app.nav.label())
                    .show_ui(ui, |ui| {
                        for n in [NavPreset::AltMouse, NavPreset::MiddleMouse, NavPreset::RightMouse] {
                            ui.selectable_value(&mut app.nav, n, n.label());
                        }
                    })
                    .response
                    .on_hover_text(tl!("Navigation"));
                set_name(ui, &nav, tl!("Navigation"));
            });
        });
    });
}

pub(crate) fn drag(ui: &mut egui::Ui, v: &mut f32, speed: f32, range: std::ops::RangeInclusive<f32>, name: String) -> egui::Response {
    ui.label(name.clone());
    let r = ui.add(egui::DragValue::new(v).speed(speed).range(range).max_decimals(3));
    set_name(ui, &r, name);
    r
}

pub fn tool_options(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::Panel::top("tool-options").frame(egui::Frame::NONE.fill(theme::PANEL).inner_margin(egui::Margin::symmetric(8, 3))).show(ui, |ui| {
        ui.horizontal(|ui| match app.workspace {
            Workspace::Model => match app.mode {
                SelectMode::Face => {
                    let mut p = app.ops;
                    if ui.button(Action::Extrude.label()).clicked() {
                        app.act(&ctx, Action::Extrude);
                    }
                    drag(ui, &mut p.extrude, 0.01, -100.0..=100.0, tl!("Distance"));
                    ui.separator();
                    if ui.button(Action::Inset.label()).clicked() {
                        app.act(&ctx, Action::Inset);
                    }
                    drag(ui, &mut p.inset, 0.01, 0.0..=0.99, tl!("Amount"));
                    ui.separator();
                    for a in [Action::Bridge, Action::GrowSelection, Action::ShrinkSelection, Action::Flip, Action::Delete] {
                        if ui.button(a.label()).clicked() {
                            app.act(&ctx, a);
                        }
                    }
                    app.ops = p;
                }
                SelectMode::Edge => {
                    let mut p = app.ops;
                    if ui.button(Action::Bevel.label()).clicked() {
                        app.act(&ctx, Action::Bevel);
                    }
                    drag(ui, &mut p.bevel, 0.005, 0.0..=10.0, tl!("Width"));
                    ui.separator();
                    if ui.button(Action::LoopCut.label()).clicked() {
                        app.act(&ctx, Action::LoopCut);
                    }
                    ui.label(tl!("Cuts"));
                    let r = ui.add(egui::DragValue::new(&mut p.cuts).range(1..=64));
                    set_name(ui, &r, tl!("Cuts"));
                    ui.separator();
                    for a in [Action::SelectLoop, Action::SelectRing, Action::FillHole, Action::Delete] {
                        if ui.button(a.label()).clicked() {
                            app.act(&ctx, a);
                        }
                    }
                    app.ops = p;
                }
                SelectMode::Vertex => {
                    for a in [Action::Merge, Action::Delete, Action::TransformDialog] {
                        if ui.button(a.label()).clicked() {
                            app.act(&ctx, a);
                        }
                    }
                }
                SelectMode::Object => {
                    let mut p = app.ops;
                    if ui.button(Action::Subdivide.label()).clicked() {
                        app.act(&ctx, Action::Subdivide);
                    }
                    ui.label(tl!("Levels"));
                    let r = ui.add(egui::DragValue::new(&mut p.subdivide).range(1..=4));
                    set_name(ui, &r, tl!("Levels"));
                    app.ops = p;
                    ui.separator();
                    for a in [Action::AddCage, Action::Mirror(0), Action::Collapse, Action::TransformDialog] {
                        if ui.button(a.label()).clicked() {
                            app.act(&ctx, a);
                        }
                    }
                }
            },
            Workspace::Sculpt => {
                ui.label(RichText::new(actions::brush_label(app.brush.kind)).color(theme::ACCENT).strong());
                ui.separator();
                ui.label(tl!("Radius"));
                let r = ui.add(egui::Slider::new(&mut app.brush_px, 4.0..=400.0).suffix(" px"));
                set_name(ui, &r, tl!("Brush radius"));
                ui.label(tl!("Strength"));
                let r = ui.add(egui::Slider::new(&mut app.brush.strength, 0.0..=1.0));
                set_name(ui, &r, tl!("Brush strength"));
                ui.separator();
                ui.checkbox(&mut app.brush.symmetry.x, tl!("Mirror X"));
                ui.checkbox(&mut app.brush.invert, tl!("Invert"));
                ui.checkbox(&mut app.brush.pressure_strength, tl!("Pressure sets strength"));
                ui.checkbox(&mut app.brush.pressure_radius, tl!("Pressure sets size"));
            }
            Workspace::Paint => {
                ui.checkbox(&mut app.show_patches, tl!("Show Patches"));
                ui.separator();
                if ui.button(tl!("Export GLB for TextureCraft…")).clicked() {
                    app.act(&ctx, Action::ExportGlb);
                }
            }
        });
    });
}

pub fn status_bar(app: &mut ModelApp, ui: &mut egui::Ui) {
    let tris = app.doc.triangle_count();
    egui::Panel::bottom("status").frame(egui::Frame::NONE.fill(theme::PANEL_DEEP).inner_margin(egui::Margin::symmetric(8, 2))).show(ui, |ui| {
        ui.horizontal(|ui| {
            let status = ui.label(RichText::new(&app.status).color(theme::TEXT));
            ui.ctx().accesskit_node_builder(status.id, |n| n.set_live(egui::accesskit::Live::Polite));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dim = |s: String| RichText::new(s).color(theme::TEXT_DIM).small();
                let ft: Vec<f32> = app.frame_times.iter().copied().collect();
                if !ft.is_empty() {
                    let mean = ft.iter().sum::<f32>() / ft.len() as f32;
                    ui.label(dim(crate::tlf!("{fps} fps", fps = format!("{:.0}", 1.0 / mean.max(1e-4)))));
                }
                match &app.gpu {
                    Some(g) => ui.label(dim(g.rs.adapter.get_info().name)),
                    None => ui.label(dim(tl!("no GPU"))),
                };
                ui.label(dim(crate::tlf!("{count} history steps", count = app.doc.undo.len())));
                ui.label(dim(crate::tlf!("{count} triangles", count = group_digits(tris))));
                if app.mode != SelectMode::Object && app.workspace == Workspace::Model {
                    ui.label(dim(crate::tlf!("{count} components selected", count = app.components.len())));
                }
                if let (Some(g), Some(_)) = (&app.gpu, &app.doc.sculpt) {
                    ui.label(dim(crate::tlf!("upload {ms} ms", ms = format!("{:.2}", g.upload_ms))));
                }
            });
        });
    });
}

pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn dock_panel(app: &mut ModelApp, ui: &mut egui::Ui, p: PanelId) {
    let idx = PanelId::ALL.iter().position(|x| *x == p).unwrap_or(0);
    ui.horizontal(|ui| {
        let header_id = egui::Id::unique(("panel-focus", idx));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 30.0, 20.0), egui::Sense::hover());
        let resp = ui.interact(rect, header_id, egui::Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, p.label()));
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            p.label().to_uppercase(),
            egui::FontId::proportional(11.0),
            if resp.has_focus() { theme::ACCENT } else { theme::TEXT_DIM },
        );
        let menu = ui.menu_button("...", |ui| {
            for (d, l) in [(Dock::Left, tl!("Dock Left")), (Dock::Right, tl!("Dock Right")), (Dock::Float, tl!("Float")), (Dock::Hidden, tl!("Hide"))] {
                if ui.button(l).clicked() {
                    app.layout.set(app.workspace, p, d);
                    ui.close();
                }
            }
        });
        set_name(ui, &menu.response, crate::tlf!("Panel options: {panel}", panel = p.label()));
    });
    ui.add_space(2.0);
    panel_body(app, ui, p);
    ui.add_space(8.0);
    ui.separator();
}

pub fn panel_body(app: &mut ModelApp, ui: &mut egui::Ui, p: PanelId) {
    match p {
        PanelId::Tree => tree(app, ui),
        PanelId::Properties => properties(app, ui),
        PanelId::Brushes => brushes(app, ui),
        PanelId::Paint => paint(app, ui),
        PanelId::History => undo_history(app, ui),
    }
}

// ---------------------------------------------------------------- scene tree

/// A small drawn badge for an object's kind (fonts may lack symbol glyphs, so nothing here is text).
fn kind_badge(ui: &mut egui::Ui, o: &Object) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center();
    let blue = Color32::from_rgb(120, 180, 240);
    match &o.kind {
        ObjectKind::Group => {
            p.rect_stroke(rect.shrink(2.0), 1.0, egui::Stroke::new(1.2, theme::TEXT_DIM), egui::StrokeKind::Inside);
        }
        ObjectKind::Mesh { steps, .. } => {
            let pts = vec![c + egui::vec2(0.0, -5.0), c + egui::vec2(5.0, 0.0), c + egui::vec2(0.0, 5.0), c + egui::vec2(-5.0, 0.0)];
            if steps.is_empty() {
                p.add(egui::Shape::closed_line(pts, egui::Stroke::new(1.2, blue)));
            } else {
                p.add(egui::Shape::convex_polygon(pts, blue, egui::Stroke::NONE));
            }
        }
        ObjectKind::SmoothCage { .. } => {
            let g = Color32::from_rgb(130, 210, 120);
            p.circle_stroke(c, 5.0, egui::Stroke::new(1.2, g));
            p.circle_filled(c, 2.0, g);
        }
        ObjectKind::Deformer { .. } => {
            let m = Color32::from_rgb(200, 140, 230);
            let pts: Vec<egui::Pos2> = (0..=10).map(|k| c + egui::vec2(k as f32 - 5.0, (k as f32 * 1.2).sin() * 3.0)).collect();
            p.add(egui::Shape::line(pts, egui::Stroke::new(1.4, m)));
        }
    }
}

/// A drawn eye toggle (filled dot shown, hollow hidden) with an accessible name.
fn visibility_toggle(ui: &mut egui::Ui, visible: bool, name: String) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, visible, &name));
    let col = if resp.hovered() { Color32::WHITE } else { theme::TEXT_DIM };
    if visible {
        ui.painter().circle_filled(rect.center(), 3.5, col);
    } else {
        ui.painter().circle_stroke(rect.center(), 3.5, egui::Stroke::new(1.0, col));
    }
    resp
}

fn tree(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let order = app.doc.scene.tree_order();
    if order.is_empty() {
        ui.label(RichText::new(tl!("The scene is empty. Add a shape from the Create menu or the command palette.")).color(theme::TEXT_DIM));
        return;
    }
    let tree_id = ui.make_persistent_id("scene-tree");
    let mut move_to: Option<(usize, i32)> = None;
    let mut toggle_vis: Option<(ObjectId, bool)> = None;
    let mut reparent: Option<(ObjectId, Option<ObjectId>)> = None;
    for (row, (id, depth)) in order.iter().enumerate() {
        let Some(o) = app.doc.scene.get(*id).cloned() else { continue };
        let selected = app.selection.contains(id);
        let name = o.name.clone();
        let visible = o.visible;
        ui.horizontal(|ui| {
            ui.add_space(*depth as f32 * 14.0);
            let vis = visibility_toggle(ui, visible, crate::tlf!("Visible: {name}", name = name));
            if vis.clicked() {
                toggle_vis = Some((*id, !visible));
            }
            kind_badge(ui, &o);
            let text = if visible { RichText::new(&name) } else { RichText::new(&name).color(theme::TEXT_DIM) };
            let r = ui.add(egui::Button::selectable(selected, text).frame_when_inactive(false));
            ui.ctx().accesskit_node_builder(r.id, |n| {
                n.set_role(egui::accesskit::Role::TreeItem);
                n.set_label(name.clone());
                n.set_level(depth + 1);
                n.set_selected(selected);
            });
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
                ui.memory_mut(|m| m.data.insert_temp(tree_id, row));
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
            // Remember the row widget ids for keyboard moves.
            ui.memory_mut(|m| m.data.insert_temp(tree_id.with(row), r.id));
        });
    }
    if let Some((row, dir)) = move_to {
        let next = (row as i32 + dir).clamp(0, order.len() as i32 - 1) as usize;
        if let Some((id, _)) = order.get(next) {
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
    if let Some((id, parent)) = reparent {
        app.run(Command::Reparent { id, parent });
    }
}

// ---------------------------------------------------------------- properties

/// Edit a value with live preview while dragging and one undo step at the end.
pub(crate) fn commit_edit(app: &mut ModelApp, resp: &egui::Response, preview: impl FnOnce(&mut Object), cmd: impl FnOnce() -> Command, id: ObjectId) {
    if resp.changed() && resp.dragged() {
        if app.prop_snapshot.is_none() {
            app.prop_snapshot = Some(Box::new(app.doc.scene.clone()));
        }
        if let Ok(o) = app.doc.scene.edit(id) {
            preview(o);
        }
        return;
    }
    let end = resp.drag_stopped() || (resp.changed() && !resp.dragged());
    if !end {
        if resp.changed()
            && let Ok(o) = app.doc.scene.edit(id)
        {
            preview(o);
        }
        return;
    }
    if let Some(s) = app.prop_snapshot.take() {
        app.doc.scene = *s;
    }
    app.run(cmd());
}

fn vec3_row(ui: &mut egui::Ui, label: &str, v: &mut Vec3, speed: f32) -> Vec<egui::Response> {
    let mut out = Vec::new();
    ui.horizontal(|ui| {
        ui.add_sized([62.0, 18.0], egui::Label::new(RichText::new(label).color(theme::TEXT_DIM)));
        for (axis, c, val) in [("X", theme::AXIS_X, &mut v.x), ("Y", theme::AXIS_Y, &mut v.y), ("Z", theme::AXIS_Z, &mut v.z)] {
            ui.label(RichText::new(axis).color(c).small());
            let r = ui.add_sized([58.0, 18.0], egui::DragValue::new(val).speed(speed).max_decimals(3));
            set_name(ui, &r, format!("{label} {axis}"));
            out.push(r);
        }
    });
    out
}

fn properties(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let Some(id) = app.active() else {
        ui.label(RichText::new(tl!("Nothing selected.")).color(theme::TEXT_DIM));
        return;
    };
    let Some(obj) = app.doc.scene.get(id).cloned() else { return };
    ui.horizontal(|ui| {
        kind_badge(ui, &obj);
        ui.label(RichText::new(&obj.name).strong());
        ui.label(RichText::new(tl!(obj.kind.type_name())).color(theme::TEXT_DIM).small());
    });
    // Transform.
    egui::CollapsingHeader::new(tl!("Transform")).default_open(true).show(ui, |ui| {
        let mut t = obj.transform;
        let mut euler = t.euler_degrees();
        let pos = vec3_row(ui, &tl!("Position"), &mut t.translation, 0.01);
        let rot = vec3_row(ui, &tl!("Rotation"), &mut euler, 0.5);
        let scl = vec3_row(ui, &tl!("Scale"), &mut t.scale, 0.01);
        t.set_euler_degrees(euler);
        let t = t.sanitized();
        for r in pos.iter().chain(&rot).chain(&scl) {
            if r.changed() || r.drag_stopped() {
                commit_edit(app, r, |o| o.transform = t, || Command::SetTransform { id, transform: t }, id);
                break;
            }
        }
    });
    match &obj.kind {
        ObjectKind::Mesh { source: Source::Primitive { primitive }, .. } => {
            egui::CollapsingHeader::new(tl!("Shape")).default_open(true).show(ui, |ui| primitive_fields(app, ui, id, *primitive));
        }
        ObjectKind::SmoothCage { levels } => {
            egui::CollapsingHeader::new(tl!("Smooth Cage")).default_open(true).show(ui, |ui| {
                let mut l = *levels;
                ui.horizontal(|ui| {
                    ui.label(tl!("Levels"));
                    let r = ui.add(egui::DragValue::new(&mut l).range(0..=6));
                    set_name(ui, &r, tl!("Smooth cage levels"));
                    commit_edit(app, &r, |o| o.kind = ObjectKind::SmoothCage { levels: l }, || Command::SetCageLevels { id, levels: l }, id);
                });
                ui.label(RichText::new(tl!("Shows its first mesh child subdivided. Edit the child; the cage follows.")).color(theme::TEXT_DIM).small());
            });
        }
        ObjectKind::Deformer { deformer } => {
            egui::CollapsingHeader::new(actions::deformer_label(deformer.kind())).default_open(true).show(ui, |ui| deformer_fields(app, ui, id, *deformer));
        }
        _ => {}
    }
    if matches!(obj.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. }) {
        egui::CollapsingHeader::new(tl!("Material")).default_open(false).show(ui, |ui| {
            let mut m = obj.material.clone();
            let mut rgb = [m.base_color[0], m.base_color[1], m.base_color[2]];
            ui.horizontal(|ui| {
                ui.label(tl!("Base colour"));
                let r = ui.color_edit_button_rgb(&mut rgb);
                set_name(ui, &r, tl!("Base colour"));
                m.base_color = [rgb[0], rgb[1], rgb[2], m.base_color[3]];
                let mm = m.clone();
                commit_edit(app, &r, |o| o.material = mm.clone(), || Command::SetMaterial { id, material: mm.clone() }, id);
            });
            let mut m2 = obj.material.clone();
            ui.horizontal(|ui| {
                let r = drag(ui, &mut m2.metallic, 0.01, 0.0..=1.0, tl!("Metallic"));
                let mm = m2.clone();
                commit_edit(app, &r, |o| o.material = mm.clone(), || Command::SetMaterial { id, material: mm.clone() }, id);
            });
            let mut m3 = obj.material.clone();
            ui.horizontal(|ui| {
                let r = drag(ui, &mut m3.roughness, 0.01, 0.0..=1.0, tl!("Roughness"));
                let mm = m3.clone();
                commit_edit(app, &r, |o| o.material = mm.clone(), || Command::SetMaterial { id, material: mm.clone() }, id);
            });
            crate::material_ui::rows(app, ui, id, &obj);
            ui.horizontal(|ui| {
                ui.label(tl!("MaterialCraft link"));
                ui.label(RichText::new(obj.material.linked.clone().unwrap_or_else(|| tl!("none"))).color(theme::TEXT_DIM));
                if ui.button(tl!("Link .mcmat…")).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter("MaterialCraft", &["mcmat"]).pick_file()
                {
                    let mut m = obj.material.clone();
                    m.linked = Some(p.display().to_string());
                    app.run(Command::SetMaterial { id, material: m });
                }
            });
        });
    }
    if let Some(target) = app.edit_target() {
        history(app, ui, target);
    }
    if let Some((verts, tris)) = app.doc.sculpt.as_ref().map(|s| (s.mesh.vertex_count(), s.mesh.triangle_count())) {
        egui::CollapsingHeader::new(tl!("Sculpt")).default_open(true).show(ui, |ui| {
            ui.label(crate::tlf!("{count} vertices", count = group_digits(verts)));
            ui.label(crate::tlf!("{count} triangles", count = group_digits(tris)));
            ui.horizontal_wrapped(|ui| {
                for a in [Action::MaskClear, Action::MaskInvert, Action::MaskFill] {
                    if ui.button(a.label()).clicked() {
                        app.act(&ctx, a);
                    }
                }
            });
        });
    }
}

fn history(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let ctx = ui.ctx().clone();
    let Some(obj) = app.doc.scene.get(id).cloned() else { return };
    let errors = app.doc.eval.evaluate(&app.doc.scene, id).errors;
    egui::CollapsingHeader::new(tl!("Build History")).default_open(true).show(ui, |ui| {
        let base = match &obj.kind {
            ObjectKind::Mesh { source: Source::Primitive { primitive }, .. } => crate::tlf!("Source: {shape}", shape = primitive.kind().replace('_', " ")),
            _ => tl!("Source: stored mesh"),
        };
        ui.label(RichText::new(base).color(theme::TEXT_DIM));
        let steps = obj.steps().to_vec();
        if steps.is_empty() {
            ui.label(RichText::new(tl!("No steps yet: edit components to build history.")).color(theme::TEXT_DIM).small());
        }
        let mut remove = None;
        for (i, step) in steps.iter().enumerate() {
            let err = errors.iter().find(|e| e.step == i);
            ui.horizontal(|ui| {
                let mut on = step.enabled;
                let label = tl!(step.op.name());
                let r = ui.checkbox(&mut on, format!("{}. {}", i + 1, label));
                set_name(ui, &r, crate::tlf!("Step {n}: {name}", n = i + 1, name = label));
                if r.changed() {
                    app.run(Command::EnableStep { id, index: i, enabled: on });
                }
                if let Some(e) = err {
                    ui.label(RichText::new("!").strong().color(Color32::from_rgb(240, 180, 60))).on_hover_text(&e.message);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let x = ui.small_button("x");
                    set_name(ui, &x, crate::tlf!("Remove step {n}", n = i + 1));
                    if x.clicked() {
                        remove = Some(i);
                    }
                });
            });
            step_fields(app, ui, id, i, &step.op);
        }
        if let Some(i) = remove {
            app.run(Command::RemoveStep { id, index: i });
        }
        if !steps.is_empty() && ui.button(Action::Collapse.label()).clicked() {
            app.act(&ctx, Action::Collapse);
        }
    });
}

fn step_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, i: usize, op: &StepOp) {
    let mut edited = op.clone();
    let r = ui.indent(("step", i), |ui| {
        ui.horizontal(|ui| match &mut edited {
            StepOp::Extrude { distance, .. } => Some(drag(ui, distance, 0.01, -100.0..=100.0, tl!("Distance"))),
            StepOp::Inset { amount, .. } => Some(drag(ui, amount, 0.01, 0.0..=0.99, tl!("Amount"))),
            StepOp::Bevel { width, .. } => Some(drag(ui, width, 0.005, 0.0..=10.0, tl!("Width"))),
            StepOp::Weld { distance } => Some(drag(ui, distance, 0.0001, 0.0..=1.0, tl!("Distance"))),
            StepOp::LoopCut { cuts, .. } => {
                ui.label(tl!("Cuts"));
                let r = ui.add(egui::DragValue::new(cuts).range(1..=64));
                set_name(ui, &r, tl!("Cuts"));
                Some(r)
            }
            StepOp::EvenRemesh { cell } => Some(drag(ui, cell, 0.001, 0.0001..=1000.0, tl!("Cell size"))),
            StepOp::Subdivide { levels } => {
                ui.label(tl!("Levels"));
                let r = ui.add(egui::DragValue::new(levels).range(1..=4));
                set_name(ui, &r, tl!("Levels"));
                Some(r)
            }
            _ => None,
        })
        .inner
    });
    if let Some(r) = r.inner {
        let e = edited.clone();
        commit_edit(
            app,
            &r,
            |o| {
                if let ObjectKind::Mesh { steps, .. } = &mut o.kind
                    && let Some(s) = steps.get_mut(i)
                {
                    s.op = e.clone();
                }
            },
            || Command::SetStep { id, index: i, op: edited.clone() },
            id,
        );
    }
}

fn primitive_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, p: Primitive) {
    let mut q = p;
    let mut rs: Vec<egui::Response> = Vec::new();
    let int = |ui: &mut egui::Ui, v: &mut u32, max: u32, name: String| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(v).range(1..=max));
        set_name(ui, &r, name);
        r
    };
    ui.horizontal_wrapped(|ui| match &mut q {
        Primitive::Cube { size, segments } => {
            rs.extend(vec3_row(ui, &tl!("Size"), size, 0.01));
            rs.push(int(ui, segments, 256, tl!("Segments")));
        }
        Primitive::Plane { width, depth, segments } => {
            rs.push(drag(ui, width, 0.01, 0.001..=1e4, tl!("Width")));
            rs.push(drag(ui, depth, 0.01, 0.001..=1e4, tl!("Depth")));
            rs.push(int(ui, segments, 1024, tl!("Segments")));
        }
        Primitive::Sphere { radius, segments, rings } => {
            rs.push(drag(ui, radius, 0.01, 0.001..=1e4, tl!("Radius")));
            rs.push(int(ui, segments, 1024, tl!("Segments")));
            rs.push(int(ui, rings, 1024, tl!("Rings")));
        }
        Primitive::RoundCube { radius, segments } => {
            rs.push(drag(ui, radius, 0.01, 0.001..=1e4, tl!("Radius")));
            rs.push(int(ui, segments, 1024, tl!("Segments")));
        }
        Primitive::Cylinder { radius, height, sides, rings, caps } => {
            rs.push(drag(ui, radius, 0.01, 0.001..=1e4, tl!("Radius")));
            rs.push(drag(ui, height, 0.01, 0.001..=1e4, tl!("Height")));
            rs.push(int(ui, sides, 1024, tl!("Sides")));
            rs.push(int(ui, rings, 1024, tl!("Rings")));
            rs.push(ui.checkbox(caps, tl!("Caps")));
        }
        Primitive::Cone { radius, height, sides, caps } => {
            rs.push(drag(ui, radius, 0.01, 0.001..=1e4, tl!("Radius")));
            rs.push(drag(ui, height, 0.01, 0.001..=1e4, tl!("Height")));
            rs.push(int(ui, sides, 1024, tl!("Sides")));
            rs.push(ui.checkbox(caps, tl!("Caps")));
        }
        Primitive::Torus { radius, tube, segments, sides } => {
            rs.push(drag(ui, radius, 0.01, 0.001..=1e4, tl!("Radius")));
            rs.push(drag(ui, tube, 0.01, 0.001..=1e4, tl!("Tube")));
            rs.push(int(ui, segments, 1024, tl!("Segments")));
            rs.push(int(ui, sides, 1024, tl!("Sides")));
        }
    });
    if let Some(r) = rs.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(
            app,
            r,
            |o| {
                if let ObjectKind::Mesh { source, .. } = &mut o.kind {
                    *source = Source::Primitive { primitive: q };
                }
            },
            || Command::SetPrimitive { id, primitive: q },
            id,
        );
    }
}

fn deformer_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, d: Deformer) {
    let mut q = d;
    let r = ui
        .horizontal(|ui| match &mut q {
            Deformer::Bend { angle } | Deformer::Twist { angle } => vec![drag(ui, angle, 0.5, -3600.0..=3600.0, tl!("Angle"))],
            Deformer::Taper { amount } => vec![drag(ui, amount, 0.01, -10.0..=10.0, tl!("Amount"))],
            Deformer::Ripple { amplitude, frequency } => {
                vec![drag(ui, amplitude, 0.005, -10.0..=10.0, tl!("Amplitude")), drag(ui, frequency, 0.01, 0.0..=100.0, tl!("Frequency"))]
            }
            Deformer::Round { amount } => vec![drag(ui, amount, 0.01, 0.0..=1.0, tl!("Amount"))],
        })
        .inner;
    if let Some(r) = r.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Deformer { deformer: q }, || Command::SetDeformer { id, deformer: q }, id);
    }
}

/// The undo history as a list: activating an entry undoes or redoes to just after it.
fn undo_history(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let done = app.doc.undo.labels();
    let undone = app.doc.undo.redo_labels();
    if done.is_empty() && undone.is_empty() {
        ui.label(RichText::new(tl!("Nothing to undo yet.")).color(theme::TEXT_DIM));
        return;
    }
    let mut target: Option<isize> = None;
    let n = done.len();
    let first = n.saturating_sub(200);
    if first > 0 {
        ui.label(RichText::new(crate::tlf!("{count} earlier steps", count = first)).color(theme::TEXT_DIM).small());
    }
    for (i, l) in done.iter().enumerate().skip(first) {
        let r = ui.add(egui::Button::selectable(i + 1 == n, format!("{}. {}", i + 1, l)).frame_when_inactive(false));
        set_name(ui, &r, crate::tlf!("History step {n}: {name}", n = i + 1, name = l));
        if r.clicked() {
            target = Some(i as isize + 1);
        }
    }
    for (k, l) in undone.iter().enumerate() {
        let r = ui.add(egui::Button::selectable(false, RichText::new(format!("{}. {}", n + k + 1, l)).color(theme::TEXT_DIM)).frame_when_inactive(false));
        set_name(ui, &r, crate::tlf!("Undone step {n}: {name}", n = n + k + 1, name = l));
        if r.clicked() {
            target = Some((n + k + 1) as isize);
        }
    }
    if let Some(t) = target {
        let delta = t - n as isize;
        let a = if delta < 0 { Action::Undo } else { Action::Redo };
        for _ in 0..delta.unsigned_abs() {
            app.act(&ctx, a);
        }
    }
}

// ---------------------------------------------------------------- brushes and paint

fn brushes(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::Grid::new("brush-grid").num_columns(2).spacing([4.0, 4.0]).show(ui, |ui| {
        for (n, k) in modelcraft_engine::sculpt::brush::KINDS.iter().enumerate() {
            let a = Action::Brush(*k);
            let on = app.brush.kind == *k;
            let tip = format!("{} ({})", a.label(), a.shortcut_text(app));
            let r = ui.add_sized([104.0, 22.0], egui::Button::selectable(on, actions::brush_label(*k))).on_hover_text(tip);
            if r.clicked() {
                app.act(&ctx, a);
            }
            if n % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.add_space(6.0);
    egui::Grid::new("brush-settings").num_columns(2).show(ui, |ui| {
        ui.label(tl!("Falloff"));
        let r = egui::ComboBox::from_id_salt("falloff").selected_text(falloff_label(app.brush.falloff)).show_ui(ui, |ui| {
            for f in [Falloff::Smooth, Falloff::Sphere, Falloff::Linear, Falloff::Sharp, Falloff::Constant, Falloff::Root, Falloff::Needle] {
                ui.selectable_value(&mut app.brush.falloff, f, falloff_label(f));
            }
        });
        set_name(ui, &r.response, tl!("Falloff"));
        ui.end_row();
        ui.label(tl!("Stamp"));
        let r = egui::ComboBox::from_id_salt("stamp").selected_text(stamp_label(app.brush.stamp)).show_ui(ui, |ui| {
            for s in [Stamp::None, Stamp::Noise { scale: 6.0 }, Stamp::Rings { count: 4.0 }, Stamp::Square] {
                ui.selectable_value(&mut app.brush.stamp, s, stamp_label(s));
            }
            for a in modelcraft_engine::sculpt::alpha::ALPHAS {
                let s = Stamp::Image { alpha: *a, turn: 0.0 };
                let on = matches!(app.brush.stamp, Stamp::Image { alpha, .. } if alpha == *a);
                if ui.selectable_label(on, stamp_label(s)).clicked() {
                    app.brush.stamp = s;
                }
            }
        });
        set_name(ui, &r.response, tl!("Stamp"));
        ui.end_row();
        ui.label(tl!("Symmetry"));
        ui.horizontal(|ui| {
            ui.checkbox(&mut app.brush.symmetry.x, "X");
            ui.checkbox(&mut app.brush.symmetry.y, "Y");
            ui.checkbox(&mut app.brush.symmetry.z, "Z");
        });
        ui.end_row();
    });
    crate::sculpt_ui::brush_options(app, ui);
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Ctrl inverts, Shift smooths. Pen pressure scales strength (and size when enabled).")).color(theme::TEXT_DIM).small());
    if app.doc.sculpt.is_some() && app.brush.kind == BrushKind::Mask {
        ui.horizontal_wrapped(|ui| {
            for a in [Action::MaskClear, Action::MaskInvert, Action::MaskFill] {
                if ui.button(a.label()).clicked() {
                    app.act(&ctx, a);
                }
            }
        });
    }
    crate::sculpt_ui::layers(app, ui);
}

fn falloff_label(f: Falloff) -> String {
    match f {
        Falloff::Smooth => tl!("Smooth"),
        Falloff::Sphere => tl!("Dome"),
        Falloff::Linear => tl!("Linear"),
        Falloff::Sharp => tl!("Sharp"),
        Falloff::Constant => tl!("Flat"),
        Falloff::Root => tl!("Shoulder"),
        Falloff::Needle => tl!("Needle"),
    }
}

fn stamp_label(s: Stamp) -> String {
    match s {
        Stamp::None => tl!("None"),
        Stamp::Noise { .. } => tl!("Noise"),
        Stamp::Rings { .. } => tl!("Rings"),
        Stamp::Square => tl!("Square"),
        Stamp::Image { alpha, .. } => crate::sculpt_ui::alpha_label(alpha),
    }
}

fn paint(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.checkbox(&mut app.show_patches, tl!("Show Patches"));
    ui.label(RichText::new(tl!("Texture and vertex painting are planned for a later milestone. Today, paint in TextureCraft: export a GLB (Y up, metres, UVs kept) and open it there.")).color(theme::TEXT_DIM).small());
    if ui.button(tl!("Export GLB for TextureCraft…")).clicked() {
        app.act(&ctx, Action::ExportGlb);
    }
}

// ---------------------------------------------------------------- dialogs

#[derive(Default)]
pub struct Palette {
    pub query: String,
    pub index: usize,
    pub opened: bool,
}

pub fn dialogs(app: &mut ModelApp, ctx: &egui::Context) {
    crate::keys::dialog(app, ctx);
    crate::render_view::show(app, ctx);
    save_prompt(app, ctx);
    palette(app, ctx);
    numeric(app, ctx);
    rename(app, ctx);
    if app.about {
        let mut open = true;
        egui::Window::new(tl!("About ModelCraft")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
            ctx,
            |ui| {
                ui.label(RichText::new(crate::args::version()).strong());
                ui.label(tl!("Open-source 3D modeling and sculpting, built in pure Rust. Part of the ArtCraft Crafting Apps."));
                ui.label(tl!("Licensed under MIT or Apache-2.0."));
            },
        );
        app.about = open;
    }
    if app.recovery.is_some() {
        egui::Window::new(tl!("Recover unsaved work")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            ui.label(tl!("ModelCraft closed before your last changes were saved. Replay them from the journal?"));
            ui.horizontal(|ui| {
                if ui.button(tl!("Recover")).clicked() {
                    app.recover(true);
                }
                if ui.button(tl!("Discard")).clicked() {
                    app.recover(false);
                }
            });
        });
    }
}

/// "Save changes?" before New, Open or Quit. Save is focused, so Enter saves and Esc cancels.
fn save_prompt(app: &mut ModelApp, ctx: &egui::Context) {
    if app.pending.is_none() {
        return;
    }
    let mut answer: Option<Option<bool>> = None;
    let name = app.doc.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tl!("Untitled"));
    egui::Window::new(tl!("Save changes?")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(crate::tlf!("{file} has changes that are not saved.", file = name));
        ui.label(RichText::new(tl!("The crash journal keeps them until you choose.")).color(theme::TEXT_DIM).small());
        ui.horizontal(|ui| {
            let save = ui.button(tl!("Save"));
            if !ui.memory(|m| m.focused().is_some()) {
                save.request_focus();
            }
            if save.clicked() {
                answer = Some(Some(true));
            }
            if ui.button(tl!("Don't Save")).clicked() {
                answer = Some(Some(false));
            }
            if ui.button(tl!("Cancel")).clicked() {
                answer = Some(None);
            }
        });
    });
    if let Some(a) = answer {
        app.answer_pending(a);
    }
}

fn palette(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut p) = app.palette.take() else { return };
    let q = p.query.to_lowercase();
    let matches: Vec<(Action, String)> =
        actions::all().into_iter().filter(|a| q.is_empty() || a.label().to_lowercase().contains(&q)).map(|a| (a, a.shortcut_text(app))).collect();
    let mut run = None;
    let mut keep = true;
    egui::Window::new(tl!("Command Palette")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_TOP, [0.0, 80.0]).default_width(420.0).show(
        ctx,
        |ui| {
            let edit = ui.add(egui::TextEdit::singleline(&mut p.query).hint_text(tl!("Type a command…")).desired_width(400.0));
            set_name(ui, &edit, tl!("Command search"));
            if !p.opened {
                edit.request_focus();
                p.opened = true;
            }
            let (up, down, enter, esc) = ui.input(|i| {
                (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape))
            });
            if edit.changed() {
                p.index = 0;
            }
            if down {
                p.index = (p.index + 1).min(matches.len().saturating_sub(1));
            }
            if up {
                p.index = p.index.saturating_sub(1);
            }
            if enter {
                run = matches.get(p.index).map(|m| m.0);
            }
            if esc {
                keep = false;
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (i, (a, keys)) in matches.iter().enumerate().take(200) {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::Button::selectable(i == p.index, a.label()));
                        if i == p.index {
                            r.scroll_to_me(None);
                        }
                        if r.clicked() {
                            run = Some(*a);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(keys.as_str()).color(theme::TEXT_DIM).small());
                        });
                    });
                }
            });
        },
    );
    if let Some(a) = run {
        app.act(ctx, a);
        return;
    }
    if keep {
        app.palette = Some(p);
    }
}

fn numeric(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(mut n) = app.numeric else { return };
    let mut open = true;
    let mut apply = false;
    let target = if app.mode != SelectMode::Object && !app.components.is_empty() {
        tl!("selected components")
    } else {
        app.active().and_then(|id| app.doc.scene.get(id)).map(|o| o.name.clone()).unwrap_or_else(|| tl!("nothing"))
    };
    egui::Window::new(tl!("Transform by Numbers")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(
        ctx,
        |ui| {
            ui.label(crate::tlf!("Applies to: {target}", target = target));
            let first = vec3_row(ui, &tl!("Move"), &mut n.translate, 0.01);
            vec3_row(ui, &tl!("Rotate"), &mut n.rotate_deg, 0.5);
            vec3_row(ui, &tl!("Scale"), &mut n.scale, 0.01);
            if app.numeric.is_some_and(|o| o.translate == Vec3::ZERO && o.rotate_deg == Vec3::ZERO && o.scale == Vec3::ONE)
                && !ui.memory(|m| m.focused().is_some())
                && let Some(r) = first.first()
            {
                r.request_focus();
            }
            ui.horizontal(|ui| {
                if ui.button(tl!("Apply")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    apply = true;
                }
            });
        },
    );
    if apply {
        app.apply_numeric(n);
        app.numeric = None;
        return;
    }
    app.numeric = if open { Some(n) } else { None };
}

fn rename(app: &mut ModelApp, ctx: &egui::Context) {
    let Some((id, mut name)) = app.rename.take() else { return };
    let mut open = true;
    let mut done = false;
    egui::Window::new(tl!("Rename")).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut name));
        set_name(ui, &r, tl!("New name"));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            done = true;
        }
        if ui.button(tl!("Rename")).clicked() {
            done = true;
        }
    });
    if done {
        app.run(Command::Rename { id, name });
    } else if open {
        app.rename = Some((id, name));
    }
}
