//! The Animation panel: transport and time fields, the time ruler, the Key Sheet (every curve as
//! a row of keys) and the Curve Editor (values over time with tangent handles).
//!
//! Everything a drag does here has a typed or keyboard path: the Frame field and the arrow keys on
//! the ruler replace scrubbing; the key fields (frame, value, slopes, handle lengths), "Shift" and
//! "Scale" and Alt+arrows replace dragging keys and tangent handles. Every key is a node in the
//! accessibility tree with its attribute, frame, value and selected state.

use std::collections::BTreeSet;
use std::sync::Arc;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke};
use modelcraft_engine::animation::{Curve, Extrapolation, Interp, Key, LoopMode, Playback, Tangent};
use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{AnimCommand, Command, KeyRef};

use crate::app::ModelApp;
use crate::theme;

/// Width of the column with the track names.
const LABEL_W: f32 = 210.0;
const ROW_H: f32 = 20.0;
/// Most rows drawn (the rest is reachable by selecting fewer objects).
const MAX_ROWS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimView {
    Sheet,
    Curves,
}

/// A tangent handle being dragged in the Curve Editor.
#[derive(Clone, Debug, PartialEq)]
pub struct HandleDrag {
    pub key: KeyRef,
    pub out: bool,
    pub slope: f64,
    pub weight: f64,
}

pub struct AnimUi {
    pub open: bool,
    pub view: AnimView,
    pub playback: Playback,
    pub last_tick: Option<std::time::Instant>,
    /// Selected keys; the last one is the key the fields edit.
    pub selected: Vec<KeyRef>,
    /// Show the curves of every object, not only the selected ones.
    pub all_objects: bool,
    /// The attribute "Key Attribute" keys.
    pub attribute: String,
    pub shift_frames: f64,
    pub shift_value: f64,
    pub scale: f64,
    /// Visible frames (`None`: the playback range).
    pub zoom: Option<(f64, f64)>,
    /// Curves hidden in the Curve Editor.
    pub hidden: BTreeSet<(ObjectId, String)>,
    /// Keys being dragged: frames and value moved so far.
    pub key_drag: Option<(f64, f64)>,
    pub handle_drag: Option<HandleDrag>,
    /// The key that takes keyboard focus when it is drawn next.
    pub focus_key: Option<KeyRef>,
    /// The scene had curves in the last frame (the panel opens when the first one appears).
    pub had_curves: bool,
    /// Draw the path of the selected moving objects in the viewport.
    pub show_paths: bool,
}

impl Default for AnimUi {
    fn default() -> Self {
        AnimUi {
            open: false,
            view: AnimView::Sheet,
            playback: Playback::default(),
            last_tick: None,
            selected: Vec::new(),
            all_objects: false,
            attribute: "translate.x".into(),
            shift_frames: 1.0,
            shift_value: 0.0,
            scale: 2.0,
            zoom: None,
            hidden: BTreeSet::new(),
            key_drag: None,
            handle_drag: None,
            focus_key: None,
            had_curves: false,
            show_paths: true,
        }
    }
}

// ---------------------------------------------------------------- actions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimAction {
    TogglePanel,
    PlayPause,
    KeyTransform,
    AutoKey,
    NextFrame,
    PrevFrame,
    NextKey,
    PrevKey,
    FirstFrame,
    LastFrame,
    DeleteKeys,
    MotionPaths,
    EulerFilter,
    Turntable,
}

impl AnimAction {
    pub const ALL: [AnimAction; 14] = [
        AnimAction::TogglePanel,
        AnimAction::PlayPause,
        AnimAction::KeyTransform,
        AnimAction::AutoKey,
        AnimAction::NextFrame,
        AnimAction::PrevFrame,
        AnimAction::NextKey,
        AnimAction::PrevKey,
        AnimAction::FirstFrame,
        AnimAction::LastFrame,
        AnimAction::DeleteKeys,
        AnimAction::MotionPaths,
        AnimAction::EulerFilter,
        AnimAction::Turntable,
    ];

    pub fn label(self) -> String {
        match self {
            AnimAction::TogglePanel => tl!("Animation Panel"),
            AnimAction::PlayPause => tl!("Play / Pause"),
            AnimAction::KeyTransform => tl!("Key Transform"),
            AnimAction::AutoKey => tl!("Toggle Auto-Key"),
            AnimAction::NextFrame => tl!("Next Frame"),
            AnimAction::PrevFrame => tl!("Previous Frame"),
            AnimAction::NextKey => tl!("Next Key"),
            AnimAction::PrevKey => tl!("Previous Key"),
            AnimAction::FirstFrame => tl!("Go to Start"),
            AnimAction::LastFrame => tl!("Go to End"),
            AnimAction::DeleteKeys => tl!("Delete Selected Keys"),
            AnimAction::MotionPaths => tl!("Show Motion Paths"),
            AnimAction::EulerFilter => tl!("Smooth Rotation Keys"),
            AnimAction::Turntable => tl!("Turntable"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            AnimAction::TogglePanel => "anim.panel",
            AnimAction::PlayPause => "anim.play",
            AnimAction::KeyTransform => "anim.key_transform",
            AnimAction::AutoKey => "anim.auto_key",
            AnimAction::NextFrame => "anim.next_frame",
            AnimAction::PrevFrame => "anim.prev_frame",
            AnimAction::NextKey => "anim.next_key",
            AnimAction::PrevKey => "anim.prev_key",
            AnimAction::FirstFrame => "anim.first_frame",
            AnimAction::LastFrame => "anim.last_frame",
            AnimAction::DeleteKeys => "anim.delete_keys",
            AnimAction::MotionPaths => "anim.motion_paths",
            AnimAction::EulerFilter => "anim.euler_filter",
            AnimAction::Turntable => "anim.turntable",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            AnimAction::TogglePanel => "Cmd+Shift+T",
            AnimAction::PlayPause => "Shift+Space",
            AnimAction::KeyTransform => "Model: K",
            AnimAction::AutoKey => "Model: Shift+K",
            AnimAction::NextFrame => "Model: Period",
            AnimAction::PrevFrame => "Model: Comma",
            AnimAction::NextKey => "Model: Alt+Period",
            AnimAction::PrevKey => "Model: Alt+Comma",
            AnimAction::FirstFrame => "Model: Shift+Comma",
            AnimAction::LastFrame => "Model: Shift+Period",
            AnimAction::DeleteKeys | AnimAction::MotionPaths | AnimAction::EulerFilter | AnimAction::Turntable => return None,
        })
    }
}

fn frame_text(f: f64) -> String {
    if (f - f.round()).abs() < 1e-6 { format!("{}", f.round() as i64) } else { format!("{f:.2}") }
}

fn value_text(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" { "0".into() } else { s.to_string() }
}

/// A readable name for an attribute path.
pub fn attr_label(path: &str) -> String {
    let (head, axis) = path.split_once('.').unwrap_or((path, ""));
    let axis = axis.to_uppercase();
    match head {
        "translate" => crate::tlf!("Position {axis}", axis = axis),
        "rotate" => crate::tlf!("Rotation {axis}", axis = axis),
        "scale" => crate::tlf!("Scale {axis}", axis = axis),
        "visible" => tl!("Visible"),
        _ => path.to_string(),
    }
}

fn object_name(app: &ModelApp, id: ObjectId) -> String {
    app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default()
}

fn go_to(app: &mut ModelApp, frame: f64) {
    app.doc.set_frame(frame);
    app.status = crate::tlf!("Frame {frame}", frame = frame_text(app.doc.scene.anim.frame));
}

