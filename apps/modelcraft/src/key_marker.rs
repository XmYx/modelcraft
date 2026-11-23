//! Key markers: a small toggle before each keyable number in Properties.
//!
//! The marker shows one of three states of the attribute at the current frame: not animated,
//! animated (it has a curve in the active layer or another layer, with no key on this frame), or
//! key on this frame. Pressing it keys the attribute at the current frame, or removes the key
//! that is there; Shift+click shows the attribute's curve in the Curve Editor. It is a toggle
//! button in the accessibility tree, named after its field ("Position X key") with its state as
//! the value, and it is reached by Tab right before the field.
//!
//! The field helpers in `panels.rs` do not know the object or the scene, so the Properties panel
//! opens a *scope* for the selected object ([`begin`]), the helpers draw markers into it
//! ([`marker`]) and queue what was pressed, and [`end`] runs the queue through the same commands
//! the Animation panel uses (`SetKey`, `RemoveKeys`). Outside a scope (Transform by Numbers, the
//! tool options) no marker is drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use egui::{Color32, Id, Pos2, Sense, Shape, Stroke, Ui, vec2};
use modelcraft_engine::scene::{ObjectId, Scene};
use modelcraft_engine::{AnimCommand, Command, KeyRef};

use crate::app::ModelApp;
use crate::theme;

/// What the curves say about one attribute at the current frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// No curve drives it.
    Plain,
    /// A curve drives it, with no key on this frame.
    Animated,
    /// The active layer has a key on this frame.
    Keyed,
}

impl KeyState {
    pub fn label(self) -> String {
        match self {
            KeyState::Plain => tl!("Not animated"),
            KeyState::Animated => tl!("Animated"),
            KeyState::Keyed => tl!("Key on this frame"),
        }
    }
}

/// The state of an attribute at the scene's current frame.
pub fn state_of(scene: &Scene, id: ObjectId, path: &str) -> KeyState {
    match scene.anim.curve(id, path) {
        Some(curve) if curve.key_at(scene.anim.frame).is_some() => KeyState::Keyed,
        Some(_) => KeyState::Animated,
        None if scene.anim.has_curve(id, path) => KeyState::Animated,
        None => KeyState::Plain,
    }
}

/// What a press on a marker asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Press {
    /// Key the attribute here, or remove the key that is here.
    Toggle(String),
    /// Show the attribute's curve in the Curve Editor.
    ShowCurve(String),
    /// Give the attribute a formula (empty removes it); see `formula_field`.
    Formula(String, String),
    /// Remove the attribute's formula and set it to this number.
    Value(String, f64),
}

/// A formula on one attribute of the scope's object, as the field shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldFormula {
    pub expr: String,
    /// Why the last solve skipped it, if it did.
    pub problem: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct Scope {
    pub(crate) id: ObjectId,
    /// The attributes the object has (only those can be keyed); a field whose path is missing
    /// gets no marker.
    pub(crate) valid: Arc<BTreeSet<String>>,
    states: Arc<BTreeMap<String, KeyState>>,
    /// The formulas on the object's attributes, by path.
    pub(crate) formulas: Arc<BTreeMap<String, FieldFormula>>,
    pressed: Arc<Mutex<Vec<Press>>>,
}

impl Scope {
    /// Queue something for [`end`] to run.
    pub(crate) fn press(&self, p: Press) {
        if let Ok(mut q) = self.pressed.lock() {
            q.push(p);
        }
    }
}

/// The open scope, if a panel opened one this frame.
pub(crate) fn scope(ui: &Ui) -> Option<Scope> {
    ui.ctx().data(|d| d.get_temp::<Scope>(scope_id()))
}

fn scope_id() -> Id {
    Id::unique("modelcraft-key-marker-scope")
}

/// Open the scope for `id`: read the state of every animated attribute once.
pub fn begin(app: &ModelApp, ui: &Ui, id: ObjectId) {
    let scene = &app.doc.scene;
    let states: BTreeMap<String, KeyState> = scene.anim.tracks_of(id).map(|t| (t.path.clone(), state_of(scene, id, &t.path))).collect();
    let valid: BTreeSet<String> = scene.get(id).map(|o| modelcraft_engine::scene::attr::list(o).into_iter().map(|(p, _)| p).collect()).unwrap_or_default();
    let formulas: BTreeMap<String, FieldFormula> = scene
        .constraints
        .of(id)
        .filter_map(|c| match &c.kind {
            modelcraft_engine::scene::constraint::ConstraintKind::Expression { path, expr } => {
                let problem = scene.constraints.problems().iter().find(|p| p.constraint == c.id).map(|p| p.message.clone());
                Some((path.clone(), FieldFormula { expr: expr.clone(), problem }))
            }
            _ => None,
        })
        .collect();
    let scope = Scope { id, valid: Arc::new(valid), states: Arc::new(states), formulas: Arc::new(formulas), pressed: Arc::default() };
    ui.ctx().data_mut(|d| d.insert_temp(scope_id(), scope));
}

