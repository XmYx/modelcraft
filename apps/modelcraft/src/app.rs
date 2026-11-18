//! Application state, actions, files, layout and shortcuts.

use std::collections::BTreeSet;
use std::path::PathBuf;

use glam::{Quat, Vec3};
use modelcraft_engine::scene::{ObjectId, ObjectKind, StepOp};
use modelcraft_engine::sculpt::{BrushKind, BrushSettings};
use modelcraft_engine::{Command, Document};
use modelcraft_render::{Camera, ViewPreset};

use crate::actions::Action;
use crate::args::{Cli, Demo, StartWorkspace};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Workspace {
    Model,
    Sculpt,
    Paint,
}

impl Workspace {
    pub fn label(self) -> String {
        match self {
            Workspace::Model => tl!("Model"),
            Workspace::Sculpt => tl!("Sculpt"),
            Workspace::Paint => tl!("Paint"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    Object,
    Vertex,
    Edge,
    Face,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Move,
    Rotate,
    Scale,
}

/// Mouse navigation presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum NavPreset {
    /// Alt + left orbits, Alt + middle pans, Alt + right zooms.
    AltMouse,
    /// Middle orbits, Shift + middle pans, wheel zooms.
    MiddleMouse,
    /// Right orbits, Shift + right pans (the left button stays free for brushes).
    RightMouse,
}

impl NavPreset {
    pub fn label(self) -> String {
        match self {
            NavPreset::AltMouse => tl!("Alt + mouse buttons"),
            NavPreset::MiddleMouse => tl!("Middle mouse button"),
            NavPreset::RightMouse => tl!("Right mouse button"),
        }
    }
}

/// Where a panel lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Left,
    Right,
    Float,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelId {
    Tree,
    Properties,
    Brushes,
    Paint,
    History,
}

impl PanelId {
    pub const ALL: [PanelId; 5] = [PanelId::Tree, PanelId::Properties, PanelId::Brushes, PanelId::Paint, PanelId::History];
    pub fn label(self) -> String {
        match self {
            PanelId::Tree => tl!("Scene"),
            PanelId::Properties => tl!("Properties"),
            PanelId::Brushes => tl!("Brushes"),
            PanelId::Paint => tl!("Paint"),
            PanelId::History => tl!("Undo History"),
        }
    }
}

/// Panel placement per workspace (the workspace layouts).
#[derive(Clone, Debug)]
pub struct Layout {
    pub docks: std::collections::HashMap<(Workspace, PanelId), Dock>,
}

impl Default for Layout {
    fn default() -> Self {
        use Dock::*;
        use PanelId::*;
        use Workspace::*;
        let mut d = std::collections::HashMap::new();
        for (ws, panel, dock) in [
            (Model, Tree, Left),
            (Model, Properties, Right),
            (Model, Brushes, Hidden),
            (Model, PanelId::Paint, Hidden),
            (Model, History, Left),
            (Sculpt, History, Hidden),
            (Workspace::Paint, History, Hidden),
            (Sculpt, Tree, Hidden),
            (Sculpt, Brushes, Left),
            (Sculpt, Properties, Right),
            (Sculpt, PanelId::Paint, Hidden),
            (Workspace::Paint, Tree, Left),
            (Workspace::Paint, Properties, Hidden),
            (Workspace::Paint, Brushes, Hidden),
            (Workspace::Paint, PanelId::Paint, Right),
        ] {
            d.insert((ws, panel), dock);
        }
        Layout { docks: d }
    }
}

impl Layout {
    pub fn dock(&self, ws: Workspace, p: PanelId) -> Dock {
        self.docks.get(&(ws, p)).copied().unwrap_or(Dock::Hidden)
    }
    pub fn set(&mut self, ws: Workspace, p: PanelId, d: Dock) {
        self.docks.insert((ws, p), d);
    }
}

/// One viewport of the single or four-view layout.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub camera: Camera,
    pub preset: ViewPreset,
}

/// Parameters of the modeling operations, edited in the tool options bar.
#[derive(Clone, Copy, Debug)]
pub struct OpParams {
    pub extrude: f32,
    pub inset: f32,
    pub bevel: f32,
    pub cuts: u32,
    pub subdivide: u32,
}

impl Default for OpParams {
    fn default() -> Self {
        OpParams { extrude: 0.5, inset: 0.3, bevel: 0.1, cuts: 1, subdivide: 1 }
    }
}

/// The "Transform by Numbers" dialog: the non-drag path for gizmo transforms.
#[derive(Clone, Copy, Debug)]
pub struct NumericTransform {
    pub translate: Vec3,
    pub rotate_deg: Vec3,
    pub scale: Vec3,
}

impl Default for NumericTransform {
    fn default() -> Self {
        NumericTransform { translate: Vec3::ZERO, rotate_deg: Vec3::ZERO, scale: Vec3::ONE }
    }
}

/// An action waiting for the answer to "save changes?".
#[derive(Clone, Debug, PartialEq)]
pub enum Pending {
    New,
    Open,
    OpenPath(PathBuf),
    Quit,
}

pub struct ModelApp {
    pub doc: Document,
    pub keymap: craft_keymap::Keymap,
    pub dispatcher: craft_keymap::egui::Dispatcher,
    pub shortcuts_state: Option<craft_keymap::ui::DialogState>,
    /// Preferences on disk (`None` in tests: nothing is read or written).
    pub prefs: Option<craft_settings::store::SettingsStore<crate::prefs::Prefs>>,
    pub recent: Vec<String>,
    pub snap: bool,
    pub snap_steps: (f32, f32, f32),
    pub pending: Option<Pending>,
    pub close_confirmed: bool,
    pub quit_requested: bool,
    pub workspace: Workspace,
    pub mode: SelectMode,
    pub tool: Tool,
    pub selection: Vec<ObjectId>,
    pub components: BTreeSet<u32>,
    /// The vertices of the component selection when a gizmo drag started.
    pub component_vertices_cached: Vec<u32>,
    /// The scene-tree row that takes keyboard focus when it is drawn next.
    pub tree_focus: Option<ObjectId>,
    /// Scene before a live property drag (restored before the command commits).
    pub prop_snapshot: Option<Box<modelcraft_engine::scene::Scene>>,
    pub views: [View; 4],
    pub quad: bool,
    pub active_view: usize,
    pub nav: NavPreset,
    pub brush: BrushSettings,
    /// Brush radius in screen pixels.
    pub brush_px: f32,
    /// Brush presets the user saved (shown after the built-in ones).
    pub user_presets: Vec<modelcraft_engine::sculpt::preset::BrushPreset>,
    pub ops: OpParams,
    pub layout: Layout,
    pub status: String,
    pub palette: Option<crate::panels::Palette>,
    pub numeric: Option<NumericTransform>,
    pub rename: Option<(ObjectId, String)>,
    pub about: bool,
    pub render_view: crate::render_view::RenderView,
    /// Up axis and unit of files read and written (File ▸ Exchange Settings).
    pub exchange: modelcraft_engine::formats::exchange::Convert,
    pub script: crate::script_ui::ScriptState,
    pub mesh_tools: crate::mesh_tools::State,
    pub radial: Option<crate::radial::Radial>,
    pub prefs_window: crate::prefs_ui::PrefsWindow,
    /// Action ids on the shelf.
    pub shelf: Vec<String>,
    pub show_grid: bool,
    pub show_axes: bool,
    pub unit: crate::prefs_ui::Unit,
    /// Interface language code; `None` follows the system.
    pub language: Option<String>,
    pub objects: crate::object_manager::State,
    pub recovery: Option<PathBuf>,
    pub gpu: Option<crate::gpu::GpuState>,
    pub gpu_failed: bool,
    pub drag: Option<crate::viewport::Drag>,
    pub stroke: Option<crate::viewport::StrokeInput>,
    pub show_wire: bool,
    pub show_patches: bool,
    /// Patches and visibility while sculpting: the region tool, its dialogs and the patch list.
    pub sculpt_groups: crate::sculpt_groups_ui::State,
    pub screenshot: Option<PathBuf>,
    pub screenshot_sent: bool,
    pub shot: std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<egui::ColorImage>>>>,
    pub frames: u64,
    pub frame_times: std::collections::VecDeque<f32>,
    pub bench: Option<crate::viewport::Bench>,
    pub focus_panel: usize,
    pub last_frame: Option<std::time::Instant>,
    pub autosave_at: Option<std::time::Instant>,
    pub recovery_dir: Option<PathBuf>,
    /// The Animation panel: playback, key selection, views.
    pub anim: crate::anim_ui::AnimUi,
    pub agents: crate::agents::Agents,
    /// The allow-agents preference.
    pub allow_agents: bool,
    /// The UV Editor window: selection, pins, view and display settings.
    pub uv: crate::uv_ui::UvUi,
}

/// The engine's note about a restored or dropped undo history, in the interface language.
fn history_note(n: &str) -> String {
    if let Some(count) = n.strip_prefix("Restored ").and_then(|r| r.split(' ').next()) {
        return crate::tlf!("Restored {count} undo steps from the last session.", count = count);
    }
    if n.contains("left out") || n.contains("could not be read") {
        return tl!("The saved undo history could not be rebuilt; undo starts here.");
    }
    n.to_string()
}

pub fn recovery_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("modelcraft").join("recovery"))
}

