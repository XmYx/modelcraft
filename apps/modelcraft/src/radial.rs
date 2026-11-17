//! The radial menu: eight actions around the pointer, chosen for what is being done (adding
//! objects, editing components, sculpting, switching workspace). Opened with Shift+Q (a
//! rebindable action). Every slice is a button with its name: click it, press its number 1–8,
//! move with Tab and press Enter, or flick the pointer towards it and click; Esc closes.

use egui::{Pos2, Vec2};

use crate::actions::Action;
use crate::app::{ModelApp, SelectMode, Workspace};

pub struct Radial {
    pub center: Pos2,
    /// The slice the pointer points at, if it moved far enough from the centre.
    hover: Option<usize>,
    /// Focus the first slice when it opens.
    focus_first: bool,
}

/// Radius of the ring of slices, in points.
const RING: f32 = 110.0;

/// The actions for the current context, at most eight, clockwise from the top.
pub fn items(app: &ModelApp) -> Vec<Action> {
    use modelcraft_engine::sculpt::BrushKind::*;
    match (app.workspace, app.mode) {
        (Workspace::Sculpt, _) => [Draw, Clay, Smooth, Inflate, Grab, Pinch, Flatten, Crease].into_iter().map(Action::Brush).collect(),
        (Workspace::Model, SelectMode::Object) => {
            let mut v: Vec<Action> = ["cube", "sphere", "cylinder", "torus", "plane", "cone"].into_iter().map(Action::AddPrimitive).collect();
            v.extend([Action::AddCage, Action::AddGroup]);
            v
        }
        (Workspace::Model, _) => {
            vec![Action::Extrude, Action::Inset, Action::Bevel, Action::LoopCut, Action::Merge, Action::Bridge, Action::FillHole, Action::Subdivide]
        }
        (Workspace::Paint, _) => vec![
            Action::Workspace(Workspace::Model),
            Action::Workspace(Workspace::Sculpt),
            Action::Frame,
            Action::ToggleQuad,
            Action::Undo,
            Action::Redo,
            Action::Palette,
            Action::RenderView,
        ],
    }
}

/// Where slice `i` of `n` sits, clockwise from the top.
pub fn slot(i: usize, n: usize) -> Vec2 {
    let a = std::f32::consts::TAU * i as f32 / n.max(1) as f32 - std::f32::consts::FRAC_PI_2;
    Vec2::new(a.cos(), a.sin()) * RING
}

/// The slice a pointer offset points at, when it is far enough from the centre.
pub fn sector(offset: Vec2, n: usize) -> Option<usize> {
    if n == 0 || !offset.is_finite() || offset.length() < RING * 0.3 {
        return None;
    }
    let a = offset.y.atan2(offset.x) + std::f32::consts::FRAC_PI_2;
    let a = a.rem_euclid(std::f32::consts::TAU);
    let step = std::f32::consts::TAU / n as f32;
    Some((((a + step / 2.0) / step) as usize) % n)
}

/// Open around the pointer, or the middle of the window when opened without one.
pub fn open(app: &mut ModelApp, ctx: &egui::Context) {
    let center = ctx.pointer_latest_pos().unwrap_or_else(|| ctx.content_rect().center());
    app.radial = Some(Radial { center, hover: None, focus_first: true });
    app.status = tl!("Radial menu: pick with 1–8, Tab and Enter, or the pointer; Esc closes");
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    let Some(r) = &app.radial else { return };
    let (center, focus_first) = (r.center, r.focus_first);
    let actions = items(app);
    let n = actions.len();
    let bounds = ctx.content_rect().shrink(RING + 60.0);
    let center = if bounds.is_positive() { center.clamp(bounds.min, bounds.max) } else { ctx.content_rect().center() };
    let mut chosen: Option<Action> = None;
    let mut close = false;
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Escape) {
            close = true;
        }
        for (k, key) in [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8]
            .iter()
            .enumerate()
        {
            if i.key_pressed(*key) {
                chosen = actions.get(k).copied();
            }
        }
    });
    let pointer = ctx.pointer_latest_pos();
    let hover = pointer.and_then(|p| sector(p - center, n));
    if let Some(r) = &mut app.radial {
        r.hover = hover;
        r.focus_first = false;
    }
    let area = egui::Area::new(egui::Id::unique("radial-menu")).order(egui::Order::Foreground).fixed_pos(center - Vec2::splat(RING + 70.0));
    let resp = area.show(ctx, |ui| {
        let size = Vec2::splat(2.0 * (RING + 70.0));
        let (rect, bg) = ui.allocate_exact_size(size, egui::Sense::click());
        ui.ctx().accesskit_node_builder(bg.id, |b| {
            b.set_role(egui::accesskit::Role::Menu);
            b.set_label(tl!("Radial menu"));
        });
        let painter = ui.painter();
        painter.circle_filled(rect.center(), RING + 52.0, crate::theme::PANEL_DEEP.gamma_multiply(0.9));
        painter.circle_stroke(rect.center(), 6.0, egui::Stroke::new(1.5, crate::theme::TEXT_DIM));
        for (i, a) in actions.iter().enumerate() {
            let at = rect.center() + slot(i, n);
            let label = format!("{}  {}", i + 1, a.label());
            let b = egui::Button::new(label).selected(hover == Some(i)).min_size(egui::vec2(96.0, 26.0));
            let r = ui.put(egui::Rect::from_center_size(at, egui::vec2(132.0, 28.0)), b);
            ui.ctx().accesskit_node_builder(r.id, |node| {
                node.set_role(egui::accesskit::Role::MenuItem);
                node.set_label(a.label());
            });
            if focus_first && i == 0 {
                r.request_focus();
            }
            if r.clicked() {
                chosen = Some(*a);
            }
        }
        bg
    });
    // A click on the ring (not on a slice) takes the slice the pointer points at.
    if resp.inner.clicked() && chosen.is_none() {
        match hover {
            Some(i) => chosen = actions.get(i).copied(),
            None => close = true,
        }
    }
    if let Some(a) = chosen {
        app.radial = None;
        app.act(ctx, a);
    } else if close {
        app.radial = None;
        app.status = tl!("Radial menu closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sectors_follow_the_slots() {
        for n in [1, 4, 8] {
            for i in 0..n {
                assert_eq!(sector(slot(i, n), n), Some(i), "slice {i} of {n}");
            }
        }
        assert_eq!(sector(Vec2::new(1.0, 1.0), 8), None, "too close to the centre");
        assert_eq!(sector(Vec2::new(f32::NAN, 0.0), 8), None);
    }
}