pub fn act(app: &mut ModelApp, a: AnimAction) {
    let anim = &app.doc.scene.anim;
    let (frame, time) = (anim.frame, anim.time);
    match a {
        AnimAction::TogglePanel => {
            app.anim.open = !app.anim.open;
            app.status = if app.anim.open { tl!("Animation panel shown") } else { tl!("Animation panel hidden") };
        }
        AnimAction::PlayPause => {
            app.anim.playback.toggle();
            app.anim.last_tick = None;
            if app.anim.playback.playing && frame >= time.end && app.anim.playback.mode == LoopMode::Once {
                app.doc.set_frame(time.start);
            }
            app.status =
                if app.anim.playback.playing { tl!("Playing") } else { crate::tlf!("Paused at frame {frame}", frame = frame_text(app.doc.scene.anim.frame)) };
        }
        AnimAction::KeyTransform => {
            if app.selection.is_empty() {
                app.status = tl!("Select an object to key");
                return;
            }
            let ids = app.selection.clone();
            if app.run(Command::Anim(AnimCommand::KeyTransform { ids, frame: Some(frame), translate: true, rotate: true, scale: true })) {
                app.anim.open = true;
                app.status = crate::tlf!("Transform keyed at frame {frame}", frame = frame_text(frame));
            }
        }
        AnimAction::AutoKey => {
            let on = !app.doc.scene.anim.auto_key;
            if app.run(Command::Anim(AnimCommand::SetAutoKey { on })) {
                app.status = if on { tl!("Auto-key on: edits set keys") } else { tl!("Auto-key off") };
            }
        }
        AnimAction::NextFrame => go_to(app, (frame + 1.0).floor()),
        AnimAction::PrevFrame => go_to(app, (frame - 1.0).ceil()),
        AnimAction::NextKey | AnimAction::PrevKey => {
            let ids = shown_objects(app);
            let t = if a == AnimAction::NextKey { app.doc.scene.anim.next_key_time(&ids, frame) } else { app.doc.scene.anim.prev_key_time(&ids, frame) };
            match t {
                Some(t) => go_to(app, t),
                None => app.status = tl!("No more keys in that direction"),
            }
        }
        AnimAction::FirstFrame => go_to(app, time.start),
        AnimAction::LastFrame => go_to(app, time.end),
        AnimAction::DeleteKeys => {
            delete_selected(app);
        }
        AnimAction::MotionPaths => {
            app.anim.show_paths = !app.anim.show_paths;
            app.status = if app.anim.show_paths { tl!("Motion paths shown for selected objects") } else { tl!("Motion paths hidden") };
        }
        AnimAction::EulerFilter | AnimAction::Turntable => {
            if app.selection.is_empty() {
                app.status = tl!("Select an object first");
                return;
            }
            let ids = app.selection.clone();
            let cmd = if a == AnimAction::EulerFilter {
                AnimCommand::EulerFilter { ids }
            } else {
                AnimCommand::Turntable { ids, axis: "y".into(), turns: 1.0, start: None, end: None }
            };
            if app.run(Command::Anim(cmd)) && a == AnimAction::Turntable {
                app.status = tl!("Turntable: one turn about Y over the range, repeating");
            }
        }
    }
}

fn delete_selected(app: &mut ModelApp) {
    if app.anim.selected.is_empty() {
        app.status = tl!("No keys selected");
        return;
    }
    let keys = std::mem::take(&mut app.anim.selected);
    let n = keys.len();
    if app.run(Command::Anim(AnimCommand::RemoveKeys { keys })) {
        app.status = crate::tlf!("{count} keys removed", count = n);
    }
}

/// The objects whose curves are shown: the selection, or everything.
fn shown_objects(app: &ModelApp) -> Vec<ObjectId> {
    if app.anim.all_objects { Vec::new() } else { app.selection.clone() }
}

// ---------------------------------------------------------------- playback

/// Advance playback; called once per drawn frame.
pub fn tick(app: &mut ModelApp, ctx: &egui::Context) {
    // Keys that no longer exist (undo, deleted objects) leave the selection.
    let anim = &app.doc.scene.anim;
    app.anim.selected.retain(|k| anim.curve(k.id, &k.path).is_some_and(|c| c.key_at(k.frame).is_some()));
    // A scene that gains its first curve (a key, an opened file) shows the panel.
    let has_curves = !anim.is_empty();
    if has_curves && !app.anim.had_curves {
        app.anim.open = true;
    }
    app.anim.had_curves = has_curves;
    if !app.anim.playback.playing {
        app.anim.last_tick = None;
        return;
    }
    let now = std::time::Instant::now();
    let dt = app.anim.last_tick.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
    app.anim.last_tick = Some(now);
    let (frame, time) = (app.doc.scene.anim.frame, app.doc.scene.anim.time);
    let next = app.anim.playback.advance(frame, dt, &time);
    if next != frame {
        app.doc.set_frame(next);
    }
    ctx.request_repaint();
}

// ---------------------------------------------------------------- motion paths

/// World-space line segments (pairs of points with a colour) for the motion paths of the
/// selected objects that move: the path over the playback range, a tick at every whole frame, a
/// larger mark at every key and a cross at the current frame. Empty when paths are off.
pub fn path_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let mut out = Vec::new();
    if !app.anim.show_paths {
        return out;
    }
    let scene = &app.doc.scene;
    let time = scene.anim.time;
    let path_color = [0.91, 0.54, 0.24, 0.95];
    let tick_color = [0.87, 0.87, 0.89, 0.9];
    for id in app.selection.iter().copied().filter(|id| scene.moves(*id)).take(16) {
        let step = (time.span() / 1000.0).max(0.25);
        let path = scene.motion_path(id, time.start, time.end, step);
        let reach = path.iter().fold(0.0f32, |m, (_, p)| m.max((*p - path.first().map_or(*p, |f| f.1)).length()));
        let size = (reach * 0.012).clamp(0.01, 0.2);
        for w in path.windows(2) {
            if let [(_, a), (_, b)] = w {
                out.push((a.to_array(), b.to_array(), path_color));
            }
        }
        let mut cross = |frame: f64, s: f32, color: [f32; 4]| {
            let p = scene.world_matrix_at(id, frame).transform_point3(glam::Vec3::ZERO);
            for axis in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z] {
                out.push(((p - axis * s).to_array(), (p + axis * s).to_array(), color));
            }
        };
        // Whole frames show the timing: ticks crowd where the object is slow.
        let frames = time.span().floor() as usize;
        if frames <= 2000 {
            for i in 0..=frames {
                cross(time.start.ceil() + i as f64, size, tick_color);
            }
        }
        for t in scene.anim.key_times(&[id]).into_iter().filter(|t| *t >= time.start && *t <= time.end).take(2000) {
            cross(t, size * 2.5, tick_color);
        }
        cross(scene.anim.frame, size * 4.0, [1.0, 1.0, 1.0, 1.0]);
    }
    out
}

// ---------------------------------------------------------------- panel

fn set_name(ui: &egui::Ui, resp: &egui::Response, name: impl Into<String>) {
    let name = name.into();
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_label(name));
}

