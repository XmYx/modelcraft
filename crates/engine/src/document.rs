//! The document and command execution.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use modelcraft_formats::exchange::{self, Item, Kind};
use modelcraft_formats::{FormatError, journal, project, uvlayout};
use modelcraft_ops::deform::Deformer;
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{Cloner, Evaluator, ObjectId, ObjectKind, Scene, SceneError, SculptLayer, Source, Step, StepOp};
use modelcraft_sculpt::{BrushSettings, Dab, SculptMesh};

use crate::command::{Command, MAX_BATCH, MAX_BATCH_DEPTH, MaskOp, Outcome};
use crate::undo::{UndoEntry, UndoStack};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Scene(#[from] SceneError),
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error("{0}")]
    Invalid(String),
}

pub(crate) fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

/// Largest file the importer reads.
pub const MAX_IMPORT_BYTES: u64 = 2 << 30;

/// An object being sculpted: its sculpt layer lives here until synced back into the scene.
pub struct SculptSession {
    pub id: ObjectId,
    pub step_id: u64,
    pub mesh: SculptMesh,
    /// The layer already carried its own triangles (live detail) when the session began.
    had_topology: bool,
    /// The mesh's triangle revision last written to the layer.
    topo_synced: u64,
    /// The scene's layer is older than `mesh`.
    pub unsynced: bool,
}

/// Where an open undo group started.
struct GroupStart {
    label: String,
    /// `undo.len() + undo.dropped` when the group began, so entries dropped for memory do not
    /// shift the count.
    mark: usize,
}

pub struct Document {
    pub scene: Scene,
    pub eval: Evaluator,
    pub undo: UndoStack,
    pub path: Option<PathBuf>,
    /// Changed since the last save.
    pub dirty: bool,
    pub sculpt: Option<SculptSession>,
    journal: Option<journal::Journal>,
    replaying: bool,
    /// Open undo groups ([`Document::begin_group`]), innermost last.
    groups: Vec<GroupStart>,
    /// Journal entries found next to an opened file, waiting for [`Document::recover`].
    pub pending_recovery: usize,
    /// The frame the journal last recorded (scrubbing is journaled lazily, see `execute`).
    journal_frame: f64,
    /// Store the undo history in saved files, so the next session can undo past the open.
    pub keep_history: bool,
    /// Largest history (in command bytes) a save stores; older steps are left out.
    pub history_max_bytes: usize,
    /// What happened to the stored history on open, for the status bar.
    pub history_note: Option<String>,
    /// Counts the changes made in this session (control clients watch it).
    pub revision: u64,
}