/// Close the scope and run what was pressed in it. Returns whether anything ran.
pub fn end(app: &mut ModelApp, ui: &Ui) -> bool {
    let scope: Option<Scope> = ui.ctx().data_mut(|d| d.remove_temp(scope_id()));
    let Some(scope) = scope else { return false };
    let id = scope.id;
    let pressed = scope.pressed.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
    let mut ran = false;
    for p in pressed {
        ran |= match p {
            Press::Toggle(path) => toggle(app, id, &path),
            Press::ShowCurve(path) => show_curve(app, id, &path),
            Press::Formula(path, expr) => crate::formula_field::set(app, id, &path, &expr),
            Press::Value(path, value) => crate::formula_field::replace_with_value(app, id, &path, value),
        };
    }
    ran
}

/// Key `path` at the current frame, or remove the key that is there.
pub fn toggle(app: &mut ModelApp, id: ObjectId, path: &str) -> bool {
    let frame = app.doc.scene.anim.frame;
    let name = crate::anim_ui::attr_label(path);
    if state_of(&app.doc.scene, id, path) == KeyState::Keyed {
        let keys = vec![KeyRef { id, path: path.to_string(), frame }];
        let ok = app.run(Command::Anim(AnimCommand::RemoveKeys { keys }));
        if ok {
            app.status = crate::tlf!("Key removed from {attribute} at frame {frame}", attribute = name, frame = frame_text(frame));
        }
        ok
    } else {
        let ok = app.run(Command::Anim(AnimCommand::SetKey { id, path: path.to_string(), frame: Some(frame), value: None }));
        if ok {
            app.status = crate::tlf!("{attribute} keyed at frame {frame}", attribute = name, frame = frame_text(frame));
        }
        ok
    }
}

/// Open the Curve Editor on `path`: its keys become the selection and its curve is shown.
pub fn show_curve(app: &mut ModelApp, id: ObjectId, path: &str) -> bool {
    let Some(curve) = app.doc.scene.anim.curve(id, path) else {
        app.status = crate::tlf!("{attribute} has no curve yet", attribute = crate::anim_ui::attr_label(path));
        return false;
    };
    let keys: Vec<KeyRef> = curve.keys.iter().map(|k| KeyRef { id, path: path.to_string(), frame: k.t }).collect();
    app.anim.open = true;
    app.anim.view = crate::anim_ui::AnimView::Curves;
    app.anim.hidden.remove(&(id, path.to_string()));
    app.anim.selected = keys;
    app.status = crate::tlf!("Showing the curve of {attribute}", attribute = crate::anim_ui::attr_label(path));
    true
}

fn frame_text(f: f64) -> String {
    if (f - f.round()).abs() < 1e-6 { format!("{}", f.round() as i64) } else { format!("{f:.2}") }
}

/// Draw the marker for `path` (named after `label`) when a scope is open.
pub fn marker(ui: &mut Ui, path: &str, label: &str) {
    let Some(scope) = ui.ctx().data(|d| d.get_temp::<Scope>(scope_id())) else { return };
    if !scope.valid.contains(path) {
        return;
    }
    let state = scope.states.get(path).copied().unwrap_or(KeyState::Plain);
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 18.0), Sense::click());
    let name = crate::tlf!("{field} key", field = label);
    let value = state.label();
    resp.widget_info(|| egui::WidgetInfo {
        current_text_value: Some(value.clone()),
        ..egui::WidgetInfo::selected(egui::Role::Button, true, state == KeyState::Keyed, &name)
    });
    let resp = resp.on_hover_text(match state {
        KeyState::Keyed => tl!("Remove the key on this frame (Shift: show the curve)"),
        _ => tl!("Key this value on this frame (Shift: show the curve)"),
    });
    if resp.clicked()
        && let Ok(mut q) = scope.pressed.lock()
    {
        let shift = ui.input(|i| i.modifiers.shift);
        q.push(if shift { Press::ShowCurve(path.to_string()) } else { Press::Toggle(path.to_string()) });
    }
    draw(ui, rect.center(), state, resp.hovered() || resp.has_focus());
    if resp.has_focus() {
        ui.painter().rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
    }
}

/// Three shapes, so the state never rests on colour alone: a small dot (not animated), a hollow
/// diamond (animated) and a filled diamond (key on this frame).
fn draw(ui: &Ui, c: Pos2, state: KeyState, lit: bool) {
    let p = ui.painter();
    let dim = if lit { theme::TEXT } else { theme::TEXT_DIM };
    let diamond = |r: f32| vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)];
    match state {
        KeyState::Plain => {
            p.circle_stroke(c, 2.5, Stroke::new(1.0, dim));
        }
        KeyState::Animated => {
            p.add(Shape::closed_line(diamond(4.5), Stroke::new(1.5, theme::ACCENT)));
        }
        KeyState::Keyed => {
            p.add(Shape::convex_polygon(diamond(5.0), theme::ACCENT, Stroke::new(1.0, Color32::WHITE)));
        }
    }
}