/// A number field that commits once: while it is dragged the value is kept aside, and the new
/// value is returned when the drag ends or a typed value is entered.
fn number(ui: &mut egui::Ui, name: String, current: f64, speed: f64, range: Option<(f64, f64)>, decimals: usize) -> Option<f64> {
    let id = ui.scope_id().with(("anim-number", &name));
    let mut v = ui.data(|d| d.get_temp::<f64>(id)).unwrap_or(current);
    let mut dv = egui::DragValue::new(&mut v).speed(speed).max_decimals(decimals);
    if let Some((lo, hi)) = range {
        dv = dv.range(lo..=hi);
    }
    let r = ui.add(dv);
    set_name(ui, &r, name);
    if r.dragged() {
        ui.data_mut(|d| d.insert_temp(id, v));
        return None;
    }
    ui.data_mut(|d| d.remove::<f64>(id));
    ((r.drag_stopped() || r.changed()) && (v - current).abs() > 1e-9).then_some(v)
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Animate"), |ui| {
        for a in AnimAction::ALL {
            let action = crate::actions::Action::Anim(a);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                act(app, a);
                ui.close();
            }
        }
    });
}

pub fn bottom_panel(app: &mut ModelApp, ui: &mut egui::Ui) {
    tick(app, ui.ctx());
    if !app.anim.open {
        return;
    }
    egui::Panel::bottom("animation").resizable(true).default_size(250.0).min_size(130.0).show(ui, |ui| {
        transport(app, ui);
        key_fields(app, ui);
        ui.separator();
        let rows = rows(app);
        let axis = Axis::new(app, ui.max_rect());
        ruler(app, ui, &axis, &rows);
        match app.anim.view {
            AnimView::Sheet => sheet(app, ui, &axis, &rows),
            AnimView::Curves => curves(app, ui, &axis, &rows),
        }
    });
}

fn transport(app: &mut ModelApp, ui: &mut egui::Ui) {
    let time = app.doc.scene.anim.time;
    let frame = app.doc.scene.anim.frame;
    ui.horizontal_wrapped(|ui| {
        for a in [AnimAction::FirstFrame, AnimAction::PrevKey] {
            if ui.button(a.label()).clicked() {
                act(app, a);
            }
        }
        let play = if app.anim.playback.playing { tl!("Pause") } else { tl!("Play") };
        if ui.add(egui::Button::selectable(app.anim.playback.playing, play)).clicked() {
            act(app, AnimAction::PlayPause);
        }
        for a in [AnimAction::NextKey, AnimAction::LastFrame] {
            if ui.button(a.label()).clicked() {
                act(app, a);
            }
        }
        ui.separator();
        ui.label(tl!("Frame"));
        if let Some(f) = number(ui, tl!("Frame"), frame, 0.25, None, 2) {
            go_to(app, f);
        }
        ui.label(tl!("Start"));
        if let Some(s) = number(ui, tl!("Start"), time.start, 0.25, None, 0) {
            app.run(Command::Anim(AnimCommand::SetTimeRange { start: Some(s), end: Some(time.end.max(s)), fps: None }));
        }
        ui.label(tl!("End"));
        if let Some(e) = number(ui, tl!("End"), time.end, 0.25, None, 0) {
            app.run(Command::Anim(AnimCommand::SetTimeRange { start: Some(time.start.min(e)), end: Some(e), fps: None }));
        }
        ui.label(tl!("Frames per second"));
        if let Some(fps) = number(ui, tl!("Frames per second"), time.fps, 0.1, Some((1.0, 1000.0)), 3) {
            app.run(Command::Anim(AnimCommand::SetTimeRange { start: None, end: None, fps: Some(fps) }));
        }
        let mode_label = |m: LoopMode| match m {
            LoopMode::Once => tl!("Play once"),
            LoopMode::Loop => tl!("Loop"),
            LoopMode::PingPong => tl!("Back and forth"),
        };
        let combo = egui::ComboBox::from_id_salt("anim-loop").selected_text(mode_label(app.anim.playback.mode)).show_ui(ui, |ui| {
            for m in [LoopMode::Once, LoopMode::Loop, LoopMode::PingPong] {
                ui.selectable_value(&mut app.anim.playback.mode, m, mode_label(m));
            }
        });
        set_name(ui, &combo.response, tl!("Playback mode"));
        ui.separator();
        let auto = app.doc.scene.anim.auto_key;
        if ui
            .add(egui::Button::selectable(auto, tl!("Auto-Key")))
            .on_hover_text(tl!("Edits to transforms and animated values set keys at the current frame"))
            .clicked()
        {
            act(app, AnimAction::AutoKey);
        }
        if ui.button(tl!("Key Transform")).clicked() {
            act(app, AnimAction::KeyTransform);
        }
        attribute_keying(app, ui);
        ui.separator();
        layer_controls(app, ui);
        ui.separator();
        for (v, label) in [(AnimView::Sheet, tl!("Key Sheet")), (AnimView::Curves, tl!("Curve Editor"))] {
            if ui.add(egui::Button::selectable(app.anim.view == v, label)).clicked() {
                app.anim.view = v;
            }
        }
        ui.checkbox(&mut app.anim.all_objects, tl!("All Objects"));
        ui.checkbox(&mut app.anim.show_paths, tl!("Motion Paths"))
            .on_hover_text(tl!("Draw the path of selected moving objects in the viewport, with a tick per frame"));
        if ui.button(tl!("Zoom In")).clicked() {
            zoom_by(app, 0.5);
        }
        if ui.button(tl!("Zoom Out")).clicked() {
            zoom_by(app, 2.0);
        }
        if ui.button(tl!("Fit Range")).clicked() {
            app.anim.zoom = None;
        }
    });
}

/// Key any numeric attribute of the active object: pick it from the list, press the button.
fn attribute_keying(app: &mut ModelApp, ui: &mut egui::Ui) {
    let Some(id) = app.active() else { return };
    let Some(obj) = app.doc.scene.get(id) else { return };
    let attrs = modelcraft_engine::scene::attr::list(obj);
    if !attrs.iter().any(|(p, _)| *p == app.anim.attribute) {
        app.anim.attribute = attrs.first().map(|(p, _)| p.clone()).unwrap_or_default();
    }
    let combo = egui::ComboBox::from_id_salt("anim-attribute").selected_text(attr_label(&app.anim.attribute)).show_ui(ui, |ui| {
        for (path, value) in &attrs {
            let text = format!("{}  ({})", attr_label(path), value_text(*value));
            ui.selectable_value(&mut app.anim.attribute, path.clone(), text);
        }
    });
    set_name(ui, &combo.response, tl!("Attribute to key"));
    if ui.button(tl!("Key Attribute")).clicked() {
        let (path, frame) = (app.anim.attribute.clone(), app.doc.scene.anim.frame);
        let label = attr_label(&path);
        if app.run(Command::Anim(AnimCommand::SetKey { id, path, frame: Some(frame), value: None })) {
            app.status = crate::tlf!("{attribute} keyed at frame {frame}", attribute = label, frame = frame_text(frame));
        }
    }
}