/// Default bound on the history a save stores: 256 MiB of commands.
pub const DEFAULT_HISTORY_MAX_BYTES: usize = 256 << 20;

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Document {
            scene: Scene::new(),
            eval: Evaluator::new(),
            undo: UndoStack::default(),
            path: None,
            dirty: false,
            sculpt: None,
            journal: None,
            replaying: false,
            groups: Vec::new(),
            pending_recovery: 0,
            journal_frame: 0.0,
            keep_history: true,
            history_max_bytes: DEFAULT_HISTORY_MAX_BYTES,
            history_note: None,
            revision: 0,
        }
    }

    pub fn from_scene(scene: Scene) -> Self {
        let journal_frame = scene.anim.frame;
        let mut d = Document { scene, journal_frame, ..Self::new() };
        d.undo.set_base(d.scene.clone());
        d
    }

    /// Go to a frame and pose the scene there. Not an undo step and not a change to save; the
    /// journal records the frame with the next command that depends on it.
    pub fn set_frame(&mut self, frame: f64) {
        self.scene.set_frame(frame);
    }

    /// Open a project. If a journal of unsaved work lies next to it, `pending_recovery` says how
    /// many commands it holds; call [`Document::recover`] to replay them or
    /// [`Document::discard_recovery`] to drop them.
    ///
    /// When the file stores an undo history, it is rebuilt by replaying its commands on the
    /// stored base scene; if the replay does not reproduce the saved scene exactly, the history is
    /// dropped, the saved scene opens as it is, and `history_note` says why.
    pub fn open(path: &Path) -> Result<Document, EngineError> {
        let (scene, history) = project::open_with_history::<Command>(path)?;
        let mut d = match history {
            project::HistoryRead::None => Document::from_scene(scene),
            project::HistoryRead::Dropped(why) => {
                let mut d = Document::from_scene(scene);
                d.history_note = Some(format!("The undo history in the file could not be read ({why}); it starts here."));
                d
            }
            project::HistoryRead::Found(h) => {
                let steps = h.done.len();
                match Document::rebuilt(h, &scene) {
                    Ok(d) => {
                        let mut d = d;
                        d.history_note = (steps > 0).then(|| format!("Restored {steps} undo steps from the last session."));
                        d
                    }
                    Err(why) => {
                        let mut d = Document::from_scene(scene);
                        d.history_note = Some(format!("The undo history in the file was left out ({why}); it starts here."));
                        d
                    }
                }
            }
        };
        d.path = Some(path.to_path_buf());
        let jp = journal::path_for(path);
        if jp.is_file() {
            d.pending_recovery = journal::read::<Command>(&jp).map(|v| v.len()).unwrap_or(0);
        }
        if d.pending_recovery == 0 {
            d.journal = journal::Journal::create(&jp).ok();
        }
        Ok(d)
    }

    /// Rebuild a document with its undo history: replay `done` and `undone` on the base, undo
    /// the `undone` part, and check that the result is the `saved` scene.
    fn rebuilt(h: project::History<Command>, saved: &Scene) -> Result<Document, String> {
        let mut d = Document::from_scene(h.base);
        d.replaying = true;
        let redo = h.undone.len();
        for (n, c) in h.done.into_iter().chain(h.undone).enumerate() {
            if matches!(c, Command::Undo | Command::Redo | Command::EndSculpt) {
                return Err(format!("step {} is not an edit", n + 1));
            }
            let label = c.label();
            d.execute(c).map_err(|e| format!("step {} ({label}) failed: {e}", n + 1))?;
        }
        for _ in 0..redo {
            d.undo_once();
        }
        d.sync_sculpt();
        d.sculpt = None;
        d.replaying = false;
        d.dirty = false;
        if !same_content(&d.scene, saved) {
            return Err("replaying it does not give the saved scene".into());
        }
        // The saved scene is the truth; keep it (revisions included) with the rebuilt stack.
        d.scene = saved.clone();
        d.eval = Evaluator::new();
        Ok(d)
    }

    /// Replay the journal found by [`Document::open`] (or at `journal_path`).
    pub fn recover_from(&mut self, journal_path: &Path) -> Result<usize, EngineError> {
        let cmds: Vec<Command> = journal::read(journal_path)?;
        self.replaying = true;
        let mut n = 0;
        for c in &cmds {
            // A command that fails during replay failed the same way originally.
            if self.execute(c.clone()).is_ok() {
                n += 1;
            }
        }
        self.replaying = false;
        self.journal = journal::Journal::append(journal_path, cmds.len() as u64).ok();
        self.pending_recovery = 0;
        self.dirty = n > 0;
        Ok(n)
    }

    pub fn recover(&mut self) -> Result<usize, EngineError> {
        let Some(p) = self.path.clone() else { return invalid("the document has no file") };
        self.recover_from(&journal::path_for(&p))
    }

    pub fn discard_recovery(&mut self) {
        if let Some(p) = &self.path {
            self.journal = journal::Journal::create(&journal::path_for(p)).ok();
        }
        self.pending_recovery = 0;
    }

    /// Stop journaling (batch tools that save explicitly); an empty journal file is removed.
    pub fn detach_journal(&mut self) {
        if let Some(j) = self.journal.take() {
            let p = j.path().to_path_buf();
            drop(j);
            if std::fs::metadata(&p).is_ok_and(|m| m.len() == 0) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    /// Journal every following command to `path` (truncating it).
    pub fn attach_journal(&mut self, path: &Path) -> Result<(), EngineError> {
        self.journal = Some(journal::Journal::create(path)?);
        Ok(())
    }

    pub fn journal_path(&self) -> Option<&Path> {
        self.journal.as_ref().map(|j| j.path())
    }

    /// Write the scene to `snapshot` and restart the journal next to it (autosave of an
    /// untitled document).
    pub fn checkpoint(&mut self, snapshot: &Path) -> Result<(), EngineError> {
        self.sync_sculpt();
        project::save_with_history(&self.scene, self.stored_history().as_ref(), snapshot)?;
        self.journal = Some(journal::Journal::create(&journal::path_for(snapshot))?);
        Ok(())
    }

    /// The history a save stores, if any (see [`Document::keep_history`]).
    fn stored_history(&self) -> Option<project::History<Command>> {
        if !self.keep_history {
            return None;
        }
        self.undo.persisted(self.history_max_bytes).map(|(base, done, undone)| project::History { base, done, undone })
    }

    pub fn save(&mut self) -> Result<(), EngineError> {
        let Some(p) = self.path.clone() else { return invalid("choose a file name first") };
        self.save_as(&p)
    }

    /// Atomic save; the journal restarts empty next to the new file.
    pub fn save_as(&mut self, path: &Path) -> Result<(), EngineError> {
        self.sync_sculpt();
        project::save_with_history(&self.scene, self.stored_history().as_ref(), path)?;
        self.path = Some(path.to_path_buf());
        self.journal = journal::Journal::create(&journal::path_for(path)).ok();
        self.dirty = false;
        Ok(())
    }

    pub fn sync_journal(&mut self) {
        if let Some(j) = &mut self.journal {
            let _ = j.sync();
        }
    }

    fn log(&mut self, cmd: &Command) {
        if self.replaying {
            return;
        }
        if let Some(j) = &mut self.journal
            && j.write(cmd).is_err()
        {
            // A journal that cannot be written is dropped rather than retried every command.
            self.journal = None;
        }
    }

    // ---- sculpt session ----

    /// Write the session's positions and mask back into the scene's sculpt layer.
    pub fn sync_sculpt(&mut self) {
        let Some(s) = &mut self.sculpt else { return };
        if !s.unsynced {
            return;
        }
        s.unsynced = false;
        let (pos, mask) = (Arc::new(s.mesh.positions.clone()), Arc::new(s.mesh.mask.clone()));
        let (id, step_id) = (s.id, s.step_id);
        // Live detail changed the triangles: the layer takes them over (or gives them back
        // when every live stroke has been undone).
        let topology = (s.mesh.topology_rev() != s.topo_synced).then(|| {
            s.topo_synced = s.mesh.topology_rev();
            if s.mesh.topology_changed() || s.had_topology { s.mesh.to_poly_mesh().ok().filter(|m| m.vertex_count() == pos.len()).map(Arc::new) } else { None }
        });
        if let Ok(o) = self.scene.edit(id)
            && let ObjectKind::Mesh { steps, .. } = &mut o.kind
            && let Some(step) = steps.iter_mut().find(|st| st.id == step_id)
            && let StepOp::Sculpt { layer } = &mut step.op
        {
            layer.positions = pos;
            layer.mask = mask;
            if let Some(t) = topology {
                layer.topology = t;
            }
        }
    }

    /// Make sure a session exists for `id` when its last step is a sculpt layer (no scene change).
    pub fn ensure_session(&mut self, id: ObjectId) -> Result<(), EngineError> {
        if self.sculpt.as_ref().is_some_and(|s| s.id == id) {
            return Ok(());
        }
        self.sync_sculpt();
        self.sculpt = None;
        let o = self.scene.get(id).ok_or(SceneError::NoObject(id))?;
        let steps = o.steps();
        let Some((last, Step { id: step_id, op: StepOp::Sculpt { layer }, .. })) = steps.len().checked_sub(1).and_then(|i| steps.get(i).map(|s| (i, s))) else {
            return invalid("the object has no sculpt layer");
        };
        let (step_id, layer) = (*step_id, layer.clone());
        let input = match &layer.topology {
            Some(t) => Arc::clone(t),
            None => self.eval.steps_output(&self.scene, id, last).mesh,
        };
        let input = crate::layers::level_mesh(&input, layer.level)?;
        if layer.positions.len() != input.vertex_count() {
            return invalid("the sculpt layer no longer matches the mesh (an earlier step changed the vertex count)");
        }
        let mesh = SculptMesh::new(&input, Some(&layer.positions), Some(&layer.mask));
        self.sculpt = Some(SculptSession { id, step_id, mesh, unsynced: false, had_topology: layer.topology.is_some(), topo_synced: 0 });
        Ok(())
    }

    /// After the scene's sculpt layer changed under the session: reuse the session when only
    /// positions moved, rebuild it when the detail level (the topology) changed.
    fn refresh_session(&mut self, id: ObjectId) -> Result<(), EngineError> {
        let positions = self.sculpt_layer(id).map(|l| Arc::clone(&l.positions));
        let same = match (&mut self.sculpt, positions) {
            (Some(s), Some(p)) if s.id == id => {
                s.unsynced = false;
                s.mesh.set_positions(&p)
            }
            _ => false,
        };
        if !same {
            self.sculpt = None;
            self.ensure_session(id)?;
        }
        Ok(())
    }

    /// Why live detail cannot be used on `id` right now (None: it can). Detail levels and shape
    /// layers hold one value per vertex of fixed triangles, which live detail would change.
    pub fn live_detail_blocked(&self, id: ObjectId) -> Option<&'static str> {
        let l = self.sculpt_layer(id)?;
        if l.level != 0 || !l.finer.is_empty() {
            Some("live detail works on the base detail level only, with no finer levels kept")
        } else if !l.shapes.is_empty() {
            Some("merge or remove the shape layers before using live detail")
        } else {
            None
        }
    }

    /// Interactive stroke: begin (requires a session).
    pub fn stroke_begin(&mut self, mut brush: BrushSettings) -> bool {
        // A drag stroke re-places its dab on every move, which live detail cannot take back.
        if brush.live_detail.is_some()
            && (brush.stroke == modelcraft_sculpt::StrokeMode::Drag || self.sculpt.as_ref().is_some_and(|s| self.live_detail_blocked(s.id).is_some()))
        {
            brush.live_detail = None;
        }
        match &mut self.sculpt {
            Some(s) => {
                s.mesh.begin_stroke(brush);
                true
            }
            None => false,
        }
    }

    pub fn stroke_dab(&mut self, dab: Dab) {
        if let Some(s) = &mut self.sculpt {
            s.mesh.dab(dab);
            s.unsynced = true;
        }
    }

    /// Take back what the open stroke has done so far (drag strokes).
    pub fn stroke_rewind(&mut self) -> bool {
        self.sculpt.as_mut().is_some_and(|s| s.mesh.rewind_stroke())
    }

    /// Finish the stroke: one undo entry and one journal line.
    pub fn stroke_end(&mut self) {
        let Some(s) = &mut self.sculpt else { return };
        let id = s.id;
        let Some((change, stroke)) = s.mesh.end_stroke() else { return };
        if change.is_empty() {
            return;
        }
        let cmd = Command::SculptStroke { id, stroke };
        self.undo.push(UndoEntry::Sculpt { label: cmd.label(), cmd: Box::new(cmd.clone()), id, change });
        self.log(&cmd);
        self.dirty = true;
        self.revision += 1;
    }

    // ---- execution ----

    /// Run a command: apply it, record undo, write the journal.
    pub fn execute(&mut self, cmd: Command) -> Result<Outcome, EngineError> {
        // Scrubbing is not journaled; record the frame before a command that may depend on it.
        if !self.replaying && self.scene.anim.frame != self.journal_frame && !matches!(cmd, Command::Anim(crate::anim::AnimCommand::SetTime { .. })) {
            self.journal_frame = self.scene.anim.frame;
            self.log(&Command::Anim(crate::anim::AnimCommand::SetTime { frame: self.journal_frame }));
        }
        if !self.groups.is_empty() && matches!(cmd, Command::Undo | Command::Redo) {
            return invalid("undo and redo are not available inside an undo group");
        }
        let out = match &cmd {
            Command::Anim(crate::anim::AnimCommand::SetTime { frame }) => {
                if !frame.is_finite() {
                    return invalid("the frame must be a finite number");
                }
                self.set_frame(*frame);
                self.journal_frame = self.scene.anim.frame;
                self.log(&cmd);
                return Ok(Outcome::default());
            }
            Command::Undo => {
                self.undo_once();
                Outcome::default()
            }
            Command::Redo => {
                self.redo_once();
                Outcome::default()
            }
            Command::BeginSculpt { id } => {
                let id = *id;
                let needs_layer = !matches!(self.scene.get(id).ok_or(SceneError::NoObject(id))?.steps().last(), Some(Step { op: StepOp::Sculpt { .. }, .. }));
                if !matches!(self.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })) {
                    return invalid("only mesh objects can be sculpted");
                }
                if let Some(o) = self.scene.get(id).filter(|o| o.locked) {
                    return invalid(format!("{} is locked; unlock it first", o.name));
                }
                if needs_layer {
                    let input = self.eval.steps_output(&self.scene, id, usize::MAX).mesh;
                    let layer = SculptLayer::new(input.positions().to_vec());
                    self.scene_change(&cmd, |doc| {
                        let sid = doc.scene.fresh_id();
                        if let ObjectKind::Mesh { steps, .. } = &mut doc.scene.edit(id)?.kind {
                            steps.push(Step { id: sid, enabled: true, op: StepOp::Sculpt { layer } });
                        }
                        Ok(Outcome::default())
                    })?;
                }
                self.ensure_session(id)?;
                Outcome::default()
            }
            Command::EndSculpt => {
                self.sync_sculpt();
                self.sculpt = None;
                Outcome::default()
            }
            Command::SculptStroke { id, stroke } => {
                self.ensure_session(*id)?;
                if stroke.brush.live_detail.is_some()
                    && let Some(why) = self.live_detail_blocked(*id)
                {
                    return invalid(why);
                }
                let Some(s) = &mut self.sculpt else { return invalid("not sculpting") };
                let change = s.mesh.replay(stroke);
                s.unsynced = true;
                if !change.is_empty() {
                    self.undo.push(UndoEntry::Sculpt { label: cmd.label(), cmd: Box::new(cmd.clone()), id: *id, change });
                }
                Outcome::default()
            }
            Command::SculptMask { id, op } => {
                self.ensure_session(*id)?;
                let Some(s) = &mut self.sculpt else { return invalid("not sculpting") };
                let change = match op {
                    MaskOp::Clear => s.mesh.mask_all(0.0),
                    MaskOp::Fill => s.mesh.mask_all(1.0),
                    MaskOp::Invert => s.mesh.mask_invert(),
                    MaskOp::Patch { triangle } => s.mesh.mask_patch(*triangle),
                    MaskOp::Blur => s.mesh.mask_blur(),
                    MaskOp::Sharpen => s.mesh.mask_sharpen(),
                    MaskOp::Grow => s.mesh.mask_grow(),
                    MaskOp::Shrink => s.mesh.mask_shrink(),
                    MaskOp::Hollows => s.mesh.mask_by_bend(false),
                    MaskOp::Ridges => s.mesh.mask_by_bend(true),
                };
                s.unsynced = true;
                if !change.is_empty() {
                    self.undo.push(UndoEntry::Sculpt { label: cmd.label(), cmd: Box::new(cmd.clone()), id: *id, change });
                }
                Outcome::default()
            }
            Command::SculptDetail { id, .. } | Command::SculptShape { id, .. } => {
                let id = *id;
                self.ensure_session(id)?;
                if self.sculpt.as_ref().is_some_and(|s| s.mesh.in_stroke()) {
                    return invalid("finish the stroke first");
                }
                let c = cmd.clone();
                self.scene_change(&cmd, move |doc| match &c {
                    Command::SculptDetail { op, .. } => doc.apply_detail(id, *op).map(|()| Outcome::default()),
                    Command::SculptShape { op, .. } => doc.apply_shape(id, op).map(|()| Outcome::default()),
                    _ => Ok(Outcome::default()),
                })?;
                self.refresh_session(id)?;
                Outcome::default()
            }
            _ => {
                let c = cmd.clone();
                self.scene_change(&cmd, move |doc| doc.apply_scene(&c))?
            }
        };
        self.log(&cmd);
        if !matches!(cmd, Command::BeginSculpt { .. } | Command::EndSculpt) {
            self.dirty = true;
            self.revision += 1;
        }
        Ok(out)
    }

    /// Run a scene-changing closure with undo snapshots; on error the scene is left untouched.
    fn scene_change(&mut self, cmd: &Command, f: impl FnOnce(&mut Document) -> Result<Outcome, EngineError>) -> Result<Outcome, EngineError> {
        self.sync_sculpt();
        let before = self.scene.clone();
        match f(self) {
            Ok(out) => {
                if self.scene.anim.auto_key && !matches!(cmd, Command::Anim(_)) {
                    modelcraft_scene::anim::auto_key(&before, &mut self.scene);
                }
                // Constraints follow whatever the command moved.
                self.scene.solve_constraints();
                let after = self.scene.clone();
                let bytes = 512 + self.scene.len() * 300 + self.scene.anim.heap_bytes();
                self.undo.push(UndoEntry::Scene { label: cmd.label(), cmd: Box::new(cmd.clone()), before: Box::new(before), after: Box::new(after), bytes });
                self.check_session();
                self.eval.retain(&self.scene);
                Ok(out)
            }
            Err(e) => {
                self.scene = before;
                Err(e)
            }
        }
    }

    /// Drop the session if its object or layer is gone.
    fn check_session(&mut self) {
        let keep = self.sculpt.as_ref().is_some_and(|s| {
            self.scene.get(s.id).is_some_and(|o| o.steps().last().is_some_and(|st| st.id == s.step_id && matches!(st.op, StepOp::Sculpt { .. })))
        });
        if !keep {
            self.sculpt = None;
        }
    }

    fn undo_once(&mut self) {
        let Some(e) = self.undo.pop_undo() else { return };
        match &e {
            UndoEntry::Scene { before, .. } => {
                self.sync_sculpt();
                let sculpting = self.sculpt.take().map(|s| s.id);
                let frame = self.scene.anim.frame;
                self.scene = (**before).clone().repaired_or(before);
                self.scene.set_frame(frame);
                // Keep sculpting the same object when it still has its sculpt layer.
                if let Some(id) = sculpting {
                    let _ = self.ensure_session(id);
                }
            }
            UndoEntry::Sculpt { id, change, .. } => {
                if self.ensure_session(*id).is_ok()
                    && let Some(s) = &mut self.sculpt
                {
                    s.mesh.apply_undo(change, false);
                    s.unsynced = true;
                }
            }
        }
        self.undo.push_redo(e);
        self.dirty = true;
    }

    fn redo_once(&mut self) {
        let Some(e) = self.undo.pop_redo() else { return };
        match &e {
            UndoEntry::Scene { after, .. } => {
                self.sync_sculpt();
                let sculpting = self.sculpt.take().map(|s| s.id);
                let frame = self.scene.anim.frame;
                self.scene = (**after).clone().repaired_or(after);
                self.scene.set_frame(frame);
                // Keep sculpting the same object when it still has its sculpt layer.
                if let Some(id) = sculpting {
                    let _ = self.ensure_session(id);
                }
            }
            UndoEntry::Sculpt { id, change, .. } => {
                if self.ensure_session(*id).is_ok()
                    && let Some(s) = &mut self.sculpt
                {
                    s.mesh.apply_undo(change, true);
                    s.unsynced = true;
                }
            }
        }
        self.undo.push_done(e);
        self.dirty = true;
    }

    fn mesh_steps(&mut self, id: ObjectId) -> Result<&mut Vec<Step>, EngineError> {
        match &mut self.scene.edit(id)?.kind {
            ObjectKind::Mesh { steps, .. } => Ok(steps),
            _ => invalid("not a mesh object"),
        }
    }

    fn apply_batch(&mut self, commands: &[Command], depth: usize, out: &mut Outcome) -> Result<(), EngineError> {
        if depth >= MAX_BATCH_DEPTH {
            return invalid("batches are nested too deeply");
        }
        if commands.len() > MAX_BATCH {
            return invalid(format!("a batch holds at most {MAX_BATCH} commands"));
        }
        for (i, c) in commands.iter().enumerate() {
            let r = match c {
                Command::Batch { commands, .. } => {
                    let mut inner = Outcome::default();
                    self.apply_batch(commands, depth + 1, &mut inner).map(|()| inner)
                }
                _ => self.apply_scene(c),
            };
            match r {
                Ok(o) => {
                    out.created.extend(o.created);
                    out.warnings.extend(o.warnings);
                }
                Err(e) => return invalid(format!("batch command {} ({}): {e}", i + 1, c.label())),
            }
        }
        Ok(())
    }

    fn apply_scene(&mut self, cmd: &Command) -> Result<Outcome, EngineError> {
        self.check_unlocked(cmd)?;
        self.apply_scene_unchecked(cmd)
    }

    /// Refuse an edit of a locked object.
    fn check_unlocked(&self, cmd: &Command) -> Result<(), EngineError> {
        let ids: Vec<ObjectId> = match cmd {
            Command::Delete { ids } => ids.clone(),
            Command::Duplicate { .. } | Command::SetVisible { .. } | Command::SetLocked { .. } | Command::SetTemplate { .. } => return Ok(()),
            Command::Rename { id, .. }
            | Command::Reparent { id, .. }
            | Command::SetTransform { id, .. }
            | Command::SetMaterial { id, .. }
            | Command::SetPrimitive { id, .. }
            | Command::SetCageLevels { id, .. }
            | Command::SetDeformer { id, .. }
            | Command::AddStep { id, .. }
            | Command::SetStep { id, .. }
            | Command::EnableStep { id, .. }
            | Command::RemoveStep { id, .. }
            | Command::Collapse { id } => vec![*id],
            _ => return Ok(()),
        };
        for id in ids {
            if let Some(o) = self.scene.get(id).filter(|o| o.locked) {
                return invalid(format!("{} is locked; unlock it first", o.name));
            }
        }
        Ok(())
    }

    fn apply_scene_unchecked(&mut self, cmd: &Command) -> Result<Outcome, EngineError> {
        let mut out = Outcome::default();
        match cmd {
            Command::AddPrimitive { kind, name, parent } => {
                let primitive = Primitive::default_of(kind).ok_or_else(|| EngineError::Invalid(format!("unknown primitive {kind:?}")))?;
                let base = name.clone().unwrap_or_else(|| title(kind));
                let name = self.scene.unique_name(&base);
                let id = self.scene.add(name, ObjectKind::Mesh { source: Source::Primitive { primitive }, steps: vec![] }, *parent)?;
                out.created.push(id);
            }
            Command::AddGroup { name } => {
                let name = self.scene.unique_name(name.as_deref().unwrap_or("Group"));
                out.created.push(self.scene.add(name, ObjectKind::Group, None)?);
            }
            Command::AddSmoothCage { input, levels } => {
                let parent = input.and_then(|i| self.scene.get(i)).and_then(|o| o.parent);
                let name = self.scene.unique_name("Smooth cage");
                let cage = self.scene.add(name, ObjectKind::SmoothCage { levels: (*levels).min(6) }, parent)?;
                if let Some(i) = input {
                    self.scene.reparent(*i, Some(cage))?;
                }
                out.created.push(cage);
            }
            Command::AddCloner { kind, input } => {
                let cloner = Cloner::default_of(kind).ok_or_else(|| EngineError::Invalid(format!("unknown clone layout {kind:?}")))?;
                if let Some(i) = input
                    && !matches!(self.scene.get(*i).map(|o| &o.kind), Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. }))
                {
                    return invalid("clones need a mesh or smooth cage as their source");
                }
                let parent = input.and_then(|i| self.scene.get(i)).and_then(|o| o.parent);
                let name = self.scene.unique_name("Clones");
                let id = self.scene.add(name, ObjectKind::Cloner { cloner, target: None }, parent)?;
                match input {
                    Some(i) => self.scene.reparent(*i, Some(id))?,
                    None => {
                        let primitive = Primitive::Cube { size: glam::Vec3::splat(0.5), segments: 1 };
                        let name = self.scene.unique_name("Clone source");
                        self.scene.add(name, ObjectKind::Mesh { source: Source::Primitive { primitive }, steps: vec![] }, Some(id))?;
                    }
                }
                out.warnings.extend(self.eval.evaluate(&self.scene, id).errors.iter().map(|e| e.message.clone()));
                out.created.push(id);
            }
            Command::AddDeformer { kind, target } => {
                let deformer = Deformer::default_of(kind).ok_or_else(|| EngineError::Invalid(format!("unknown deformer {kind:?}")))?;
                if !matches!(self.scene.get(*target).map(|o| &o.kind), Some(ObjectKind::Mesh { .. })) {
                    return invalid("deformers go under a mesh object");
                }
                let name = self.scene.unique_name(&title(kind));
                out.created.push(self.scene.add(name, ObjectKind::Deformer { deformer }, Some(*target))?);
            }
            Command::Delete { ids } => {
                for id in ids {
                    if self.scene.get(*id).is_none() {
                        return Err(SceneError::NoObject(*id).into());
                    }
                    self.scene.remove(*id);
                }
            }
            Command::Duplicate { id } => {
                let ids = self.scene.subtree(*id);
                if ids.is_empty() {
                    return Err(SceneError::NoObject(*id).into());
                }
                let mut map = std::collections::HashMap::new();
                for old in ids {
                    let Some(o) = self.scene.get(old).cloned() else { continue };
                    let parent = if old == *id { o.parent } else { o.parent.and_then(|p| map.get(&p).copied()) };
                    let name = if old == *id { self.scene.unique_name(&o.name) } else { o.name.clone() };
                    let nid = self.scene.add(name, o.kind.clone(), parent)?;
                    let n = self.scene.edit(nid)?;
                    n.transform = o.transform;
                    n.material = o.material.clone();
                    n.visible = o.visible;
                    map.insert(old, nid);
                    out.created.push(nid);
                }
            }
            Command::Rename { id, name } => {
                let name = name.trim();
                if name.is_empty() {
                    return invalid("a name cannot be empty");
                }
                self.scene.edit(*id)?.name = name.chars().take(256).collect();
            }
            Command::Reparent { id, parent } => self.scene.reparent(*id, *parent)?,
            Command::SetTransform { id, transform } => self.scene.edit(*id)?.transform = transform.sanitized(),
            Command::SetVisible { id, visible } => self.scene.edit(*id)?.visible = *visible,
            Command::SetMaterial { id, material } => {
                let mut m = material.clone();
                m.base_color = m.base_color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 1.0 });
                m.metallic = if m.metallic.is_finite() { m.metallic.clamp(0.0, 1.0) } else { 0.0 };
                m.roughness = if m.roughness.is_finite() { m.roughness.clamp(0.0, 1.0) } else { 0.5 };
                self.scene.edit(*id)?.material = m;
            }
            Command::SetPrimitive { id, primitive } => match &mut self.scene.edit(*id)?.kind {
                ObjectKind::Mesh { source, .. } if matches!(source, Source::Primitive { .. }) => *source = Source::Primitive { primitive: *primitive },
                _ => return invalid("not a primitive"),
            },
            Command::SetCageLevels { id, levels } => match &mut self.scene.edit(*id)?.kind {
                ObjectKind::SmoothCage { levels: l } => *l = (*levels).min(6),
                _ => return invalid("not a smooth cage"),
            },
            Command::SetDeformer { id, deformer } => match &mut self.scene.edit(*id)?.kind {
                ObjectKind::Deformer { deformer: d } => *d = *deformer,
                _ => return invalid("not a deformer"),
            },
            Command::SetCloner { id, cloner, target } => {
                if target.is_some_and(|t| t == *id || self.scene.get(t).is_none()) {
                    return invalid("the target must be another object in the scene");
                }
                match &mut self.scene.edit(*id)?.kind {
                    ObjectKind::Cloner { cloner: c, target: t } => {
                        *c = cloner.sanitized();
                        *t = *target;
                    }
                    _ => return invalid("not a clone generator"),
                }
                // A missing source or target, or a cut-off count, reaches the status line (a
                // live region) so it is announced where the edit was made.
                out.warnings.extend(self.eval.evaluate(&self.scene, *id).errors.iter().map(|e| e.message.clone()));
            }
            Command::AddStep { id, op } => {
                if matches!(op, StepOp::Sculpt { .. }) {
                    return invalid("sculpt layers are added by begin_sculpt");
                }
                let sid = self.scene.fresh_id();
                self.mesh_steps(*id)?.push(Step { id: sid, enabled: true, op: op.clone() });
                let e = self.eval.evaluate(&self.scene, *id);
                out.warnings.extend(e.errors.iter().map(|e| e.message.clone()));
            }
            Command::SetStep { id, index, op } => {
                let steps = self.mesh_steps(*id)?;
                let s = steps.get_mut(*index).ok_or_else(|| EngineError::Invalid(format!("no step {index}")))?;
                if matches!(op, StepOp::Sculpt { .. }) || matches!(s.op, StepOp::Sculpt { .. }) {
                    return invalid("sculpt layers change by sculpting");
                }
                s.op = op.clone();
            }
            Command::EnableStep { id, index, enabled } => {
                self.mesh_steps(*id)?.get_mut(*index).ok_or_else(|| EngineError::Invalid(format!("no step {index}")))?.enabled = *enabled;
            }
            Command::RemoveStep { id, index } => {
                let steps = self.mesh_steps(*id)?;
                if *index >= steps.len() {
                    return invalid(format!("no step {index}"));
                }
                steps.remove(*index);
            }
            Command::Collapse { id } => self.scene.collapse(*id, &mut self.eval)?,
            Command::Import { path, up, scale } => {
                let p = Path::new(path);
                let kind = Kind::from_path(p).ok_or_else(|| EngineError::Invalid(format!("{path}: not a glTF, GLB, OBJ, STL, PLY, FBX or 3MF file")))?;
                let meta = std::fs::metadata(p).map_err(|e| FormatError::io(p, e))?;
                if meta.len() > MAX_IMPORT_BYTES {
                    return invalid(format!("{path} is too large"));
                }
                let bytes = std::fs::read(p).map_err(|e| FormatError::io(p, e))?;
                let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
                let mut resolve = |name: &str| -> Option<Vec<u8>> {
                    // Only plain sibling file names: no directories, no escaping the folder.
                    let file = Path::new(name).file_name()?;
                    if file != std::ffi::OsStr::new(name) {
                        return None;
                    }
                    let q = dir.join(file);
                    (std::fs::metadata(&q).ok()?.len() <= MAX_IMPORT_BYTES).then(|| std::fs::read(q).ok()).flatten()
                };
                let (items, warnings) = exchange::import_with(&bytes, kind, &mut resolve, exchange::Convert { up: *up, scale: *scale })?;
                out.warnings = warnings;
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("Import").to_string();
                let root = if items.len() > 1 { Some(self.scene.add(self.scene.unique_name(&stem), ObjectKind::Group, None)?) } else { None };
                out.created.extend(root);
                let mut ids: Vec<ObjectId> = Vec::with_capacity(items.len());
                for it in items {
                    let parent = it.parent.and_then(|i| ids.get(i).copied()).or(root);
                    let kind = match it.mesh {
                        Some(mesh) => ObjectKind::Mesh { source: Source::Mesh { mesh }, steps: vec![] },
                        None => ObjectKind::Group,
                    };
                    let id = self.scene.add(self.scene.unique_name(&it.name), kind, parent)?;
                    let o = self.scene.edit(id)?;
                    o.transform = it.transform;
                    o.material = it.material;
                    ids.push(id);
                }
                out.created.extend(ids);
            }
            Command::Anim(a) => crate::anim::apply(&mut self.scene, a, &mut out)?,
            Command::AddMesh { name, parent, positions, faces } => {
                if faces.is_empty() {
                    return invalid("a mesh needs at least one face");
                }
                let pos: Vec<glam::Vec3> = positions.iter().map(|p| glam::Vec3::from_array(*p)).collect();
                let polys: Vec<modelcraft_mesh::Polygon> = faces.iter().map(|f| modelcraft_mesh::Polygon::new(f.clone())).collect();
                let (mesh, report) = modelcraft_mesh::PolyMesh::build(pos, polys).map_err(|e| EngineError::Invalid(e.to_string()))?;
                if mesh.face_count() == 0 {
                    return invalid("none of the faces fit the vertices");
                }
                if report.dropped > 0 {
                    out.warnings.push(format!("{} faces were dropped (an index out of range, fewer than three corners or a repeated vertex)", report.dropped));
                }
                if report.fixed_positions > 0 {
                    out.warnings.push(format!("{} non-finite positions were moved to the origin", report.fixed_positions));
                }
                let name = self.scene.unique_name(name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Mesh"));
                let id = self.scene.add(name, ObjectKind::Mesh { source: Source::Mesh { mesh: Arc::new(mesh) }, steps: vec![] }, *parent)?;
                out.created.push(id);
            }
            Command::SetPositions { id, positions } => match &mut self.scene.edit(*id)?.kind {
                ObjectKind::Mesh { source: Source::Mesh { mesh }, .. } => {
                    let pos: Vec<glam::Vec3> = positions.iter().map(|p| glam::Vec3::from_array(*p)).collect();
                    let mut m = (**mesh).clone();
                    m.set_positions(&pos).map_err(|e| EngineError::Invalid(e.to_string()))?;
                    *mesh = Arc::new(m);
                }
                _ => return invalid("only objects that store their own mesh have positions to set (collapse a primitive first)"),
            },
            Command::Batch { commands, .. } => self.apply_batch(commands, 0, &mut out)?,
            Command::Rig(r) => crate::rig::apply(self, r, &mut out)?,
            Command::Hair(h) => crate::hair::apply(self, h, &mut out)?,
            Command::SetLocked { ids, locked } => {
                for id in ids {
                    self.scene.edit(*id)?.locked = *locked;
                }
            }
            Command::SetTemplate { ids, template } => {
                for id in ids {
                    self.scene.edit(*id)?.template = *template;
                }
            }
            Command::AddSelectionSet { name, ids } => {
                let name: String = name.trim().chars().take(256).collect();
                if name.is_empty() {
                    return invalid("a selection set needs a name");
                }
                if self.scene.sets.len() >= modelcraft_scene::MAX_SETS {
                    return invalid("too many selection sets");
                }
                let mut ids: Vec<ObjectId> = ids.iter().copied().filter(|i| self.scene.get(*i).is_some()).collect();
                ids.dedup();
                if ids.is_empty() {
                    return invalid("select objects first");
                }
                self.scene.sets.push(modelcraft_scene::SelectionSet { name, ids });
            }
            Command::RemoveSelectionSet { index } => {
                if *index >= self.scene.sets.len() {
                    return invalid(format!("no selection set {index}"));
                }
                self.scene.sets.remove(*index);
            }
            Command::Undo
            | Command::Redo
            | Command::BeginSculpt { .. }
            | Command::EndSculpt
            | Command::SculptStroke { .. }
            | Command::SculptMask { .. }
            | Command::SculptDetail { .. }
            | Command::SculptShape { .. } => {
                return invalid("not a scene command");
            }
        }
        Ok(out)
    }

    // ---- undo groups ----

    /// Start an undo group: every change until the matching [`Document::end_group`] becomes one
    /// undo step named `label`. Groups nest; only the outermost one makes a step.
    pub fn begin_group(&mut self, label: &str) {
        self.sync_sculpt();
        let mark = self.undo.len() + self.undo.dropped;
        self.groups.push(GroupStart { label: label.to_string(), mark });
    }

    /// Close the innermost group. With `rollback` its changes are undone and forgotten (no redo
    /// step is left); otherwise the entries it made are merged into one. Sculpt strokes inside a
    /// group stay separate entries (they store vertex deltas, not scenes). Returns how many
    /// entries the group made.
    pub fn end_group(&mut self, rollback: bool) -> usize {
        let Some(g) = self.groups.pop() else { return 0 };
        self.sync_sculpt();
        let made = (self.undo.len() + self.undo.dropped).saturating_sub(g.mark).min(self.undo.len());
        if rollback {
            for _ in 0..made {
                self.undo_once();
            }
            self.undo.clear_redo();
            return made;
        }
        if made > 1 && self.groups.is_empty() {
            let entries = self.undo.take_last(made);
            if entries.iter().all(|e| matches!(e, UndoEntry::Scene { .. })) {
                let first = entries.first();
                let last = entries.last();
                if let (Some(UndoEntry::Scene { before, .. }), Some(UndoEntry::Scene { after, .. })) = (first, last) {
                    let (before, after) = (before.clone(), after.clone());
                    let bytes = 512 + self.scene.len() * 300;
                    // The merged step replays as one batch of what the group ran.
                    let cmd = Command::Batch { label: Some(g.label.clone()), commands: entries.iter().map(|e| e.command().clone()).collect() };
                    self.undo.push(UndoEntry::Scene { label: g.label, cmd: Box::new(cmd), before, after, bytes });
                    return made;
                }
            }
            for e in entries {
                self.undo.push_done(e);
            }
        }
        made
    }

    /// How many undo groups are open.
    pub fn group_depth(&self) -> usize {
        self.groups.len()
    }

    // ---- export ----

    /// The scene as export items: evaluated meshes, smooth cages replacing their inputs.
    pub fn export_items(&mut self) -> Vec<Item> {
        self.sync_sculpt();
        let order = self.scene.tree_order();
        let mut index: std::collections::HashMap<ObjectId, usize> = std::collections::HashMap::new();
        let mut items = Vec::new();
        for (id, _) in order {
            let Some(o) = self.scene.get(id).cloned() else { continue };
            if self.scene.is_cage_input(id)
                || matches!(o.kind, ObjectKind::Deformer { .. } | ObjectKind::Skin { .. } | ObjectKind::PointDeformer { .. })
                || !self.scene.visible(id)
            {
                continue;
            }
            let evaluated = match o.kind {
                // Joints export as empty nodes; a skinned mesh exports in its current pose.
                ObjectKind::Group | ObjectKind::Joint { .. } => None,
                _ => Some(self.eval.evaluate(&self.scene, id)),
            };
            let parent = o.parent.and_then(|p| index.get(&p).copied());
            index.insert(id, items.len());
            // A clone generator exports as a group with one node per clone, all sharing the source
            // mesh (glTF nodes may share a mesh).
            let instances = evaluated.as_ref().and_then(|e| e.instances.clone());
            let mesh = if instances.is_some() { None } else { evaluated.map(|e| e.mesh) };
            let here = items.len();
            items.push(Item { name: o.name.clone(), parent, transform: o.transform, mesh, material: o.material.clone() });
            if let Some(inst) = instances {
                let material = inst.source_object.and_then(|s| self.scene.get(s)).map(|s| s.material.clone()).unwrap_or_default();
                let mut baked_vertices = 0usize;
                for c in &inst.clones.instances {
                    let (transform, mesh) = clone_node(c.matrix, &inst.source, &mut baked_vertices);
                    items.push(Item {
                        name: format!("{} {}", o.name, c.index + 1),
                        parent: Some(here),
                        transform,
                        mesh: Some(mesh),
                        material: material.clone(),
                    });
                }
            }
        }
        items
    }

    pub fn export(&mut self, path: &Path) -> Result<(), EngineError> {
        self.export_with(path, exchange::Convert::default())
    }

    /// Export into another up axis or unit (`convert.scale`: metres per file unit).
    pub fn export_with(&mut self, path: &Path, convert: exchange::Convert) -> Result<(), EngineError> {
        let kind = Kind::from_path(path).filter(|k| k.can_write()).ok_or_else(|| EngineError::Invalid("export to .glb, .obj, .stl or .fbx".into()))?;
        let bytes = exchange::export_with(&self.export_items(), kind, convert)?;
        craft_atomic_write(path, &bytes)
    }

    /// Bake the vertices of one object over a run of frames: its final mesh (history, deformers,
    /// animated parameters) at every `step` frames from `start` to `end`, in the object's own
    /// space or, with `world`, in world space with its animated transform applied. The current
    /// frame is restored afterwards. Fails when the vertex count changes during the span.
    pub fn bake_points(
        &mut self,
        id: ObjectId,
        start: f64,
        end: f64,
        step: f64,
        world: bool,
    ) -> Result<modelcraft_formats::pointcache::PointCache, EngineError> {
        if self.scene.get(id).is_none() {
            return Err(SceneError::NoObject(id).into());
        }
        if !(start.is_finite() && end.is_finite() && step.is_finite()) || end < start || step <= 0.0 || (end - start) / step > 1_000_000.0 {
            return invalid("a bake needs start <= end, a positive step and at most a million samples");
        }
        self.sync_sculpt();
        let now = self.scene.anim.frame;
        let frames = ((end - start) / step).floor() as usize + 1;
        let mut cache: Option<modelcraft_formats::pointcache::PointCache> = None;
        let mut failed = None;
        for i in 0..frames {
            let f = start + i as f64 * step;
            self.scene.set_frame(f);
            let mesh = self.eval.evaluate(&self.scene, id).mesh;
            let m = if world { self.scene.world_matrix(id) } else { glam::Mat4::IDENTITY };
            let pts: Vec<glam::Vec3> = mesh.positions().iter().map(|p| m.transform_point3(*p)).collect();
            let c = cache.get_or_insert_with(|| modelcraft_formats::pointcache::PointCache::new(self.scene.anim.time.fps, start, step, pts.len()));
            if let Err(e) = c.push(&pts) {
                failed = Some(EngineError::Invalid(format!("frame {f}: {e} (the mesh must keep its vertex count)")));
                break;
            }
        }
        self.scene.set_frame(now);
        match (failed, cache) {
            (Some(e), _) => Err(e),
            (None, Some(c)) => Ok(c),
            (None, None) => invalid("nothing to bake"),
        }
    }

    /// Write the UV layout of a mesh object as `.svg` or `.png`: the given object, or the first
    /// visible mesh that has UVs. `size` is the picture's side in pixels.
    pub fn export_uv_layout(&mut self, path: &Path, object: Option<ObjectId>, size: u32) -> Result<(), EngineError> {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if ext != "svg" && ext != "png" {
            return Err(EngineError::Invalid("export the UV layout to .svg or .png".into()));
        }
        self.sync_sculpt();
        let id = match object {
            Some(id) => id,
            None => {
                let ids: Vec<ObjectId> = self.scene.tree_order().into_iter().map(|(id, _)| id).collect();
                ids.into_iter()
                    .find(|id| {
                        matches!(self.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }))
                            && self.scene.visible(*id)
                            && self.eval.evaluate(&self.scene, *id).mesh.has_uvs()
                    })
                    .ok_or_else(|| EngineError::Invalid("no visible mesh has UVs".into()))?
            }
        };
        if self.scene.get(id).is_none() {
            return Err(EngineError::Invalid(format!("no object {}", id.0)));
        }
        let mesh = self.eval.evaluate(&self.scene, id).mesh;
        let bytes = if ext == "svg" { uvlayout::svg(&mesh, size)?.into_bytes() } else { uvlayout::png(&mesh, size)? };
        craft_atomic_write(path, &bytes)
    }

    /// Total triangles of everything visible (for the status bar).
    pub fn triangle_count(&mut self) -> usize {
        let ids: Vec<ObjectId> = self.scene.objects().iter().map(|o| o.id).collect();
        ids.into_iter()
            .filter(|id| !self.scene.is_cage_input(*id))
            .map(|id| match &self.sculpt {
                Some(s) if s.id == id => s.mesh.triangle_count(),
                _ => {
                    let e = self.eval.evaluate(&self.scene, id);
                    e.mesh.triangle_count() + e.instances.map_or(0, |i| i.triangle_count())
                }
            })
            .sum()
    }
}

