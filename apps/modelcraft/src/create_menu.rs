//! The grouped Create menu, the optional icon palettes and the action strips of the UV, Animate and
//! Groom workspaces.
//!
//! The Create menu has one submenu per kind of thing (primitives, generators, clones, deformers,
//! fields, rig, simulation); lights and cameras will join when those objects exist. The icon
//! palettes (View ▸ Icon Palettes, off by default) put the modes and tools in a column on the left
//! and the same groups, as fold-outs of icon buttons, in a column on the right. The icons are
//! drawn with the painter, so there are no asset files; every button has a translated name and
//! the same action as its menu entry.

use egui::{Align2, Color32, FontId, Painter, Rect, Sense, Stroke, vec2};

use crate::actions::{ARRAYS, Action, CLONERS, DEFORMERS, FIELDS, MODIFIERS, PRIMITIVES};
use crate::app::{ModelApp, SelectMode, Tool, Workspace};
use crate::theme;

/// One group of create commands.
pub struct Group {
    pub name: String,
    pub items: Vec<Action>,
}

/// The create commands by group, in menu order.
pub fn groups() -> Vec<Group> {
    use crate::rig_ui::RigAction;
    use crate::sim_ui::SimAction;
    let mut generators = vec![Action::AddCage];
    generators.extend(ARRAYS.iter().map(|a| Action::AddArray(a)));
    generators.extend([Action::AddSymmetry, Action::AddGroup]);
    let mut clones: Vec<Action> = CLONERS.iter().map(|c| Action::AddCloner(c)).collect();
    clones.extend(MODIFIERS.iter().map(|m| Action::AddCloneModifier(m)));
    vec![
        Group { name: tl!("Primitives"), items: PRIMITIVES.iter().map(|p| Action::AddPrimitive(p)).collect() },
        Group { name: tl!("Generators"), items: generators },
        Group { name: tl!("Clones"), items: clones },
        Group { name: tl!("Deformers"), items: DEFORMERS.iter().map(|d| Action::AddDeformer(d)).collect() },
        Group { name: tl!("Fields"), items: FIELDS.iter().map(|f| Action::AddField(f)).collect() },
        Group { name: tl!("Lights and cameras"), items: crate::stage_ui::StageAction::ALL.iter().take(7).copied().map(Action::Stage).collect() },
        Group {
            name: tl!("Rig"),
            items: [RigAction::AddJoint, RigAction::AddIk, RigAction::AddBlendShape, RigAction::AddLattice, RigAction::AddCluster].map(Action::Rig).to_vec(),
        },
        Group {
            name: tl!("Simulation"),
            items: [SimAction::AddRigidBody, SimAction::AddCollider, SimAction::AddCloth, SimAction::AddParticles].map(Action::Sim).to_vec(),
        },
    ]
}

/// The Create menu: a submenu per group.
pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Create"), |ui| {
        for g in groups() {
            ui.menu_button(g.name, |ui| {
                for a in g.items {
                    crate::panels::action_button(app, ui, a);
                }
            });
        }
    });
}

/// The actions the UV, Animate and Groom workspaces keep at hand under the toolbar.
pub fn workspace_actions(ws: Workspace) -> Vec<Action> {
    use crate::anim_ui::AnimAction;
    use crate::hair_ui::HairAction;
    use crate::uv_ui::UvAction;
    match ws {
        Workspace::Uv => {
            [UvAction::Editor, UvAction::MarkSeam, UvAction::ClearSeam, UvAction::Unwrap, UvAction::Relax, UvAction::Pack].map(Action::Uv).to_vec()
        }
        Workspace::Animate => {
            [AnimAction::PlayPause, AnimAction::PrevFrame, AnimAction::NextFrame, AnimAction::KeyTransform, AnimAction::AutoKey, AnimAction::TogglePanel]
                .map(Action::Anim)
                .to_vec()
        }
        Workspace::Groom => [HairAction::Add, HairAction::ToggleGroom, HairAction::Regrow, HairAction::Bake].map(Action::Hair).to_vec(),
        _ => Vec::new(),
    }
}

/// The workspace's action strip (nothing for Model).
pub fn workspace_strip(app: &mut ModelApp, ui: &mut egui::Ui) {
    let actions = workspace_actions(app.workspace);
    if actions.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    for a in actions {
        if ui.button(a.label()).clicked() {
            app.act(&ctx, a);
        }
    }
    ui.separator();
}

// ---------------------------------------------------------------- icon palettes

const BUTTON: f32 = 28.0;

/// Draw the icon columns when they are on.
pub fn palettes(app: &mut ModelApp, ui: &mut egui::Ui) {
    if !app.icon_palettes {
        return;
    }
    let frame = egui::Frame::NONE.fill(theme::PANEL_DEEP).inner_margin(egui::Margin::symmetric(4, 6));
    egui::Panel::left("tool-palette").resizable(false).exact_size(BUTTON + 12.0).frame(frame).show(ui, |ui| tool_column(app, ui));
    if app.workspace.models() {
        egui::Panel::right("create-palette").resizable(false).exact_size(BUTTON * 3.0 + 40.0).frame(frame).show(ui, |ui| create_column(app, ui));
    }
}