/// Animation layers: which layer keys go to, and the active layer's weight, mute and mode.
fn layer_controls(app: &mut ModelApp, ui: &mut egui::Ui) {
    use modelcraft_engine::scene::{BASE_LAYER, LayerMode};
    let anim = &app.doc.scene.anim;
    let active = anim.active_layer();
    let layers: Vec<(u32, String)> = std::iter::once((BASE_LAYER, tl!("Base layer"))).chain(anim.layers().iter().map(|l| (l.id, l.name.clone()))).collect();
    let current = layers.iter().find(|(id, _)| *id == active).map(|(_, n)| n.clone()).unwrap_or_default();
    let mut pick = active;
    let combo = egui::ComboBox::from_id_salt("anim-layer").selected_text(current).show_ui(ui, |ui| {
        for (id, name) in &layers {
            ui.selectable_value(&mut pick, *id, name);
        }
    });
    set_name(ui, &combo.response, tl!("Animation layer"));
    if pick != active && app.run(Command::Anim(AnimCommand::SetActiveLayer { layer: pick })) {
        app.anim.selected.clear();
    }
    if ui.button(tl!("Add Layer")).on_hover_text(tl!("A new layer whose keys are added to the animation below")).clicked() {
        app.run(Command::Anim(AnimCommand::AddLayer { name: None, mode: LayerMode::Add }));
        app.anim.selected.clear();
    }
    let Some(layer) = app.doc.scene.anim.layer(active).cloned() else { return };
    ui.label(tl!("Layer weight"));
    if let Some(w) = number(ui, tl!("Layer weight"), layer.weight, 0.01, Some((0.0, 1.0)), 3) {
        app.run(Command::Anim(AnimCommand::SetLayer { layer: active, name: None, weight: Some(w), mute: None, mode: None }));
    }
    let mut mute = layer.mute;
    if ui.checkbox(&mut mute, tl!("Mute Layer")).changed() {
        app.run(Command::Anim(AnimCommand::SetLayer { layer: active, name: None, weight: None, mute: Some(mute), mode: None }));
    }
    let mut replace = layer.mode == LayerMode::Override;
    if ui
        .checkbox(&mut replace, tl!("Layer Replaces"))
        .on_hover_text(tl!("On: the layer's curves replace the animation below. Off: they are added to it."))
        .changed()
    {
        let mode = if replace { LayerMode::Override } else { LayerMode::Add };
        app.run(Command::Anim(AnimCommand::SetLayer { layer: active, name: None, weight: None, mute: None, mode: Some(mode) }));
    }
    if ui.button(tl!("Remove Layer")).clicked() {
        app.run(Command::Anim(AnimCommand::RemoveLayer { layer: active }));
        app.anim.selected.clear();
    }
    if ui.button(tl!("Flatten Layers")).on_hover_text(tl!("Bake every layer into the base curves and remove the layers")).clicked() {
        app.run(Command::Anim(AnimCommand::FlattenLayers { step: 1.0 }));
        app.anim.selected.clear();
    }
}

fn zoom_by(app: &mut ModelApp, factor: f64) {
    let time = app.doc.scene.anim.time;
    let (a, b) = app.anim.zoom.unwrap_or((time.start, time.end));
    let mid = app.doc.scene.anim.frame.clamp(a, b);
    let half = ((b - a) * 0.5 * factor).clamp(2.0, 500_000.0);
    app.anim.zoom = Some((mid - half, mid + half));
}

/// The fields of the selected keys: the typed path for everything a drag does.
fn key_fields(app: &mut ModelApp, ui: &mut egui::Ui) {
    let Some(primary) = app.anim.selected.last().cloned() else {
        ui.label(
            RichText::new(tl!("No key selected. Click a key, or move to one with Tab and the arrow keys and press Space.")).color(theme::TEXT_DIM).small(),
        );
        return;
    };
    let Some(curve) = app.doc.scene.anim.track(primary.id, &primary.path).map(|t| t.curve.clone()) else { return };
    let Some(key) = curve.key_at(primary.frame).and_then(|i| curve.keys.get(i)).copied() else { return };
    let all = app.anim.selected.clone();
    let edit = |key: &KeyRef| AnimCommand::EditKey {
        key: key.clone(),
        value: None,
        interp: None,
        tangent: None,
        in_slope: None,
        out_slope: None,
        in_weight: None,
        out_weight: None,
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(crate::tlf!("{count} keys selected", count = all.len())).strong());
        ui.label(tl!("Key frame"));
        if let Some(f) = number(ui, tl!("Key frame"), key.t, 0.25, None, 2) {
            let moved = KeyRef { frame: f, ..primary.clone() };
            if app.run(Command::Anim(AnimCommand::MoveKeys { keys: vec![primary.clone()], frames: f - key.t, value: 0.0 })) {
                app.anim.selected.retain(|k| *k != primary);
                app.anim.selected.push(moved);
            }
        }
        ui.label(tl!("Key value"));
        if let Some(v) = number(ui, tl!("Key value"), key.v, 0.01, None, 4) {
            let mut c = edit(&primary);
            if let AnimCommand::EditKey { value, .. } = &mut c {
                *value = Some(v);
            }
            app.run(Command::Anim(c));
        }
        let interp_label = |i: Interp| match i {
            Interp::Stepped => tl!("Stepped"),
            Interp::Linear => tl!("Linear"),
            Interp::Bezier => tl!("Curved"),
        };
        let mut interp = key.interp;
        let combo = egui::ComboBox::from_id_salt("anim-interp").selected_text(interp_label(interp)).show_ui(ui, |ui| {
            for i in Interp::ALL {
                ui.selectable_value(&mut interp, i, interp_label(i));
            }
        });
        set_name(ui, &combo.response, tl!("Interpolation to the next key"));
        if interp != key.interp {
            app.run(Command::Anim(AnimCommand::SetKeyShape { keys: all.clone(), interp: Some(interp), tangent: None }));
        }
        let tangent_label = |t: Tangent| match t {
            Tangent::Auto => tl!("Automatic"),
            Tangent::Flat => tl!("Flat"),
            Tangent::Free => tl!("Free"),
            Tangent::Broken => tl!("Split"),
        };
        let mut tangent = key.tangent;
        let combo = egui::ComboBox::from_id_salt("anim-tangent").selected_text(tangent_label(tangent)).show_ui(ui, |ui| {
            for t in Tangent::ALL {
                ui.selectable_value(&mut tangent, t, tangent_label(t));
            }
        });
        set_name(ui, &combo.response, tl!("Tangent"));
        if tangent != key.tangent {
            app.run(Command::Anim(AnimCommand::SetKeyShape { keys: all.clone(), interp: None, tangent: Some(tangent) }));
        }
        for (name, current, which) in [
            (tl!("Slope in"), key.in_slope, 0),
            (tl!("Slope out"), key.out_slope, 1),
            (tl!("Handle in"), key.in_weight, 2),
            (tl!("Handle out"), key.out_weight, 3),
        ] {
            ui.label(name.clone());
            let range = (which >= 2).then_some((0.0, 1.0));
            if let Some(v) = number(ui, name, current, 0.01, range, 4) {
                let mut c = edit(&primary);
                if let AnimCommand::EditKey { in_slope, out_slope, in_weight, out_weight, .. } = &mut c {
                    match which {
                        0 => *in_slope = Some(v),
                        1 => *out_slope = Some(v),
                        2 => *in_weight = Some(v),
                        _ => *out_weight = Some(v),
                    }
                }
                app.run(Command::Anim(c));
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(tl!("Shift by frames"));
        let r = ui.add(egui::DragValue::new(&mut app.anim.shift_frames).speed(0.25).max_decimals(2));
        set_name(ui, &r, tl!("Shift by frames"));
        ui.label(tl!("Shift by value"));
        let r = ui.add(egui::DragValue::new(&mut app.anim.shift_value).speed(0.01).max_decimals(4));
        set_name(ui, &r, tl!("Shift by value"));
        if ui.button(tl!("Shift Keys")).clicked() {
            move_selected(app, app.anim.shift_frames, app.anim.shift_value);
        }
        ui.label(tl!("Scale time by"));
        let r = ui.add(egui::DragValue::new(&mut app.anim.scale).speed(0.01).range(-100.0..=100.0).max_decimals(3));
        set_name(ui, &r, tl!("Scale time by"));
        if ui.button(tl!("Scale Keys")).on_hover_text(tl!("Scale the selected keys' times around the current frame")).clicked() {
            let (pivot, factor) = (app.doc.scene.anim.frame, app.anim.scale);
            let before: Vec<KeyRef> = app.anim.selected.clone();
            if app.run(Command::Anim(AnimCommand::ScaleKeys { keys: before.clone(), pivot, time: factor, value_pivot: 0.0, value: 1.0 })) {
                app.anim.selected = before.into_iter().map(|k| KeyRef { frame: pivot + (k.frame - pivot) * factor, ..k }).collect();
            }
        }
        if ui.button(tl!("Delete Keys")).clicked() {
            delete_selected(app);
        }
        let extra_label = |e: Extrapolation| match e {
            Extrapolation::Hold => tl!("Hold"),
            Extrapolation::Linear => tl!("Continue"),
            Extrapolation::Cycle => tl!("Repeat"),
            Extrapolation::CycleOffset => tl!("Repeat with offset"),
            Extrapolation::PingPong => tl!("Back and forth"),
        };
        for (name, current, before) in [(tl!("Before first key"), curve.pre, true), (tl!("After last key"), curve.post, false)] {
            let mut e = current;
            let combo = egui::ComboBox::from_id_salt(("anim-extra", before)).selected_text(extra_label(e)).show_ui(ui, |ui| {
                for x in Extrapolation::ALL {
                    ui.selectable_value(&mut e, x, extra_label(x));
                }
            });
            set_name(ui, &combo.response, name);
            if e != current {
                let (pre, post) = if before { (Some(e), None) } else { (None, Some(e)) };
                app.run(Command::Anim(AnimCommand::SetExtrapolation { id: primary.id, path: primary.path.clone(), pre, post }));
            }
        }
        if ui.button(tl!("Remove Curve")).clicked() {
            app.run(Command::Anim(AnimCommand::RemoveTrack { id: primary.id, path: Some(primary.path.clone()) }));
        }
    });
}