/// Most vertices all the baked clones of one generator may add to an export.
const MAX_BAKED_VERTICES: usize = 16_000_000;

/// The node for one clone. A glTF node carries translation, rotation and scale only, so a clone
/// whose matrix has shear (a stretch applied after a rotation) cannot be one node over the shared
/// mesh: it gets its own copy of the mesh with the matrix applied to the points, and an identity
/// transform. `baked_vertices` counts those copies; past [`MAX_BAKED_VERTICES`] a sheared clone
/// falls back to its nearest rotation and scale.
fn clone_node(
    matrix: glam::Mat4,
    source: &std::sync::Arc<modelcraft_scene::PolyMesh>,
    baked_vertices: &mut usize,
) -> (modelcraft_scene::Transform, std::sync::Arc<modelcraft_scene::PolyMesh>) {
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let rebuilt = glam::Mat4::from_scale_rotation_translation(scale, rotation, translation);
    let tolerance = 1e-4 * matrix.to_cols_array().iter().fold(1.0f32, |m, v| m.max(v.abs()));
    let trs = rebuilt.to_cols_array().iter().zip(matrix.to_cols_array()).all(|(a, b)| (a - b).abs() <= tolerance);
    if trs || !matrix.is_finite() || *baked_vertices + source.vertex_count() > MAX_BAKED_VERTICES {
        return (modelcraft_scene::Transform { translation, rotation, scale }.sanitized(), source.clone());
    }
    *baked_vertices += source.vertex_count();
    let mut baked = (**source).clone();
    baked.map_positions(|_, p| matrix.transform_point3(p));
    // A matrix that mirrors turns the faces inside out; put them back the right way round.
    if matrix.determinant() < 0.0 {
        let all: Vec<u32> = (0..baked.face_count() as u32).collect();
        if let Ok(flipped) = modelcraft_ops::edit::flip_faces(&baked, &all) {
            baked = flipped;
        }
    }
    (modelcraft_scene::Transform::default(), std::sync::Arc::new(baked))
}

fn craft_atomic_write(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    craft_atomic::atomic_write(path, bytes).map_err(|e| FormatError::io(path, e).into())
}

fn title(kind: &str) -> String {
    let s = kind.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => s,
    }
}

/// Two scenes hold the same objects (revisions, which are cache keys, aside).
fn same_content(a: &Scene, b: &Scene) -> bool {
    a.len() == b.len()
        && a.objects().iter().zip(b.objects()).all(|(x, y)| {
            let mut x = x.clone();
            x.rev = y.rev;
            x == *y
        })
}

trait RepairedOr {
    fn repaired_or(self, fallback: &Scene) -> Scene;
}

impl RepairedOr for Scene {
    /// Bump revisions on a restored snapshot so caches re-evaluate.
    fn repaired_or(self, fallback: &Scene) -> Scene {
        self.repaired().unwrap_or_else(|_| fallback.clone())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
