//! The docked workspace (craft-ui docking): the viewport in the middle, the panels around it in
//! tab groups and splits that can be dragged, tabbed together, floated and closed. Each workspace
//! (Model, Sculpt, Paint) keeps its own layout, saved in the preferences. Every move has a
//! keyboard path: the panel's options menu (Dock Left, Dock Right, Tab With …, Float, Hide), the
//! tab menu craft-ui draws, and View ▸ Panels.

use craft_ui::docking::{Action, DockArea, DockStyle, Layout, Node, Permissions, Placement, Zone};
use craft_ui::layout::{SplitAxis, SplitSize};
use serde::{Deserialize, Serialize};

use crate::app::{ModelApp, PanelId, Workspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pane {
    Viewport,
    Tree,
    Properties,
    Brushes,
    Paint,
    History,
}

impl Pane {
    pub fn panel(self) -> Option<PanelId> {
        Some(match self {
            Pane::Viewport => return None,
            Pane::Tree => PanelId::Tree,
            Pane::Properties => PanelId::Properties,
            Pane::Brushes => PanelId::Brushes,
            Pane::Paint => PanelId::Paint,
            Pane::History => PanelId::History,
        })
    }
    pub fn of(p: PanelId) -> Pane {
        match p {
            PanelId::Tree => Pane::Tree,
            PanelId::Properties => Pane::Properties,
            PanelId::Brushes => Pane::Brushes,
            PanelId::Paint => Pane::Paint,
            PanelId::History => Pane::History,
        }
    }
    pub fn label(self) -> String {
        match self.panel() {
            Some(p) => p.label(),
            None => tl!("Viewport"),
        }
    }
    fn permissions(self) -> Permissions {
        match self {
            // The viewport stays put and is never closed; panels may split it.
            Pane::Viewport => Permissions { movable: false, floatable: false, closable: false, accepts_tabs: false, accepts_splits: true },
            _ => Permissions::default(),
        }
    }
}

fn tabs(panels: &[Pane]) -> Node<Pane> {
    Node::Tabs { panels: panels.to_vec(), active: 0 }
}

fn split(axis: SplitAxis, size: SplitSize, first: Node<Pane>, second: Node<Pane>) -> Node<Pane> {
    Node::Split { axis, size, first: Box::new(first), second: Box::new(second) }
}

/// The layout a workspace starts with (and View ▸ Reset Panels restores).
pub fn default_layout(ws: Workspace) -> Layout<Pane> {
    let viewport = tabs(&[Pane::Viewport]);
    let h = SplitAxis::Horizontal;
    let root = match ws {
        Workspace::Model => {
            let left = split(SplitAxis::Vertical, SplitSize::Ratio(0.62), tabs(&[Pane::Tree]), tabs(&[Pane::History]));
            split(h, SplitSize::FixedFirst(250.0), left, split(h, SplitSize::FixedSecond(300.0), viewport, tabs(&[Pane::Properties])))
        }
        Workspace::Sculpt => split(h, SplitSize::FixedFirst(250.0), tabs(&[Pane::Brushes]), split(h, SplitSize::FixedSecond(300.0), viewport, tabs(&[Pane::Properties]))),
        Workspace::Paint => split(h, SplitSize::FixedFirst(250.0), tabs(&[Pane::Tree]), split(h, SplitSize::FixedSecond(300.0), viewport, tabs(&[Pane::Paint]))),
    };
    Layout { root: Some(root), floating: Vec::new() }
}

pub const WORKSPACES: [Workspace; 3] = [Workspace::Model, Workspace::Sculpt, Workspace::Paint];

fn key(ws: Workspace) -> &'static str {
    match ws {
        Workspace::Model => "model",
        Workspace::Sculpt => "sculpt",
        Workspace::Paint => "paint",
    }
}

/// The layouts of every workspace.
#[derive(Clone, Debug)]
pub struct Layouts {
    pub per: [Layout<Pane>; 3],
    raise: Option<Pane>,
}

impl Default for Layouts {
    fn default() -> Self {
        Layouts { per: WORKSPACES.map(default_layout), raise: None }
    }
}