/// Move the selected keys and keep them selected.
fn move_selected(app: &mut ModelApp, frames: f64, value: f64) {
    if app.anim.selected.is_empty() || (frames == 0.0 && value == 0.0) {
        return;
    }
    let keys = app.anim.selected.clone();
    if app.run(Command::Anim(AnimCommand::MoveKeys { keys: keys.clone(), frames, value })) {
        app.anim.selected = keys.into_iter().map(|k| KeyRef { frame: k.frame + frames, ..k }).collect();
        app.anim.focus_key = app.anim.selected.last().cloned();
        app.status = crate::tlf!("{count} keys moved", count = app.anim.selected.len());
    }
}

// ---------------------------------------------------------------- rows and the time axis

struct Row {
    id: ObjectId,
    name: String,
    path: String,
    curve: Arc<Curve>,
}

impl Row {
    fn label(&self) -> String {
        format!("{} · {}", self.name, attr_label(&self.path))
    }
    fn key_ref(&self, t: f64) -> KeyRef {
        KeyRef { id: self.id, path: self.path.clone(), frame: t }
    }
}

fn rows(app: &ModelApp) -> Vec<Row> {
    let ids = shown_objects(app);
    let layer = app.doc.scene.anim.active_layer();
    app.doc
        .scene
        .anim
        .tracks()
        .iter()
        .filter(|t| t.layer == layer && (ids.is_empty() || ids.contains(&t.target)))
        .take(MAX_ROWS)
        .map(|t| Row { id: t.target, name: object_name(app, t.target), path: t.path.clone(), curve: t.curve.clone() })
        .collect()
}

/// Frames to pixels along the panel.
#[derive(Clone, Copy)]
struct Axis {
    x0: f32,
    x1: f32,
    t0: f64,
    t1: f64,
}

impl Axis {
    fn new(app: &ModelApp, rect: Rect) -> Axis {
        let time = app.doc.scene.anim.time;
        let (a, b) = app.anim.zoom.unwrap_or_else(|| {
            let pad = (time.span() * 0.03).max(1.0);
            (time.start - pad, time.end + pad)
        });
        Axis { x0: rect.left() + LABEL_W, x1: (rect.right() - 8.0).max(rect.left() + LABEL_W + 40.0), t0: a, t1: b.max(a + 1.0) }
    }
    fn px_per_frame(&self) -> f64 {
        ((self.x1 - self.x0) as f64 / (self.t1 - self.t0)).max(1e-6)
    }
    fn x(&self, t: f64) -> f32 {
        self.x0 + ((t - self.t0) * self.px_per_frame()) as f32
    }
    fn t(&self, x: f32) -> f64 {
        self.t0 + (x - self.x0) as f64 / self.px_per_frame()
    }
    fn shows(&self, t: f64) -> bool {
        t >= self.t0 && t <= self.t1
    }
    /// Frames between labelled ticks: 1, 2, 5, 10 … so that labels are at least 56 px apart.
    fn tick_step(&self) -> f64 {
        let want = 56.0 / self.px_per_frame();
        let mut step = 1.0;
        for _ in 0..24 {
            for m in [1.0, 2.0, 5.0] {
                if step * m >= want {
                    return step * m;
                }
            }
            step *= 10.0;
        }
        step
    }
}

/// The time ruler: frame numbers, key marks, the range and the current frame. A slider for
/// assistive technology; Left/Right step a frame (Shift: ten), Home/End go to the range ends.
fn ruler(app: &mut ModelApp, ui: &mut egui::Ui, axis: &Axis, rows: &[Row]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), Sense::hover());
    let strip = Rect::from_min_max(Pos2::new(axis.x0, rect.top()), Pos2::new(axis.x1, rect.bottom()));
    let resp = ui.interact(strip, egui::Id::unique("anim-ruler"), Sense::click_and_drag());
    let frame = app.doc.scene.anim.frame;
    let time = app.doc.scene.anim.time;
    resp.widget_info(|| egui::WidgetInfo::slider(true, frame, tl!("Current frame")));
    if (resp.clicked() || resp.dragged())
        && let Some(p) = resp.interact_pointer_pos()
    {
        resp.request_focus();
        let f = axis.t(p.x).round();
        if f != frame {
            app.doc.set_frame(f);
        }
    }
    if resp.has_focus() {
        if let Some(m) = focused_shortcuts(ui, false) {
            undo_redo(app, ui.ctx(), &m);
        }
        let (left, right, shift, home, end) = ui.input_mut(|i| {
            let shift = i.modifiers.shift;
            let m = if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE };
            (
                i.consume_key(m, egui::Key::ArrowLeft),
                i.consume_key(m, egui::Key::ArrowRight),
                shift,
                i.consume_key(egui::Modifiers::NONE, egui::Key::Home),
                i.consume_key(egui::Modifiers::NONE, egui::Key::End),
            )
        });
        let step = if shift { 10.0 } else { 1.0 };
        if left {
            go_to(app, (frame - step).ceil());
        } else if right {
            go_to(app, (frame + step).floor());
        } else if home {
            go_to(app, time.start);
        } else if end {
            go_to(app, time.end);
        }
    }
    let p = ui.painter_at(rect);
    p.rect_filled(strip, 2.0, theme::PANEL_DEEP);
    // The playback range.
    let range =
        Rect::from_min_max(Pos2::new(axis.x(time.start).max(strip.left()), strip.top()), Pos2::new(axis.x(time.end).min(strip.right()), strip.bottom()));
    if range.width() > 0.0 {
        p.rect_filled(range, 0.0, theme::WIDGET);
    }
    let step = axis.tick_step();
    let mut t = (axis.t0 / step).ceil() * step;
    let font = egui::FontId::proportional(10.5);
    for _ in 0..400 {
        if t > axis.t1 {
            break;
        }
        let x = axis.x(t);
        p.line_segment([Pos2::new(x, strip.bottom() - 7.0), Pos2::new(x, strip.bottom())], Stroke::new(1.0, theme::TEXT_DIM));
        p.text(Pos2::new(x + 3.0, strip.top() + 1.0), egui::Align2::LEFT_TOP, frame_text(t), font.clone(), theme::TEXT_DIM);
        t += step;
    }
    // Key marks of the shown curves.
    let mut marks: Vec<f64> = rows.iter().flat_map(|r| r.curve.keys.iter().map(|k| k.t)).filter(|t| axis.shows(*t)).collect();
    marks.sort_by(f64::total_cmp);
    marks.dedup();
    for t in marks.iter().take(4000) {
        let x = axis.x(*t);
        p.line_segment([Pos2::new(x, strip.bottom() - 11.0), Pos2::new(x, strip.bottom() - 1.0)], Stroke::new(2.0, theme::ACCENT));
    }
    // The current frame.
    let x = axis.x(frame);
    if axis.shows(frame) {
        p.line_segment([Pos2::new(x, strip.top()), Pos2::new(x, strip.bottom())], Stroke::new(2.0, Color32::WHITE));
    }
    let focus = if resp.has_focus() { theme::ACCENT } else { theme::TEXT };
    p.text(
        Pos2::new(rect.left() + 4.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        crate::tlf!("Frame {frame} of {start}–{end}", frame = frame_text(frame), start = frame_text(time.start), end = frame_text(time.end)),
        egui::FontId::proportional(12.0),
        focus,
    );
    if resp.has_focus() {
        p.rect_stroke(strip, 2.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Inside);
    }
}