fn tool_column(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        for ws in crate::dock::WORKSPACES {
            let a = Action::Workspace(ws);
            let on = app.workspace == ws;
            let initial = ws.label().chars().next().unwrap_or('?').to_string();
            if icon_button(ui, a.label(), Some(on), |p, r, c| letter(p, r, c, &initial)).clicked() {
                app.act(&ctx, a);
            }
        }
        if app.workspace.models() {
            ui.separator();
            for (m, a, kind) in [
                (SelectMode::Object, Action::ModeObject, Glyph::Object),
                (SelectMode::Vertex, Action::ModeVertex, Glyph::Vertex),
                (SelectMode::Edge, Action::ModeEdge, Glyph::Edge),
                (SelectMode::Face, Action::ModeFace, Glyph::Face),
            ] {
                if icon_button(ui, a.label(), Some(app.mode == m), |p, r, c| glyph(p, r, c, kind)).clicked() {
                    app.act(&ctx, a);
                }
            }
            ui.separator();
            for (t, a, kind) in [
                (Tool::Select, Action::ToolSelect, Glyph::Select),
                (Tool::Move, Action::ToolMove, Glyph::Move),
                (Tool::Rotate, Action::ToolRotate, Glyph::Rotate),
                (Tool::Scale, Action::ToolScale, Glyph::Scale),
            ] {
                if icon_button(ui, a.label(), Some(app.tool == t), |p, r, c| glyph(p, r, c, kind)).clicked() {
                    app.act(&ctx, a);
                }
            }
        }
    });
}

fn create_column(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().id_salt("create-palette-scroll").show(ui, |ui| {
        for (n, g) in groups().into_iter().enumerate() {
            egui::CollapsingHeader::new(g.name).id_salt(("create-group", n)).default_open(n == 0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                    for a in g.items {
                        let name = a.label();
                        let kind = icon_of(a);
                        if icon_button(ui, name.clone(), None, |p, r, c| match &kind {
                            Icon::Shape(s) => glyph(p, r, c, *s),
                            Icon::Letter(l) => letter(p, r, c, l),
                        })
                        .clicked()
                        {
                            app.act(&ctx, a);
                        }
                    }
                });
            });
        }
    });
}