fn index(ws: Workspace) -> usize {
    WORKSPACES.iter().position(|w| *w == ws).unwrap_or(0)
}

/// A layout read back from the preferences is used only when it is valid and still holds the
/// viewport; anything else falls back to the default.
fn usable(l: Layout<Pane>) -> Option<Layout<Pane>> {
    (l.validate().is_ok() && l.contains(&Pane::Viewport)).then_some(l)
}

impl Layouts {
    pub fn get(&self, ws: Workspace) -> &Layout<Pane> {
        &self.per[index(ws)]
    }
    pub fn get_mut(&mut self, ws: Workspace) -> &mut Layout<Pane> {
        &mut self.per[index(ws)]
    }
    pub fn shown(&self, ws: Workspace, p: PanelId) -> bool {
        self.get(ws).contains(&Pane::of(p))
    }
    /// The panels shown in a workspace, in [`PanelId::ALL`] order.
    pub fn shown_panels(&self, ws: Workspace) -> Vec<PanelId> {
        PanelId::ALL.iter().copied().filter(|p| self.shown(ws, *p)).collect()
    }
    /// Apply a move; a refused one leaves the layout as it was and says why.
    pub fn apply(&mut self, ws: Workspace, a: Action<Pane>) -> Result<(), String> {
        self.get_mut(ws).apply_with_permissions(a, |p| p.permissions()).map(|_| ()).map_err(|e| format!("{e:?}"))
    }
    pub fn reset(&mut self, ws: Workspace) {
        self.per[index(ws)] = default_layout(ws);
    }
    /// Bring a panel's tab to the front next frame.
    pub fn raise(&mut self, p: Pane) {
        self.raise = Some(p);
    }

    /// For the preferences: one JSON text per workspace.
    pub fn to_saved(&self) -> std::collections::BTreeMap<String, String> {
        WORKSPACES.iter().filter_map(|ws| Some((key(*ws).to_string(), serde_json::to_string(self.get(*ws)).ok()?))).collect()
    }
    pub fn from_saved(saved: &std::collections::BTreeMap<String, String>) -> Layouts {
        let mut l = Layouts::default();
        for ws in WORKSPACES {
            if let Some(text) = saved.get(key(ws)).filter(|t| t.len() < 256 * 1024)
                && let Some(layout) = serde_json::from_str::<Layout<Pane>>(text).ok().and_then(usable)
            {
                l.per[index(ws)] = layout;
            }
        }
        l
    }
}

/// Where a panel options menu can put a panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Below,
    TabWith(PanelId),
    Float,
    Hide,
    Show,
}

/// Move a panel by a menu choice (the keyboard alternative to dragging).
pub fn move_panel(app: &mut ModelApp, p: PanelId, m: Move) {
    let ws = app.workspace;
    let pane = Pane::of(p);
    let shown = app.layouts.shown(ws, p);
    let action = match m {
        Move::Left | Move::Right | Move::Below => {
            let zone = match m {
                Move::Left => Zone::Left,
                Move::Right => Zone::Right,
                _ => Zone::Bottom,
            };
            if shown {
                Action::Move { panel: pane, anchor: Pane::Viewport, placement: Placement::Split(zone) }
            } else {
                Action::OpenAt { panel: pane, anchor: Pane::Viewport, placement: Placement::Split(zone) }
            }
        }
        Move::TabWith(other) => {
            let anchor = Pane::of(other);
            if !app.layouts.shown(ws, other) || other == p {
                app.status = crate::tlf!("{panel} is not shown", panel = other.label());
                return;
            }
            if shown {
                Action::Move { panel: pane, anchor, placement: Placement::Tab { before: None } }
            } else {
                Action::Open { panel: pane, anchor: Some(anchor) }
            }
        }
        Move::Float => {
            if !shown {
                if let Err(e) = app.layouts.apply(ws, Action::OpenAt { panel: pane, anchor: Pane::Viewport, placement: Placement::Split(Zone::Right) }) {
                    app.status = e;
                    return;
                }
            }
            Action::Float { panel: pane, rect: [320.0, 160.0, 300.0, 420.0] }
        }
        Move::Hide if shown => Action::Close { panel: pane },
        Move::Show if !shown => Action::OpenAt { panel: pane, anchor: Pane::Viewport, placement: Placement::Split(Zone::Right) },
        Move::Hide | Move::Show => return,
    };
    match app.layouts.apply(ws, action) {
        Ok(()) => {
            app.layouts.raise(pane);
            app.status = match m {
                Move::Hide => crate::tlf!("{panel} hidden", panel = p.label()),
                _ => crate::tlf!("{panel} moved", panel = p.label()),
            };
        }
        Err(e) => app.status = crate::tlf!("{panel} cannot move there: {error}", panel = p.label(), error = e),
    }
}