fn playhead(ui: &egui::Ui, axis: &Axis, rect: Rect, frame: f64) {
    if axis.shows(frame) {
        let x = axis.x(frame);
        ui.painter_at(rect).line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, Color32::from_white_alpha(120)));
    }
}

// ---------------------------------------------------------------- keys

/// What the keyboard asked of a focused key.
enum KeyMove {
    /// Focus another key: rows down, keys right.
    Focus(i32, i32),
    /// Nudge the selected keys by frames and by value steps.
    Nudge(f64, f64),
    Delete,
    Undo,
    Redo,
}

/// Undo, redo and (for keys) Delete while a widget of the panel has keyboard focus. The app's
/// shortcut handler leaves every key to a focused widget, so the panel takes these itself.
fn focused_shortcuts(ui: &egui::Ui, with_delete: bool) -> Option<KeyMove> {
    ui.input_mut(|i| {
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Y) {
            Some(KeyMove::Redo)
        } else if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z) {
            Some(KeyMove::Undo)
        } else if with_delete && (i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
            Some(KeyMove::Delete)
        } else {
            None
        }
    })
}

fn undo_redo(app: &mut ModelApp, ctx: &egui::Context, m: &KeyMove) -> bool {
    match m {
        KeyMove::Undo => app.act(ctx, crate::actions::Action::Undo),
        KeyMove::Redo => app.act(ctx, crate::actions::Action::Redo),
        _ => return false,
    }
    true
}

/// One key as a widget: click selects (Shift/Ctrl adds), double-click goes to its frame, dragging
/// moves the selected keys, arrows move focus between keys, Alt+arrows nudge.
fn key_widget(
    app: &mut ModelApp,
    ui: &mut egui::Ui,
    rect: Rect,
    row: &Row,
    key: &Key,
    view: AnimView,
    axis: &Axis,
    value_px: f64,
) -> (egui::Response, Option<KeyMove>) {
    let kref = row.key_ref(key.t);
    let id = egui::Id::unique(("anim-key", view == AnimView::Curves, row.id.0, &row.path, key.t.to_bits()));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let selected = app.anim.selected.contains(&kref);
    let label = crate::tlf!("{track} key, frame {frame}, value {value}", track = row.label(), frame = frame_text(key.t), value = value_text(key.v));
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &label));
    // Say "not selected" too, so the state is always in the tree.
    ui.ctx().accesskit_node_builder(resp.id, |n| n.set_selected(selected));
    if app.anim.focus_key.as_ref() == Some(&kref) {
        resp.request_focus();
        app.anim.focus_key = None;
    }
    if resp.clicked() {
        resp.request_focus();
        let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        if add {
            match app.anim.selected.iter().position(|k| *k == kref) {
                Some(p) => {
                    app.anim.selected.remove(p);
                }
                None => app.anim.selected.push(kref.clone()),
            }
        } else {
            app.anim.selected = vec![kref.clone()];
        }
        app.status = label.clone();
    }
    if resp.double_clicked() {
        go_to(app, key.t);
    }
    if resp.drag_started() {
        if !selected {
            app.anim.selected = vec![kref.clone()];
        }
        app.anim.key_drag = Some((0.0, 0.0));
    }
    if resp.dragged() && app.anim.key_drag.is_some() {
        let d = ui.input(|i| i.pointer.press_origin().zip(i.pointer.interact_pos()).map(|(o, p)| p - o)).unwrap_or_default();
        let frames = (d.x as f64 / axis.px_per_frame()).round();
        let value = if view == AnimView::Curves && value_px > 0.0 { -(d.y as f64) / value_px } else { 0.0 };
        app.anim.key_drag = Some((frames, value));
    }
    if resp.drag_stopped()
        && let Some((frames, value)) = app.anim.key_drag.take()
    {
        move_selected(app, frames, value);
    }
    let mut moved = None;
    if resp.has_focus() {
        moved = focused_shortcuts(ui, true);
        ui.input_mut(|i| {
            for (key, dr, dk) in [(egui::Key::ArrowLeft, 0, -1), (egui::Key::ArrowRight, 0, 1), (egui::Key::ArrowUp, -1, 0), (egui::Key::ArrowDown, 1, 0)] {
                for (mods, big) in [(egui::Modifiers::ALT | egui::Modifiers::SHIFT, 10.0), (egui::Modifiers::ALT, 1.0)] {
                    if i.consume_key(mods, key) {
                        moved = Some(KeyMove::Nudge(dk as f64 * big, -dr as f64 * big));
                    }
                }
                if i.consume_key(egui::Modifiers::NONE, key) {
                    moved = Some(KeyMove::Focus(dr, dk));
                }
            }
        });
    }
    (resp, moved)
}

