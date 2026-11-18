//! What the registered script functions share during a run: the document (swapped in for the
//! duration of the run), the selection, the output, the limits and the permissions.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::Instant;

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{Command, Document, Outcome};

use crate::error::{R, limit, rt};
use crate::limits::Limits;
use crate::perm::{Access, Permissions};

/// What a run reads from the app and hands back to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunContext {
    /// The selected objects (scripts read and change it).
    pub selection: Vec<ObjectId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Print,
    Debug,
    Warning,
}

/// One line of the console.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutLine {
    pub kind: LineKind,
    pub text: String,
    /// The script line a `debug` call came from.
    pub line: Option<usize>,
}

#[derive(Default)]
pub(crate) struct Output {
    pub lines: Vec<OutLine>,
    bytes: usize,
    pub truncated: bool,
}

pub(crate) struct Shared {
    pub doc: RefCell<Document>,
    pub ctx: RefCell<RunContext>,
    pub selection_changed: Cell<bool>,
    pub out: RefCell<Output>,
    pub limits: Limits,
    pub perms: RefCell<Permissions>,
    pub deadline: Cell<Option<Instant>>,
    pub native_left: Cell<u64>,
    pub commands_run: Cell<u64>,
    pub operations: Cell<u64>,
    /// `--arg name=value` pairs and the document file, for `args()` and `current_file()`.
    pub args: RefCell<BTreeMap<String, String>>,
    pub file: RefCell<Option<String>>,
    pub registry: RefCell<crate::api::events::Registry>,
}

impl Shared {
    pub fn new(limits: Limits, perms: Permissions) -> Self {
        Shared {
            doc: RefCell::new(Document::new()),
            ctx: RefCell::new(RunContext::default()),
            selection_changed: Cell::new(false),
            out: RefCell::new(Output::default()),
            native_left: Cell::new(limits.max_native_items),
            limits,
            perms: RefCell::new(perms),
            deadline: Cell::new(None),
            commands_run: Cell::new(0),
            operations: Cell::new(0),
            args: RefCell::new(BTreeMap::new()),
            file: RefCell::new(None),
            registry: RefCell::new(Default::default()),
        }
    }

    /// Run `f` on the document. The document is only borrowed for the call, never across script code.
    pub fn with_doc<T>(&self, f: impl FnOnce(&mut Document) -> T) -> R<T> {
        let mut d = self.doc.try_borrow_mut().map_err(|_| rt("the document is busy"))?;
        Ok(f(&mut d))
    }

    pub fn with_ctx<T>(&self, f: impl FnOnce(&mut RunContext) -> T) -> R<T> {
        let mut c = self.ctx.try_borrow_mut().map_err(|_| rt("the selection is busy"))?;
        Ok(f(&mut c))
    }

    pub fn check_time(&self) -> R<()> {
        match self.deadline.get() {
            Some(t) if Instant::now() >= t => Err(limit(format!("the script ran longer than {} s and was stopped", self.limits.max_time.as_secs_f64()))),
            _ => Ok(()),
        }
    }

    /// Spend `n` of the budget for geometry a script builds or copies.
    pub fn charge(&self, n: usize) -> R<()> {
        let left = self.native_left.get();
        let n = n as u64;
        if n > left {
            self.native_left.set(0);
            return Err(limit("the script built or copied more geometry than one run may and was stopped"));
        }
        self.native_left.set(left - n);
        Ok(())
    }

    pub fn emit(&self, kind: LineKind, text: &str, line: Option<usize>) {
        let Ok(mut o) = self.out.try_borrow_mut() else { return };
        if o.truncated {
            return;
        }
        let text: String = text.chars().take(4096).collect();
        o.bytes += text.len();
        if o.lines.len() >= self.limits.max_output_lines || o.bytes > 4 << 20 {
            o.truncated = true;
            o.lines.push(OutLine { kind: LineKind::Warning, text: "(output cut: too many lines)".into(), line: None });
            return;
        }
        o.lines.push(OutLine { kind, text, line });
    }

    /// Run a command for a script: limits first, then the permission check for paths, then the
    /// document (which journals it and records undo like any other change).
    pub fn exec(&self, mut cmd: Command) -> R<Outcome> {
        self.check_time()?;
        let n = self.commands_run.get() + 1;
        if n > self.limits.max_commands {
            return Err(limit(format!("the script ran more than {} commands and was stopped", self.limits.max_commands)));
        }
        self.commands_run.set(n);
        self.permit(&mut cmd)?;
        let out = self.with_doc(|d| d.execute(cmd))?.map_err(|e| rt(e.to_string()))?;
        for w in &out.warnings {
            self.emit(LineKind::Warning, w, None);
        }
        Ok(out)
    }

    /// Commands that name files go through the permissions; the path is made absolute.
    fn permit(&self, cmd: &mut Command) -> R<()> {
        // Exhaustive on purpose: `cmd::<name>` exposes every command of the registry, so a command
        // that gains a file path must be given a decision here before this compiles. (A material's
        // linked file and similar paths are read only by the renderer, which scripts cannot call.)
        match cmd {
            Command::Import { path, .. } => {
                let full = self.path(path, Access::Read)?;
                *path = full.to_string_lossy().into_owned();
            }
            Command::Sim(modelcraft_engine::SimCommand::AttachCache { path, .. }) => {
                let full = self.path(path, Access::Read)?;
                *path = full.to_string_lossy().into_owned();
            }
            Command::Batch { commands, .. } => {
                for c in commands {
                    self.permit(c)?;
                }
            }
            Command::AddPrimitive { .. }
            | Command::AddGroup { .. }
            | Command::AddSmoothCage { .. }
            | Command::AddDeformer { .. }
            | Command::Delete { .. }
            | Command::Duplicate { .. }
            | Command::Rename { .. }
            | Command::Reparent { .. }
            | Command::SetTransform { .. }
            | Command::SetVisible { .. }
            | Command::SetMaterial { .. }
            | Command::SetPrimitive { .. }
            | Command::SetCageLevels { .. }
            | Command::SetDeformer { .. }
            | Command::AddStep { .. }
            | Command::SetStep { .. }
            | Command::EnableStep { .. }
            | Command::RemoveStep { .. }
            | Command::Collapse { .. }
            | Command::BeginSculpt { .. }
            | Command::EndSculpt
            | Command::SculptStroke { .. }
            | Command::SculptMask { .. }
            | Command::SculptDetail { .. }
            | Command::SculptShape { .. }
            | Command::SculptGroups { .. }
            | Command::SculptVisibility { .. }
            | Command::SculptRegion { .. }
            | Command::SculptClip { .. }
            | Command::SculptExtract { .. }
            | Command::SculptProject { .. }
            | Command::TransferUvs { .. }
            | Command::AddMesh { .. }
            | Command::SetPositions { .. }
            | Command::Anim(_)
            | Command::Rig(_)
            | Command::Sim(_)
            | Command::AddCloner { .. }
            | Command::SetCloner { .. }
            | Command::SetLocked { .. }
            | Command::SetTemplate { .. }
            | Command::AddSelectionSet { .. }
            | Command::RemoveSelectionSet { .. }
            | Command::Undo
            | Command::Redo => {}
        }
        Ok(())
    }

    pub fn path(&self, path: &str, access: Access) -> R<std::path::PathBuf> {
        let p = self.perms.try_borrow().map_err(|_| rt("permissions are busy"))?;
        p.check(path, access).map_err(rt)
    }
}