/// Draw the docked workspace into the space left by the bars.
pub fn show(app: &mut ModelApp, ui: &mut egui::Ui) {
    let ws = app.workspace;
    let layout = app.layouts.get(ws).clone();
    let mut style = DockStyle::from_ui(ui);
    style.float_label = tl!("Float Panel");
    style.close_label = tl!("Close Panel");
    style.move_panel_to_label = tl!("Move Panel to Group");
    style.panels_label = tl!("Panels");
    style.resize_label = tl!("Resize panels");
    style.resize_window_label = tl!("Resize panel window");
    style.close_group_label = tl!("Close Tab Group");
    style.minimize_label = tl!("Minimize");
    style.expand_label = tl!("Expand");
    style.float_group_label = tl!("Float Tab Group");
    style.move_group_to_label = tl!("Move Group To");
    let mut area = DockArea::new(egui::Id::unique(("modelcraft-dock", index(ws))));
    if let Some(p) = app.layouts.raise.take() {
        area = area.raise_panel(p);
    }
    let out = area.show(
        ui,
        &layout,
        &style,
        |p| p.label(),
        |p| p.permissions(),
        |ui, pane| match pane.panel() {
            None => {
                egui::Frame::NONE.fill(crate::theme::PANEL_DEEP).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    crate::viewport::show(app, ui)
                });
            }
            Some(p) => {
                egui::ScrollArea::vertical().id_salt(("panel-scroll", p as u8)).auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| crate::panels::dock_panel(app, ui, p));
                });
            }
        },
    );
    if let Some(e) = out.error {
        log::warn!("panel layout: {e:?}; restoring the default layout");
        app.layouts.reset(ws);
        return;
    }
    for action in out.actions {
        if let Err(e) = app.layouts.apply(ws, action) {
            log::warn!("panel move refused: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layouts_are_valid_and_survive_the_preferences() {
        let l = Layouts::default();
        for ws in WORKSPACES {
            assert!(l.get(ws).validate().is_ok());
            assert!(l.get(ws).contains(&Pane::Viewport));
        }
        assert_eq!(l.shown_panels(Workspace::Model), vec![PanelId::Tree, PanelId::Properties, PanelId::History]);
        let mut changed = l.clone();
        changed.apply(Workspace::Model, Action::Move { panel: Pane::History, anchor: Pane::Properties, placement: Placement::Tab { before: None } }).unwrap();
        let back = Layouts::from_saved(&changed.to_saved());
        assert_eq!(back.get(Workspace::Model), changed.get(Workspace::Model));
        // Hostile or stale saved text falls back to the default.
        let mut bad = std::collections::BTreeMap::new();
        bad.insert("model".to_string(), "{\"root\":null,\"floating\":[]}".to_string());
        bad.insert("sculpt".to_string(), "not json".to_string());
        let back = Layouts::from_saved(&bad);
        assert_eq!(back.get(Workspace::Model), &default_layout(Workspace::Model), "a layout without the viewport is refused");
        assert_eq!(back.get(Workspace::Sculpt), &default_layout(Workspace::Sculpt));
        // The viewport never closes.
        let mut l = Layouts::default();
        assert!(l.apply(Workspace::Model, Action::Close { panel: Pane::Viewport }).is_err());
    }
}