/// Apply what the keyboard asked of the key at (`row`, `index`).
fn apply_key_move(app: &mut ModelApp, ctx: &egui::Context, rows: &[Row], row: usize, index: usize, m: KeyMove, value_step: f64) {
    if undo_redo(app, ctx, &m) {
        return;
    }
    let Some(here) = rows.get(row) else { return };
    let Some(key) = here.curve.keys.get(index) else { return };
    match m {
        KeyMove::Focus(dr, dk) => {
            let target = if dr == 0 {
                let i = (index as i64 + dk as i64).clamp(0, here.curve.len() as i64 - 1) as usize;
                here.curve.keys.get(i).map(|k| here.key_ref(k.t))
            } else {
                // The nearest key in time on the next row that has keys.
                let mut r = row as i64 + dr as i64;
                let mut found = None;
                while r >= 0 && (r as usize) < rows.len() && found.is_none() {
                    if let Some(other) = rows.get(r as usize) {
                        found = other.curve.keys.iter().min_by(|a, b| (a.t - key.t).abs().total_cmp(&(b.t - key.t).abs())).map(|k| other.key_ref(k.t));
                    }
                    r += dr as i64;
                }
                found
            };
            if let Some(t) = target {
                app.status = crate::tlf!("{track} key, frame {frame}", track = attr_label(&t.path), frame = frame_text(t.frame));
                app.anim.focus_key = Some(t);
            }
        }
        KeyMove::Nudge(frames, steps) => {
            let kref = here.key_ref(key.t);
            if !app.anim.selected.contains(&kref) {
                app.anim.selected = vec![kref];
            }
            move_selected(app, frames, steps * value_step);
        }
        KeyMove::Delete => {
            let kref = here.key_ref(key.t);
            if app.anim.selected.is_empty() {
                app.anim.selected = vec![kref];
            }
            delete_selected(app);
        }
        KeyMove::Undo | KeyMove::Redo => {}
    }
}

fn paint_key(p: &egui::Painter, c: Pos2, selected: bool, focused: bool, interp: Interp) {
    let fill = if selected { theme::SELECT } else { theme::TEXT };
    let r = 5.0;
    // The shape says how the value leaves the key, so colour is never the only cue.
    match interp {
        Interp::Stepped => {
            p.rect_filled(Rect::from_center_size(c, egui::vec2(2.0 * r - 2.0, 2.0 * r - 2.0)), 0.0, fill);
        }
        Interp::Linear => {
            p.add(egui::Shape::convex_polygon(vec![c + egui::vec2(0.0, -r), c + egui::vec2(r, r), c + egui::vec2(-r, r)], fill, Stroke::NONE));
        }
        Interp::Bezier => {
            p.add(egui::Shape::convex_polygon(
                vec![c + egui::vec2(0.0, -r), c + egui::vec2(r, 0.0), c + egui::vec2(0.0, r), c + egui::vec2(-r, 0.0)],
                fill,
                Stroke::NONE,
            ));
        }
    }
    if selected {
        p.circle_stroke(c, r + 2.5, Stroke::new(1.5, Color32::WHITE));
    }
    if focused {
        p.rect_stroke(Rect::from_center_size(c, egui::vec2(18.0, 18.0)), 2.0, Stroke::new(1.5, theme::ACCENT), egui::StrokeKind::Outside);
    }
}

// ---------------------------------------------------------------- key sheet

fn empty_hint(app: &ModelApp, ui: &mut egui::Ui) {
    let anim = &app.doc.scene.anim;
    let text = if anim.is_empty() {
        tl!("No animation yet. Select an object and press Key Transform, or turn on Auto-Key and move something.")
    } else if !anim.tracks().iter().any(|t| t.layer == anim.active_layer()) {
        tl!("This layer has no keys yet. Keys you set now go to this layer and adjust the animation below it.")
    } else {
        tl!("The selected objects have no curves. Select an animated object or tick All Objects.")
    };
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
}

fn sheet(app: &mut ModelApp, ui: &mut egui::Ui, axis: &Axis, rows: &[Row]) {
    if rows.is_empty() {
        empty_hint(app, ui);
        return;
    }
    let frame = app.doc.scene.anim.frame;
    let drag = app.anim.key_drag;
    let mut pending: Option<(usize, usize, KeyMove)> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (ri, row) in rows.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::hover());
            let strip = Rect::from_min_max(Pos2::new(axis.x0, rect.top()), Pos2::new(axis.x1, rect.bottom()));
            let p = ui.painter_at(rect);
            if ri % 2 == 0 {
                p.rect_filled(strip, 0.0, theme::PANEL_DEEP);
            }
            let label_rect = Rect::from_min_max(rect.min, Pos2::new(axis.x0 - 6.0, rect.bottom()));
            ui.scope_builder(egui::UiBuilder::new().max_rect(label_rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(row.label()).small()).truncate());
            });
            playhead(ui, axis, strip, frame);
            for (ki, key) in row.curve.keys.iter().enumerate() {
                if !axis.shows(key.t) {
                    continue;
                }
                let selected = app.anim.selected.contains(&row.key_ref(key.t));
                let shift = if selected { drag.map_or(0.0, |d| d.0) } else { 0.0 };
                let c = Pos2::new(axis.x(key.t + shift), rect.center().y);
                let hit = Rect::from_center_size(Pos2::new(axis.x(key.t), rect.center().y), egui::vec2(14.0, ROW_H - 2.0));
                let (resp, m) = key_widget(app, ui, hit, row, key, AnimView::Sheet, axis, 0.0);
                if let Some(m) = m {
                    pending = Some((ri, ki, m));
                }
                paint_key(&p, c, selected, resp.has_focus(), key.interp);
            }
        }
    });
    if let Some((r, k, m)) = pending {
        apply_key_move(app, ui.ctx(), rows, r, k, m, 0.0);
    }
}

// ---------------------------------------------------------------- curve editor

fn curve_color(path: &str, index: usize) -> Color32 {
    match path.rsplit('.').next() {
        Some("x") | Some("0") => theme::AXIS_X,
        Some("y") | Some("1") => theme::AXIS_Y,
        Some("z") | Some("2") => theme::AXIS_Z,
        _ => [Color32::from_rgb(0xE6, 0xC2, 0x4A), Color32::from_rgb(0x5C, 0xD0, 0xC8), Color32::from_rgb(0xD0, 0x7C, 0xE0)][index % 3],
    }
}

/// The curve with the pending drag applied, so the drawing follows the pointer.
fn preview(app: &ModelApp, row: &Row) -> Arc<Curve> {
    if let Some((frames, value)) = app.anim.key_drag {
        let idx: Vec<usize> = app.anim.selected.iter().filter(|k| k.id == row.id && k.path == row.path).filter_map(|k| row.curve.key_at(k.frame)).collect();
        if !idx.is_empty() {
            let mut c = (*row.curve).clone();
            c.move_keys(&idx, frames, value);
            return Arc::new(c);
        }
    }
    if let Some(h) = &app.anim.handle_drag
        && h.key.id == row.id
        && h.key.path == row.path
        && let Some(i) = row.curve.key_at(h.key.frame)
    {
        let mut c = (*row.curve).clone();
        if let Some(k) = c.keys.get_mut(i) {
            if k.tangent != Tangent::Broken {
                k.tangent = Tangent::Free;
                k.in_slope = h.slope;
                k.out_slope = h.slope;
            } else if h.out {
                k.out_slope = h.slope;
            } else {
                k.in_slope = h.slope;
            }
            if h.out {
                k.out_weight = h.weight;
            } else {
                k.in_weight = h.weight;
            }
        }
        return Arc::new(c);
    }
    row.curve.clone()
}