/// A square icon button. `selected` is `Some` for a toggle (a mode or a tool).
fn icon_button(ui: &mut egui::Ui, name: String, selected: Option<bool>, draw: impl FnOnce(&Painter, Rect, Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(BUTTON, BUTTON), Sense::click());
    resp.widget_info(|| match selected {
        Some(on) => egui::WidgetInfo::selected(egui::Role::Button, true, on, &name),
        None => egui::WidgetInfo::labeled(egui::Role::Button, true, &name),
    });
    let on = selected == Some(true);
    let lit = resp.hovered() || resp.has_focus();
    let bg = if on {
        theme::ACCENT.gamma_multiply(0.45)
    } else if lit {
        theme::WIDGET_HOVER
    } else {
        theme::WIDGET
    };
    let p = ui.painter();
    p.rect_filled(rect, 4.0, bg);
    if resp.has_focus() {
        p.rect_stroke(rect, 4.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
    }
    draw(p, rect.shrink(6.0), if on || lit { theme::TEXT } else { theme::TEXT_DIM });
    resp.on_hover_text(name)
}

enum Icon {
    Shape(Glyph),
    Letter(String),
}

fn icon_of(a: Action) -> Icon {
    match a {
        Action::AddPrimitive(kind) => match kind {
            "cube" => Icon::Shape(Glyph::Cube),
            "sphere" => Icon::Shape(Glyph::Sphere),
            "cylinder" => Icon::Shape(Glyph::Cylinder),
            "cone" => Icon::Shape(Glyph::Cone),
            "plane" => Icon::Shape(Glyph::Plane),
            "torus" => Icon::Shape(Glyph::Torus),
            other => Icon::Letter(other.chars().next().unwrap_or('?').to_uppercase().to_string()),
        },
        other => Icon::Letter(other.label().trim_start_matches("Add ").chars().next().unwrap_or('?').to_uppercase().to_string()),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Cube,
    Sphere,
    Cylinder,
    Cone,
    Plane,
    Torus,
    Object,
    Vertex,
    Edge,
    Face,
    Select,
    Move,
    Rotate,
    Scale,
}

fn letter(p: &Painter, r: Rect, c: Color32, text: &str) {
    p.text(r.center(), Align2::CENTER_CENTER, text, FontId::proportional(14.0), c);
}

/// Original line drawings, a few strokes each, in a square `r`.
fn glyph(p: &Painter, r: Rect, c: Color32, g: Glyph) {
    let s = Stroke::new(1.4, c);
    let (l, t) = (r.left(), r.top());
    let ctr = r.center();
    let pt = |x: f32, y: f32| egui::pos2(l + x * r.width(), t + y * r.height());
    let ellipse = |center: egui::Pos2, rx: f32, ry: f32| {
        let pts: Vec<egui::Pos2> = (0..24)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / 24.0;
                center + vec2(a.cos() * rx, a.sin() * ry)
            })
            .collect();
        p.add(egui::Shape::closed_line(pts, s));
    };
    match g {
        Glyph::Cube => {
            p.rect_stroke(Rect::from_min_max(pt(0.0, 0.25), pt(0.75, 1.0)), 0.0, s, egui::StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(pt(0.25, 0.0), pt(1.0, 0.75)), 0.0, s, egui::StrokeKind::Inside);
            p.line_segment([pt(0.0, 0.25), pt(0.25, 0.0)], s);
            p.line_segment([pt(0.75, 1.0), pt(1.0, 0.75)], s);
        }
        Glyph::Sphere => {
            p.circle_stroke(ctr, r.width() * 0.5, s);
            ellipse(ctr, r.width() * 0.5, r.height() * 0.18);
        }
        Glyph::Cylinder => {
            ellipse(pt(0.5, 0.15), r.width() * 0.45, r.height() * 0.15);
            p.line_segment([pt(0.05, 0.15), pt(0.05, 0.85)], s);
            p.line_segment([pt(0.95, 0.15), pt(0.95, 0.85)], s);
            p.add(egui::Shape::closed_line(
                (0..=12)
                    .map(|i| {
                        let a = std::f32::consts::PI * i as f32 / 12.0;
                        pt(0.5 + a.cos() * 0.45, 0.85 + a.sin() * 0.15)
                    })
                    .collect(),
                s,
            ));
        }
        Glyph::Cone => {
            p.line_segment([pt(0.5, 0.0), pt(0.05, 0.85)], s);
            p.line_segment([pt(0.5, 0.0), pt(0.95, 0.85)], s);
            ellipse(pt(0.5, 0.85), r.width() * 0.45, r.height() * 0.15);
        }
        Glyph::Plane => {
            p.add(egui::Shape::closed_line(vec![pt(0.2, 0.3), pt(1.0, 0.3), pt(0.8, 0.9), pt(0.0, 0.9)], s));
        }
        Glyph::Torus => {
            ellipse(ctr, r.width() * 0.5, r.height() * 0.3);
            ellipse(ctr, r.width() * 0.2, r.height() * 0.1);
        }
        Glyph::Object => {
            p.add(egui::Shape::closed_line(vec![pt(0.5, 0.0), pt(1.0, 0.5), pt(0.5, 1.0), pt(0.0, 0.5)], s));
        }
        Glyph::Vertex => {
            p.circle_filled(ctr, 3.0, c);
            p.circle_stroke(ctr, 6.0, Stroke::new(1.0, c));
        }
        Glyph::Edge => {
            p.line_segment([pt(0.0, 1.0), pt(1.0, 0.0)], Stroke::new(2.0, c));
            p.circle_filled(pt(0.0, 1.0), 2.0, c);
            p.circle_filled(pt(1.0, 0.0), 2.0, c);
        }
        Glyph::Face => {
            p.add(egui::Shape::convex_polygon(vec![pt(0.1, 0.1), pt(0.9, 0.1), pt(0.9, 0.9), pt(0.1, 0.9)], c.gamma_multiply(0.45), s));
        }
        Glyph::Select => {
            p.add(egui::Shape::closed_line(vec![pt(0.15, 0.0), pt(0.15, 0.85), pt(0.4, 0.65), pt(0.6, 1.0), pt(0.75, 0.9), pt(0.55, 0.58), pt(0.85, 0.55)], s));
        }
        Glyph::Move => {
            p.line_segment([pt(0.0, 0.5), pt(1.0, 0.5)], s);
            p.line_segment([pt(0.5, 0.0), pt(0.5, 1.0)], s);
            for (a, b, d) in [(0.0, 0.5, 1.0), (1.0, 0.5, -1.0)] {
                p.line_segment([pt(a, b), pt(a + 0.2 * d, b - 0.15)], s);
                p.line_segment([pt(a, b), pt(a + 0.2 * d, b + 0.15)], s);
            }
        }
        Glyph::Rotate => {
            p.add(egui::Shape::line(
                (0..=18)
                    .map(|i| {
                        let a = std::f32::consts::TAU * 0.8 * i as f32 / 18.0;
                        ctr + vec2(a.cos(), a.sin()) * (r.width() * 0.45)
                    })
                    .collect(),
                s,
            ));
            p.line_segment([pt(0.95, 0.5), pt(0.8, 0.3)], s);
            p.line_segment([pt(0.95, 0.5), pt(1.0, 0.25)], s);
        }
        Glyph::Scale => {
            p.rect_stroke(Rect::from_min_max(pt(0.0, 0.4), pt(0.6, 1.0)), 0.0, s, egui::StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(pt(0.4, 0.0), pt(1.0, 0.6)), 0.0, s, egui::StrokeKind::Inside);
        }
    }
}
