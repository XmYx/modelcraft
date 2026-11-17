//! The document and command execution.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use modelcraft_formats::exchange::{self, Item, Kind};
use modelcraft_formats::{FormatError, journal, project};
use modelcraft_ops::deform::Deformer;
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{Evaluator, ObjectId, ObjectKind, Scene, SceneError, SculptLayer, Source, Step, StepOp};
use modelcraft_sculpt::{BrushSettings, Dab, SculptMesh};

use crate::command::{Command, MaskOp, Outcome};
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
    /// The scene's layer is older than `mesh`.
    pub unsynced: bool,
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
    /// Journal entries found next to an opened file, waiting for [`Document::recover`].
    pub pending_recovery: usize,
}

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
            pending_recovery: 0,
        }
    }

    pub fn from_scene(scene: Scene) -> Self {
        Document { scene, ..Self::new() }
    }

    /// Open a project. If a journal of unsaved work lies next to it, `pending_recovery` says how
    /// many commands it holds; call [`Document::recover`] to replay them or
    /// [`Document::discard_recovery`] to drop them.
    pub fn open(path: &Path) -> Result<Document, EngineError> {
        let scene = project::open(path)?;
        let mut d = Document::from_scene(scene);
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
        project::save(&self.scene, snapshot)?;
        self.journal = Some(journal::Journal::create(&journal::path_for(snapshot))?);
        Ok(())
    }

    pub fn save(&mut self) -> Result<(), EngineError> {
        let Some(p) = self.path.clone() else { return invalid("choose a file name first") };
        self.save_as(&p)
    }

    /// Atomic save; the journal restarts empty next to the new file.
    pub fn save_as(&mut self, path: &Path) -> Result<(), EngineError> {
        self.sync_sculpt();
        project::save(&self.scene, path)?;
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
        if let Ok(o) = self.scene.edit(id)
            && let ObjectKind::Mesh { steps, .. } = &mut o.kind
            && let Some(step) = steps.iter_mut().find(|st| st.id == step_id)
            && let StepOp::Sculpt { layer } = &mut step.op
        {
            layer.positions = pos;
            layer.mask = mask;
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
        let input = self.eval.steps_output(&self.scene, id, last).mesh;
        let input = crate::layers::level_mesh(&input, layer.level)?;
        if layer.positions.len() != input.vertex_count() {
            return invalid("the sculpt layer no longer matches the mesh (an earlier step changed the vertex count)");
        }
        let mesh = SculptMesh::new(&input, Some(&layer.positions), Some(&layer.mask));
        self.sculpt = Some(SculptSession { id, step_id, mesh, unsynced: false });
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

    /// Interactive stroke: begin (requires a session).
    pub fn stroke_begin(&mut self, brush: BrushSettings) -> bool {
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

    /// Finish the stroke: one undo entry and one journal line.
    pub fn stroke_end(&mut self) {
        let Some(s) = &mut self.sculpt else { return };
        let id = s.id;
        let Some((change, stroke)) = s.mesh.end_stroke() else { return };
        if change.is_empty() {
            return;
        }
        let cmd = Command::SculptStroke { id, stroke };
        self.undo.push(UndoEntry::Sculpt { label: cmd.label(), id, change });
        self.log(&cmd);
        self.dirty = true;
    }

    // ---- execution ----

    /// Run a command: apply it, record undo, write the journal.
    pub fn execute(&mut self, cmd: Command) -> Result<Outcome, EngineError> {
        let out = match &cmd {
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
                let Some(s) = &mut self.sculpt else { return invalid("not sculpting") };
                let change = s.mesh.replay(stroke);
                s.unsynced = true;
                if !change.is_empty() {
                    self.undo.push(UndoEntry::Sculpt { label: cmd.label(), id: *id, change });
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
                };
                s.unsynced = true;
                if !change.is_empty() {
                    self.undo.push(UndoEntry::Sculpt { label: cmd.label(), id: *id, change });
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
        }
        Ok(out)
    }

    /// Run a scene-changing closure with undo snapshots; on error the scene is left untouched.
    fn scene_change(&mut self, cmd: &Command, f: impl FnOnce(&mut Document) -> Result<Outcome, EngineError>) -> Result<Outcome, EngineError> {
        self.sync_sculpt();
        let before = self.scene.clone();
        match f(self) {
            Ok(out) => {
                let after = self.scene.clone();
                let bytes = 512 + self.scene.len() * 300;
                self.undo.push(UndoEntry::Scene { label: cmd.label(), before: Box::new(before), after: Box::new(after), bytes });
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
                self.scene = (**before).clone().repaired_or(before);
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
                self.scene = (**after).clone().repaired_or(after);
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

    fn apply_scene(&mut self, cmd: &Command) -> Result<Outcome, EngineError> {
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
            Command::Import { path } => {
                let p = Path::new(path);
                let kind = Kind::from_path(p).ok_or_else(|| EngineError::Invalid(format!("{path}: not a glTF, GLB, OBJ, STL or PLY file")))?;
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
                let (items, warnings) = exchange::import(&bytes, kind, &mut resolve)?;
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

    // ---- export ----

    /// The scene as export items: evaluated meshes, smooth cages replacing their inputs.
    pub fn export_items(&mut self) -> Vec<Item> {
        self.sync_sculpt();
        let order = self.scene.tree_order();
        let mut index: std::collections::HashMap<ObjectId, usize> = std::collections::HashMap::new();
        let mut items = Vec::new();
        for (id, _) in order {
            let Some(o) = self.scene.get(id).cloned() else { continue };
            if self.scene.is_cage_input(id) || matches!(o.kind, ObjectKind::Deformer { .. }) || !self.scene.visible(id) {
                continue;
            }
            let mesh = match o.kind {
                ObjectKind::Group => None,
                _ => Some(self.eval.evaluate(&self.scene, id).mesh),
            };
            let parent = o.parent.and_then(|p| index.get(&p).copied());
            index.insert(id, items.len());
            items.push(Item { name: o.name.clone(), parent, transform: o.transform, mesh, material: o.material.clone() });
        }
        items
    }

    pub fn export(&mut self, path: &Path) -> Result<(), EngineError> {
        let kind = Kind::from_path(path).filter(|k| k.can_write()).ok_or_else(|| EngineError::Invalid("export to .glb, .obj or .stl".into()))?;
        let bytes = exchange::export(&self.export_items(), kind)?;
        craft_atomic_write(path, &bytes)
    }

    /// Total triangles of everything visible (for the status bar).
    pub fn triangle_count(&mut self) -> usize {
        let ids: Vec<ObjectId> = self.scene.objects().iter().map(|o| o.id).collect();
        ids.into_iter()
            .filter(|id| !self.scene.is_cage_input(*id))
            .map(|id| match &self.sculpt {
                Some(s) if s.id == id => s.mesh.triangle_count(),
                _ => self.eval.evaluate(&self.scene, id).mesh.triangle_count(),
            })
            .sum()
    }
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