fn curves(app: &mut ModelApp, ui: &mut egui::Ui, axis: &Axis, rows: &[Row]) {
    if rows.is_empty() {
        empty_hint(app, ui);
        return;
    }
    let frame = app.doc.scene.anim.frame;
    let area = ui.available_rect_before_wrap();
    let plot = Rect::from_min_max(Pos2::new(axis.x0, area.top()), Pos2::new(axis.x1, area.bottom().max(area.top() + 60.0)));
    // The list of curves, with a switch each.
    let list = Rect::from_min_max(area.min, Pos2::new(axis.x0 - 6.0, plot.bottom()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        egui::ScrollArea::vertical().id_salt("anim-curve-list").auto_shrink([false, false]).show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let key = (row.id, row.path.clone());
                let mut shown = !app.anim.hidden.contains(&key);
                let text = RichText::new(row.label()).small().color(curve_color(&row.path, i));
                let r = ui.checkbox(&mut shown, text);
                set_name(ui, &r, crate::tlf!("Show curve {track}", track = row.label()));
                if r.changed() {
                    if shown {
                        app.anim.hidden.remove(&key);
                    } else {
                        app.anim.hidden.insert(key);
                    }
                }
            }
        });
    });
    ui.allocate_rect(area, Sense::hover());
    let visible: Vec<(usize, &Row, Arc<Curve>)> =
        rows.iter().enumerate().filter(|(_, r)| !app.anim.hidden.contains(&(r.id, r.path.clone()))).map(|(i, r)| (i, r, preview(app, r))).collect();
    // Fit the value axis to what is shown.
    let samples = 96;
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for (_, _, c) in &visible {
        for s in 0..=samples {
            let v = c.eval_or(axis.t0 + (axis.t1 - axis.t0) * s as f64 / samples as f64, 0.0);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        for k in &c.keys {
            lo = lo.min(k.v);
            hi = hi.max(k.v);
        }
    }
    if lo > hi {
        (lo, hi) = (-1.0, 1.0);
    }
    let pad = ((hi - lo) * 0.12).max(0.5);
    let (lo, hi) = (lo - pad, hi + pad);
    let value_px = (plot.height() as f64 / (hi - lo)).max(1e-9);
    let y = |v: f64| plot.bottom() - ((v - lo) * value_px) as f32;
    let p = ui.painter_at(plot);
    p.rect_filled(plot, 2.0, theme::PANEL_DEEP);
    // Value grid.
    let step = {
        let want = 28.0 / value_px;
        let mut s = 10f64.powf(want.log10().floor());
        for m in [1.0, 2.0, 5.0, 10.0] {
            if s * m >= want {
                s *= m;
                break;
            }
        }
        s.max(1e-9)
    };
    let mut v = (lo / step).ceil() * step;
    for _ in 0..200 {
        if v > hi {
            break;
        }
        let strong = v.abs() < step * 0.5;
        p.line_segment(
            [Pos2::new(plot.left(), y(v)), Pos2::new(plot.right(), y(v))],
            Stroke::new(1.0, if strong { theme::TEXT_DIM } else { Color32::from_rgb(0x30, 0x33, 0x38) }),
        );
        p.text(Pos2::new(plot.left() + 3.0, y(v) - 1.0), egui::Align2::LEFT_BOTTOM, value_text(v), egui::FontId::proportional(10.0), theme::TEXT_DIM);
        v += step;
    }
    playhead(ui, axis, plot, frame);
    let mut pending: Option<(usize, usize, KeyMove)> = None;
    for (ri, row, curve) in &visible {
        let color = curve_color(&row.path, *ri);
        let n = ((plot.width() / 2.0) as usize).clamp(8, 2000);
        let pts: Vec<Pos2> = (0..=n)
            .map(|s| {
                let t = axis.t0 + (axis.t1 - axis.t0) * s as f64 / n as f64;
                Pos2::new(axis.x(t), y(curve.eval_or(t, 0.0)))
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
        // Keys are drawn where the preview puts them, but stay addressed by their stored frame.
        for (ki, key) in row.curve.keys.iter().enumerate() {
            let kref = row.key_ref(key.t);
            let selected = app.anim.selected.contains(&kref);
            let (dt, dv) = if selected { app.anim.key_drag.unwrap_or((0.0, 0.0)) } else { (0.0, 0.0) };
            if !axis.shows(key.t) {
                continue;
            }
            let hit = Rect::from_center_size(Pos2::new(axis.x(key.t), y(key.v)), egui::vec2(14.0, 14.0));
            let (resp, m) = key_widget(app, ui, hit, row, key, AnimView::Curves, axis, value_px);
            if let Some(m) = m {
                pending = Some((*ri, ki, m));
            }
            paint_key(&p, Pos2::new(axis.x(key.t + dt), y(key.v + dv)), selected, resp.has_focus(), key.interp);
        }
    }
    handles(app, ui, axis, &visible, &p, &y, value_px);
    if let Some((r, k, m)) = pending {
        apply_key_move(app, ui.ctx(), rows, r, k, m, step / 10.0);
    }
}

/// The tangent handles of the key the fields edit. Dragging one sets its slope and length; the
/// "Slope" and "Handle" fields do the same by number.
fn handles(
    app: &mut ModelApp,
    ui: &mut egui::Ui,
    axis: &Axis,
    visible: &[(usize, &Row, Arc<Curve>)],
    p: &egui::Painter,
    y: &dyn Fn(f64) -> f32,
    value_px: f64,
) {
    let Some(primary) = app.anim.selected.last().cloned() else { return };
    if app.anim.key_drag.is_some() {
        return;
    }
    let Some((_, _, curve)) = visible.iter().find(|(_, r, _)| r.id == primary.id && r.path == primary.path) else { return };
    let Some(i) = curve.key_at(primary.frame) else { return };
    let Some(key) = curve.keys.get(i).copied() else { return };
    let prev = i.checked_sub(1).and_then(|j| curve.keys.get(j)).copied();
    let next = curve.keys.get(i + 1).copied();
    let center = Pos2::new(axis.x(key.t), y(key.v));
    for out in [false, true] {
        // A handle exists where a curved segment meets the key.
        let (span, slope, weight) = match (out, prev, next) {
            (true, _, Some(n)) if key.interp == Interp::Bezier => (n.t - key.t, key.out_slope, key.out_weight),
            (false, Some(pk), _) if pk.interp == Interp::Bezier => (key.t - pk.t, key.in_slope, key.in_weight),
            _ => continue,
        };
        let sign = if out { 1.0 } else { -1.0 };
        let reach = (span * weight).max(span * 0.02);
        let at = Pos2::new(axis.x(key.t + sign * reach), y(key.v + sign * slope * reach));
        let id = egui::Id::unique(("anim-handle", out));
        let resp = ui.interact(Rect::from_center_size(at, egui::vec2(12.0, 12.0)), id, Sense::drag());
        let name = if out { tl!("Outgoing tangent handle") } else { tl!("Incoming tangent handle") };
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &name));
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let dt = (sign * (axis.t(pos.x) - key.t)).clamp(span * 0.02, span);
            let dv = sign * -((pos.y - center.y) as f64) / value_px;
            app.anim.handle_drag = Some(HandleDrag { key: primary.clone(), out, slope: dv / dt, weight: (dt / span).clamp(0.0, 1.0) });
        }
        if resp.drag_stopped()
            && let Some(h) = app.anim.handle_drag.take()
        {
            let (mut in_slope, mut out_slope, mut in_weight, mut out_weight) = (None, None, None, None);
            if h.out {
                (out_slope, out_weight) = (Some(h.slope), Some(h.weight));
            } else {
                (in_slope, in_weight) = (Some(h.slope), Some(h.weight));
            }
            let tangent = if key.tangent == Tangent::Broken { Tangent::Broken } else { Tangent::Free };
            app.run(Command::Anim(AnimCommand::EditKey {
                key: h.key,
                value: None,
                interp: None,
                tangent: Some(tangent),
                in_slope,
                out_slope,
                in_weight,
                out_weight,
            }));
        }
        p.line_segment([center, at], Stroke::new(1.0, theme::TEXT_DIM));
        p.circle_filled(at, 3.5, if resp.hovered() || resp.dragged() { theme::SELECT } else { theme::TEXT });
    }
}