impl ModelApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cli: Cli) -> Self {
        let mut app = Self::with_context(&cc.egui_ctx, cli);
        let (store, _report) = craft_settings::store::SettingsStore::<crate::prefs::Prefs>::open(craft_settings::paths::Location::Auto);
        app.apply_prefs(store.get().clone());
        app.prefs = Some(store);
        app.recovery_dir = recovery_dir();
        app.start_autosave();
        if app.allow_agents {
            crate::agents::start(&mut app, &cc.egui_ctx);
        }
        app.script = crate::script_ui::ScriptState::new(crate::script_ui::default_dir());
        crate::script_ui::startup(&mut app);
        app
    }

    /// Set up on an egui context (also used by the UI tests: no GPU, no autosave).
    pub fn with_context(ctx: &egui::Context, cli: Cli) -> Self {
        crate::i18n::install_fonts(ctx);
        crate::i18n::set_language(None);
        crate::theme::apply(ctx);
        let views = [
            View { camera: Camera::default(), preset: ViewPreset::Perspective },
            View { camera: Camera::preset(ViewPreset::Front), preset: ViewPreset::Front },
            View { camera: Camera::preset(ViewPreset::Right), preset: ViewPreset::Right },
            View { camera: Camera::preset(ViewPreset::Top), preset: ViewPreset::Top },
        ];
        let mut app = ModelApp {
            doc: Document::new(),
            keymap: crate::keys::keymap(Default::default()),
            dispatcher: craft_keymap::egui::Dispatcher::new(),
            shortcuts_state: None,
            prefs: None,
            recent: Vec::new(),
            snap: false,
            snap_steps: (0.25, 15.0, 0.1),
            pending: None,
            close_confirmed: false,
            quit_requested: false,
            workspace: Workspace::Model,
            mode: SelectMode::Object,
            tool: Tool::Move,
            selection: Vec::new(),
            components: BTreeSet::new(),
            component_vertices_cached: Vec::new(),
            tree_focus: None,
            prop_snapshot: None,
            views,
            quad: cli.quad,
            active_view: 0,
            nav: NavPreset::AltMouse,
            brush: BrushSettings::new(BrushKind::Draw, 0.2),
            brush_px: 60.0,
            user_presets: Vec::new(),
            ops: OpParams::default(),
            layout: Layout::default(),
            status: tl!("Ready"),
            palette: None,
            numeric: None,
            rename: None,
            about: false,
            render_view: Default::default(),
            exchange: Default::default(),
            script: Default::default(),
            mesh_tools: Default::default(),
            radial: None,
            prefs_window: Default::default(),
            shelf: Vec::new(),
            show_grid: true,
            show_axes: true,
            unit: Default::default(),
            language: None,
            objects: Default::default(),
            recovery: None,
            gpu: None,
            gpu_failed: false,
            drag: None,
            stroke: None,
            show_wire: true,
            show_patches: false,
            sculpt_groups: Default::default(),
            screenshot: cli.screenshot.clone(),
            screenshot_sent: false,
            shot: Default::default(),
            frames: 0,
            frame_times: Default::default(),
            bench: None,
            focus_panel: 0,
            last_frame: None,
            autosave_at: None,
            recovery_dir: None,
            anim: Default::default(),
            agents: Default::default(),
            allow_agents: false,
            uv: Default::default(),
        };
        if let Some(port) = cli.control {
            app.agents.port = port;
            crate::agents::start(&mut app, ctx);
        }
        if let Some(n) = cli.bench_frames {
            let polys = cli.bench_polys.unwrap_or(1_000_000);
            let seg = ((polys as f32 / 6.0).sqrt().ceil() as u32).clamp(4, 1024);
            if let Some(id) = crate::demo::sculpt(&mut app.doc, seg) {
                app.select_only(id);
            }
            app.workspace = Workspace::Sculpt;
            app.bench = Some(crate::viewport::Bench::new(n));
        }
        match cli.demo {
            Demo::Model => {
                if let Some(id) = crate::demo::model(&mut app.doc) {
                    app.select_only(id);
                }
                app.views[0].camera.target = Vec3::new(0.0, 0.8, 0.0);
                app.views[0].camera.distance = 10.0;
            }
            Demo::Sculpt => {
                if let Some(id) = crate::demo::sculpt(&mut app.doc, 410) {
                    app.select_only(id);
                }
                app.workspace = Workspace::Sculpt;
                app.views[0].camera = Camera { target: Vec3::new(0.0, 1.15, 0.0), yaw: 0.45, pitch: 0.18, distance: 4.6, ..Camera::default() };
            }
            Demo::None => {}
        }
        if let Some(f) = cli.file.clone() {
            app.open_path(f);
        }
        if let Some(heat) = cli.uv_editor {
            app.uv.open = true;
            app.uv.heat = heat;
            if app.selection.is_empty()
                && let Some(id) = app.doc.scene.objects().iter().find(|o| matches!(o.kind, ObjectKind::Mesh { .. })).map(|o| o.id)
            {
                app.select_only(id);
            }
        }
        if let Some(ws) = cli.workspace {
            let ws = match ws {
                StartWorkspace::Model => Workspace::Model,
                StartWorkspace::Sculpt => Workspace::Sculpt,
                StartWorkspace::Paint => Workspace::Paint,
            };
            app.set_workspace(ws);
        }
        app.doc.dirty = false;
        app
    }

    fn apply_prefs(&mut self, p: crate::prefs::Prefs) {
        self.nav = match p.navigation.as_str() {
            "middle" => NavPreset::MiddleMouse,
            "right" => NavPreset::RightMouse,
            _ => NavPreset::AltMouse,
        };
        self.recent = p.recent.clone();
        self.snap = p.snap;
        let pos = |v: f32, d: f32| if v.is_finite() && v > 0.0 { v } else { d };
        self.snap_steps = (pos(p.snap_move, 0.25), pos(p.snap_rotate_deg, 15.0), pos(p.snap_scale, 0.1));
        self.brush_px = p.brush_px.clamp(4.0, 800.0);
        self.user_presets = p
            .brush_presets
            .iter()
            .take(modelcraft_engine::sculpt::preset::MAX_PRESETS)
            .map(|b| modelcraft_engine::sculpt::preset::BrushPreset::new(&b.name, &b.brush))
            .collect();
        self.doc.keep_history = p.keep_history;
        self.allow_agents = p.allow_agents;
        self.shelf = crate::shelf::cleaned(p.shelf.clone());
        self.show_grid = p.show_grid;
        self.show_axes = p.show_axes;
        self.show_wire = p.show_wire;
        self.show_patches = p.show_patches;
        self.unit = p.unit;
        self.language = p.language.clone().filter(|l| !l.is_empty());
        crate::i18n::set_language(self.language.as_deref());
        self.keymap = crate::keys::keymap(p.shortcuts);
    }

    /// Write changed preferences (debounced by craft-settings).
    pub fn store_prefs(&mut self) {
        let nav = match self.nav {
            NavPreset::AltMouse => "alt",
            NavPreset::MiddleMouse => "middle",
            NavPreset::RightMouse => "right",
        };
        let presets = self.user_presets.clone();
        let (recent, snap, brush_px, keep_history, allow_agents) = (self.recent.clone(), self.snap, self.brush_px, self.doc.keep_history, self.allow_agents);
        let (shelf, grid, axes, wire, patches, unit, language) =
            (self.shelf.clone(), self.show_grid, self.show_axes, self.show_wire, self.show_patches, self.unit, self.language.clone());
        let (snap_move, snap_rotate_deg, snap_scale) = self.snap_steps;
        if let Some(store) = &mut self.prefs {
            store.update(|p| {
                p.keep_history = keep_history;
                p.allow_agents = allow_agents;
                p.shelf = shelf;
                p.show_grid = grid;
                p.show_axes = axes;
                p.show_wire = wire;
                p.show_patches = patches;
                p.unit = unit;
                p.language = language;
                p.snap_move = snap_move;
                p.snap_rotate_deg = snap_rotate_deg;
                p.snap_scale = snap_scale;
                p.navigation = nav.into();
                p.recent = recent;
                p.snap = snap;
                p.brush_px = brush_px;
                p.brush_presets = presets;
            });
            let _ = store.tick(std::time::Instant::now());
        }
    }

    pub fn touch_recent(&mut self, path: &std::path::Path) {
        let mut p = crate::prefs::Prefs { recent: std::mem::take(&mut self.recent), ..Default::default() };
        p.touch_recent(path);
        self.recent = p.recent;
    }

    /// Run `p` now, or ask to save first when there are unsaved changes.
    pub fn guard(&mut self, p: Pending) {
        if self.doc.dirty && !self.doc.scene.is_empty() {
            self.pending = Some(p);
        } else {
            self.run_pending(p);
        }
    }

    pub fn run_pending(&mut self, p: Pending) {
        match p {
            Pending::New => {
                let keep = self.doc.keep_history;
                self.doc = Document::new();
                self.doc.keep_history = keep;
                self.selection.clear();
                self.components.clear();
                crate::script_ui::scene_loaded(self);
                self.start_autosave();
                self.status = tl!("New scene");
            }
            Pending::Open => self.open_dialog(),
            Pending::OpenPath(path) => self.open_path(path),
            Pending::Quit => {
                self.close_confirmed = true;
                self.quit_requested = true;
            }
        }
    }

    /// Answer to the save prompt: `Some(true)` save first, `Some(false)` discard, `None` cancel.
    pub fn answer_pending(&mut self, save: Option<bool>) {
        let Some(p) = self.pending.take() else { return };
        match save {
            None => {}
            Some(true) => {
                self.save();
                if !self.doc.dirty {
                    self.run_pending(p);
                }
            }
            Some(false) => {
                self.doc.dirty = false;
                self.run_pending(p);
            }
        }
    }

    /// Save a numbered copy next to the file (`name_v002.modelcraft`) and continue in it.
    pub fn save_incremental(&mut self) {
        let Some(path) = self.doc.path.clone() else {
            self.save_as();
            return;
        };
        let next = next_increment(&path);
        match self.doc.save_as(&next) {
            Ok(()) => {
                self.touch_recent(&next);
                self.status = crate::tlf!("Saved {file}", file = next.display());
            }
            Err(e) => self.status = crate::tlf!("Save failed: {error}", error = e),
        }
    }

    // ---- selection helpers ----

    pub fn active(&self) -> Option<ObjectId> {
        self.selection.last().copied().filter(|id| self.doc.scene.get(*id).is_some())
    }

    pub fn select_only(&mut self, id: ObjectId) {
        self.selection = vec![id];
        self.components.clear();
        if self.workspace == Workspace::Sculpt {
            self.begin_sculpt();
        }
    }

    pub fn active_mesh(&self) -> Option<ObjectId> {
        self.active().filter(|id| matches!(self.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })))
    }

    /// The object whose components are edited: the active mesh, or a smooth cage's input.
    pub fn edit_target(&self) -> Option<ObjectId> {
        let a = self.active()?;
        match &self.doc.scene.get(a)?.kind {
            ObjectKind::Mesh { .. } => Some(a),
            ObjectKind::SmoothCage { .. } | ObjectKind::Cloner { .. } => {
                self.doc.scene.children(a).into_iter().find(|c| matches!(self.doc.scene.get(*c).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })))
            }
            _ => None,
        }
    }

    pub fn component_count(&mut self) -> usize {
        let Some(id) = self.edit_target() else { return 0 };
        let m = self.doc.eval.steps_output(&self.doc.scene, id, usize::MAX).mesh;
        match self.mode {
            SelectMode::Object => 0,
            SelectMode::Vertex => m.vertex_count(),
            SelectMode::Edge => m.edge_count(),
            SelectMode::Face => m.face_count(),
        }
    }

    // ---- running commands ----

    pub fn run(&mut self, cmd: Command) -> bool {
        let label = cmd.label();
        match self.doc.execute(cmd) {
            Ok(out) => {
                if let Some(first) = out.created.first() {
                    self.selection = vec![*first];
                    self.components.clear();
                }
                self.status = match out.warnings.first() {
                    Some(w) => crate::tlf!("{action}: {warning}", action = label, warning = w),
                    None => label,
                };
                self.prune_selection();
                true
            }
            Err(e) => {
                self.status = crate::tlf!("{action} failed: {error}", action = label, error = e);
                false
            }
        }
    }

    pub fn prune_selection(&mut self) {
        let scene = &self.doc.scene;
        self.selection.retain(|id| scene.get(*id).is_some());
        let n = self.component_count();
        self.components.retain(|c| (*c as usize) < n);
    }

    pub fn set_workspace(&mut self, ws: Workspace) {
        if ws == self.workspace {
            return;
        }
        if self.workspace == Workspace::Sculpt {
            let _ = self.doc.execute(Command::EndSculpt);
        }
        self.workspace = ws;
        if ws == Workspace::Sculpt {
            self.mode = SelectMode::Object;
            self.components.clear();
            self.begin_sculpt();
        }
        self.status = crate::tlf!("{workspace} workspace", workspace = ws.label());
    }

    pub fn begin_sculpt(&mut self) {
        let Some(id) = self.edit_target() else {
            self.status = tl!("Select a mesh to sculpt");
            return;
        };
        if self.doc.sculpt.as_ref().is_some_and(|s| s.id == id) {
            return;
        }
        if self.doc.ensure_session(id).is_err() {
            self.run(Command::BeginSculpt { id });
            self.selection = vec![id];
        }
    }

    pub fn add_step(&mut self, op: StepOp) {
        let Some(id) = self.edit_target() else {
            self.status = tl!("Select a mesh first");
            return;
        };
        if self.run(Command::AddStep { id, op }) {
            self.selection = vec![id];
            self.components.clear();
        }
    }

    fn selected_vec(&self) -> Vec<u32> {
        self.components.iter().copied().collect()
    }

    pub fn act(&mut self, ctx: &egui::Context, a: Action) {
        use Action::*;
        match a {
            New => self.guard(Pending::New),
            Open => self.guard(Pending::Open),
            SaveIncremental => self.save_incremental(),
            KeyboardShortcuts => self.shortcuts_state = Some(craft_keymap::ui::DialogState::new()),
            ToggleSnap => {
                self.snap = !self.snap;
                self.status = if self.snap { tl!("Snapping on") } else { tl!("Snapping off") };
            }
            SelectLoop | SelectRing | GrowSelection | ShrinkSelection => self.selection_op(a),
            Bridge => {
                let faces = self.selected_vec();
                match (self.mode, faces.as_slice()) {
                    (SelectMode::Face, [a, b]) => self.add_step(StepOp::Bridge { a: *a, b: *b }),
                    _ => self.status = tl!("Bridge needs exactly two selected faces (face mode)"),
                }
            }
            FillHole => match (self.mode, self.components.iter().next()) {
                (SelectMode::Edge, Some(&edge)) => self.add_step(StepOp::FillHole { edge }),
                _ => self.status = tl!("Fill hole needs a selected border edge (edge mode)"),
            },
            Mirror(axis) => self.add_step(StepOp::Mirror { axis: u32::from(axis), weld: 1e-4 }),
            Save => self.save(),
            SaveAs => self.save_as(),
            Import => self.import_dialog(),
            ExportGlb => self.export_dialog("glb"),
            ExportObj => self.export_dialog("obj"),
            ExportStl => self.export_dialog("stl"),
            ExportFbx => self.export_dialog("fbx"),
            Undo => {
                self.drag = None;
                self.run(Command::Undo);
                self.status = tl!("Undo");
            }
            Redo => {
                self.drag = None;
                self.run(Command::Redo);
                self.status = tl!("Redo");
            }
            Anim(a) => crate::anim_ui::act(self, a),
            Rig(a) => crate::rig_ui::act(self, a),
            Sculpt(a) => crate::sculpt_groups_ui::act(self, ctx, a),
            Uv(a) => crate::uv_ui::act(self, a),
            Delete => {
                if self.mode != SelectMode::Object && !self.components.is_empty() {
                    let sel = self.selected_vec();
                    let op = match self.mode {
                        SelectMode::Vertex => StepOp::DeleteVertices { verts: sel },
                        SelectMode::Edge => StepOp::DeleteEdges { edges: sel },
                        _ => StepOp::DeleteFaces { faces: sel },
                    };
                    self.add_step(op);
                } else if !self.selection.is_empty() {
                    // A refused delete (a locked object) keeps the selection.
                    let ids = self.selection.clone();
                    if self.run(Command::Delete { ids }) {
                        self.selection.clear();
                    }
                }
            }
            Duplicate => {
                if let Some(id) = self.active() {
                    self.run(Command::Duplicate { id });
                }
            }
            Rename => {
                if let Some(id) = self.active() {
                    let name = self.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
                    self.rename = Some((id, name));
                }
            }
            Frame => self.frame_selection(),
            ToggleQuad => self.quad = !self.quad,
            Workspace(ws) => self.set_workspace(ws),
            ModeObject => self.set_mode(SelectMode::Object),
            ModeVertex => self.set_mode(SelectMode::Vertex),
            ModeEdge => self.set_mode(SelectMode::Edge),
            ModeFace => self.set_mode(SelectMode::Face),
            ToolSelect => self.tool = Tool::Select,
            ToolMove => self.tool = Tool::Move,
            ToolRotate => self.tool = Tool::Rotate,
            ToolScale => self.tool = Tool::Scale,
            AddPrimitive(kind) => {
                self.run(Command::AddPrimitive { kind: kind.into(), name: None, parent: None });
            }
            AddCage => {
                let input = self.active_mesh();
                self.run(Command::AddSmoothCage { input, levels: 2 });
            }
            AddDeformer(kind) => match self.active_mesh() {
                Some(target) => {
                    self.run(Command::AddDeformer { kind: kind.into(), target });
                }
                None => self.status = tl!("Select a mesh to deform"),
            },
            AddCloner(kind) => {
                // The selected mesh (or smooth cage) becomes the source; with none, a small cube is made.
                let input = self
                    .active()
                    .filter(|id| matches!(self.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. })));
                self.run(Command::AddCloner { kind: kind.into(), input });
            }
            AddGroup => {
                self.run(Command::AddGroup { name: None });
            }
            Extrude => self.face_op(|f, p| StepOp::Extrude { faces: f, distance: p.extrude }),
            Inset => self.face_op(|f, p| StepOp::Inset { faces: f, amount: p.inset }),
            Bevel => {
                if self.mode == SelectMode::Edge && !self.components.is_empty() {
                    let w = self.ops.bevel;
                    self.add_step(StepOp::Bevel { edges: self.selected_vec(), width: w });
                } else {
                    self.status = tl!("Bevel needs selected edges (edge mode)");
                }
            }
            LoopCut => {
                if self.mode == SelectMode::Edge
                    && let Some(&edge) = self.components.iter().next()
                {
                    let cuts = self.ops.cuts;
                    self.add_step(StepOp::LoopCut { edge, cuts });
                } else {
                    self.status = tl!("Loop cut needs a selected edge (edge mode)");
                }
            }
            Merge => {
                if self.mode == SelectMode::Vertex && self.components.len() > 1 {
                    self.add_step(StepOp::Merge { verts: self.selected_vec() });
                } else {
                    self.status = tl!("Merge needs two or more selected vertices");
                }
            }
            Subdivide => {
                let levels = self.ops.subdivide;
                self.add_step(StepOp::Subdivide { levels });
            }
            Triangulate => self.add_step(StepOp::Triangulate),
            Flip => {
                let faces = if self.mode == SelectMode::Face { self.selected_vec() } else { Vec::new() };
                self.add_step(StepOp::Flip { faces });
            }
            Collapse => {
                if let Some(id) = self.edit_target() {
                    self.run(Command::Collapse { id });
                    self.components.clear();
                }
            }
            SelectAll => {
                if self.mode == SelectMode::Object {
                    self.selection = self.doc.scene.objects().iter().map(|o| o.id).collect();
                } else {
                    let n = self.component_count() as u32;
                    self.components = (0..n).collect();
                }
            }
            SelectNone => {
                if self.palette.is_some() || self.numeric.is_some() || self.rename.is_some() {
                    self.palette = None;
                    self.numeric = None;
                    self.rename = None;
                } else if !self.components.is_empty() {
                    self.components.clear();
                } else {
                    self.selection.clear();
                }
            }
            Brush(k) => {
                let keep = self.brush.clone();
                self.brush = BrushSettings::new(k, keep.radius);
                (self.brush.symmetry, self.brush.falloff, self.brush.live_detail) = (keep.symmetry, keep.falloff, keep.live_detail);
                (self.brush.hardness, self.brush.stroke, self.brush.spacing, self.brush.stamp) = (keep.hardness, keep.stroke, keep.spacing, keep.stamp);
                self.status = crate::tlf!("{brush} brush", brush = crate::actions::brush_label(k));
            }
            Symmetry => {
                self.brush.symmetry.x = !self.brush.symmetry.x;
                self.status = if self.brush.symmetry.x { tl!("X symmetry on") } else { tl!("X symmetry off") };
            }
            MaskClear => self.mask(modelcraft_engine::MaskOp::Clear),
            MaskInvert => self.mask(modelcraft_engine::MaskOp::Invert),
            MaskFill => self.mask(modelcraft_engine::MaskOp::Fill),
            MaskOp(op) => self.mask(op),
            BrushBigger => self.brush_px = (self.brush_px * 1.15).min(800.0),
            BrushSmaller => self.brush_px = (self.brush_px / 1.15).max(4.0),
            DetailUp => self.sculpt_layer_command(|id| Command::SculptDetail { id, op: modelcraft_engine::DetailOp::Up }),
            DetailDown => self.sculpt_layer_command(|id| Command::SculptDetail { id, op: modelcraft_engine::DetailOp::Down }),
            LiveDetail => crate::sculpt_ui::toggle_live_detail(self),
            EvenRemesh => crate::sculpt_ui::even_remesh(self),
            ShapeAdd => self.sculpt_layer_command(|id| Command::SculptShape { id, op: modelcraft_engine::ShapeOp::Add { name: None } }),
            ViewPerspective => self.set_view(ViewPreset::Perspective),
            ViewFront => self.set_view(ViewPreset::Front),
            ViewRight => self.set_view(ViewPreset::Right),
            ViewTop => self.set_view(ViewPreset::Top),
            TransformDialog => self.numeric = Some(NumericTransform::default()),
            Palette => self.palette = Some(crate::panels::Palette::default()),
            NextPanel => {
                // Next panel that is shown in this workspace.
                let shown: Vec<usize> =
                    PanelId::ALL.iter().enumerate().filter(|(_, p)| self.layout.dock(self.workspace, **p) != Dock::Hidden).map(|(i, _)| i).collect();
                if !shown.is_empty() {
                    self.focus_panel = self.focus_panel.wrapping_add(1);
                    let idx = shown.get(self.focus_panel % shown.len()).copied().unwrap_or(0);
                    ctx.memory_mut(|m| m.request_focus(egui::Id::unique(("panel-focus", idx))));
                }
            }
            About => self.about = true,
            RenderView => crate::render_view::toggle(self, ctx),
            ScriptWindow => crate::script_ui::toggle(self),
            ScriptRun => crate::script_ui::run_editor(self, false),
            ScriptRunSelection => crate::script_ui::run_editor(self, true),
            ScriptRecord => crate::script_macro::toggle(self),
            MeshTool(t) => crate::mesh_tools::run(self, t),
            RadialMenu => crate::radial::open(self, ctx),
            Preferences => crate::prefs_ui::open(self),
            Objects(op) => crate::object_manager::run(self, op),
        }
    }

    fn selection_op(&mut self, a: Action) {
        use modelcraft_engine::ops::select;
        let Some(id) = self.edit_target() else { return };
        let m = self.doc.eval.steps_output(&self.doc.scene, id, usize::MAX).mesh;
        let kind = match self.mode {
            SelectMode::Vertex => select::Kind::Vertex,
            SelectMode::Edge => select::Kind::Edge,
            SelectMode::Face => select::Kind::Face,
            SelectMode::Object => {
                self.status = tl!("Pick a component mode first");
                return;
            }
        };
        let sel = self.selected_vec();
        let new: BTreeSet<u32> = match a {
            Action::SelectLoop | Action::SelectRing if kind != select::Kind::Edge => {
                self.status = tl!("Loops and rings work in edge mode");
                return;
            }
            Action::SelectLoop => select::loops(&m, &sel).into_iter().collect(),
            Action::SelectRing => select::rings(&m, &sel).into_iter().collect(),
            Action::GrowSelection => select::grow(&m, kind, &self.components),
            _ => select::shrink(&m, kind, &self.components),
        };
        self.components = new;
        self.status = crate::tlf!("{count} components selected", count = self.components.len());
    }

    fn sculpt_layer_command(&mut self, cmd: impl FnOnce(ObjectId) -> Command) {
        if let Some(id) = self.doc.sculpt.as_ref().map(|s| s.id) {
            self.run(cmd(id));
        } else {
            self.status = tl!("Detail levels and shape layers work in the Sculpt workspace");
        }
    }

    fn mask(&mut self, op: modelcraft_engine::MaskOp) {
        if let Some(id) = self.doc.sculpt.as_ref().map(|s| s.id) {
            self.run(Command::SculptMask { id, op });
        } else {
            self.status = tl!("Masks work in the Sculpt workspace");
        }
    }

    fn face_op(&mut self, f: impl Fn(Vec<u32>, &OpParams) -> StepOp) {
        if self.mode == SelectMode::Face && !self.components.is_empty() {
            let op = f(self.selected_vec(), &self.ops);
            self.add_step(op);
        } else {
            self.status = tl!("Select faces first (face mode)");
        }
    }

    pub fn set_mode(&mut self, m: SelectMode) {
        if self.workspace != Workspace::Model {
            self.set_workspace(Workspace::Model);
        }
        if m != SelectMode::Object && self.edit_target().is_none() {
            self.status = tl!("Select a mesh to edit its components");
            return;
        }
        self.mode = m;
        self.components.clear();
    }

    fn set_view(&mut self, p: ViewPreset) {
        let v = &mut self.views[self.active_view.min(3)];
        let (target, distance) = (v.camera.target, v.camera.distance);
        v.camera = Camera::preset(p);
        v.camera.target = target;
        v.camera.distance = distance;
        v.preset = p;
    }

    pub fn frame_selection(&mut self) {
        let ids: Vec<ObjectId> = if self.selection.is_empty() { self.doc.scene.objects().iter().map(|o| o.id).collect() } else { self.selection.clone() };
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for id in ids {
            let m = self.doc.eval.evaluate(&self.doc.scene, id).mesh;
            if m.vertex_count() == 0 {
                continue;
            }
            let w = self.doc.scene.world_matrix(id);
            let (a, b) = m.bounds();
            for c in [a, b, Vec3::new(a.x, b.y, a.z), Vec3::new(b.x, a.y, b.z)] {
                let p = w.transform_point3(c);
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        if lo.x <= hi.x {
            let i = self.active_view.min(3);
            self.views[i].camera.frame(lo, hi);
        }
    }

    /// Apply the numeric transform: to the selected components as a history step, or to the
    /// active object's transform.
    pub fn apply_numeric(&mut self, n: NumericTransform) {
        let r = n.rotate_deg * (std::f32::consts::PI / 180.0);
        let rot = Quat::from_euler(glam::EulerRot::XYZ, r.x, r.y, r.z);
        if self.mode != SelectMode::Object && !self.components.is_empty() {
            let verts = self.component_vertices();
            self.add_step(StepOp::MoveVertices { verts, translate: n.translate, rotate: rot, scale: n.scale });
        } else if let Some(id) = self.active() {
            let Some(mut t) = self.doc.scene.get(id).map(|o| o.transform) else { return };
            t.translation += n.translate;
            t.rotation = rot * t.rotation;
            t.scale *= n.scale;
            self.run(Command::SetTransform { id, transform: t });
        }
    }

    /// The vertices of the selected components.
    pub fn component_vertices(&mut self) -> Vec<u32> {
        let Some(id) = self.edit_target() else { return Vec::new() };
        let m = self.doc.eval.steps_output(&self.doc.scene, id, usize::MAX).mesh;
        let mut out = BTreeSet::new();
        for c in &self.components {
            match self.mode {
                SelectMode::Vertex => {
                    out.insert(*c);
                }
                SelectMode::Edge => {
                    let (a, b) = m.edge_vertices(*c);
                    out.insert(a);
                    out.insert(b);
                }
                SelectMode::Face => out.extend(m.face_vertices(*c)),
                SelectMode::Object => {}
            }
        }
        out.into_iter().filter(|v| (*v as usize) < m.vertex_count()).collect()
    }

    // ---- files ----

    pub fn open_path(&mut self, path: PathBuf) {
        self.touch_recent(&path);
        let is_project = path.extension().is_some_and(|e| e == modelcraft_engine::formats::project::EXTENSION);
        if is_project {
            match Document::open(&path) {
                Ok(mut doc) => {
                    let pending = doc.pending_recovery;
                    doc.keep_history = self.doc.keep_history;
                    let note = doc.history_note.take();
                    self.doc = doc;
                    self.selection.clear();
                    self.components.clear();
                    crate::script_ui::scene_loaded(self);
                    self.status = match note {
                        Some(n) => crate::tlf!("Opened {file}. {note}", file = path.display(), note = history_note(&n)),
                        None => crate::tlf!("Opened {file}", file = path.display()),
                    };
                    if pending > 0 {
                        self.recovery = Some(path);
                    }
                    self.frame_selection();
                }
                Err(e) => self.status = crate::tlf!("Could not open {file}: {error}", file = path.display(), error = e),
            }
        } else {
            self.run(Command::Import { path: path.display().to_string(), up: self.exchange.up, scale: self.exchange.scale });
            crate::script_ui::scene_loaded(self);
            self.frame_selection();
        }
    }

    pub fn open_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new()
            .add_filter("ModelCraft", &["modelcraft"])
            .add_filter(tl!("3D files"), &["glb", "gltf", "obj", "stl", "ply", "fbx", "3mf"])
            .pick_file()
        {
            self.open_path(p);
        }
    }

    fn import_dialog(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter(tl!("3D files"), &["glb", "gltf", "obj", "stl", "ply", "fbx", "3mf"]).pick_file() {
            self.run(Command::Import { path: p.display().to_string(), up: self.exchange.up, scale: self.exchange.scale });
        }
    }

    fn export_dialog(&mut self, ext: &str) {
        if let Some(p) = rfd::FileDialog::new().add_filter(ext.to_uppercase(), &[ext]).set_file_name(format!("model.{ext}")).save_file() {
            match self.doc.export_with(&p, modelcraft_engine::formats::exchange::Convert { up: self.exchange.up, scale: self.exchange.scale }) {
                Ok(()) => self.status = crate::tlf!("Exported {file}", file = p.display()),
                Err(e) => self.status = crate::tlf!("Export failed: {error}", error = e),
            }
        }
    }

    pub fn save(&mut self) {
        if self.doc.path.is_none() {
            self.save_as();
            return;
        }
        match self.doc.save() {
            Ok(()) => {
                if let Some(p) = self.doc.path.clone() {
                    self.touch_recent(&p);
                }
                self.status = tl!("Saved");
            }
            Err(e) => self.status = crate::tlf!("Save failed: {error}", error = e),
        }
    }

    pub fn save_as(&mut self) {
        if let Some(p) = rfd::FileDialog::new().add_filter("ModelCraft", &["modelcraft"]).set_file_name("scene.modelcraft").save_file() {
            match self.doc.save_as(&p) {
                Ok(()) => {
                    self.touch_recent(&p);
                    self.status = crate::tlf!("Saved {file}", file = p.display());
                }
                Err(e) => self.status = crate::tlf!("Save failed: {error}", error = e),
            }
        }
    }

    /// Untitled documents journal into the recovery folder from the start.
    fn start_autosave(&mut self) {
        let Some(dir) = self.recovery_dir.clone() else { return };
        if self.doc.path.is_some() {
            return;
        }
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let snap = dir.join("untitled.modelcraft");
        let journal = modelcraft_engine::formats::journal::path_for(&snap);
        // An earlier session left work behind: offer it before starting over.
        if journal.is_file()
            && std::fs::metadata(&journal).map(|m| m.len() > 0).unwrap_or(false)
            && self.recovery.is_none()
            && self.screenshot.is_none()
            && self.bench.is_none()
        {
            self.recovery = Some(snap);
            return;
        }
        let _ = self.doc.checkpoint(&snap);
        self.autosave_at = Some(std::time::Instant::now());
    }

    /// Every few minutes: an untitled document is snapshotted (and its journal restarted);
    /// every document's journal is forced to disk.
    pub fn autosave_tick(&mut self) {
        let due = self.autosave_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(180));
        if !due || self.stroke.is_some() || self.drag.is_some() {
            return;
        }
        self.autosave_at = Some(std::time::Instant::now());
        self.doc.sync_journal();
        if self.doc.path.is_none()
            && self.doc.dirty
            && let Some(dir) = self.recovery_dir.clone()
        {
            let _ = self.doc.checkpoint(&dir.join("untitled.modelcraft"));
        }
    }

    pub fn recover(&mut self, accept: bool) {
        let Some(p) = self.recovery.take() else { return };
        let journal = modelcraft_engine::formats::journal::path_for(&p);
        if self.doc.path.as_deref() == Some(p.as_path()) {
            if accept {
                match self.doc.recover() {
                    Ok(n) => self.status = crate::tlf!("Recovered {count} changes", count = n),
                    Err(e) => self.status = crate::tlf!("Recovery failed: {error}", error = e),
                }
            } else {
                self.doc.discard_recovery();
            }
            return;
        }
        // Untitled recovery: the snapshot plus its journal.
        if accept {
            let mut doc = if p.is_file() { Document::open(&p).unwrap_or_default() } else { Document::new() };
            let n = doc.recover_from(&journal).unwrap_or(0);
            doc.path = None;
            self.doc = doc;
            self.status = crate::tlf!("Recovered {count} changes", count = n);
            self.frame_selection();
        }
        let _ = std::fs::remove_file(&journal);
        self.start_autosave();
    }

    /// Window title: file name, unsaved marker.
    pub fn title(&self) -> String {
        let name = self.doc.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| tl!("Untitled"));
        format!("{}{} — ModelCraft", if self.doc.dirty { "• " } else { "" }, name)
    }
}

impl eframe::App for ModelApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.gpu.is_none() && !self.gpu_failed {
            match frame.wgpu_render_state() {
                Some(rs) => self.gpu = Some(crate::gpu::GpuState::new(rs.clone())),
                None => self.gpu_failed = true,
            }
        }
        self.autosave_tick();
        self.store_prefs();
        // Closing with unsaved changes asks first (screenshots and benchmarks close on their own).
        if ctx.input(|i| i.viewport().close_requested())
            && self.doc.dirty
            && !self.doc.scene.is_empty()
            && !self.close_confirmed
            && self.screenshot.is_none()
            && self.bench.is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }
        if self.quit_requested {
            self.quit_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        self.store_prefs();
        if let Some(store) = &mut self.prefs {
            let _ = store.flush();
        }
        if let Some(dir) = &self.recovery_dir
            && !self.doc.dirty
        {
            let _ = std::fs::remove_file(modelcraft_engine::formats::journal::path_for(&dir.join("untitled.modelcraft")));
        }
    }
}

impl ModelApp {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let now = std::time::Instant::now();
        if let Some(t) = self.last_frame {
            self.frame_times.push_back(now.duration_since(t).as_secs_f32());
            if self.frame_times.len() > 120 {
                self.frame_times.pop_front();
            }
        }
        self.last_frame = Some(now);
        self.frames += 1;
        crate::agents::tick(self);
        self.shortcuts(&ctx);
        crate::constraint_ui::tick(self);
        crate::script_ui::tick(self);
        crate::panels::menu_bar(self, ui);
        crate::panels::toolbar(self, ui);
        crate::panels::tool_options(self, ui);
        crate::script_ui::quick_strip(self, ui);
        crate::shelf::bar(self, ui);
        crate::panels::status_bar(self, ui);
        crate::anim_ui::bottom_panel(self, ui);
        for side in [Dock::Left, Dock::Right] {
            let panels: Vec<PanelId> = PanelId::ALL.iter().copied().filter(|p| self.layout.dock(self.workspace, *p) == side).collect();
            if panels.is_empty() {
                continue;
            }
            let id = if side == Dock::Left { "dock-left" } else { "dock-right" };
            let panel = if side == Dock::Left { egui::Panel::left(id) } else { egui::Panel::right(id) };
            panel.resizable(true).default_size(if side == Dock::Left { 240.0 } else { 300.0 }).min_size(180.0).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for p in panels {
                        crate::panels::dock_panel(self, ui, p);
                    }
                });
            });
        }
        for p in PanelId::ALL {
            if self.layout.dock(self.workspace, p) == Dock::Float {
                let mut open = true;
                egui::Window::new(p.label())
                    .id(egui::Id::unique(("float", p as u8)))
                    .open(&mut open)
                    .default_width(280.0)
                    .show(&ctx, |ui| crate::panels::panel_body(self, ui, p));
                if !open {
                    self.layout.set(self.workspace, p, Dock::Hidden);
                }
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(crate::theme::PANEL_DEEP)).show(ui, |ui| crate::viewport::show(self, ui));
        crate::panels::dialogs(self, &ctx);
        crate::prefs_ui::show(self, &ctx);
        crate::radial::show(self, &ctx);
        self.screenshot_tick(&ctx);
        if self.stroke.is_some() || self.drag.is_some() || self.bench.is_some() || self.anim.playback.playing {
            ctx.request_repaint();
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if crate::render_view::escape(self, ctx) || self.radial.is_some() || crate::script_ui::escape(self, ctx) || crate::script_ui::keys(self, ctx) {
            return;
        }
        if crate::uv_ui::keys(self, ctx) {
            return;
        }
        if crate::sculpt_groups_ui::escape(self, ctx) {
            return;
        }
        if self.palette.is_some() || self.numeric.is_some() || self.rename.is_some() || self.pending.is_some() || self.shortcuts_state.is_some() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                self.palette = None;
                self.numeric = None;
                self.rename = None;
                self.pending = None;
            }
            return;
        }
        // Preferences answer their own keys (Esc closes them).
        if self.prefs_window.open {
            return;
        }
        let redo_y = !ctx.text_edit_focused() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y)));
        for a in crate::keys::dispatch(self, ctx) {
            self.act(ctx, a);
        }
        if redo_y {
            self.act(ctx, Action::Redo);
        }
    }

    fn screenshot_tick(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint();
        // Let a few frames settle (GPU meshes upload, layout stabilises), then ask the
        // integration for the next rendered frame.
        if self.frames < 12 {
            return;
        }
        if !self.screenshot_sent {
            self.screenshot_sent = true;
            let slot = self.shot.clone();
            ctx.request_screenshot(move |img| {
                if let Ok(mut s) = slot.lock() {
                    *s = Some(img);
                }
            });
            return;
        }
        let img = self.shot.lock().ok().and_then(|mut s| s.take());
        if let Some(img) = img {
            if let Err(e) = crate::viewport::save_png(&path, &img) {
                log::error!("screenshot: {e}");
            }
            self.screenshot = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// `scene.modelcraft` → `scene_v002.modelcraft`; `scene_v002` → `scene_v003` (skipping files that exist).
pub fn next_increment(path: &std::path::Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("modelcraft");
    let (base, n) = match stem.rsplit_once("_v") {
        Some((b, d)) if !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()) => (b.to_string(), d.parse::<u32>().unwrap_or(1)),
        _ => (stem.to_string(), 1),
    };
    let dir = path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let mut k = n.saturating_add(1);
    loop {
        let candidate = dir.join(format!("{base}_v{k:03}.{ext}"));
        if !candidate.exists() || k == u32::MAX {
            return candidate;
        }
        k = k.saturating_add(1);
    }
}
