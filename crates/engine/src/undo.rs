//! Undo history: unlimited in count, bounded in memory. Scene changes store the scene before and
//! after (cheap: meshes and layers are shared); sculpt strokes store only the vertices they moved.
//!
//! Every entry also keeps the command that made it, and the stack keeps the scene before its
//! oldest entry (`base`), so a save can store the history and the next session can rebuild it by
//! replaying those commands ([`crate::Document::open`]).

use std::collections::VecDeque;

use modelcraft_scene::{ObjectId, Scene};
use modelcraft_sculpt::StrokeUndo;

use crate::Command;

pub enum UndoEntry {
    Scene { label: String, cmd: Box<Command>, before: Box<Scene>, after: Box<Scene>, bytes: usize },
    Sculpt { label: String, cmd: Box<Command>, id: ObjectId, change: StrokeUndo },
}

impl UndoEntry {
    pub fn label(&self) -> &str {
        match self {
            UndoEntry::Scene { label, .. } | UndoEntry::Sculpt { label, .. } => label,
        }
    }
    /// The command that made this entry; replaying it redoes the entry.
    pub fn command(&self) -> &Command {
        match self {
            UndoEntry::Scene { cmd, .. } | UndoEntry::Sculpt { cmd, .. } => cmd,
        }
    }
    pub fn bytes(&self) -> usize {
        let cmd = command_bytes(self.command());
        match self {
            UndoEntry::Scene { bytes, .. } => *bytes + cmd,
            UndoEntry::Sculpt { change, .. } => change.heap_bytes() + 64 + cmd,
        }
    }
}

/// Rough heap size of a command: sculpt strokes carry their dabs, the rest is small.
fn command_bytes(c: &Command) -> usize {
    match c {
        Command::SculptStroke { stroke, .. } => 64 + stroke.dabs.len() * std::mem::size_of::<modelcraft_sculpt::Dab>(),
        _ => 64,
    }
}

/// Default memory budget of the history: 1 GiB.
pub const DEFAULT_BUDGET: usize = 1 << 30;

pub struct UndoStack {
    done: VecDeque<UndoEntry>,
    undone: Vec<UndoEntry>,
    bytes: usize,
    pub budget: usize,
    /// Entries dropped to stay within the budget.
    pub dropped: usize,
    /// The scene before the oldest entry, when known (unknown after a dropped sculpt entry).
    base: Option<Box<Scene>>,
}

impl Default for UndoStack {
    fn default() -> Self {
        UndoStack { done: VecDeque::new(), undone: Vec::new(), bytes: 0, budget: DEFAULT_BUDGET, dropped: 0, base: Some(Box::default()) }
    }
}

impl UndoStack {
    pub fn push(&mut self, e: UndoEntry) {
        self.undone.clear();
        self.bytes += e.bytes();
        self.done.push_back(e);
        while self.bytes > self.budget && self.done.len() > 1 {
            if let Some(old) = self.done.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.bytes());
                self.dropped += 1;
                // The scene after a scene entry is the scene before the next one.
                self.base = match old {
                    UndoEntry::Scene { after, .. } => Some(after),
                    UndoEntry::Sculpt { .. } => None,
                };
            }
        }
        self.recount();
    }
    fn recount(&mut self) {
        self.bytes = self.done.iter().chain(self.undone.iter()).map(UndoEntry::bytes).sum();
    }
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.done.pop_back()
    }
    pub fn push_redo(&mut self, e: UndoEntry) {
        self.undone.push(e);
        self.recount();
    }
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.undone.pop()
    }
    pub fn push_done(&mut self, e: UndoEntry) {
        self.done.push_back(e);
        self.recount();
    }
    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }
    pub fn len(&self) -> usize {
        self.done.len()
    }
    pub fn is_empty(&self) -> bool {
        self.done.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    /// Labels, oldest first.
    pub fn labels(&self) -> Vec<String> {
        self.done.iter().map(|e| e.label().to_string()).collect()
    }
    pub fn redo_labels(&self) -> Vec<String> {
        self.undone.iter().rev().map(|e| e.label().to_string()).collect()
    }
    /// Remove and return the newest `n` done entries, oldest of them first.
    pub fn take_last(&mut self, n: usize) -> Vec<UndoEntry> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            match self.done.pop_back() {
                Some(e) => v.push(e),
                None => break,
            }
        }
        v.reverse();
        self.recount();
        v
    }
    /// Forget everything that could be redone.
    pub fn clear_redo(&mut self) {
        self.undone.clear();
        self.recount();
    }
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
        self.bytes = 0;
    }

    /// Forget the history and start over from `base`.
    pub fn reset(&mut self, base: Scene) {
        self.clear();
        self.base = Some(Box::new(base));
    }

    pub(crate) fn set_base(&mut self, base: Scene) {
        self.base = Some(Box::new(base));
    }

    /// The history as a base scene and commands, for saving: `done` oldest first, `undone` the
    /// next redo first. When the scene before the oldest entry is unknown, the history starts
    /// at the oldest scene entry instead. `max_bytes` bounds the stored commands (the oldest
    /// are left out first, cutting only where the scene is known). `None` when nothing fits.
    pub fn persisted(&self, max_bytes: usize) -> Option<(Scene, Vec<Command>, Vec<Command>)> {
        let undone: Vec<Command> = self.undone.iter().rev().map(|e| e.command().clone()).collect();
        let mut total: usize = undone.iter().map(command_bytes).sum();
        if total > max_bytes {
            return None;
        }
        // Walk back from the newest entry; remember the oldest place where a cut is possible.
        let mut cut: Option<(usize, Scene)> = None;
        for (i, e) in self.done.iter().enumerate().rev() {
            total = total.saturating_add(command_bytes(e.command()));
            if total > max_bytes {
                break;
            }
            if i == 0
                && let Some(b) = &self.base
            {
                cut = Some((0, (**b).clone()));
                break;
            }
            if let UndoEntry::Scene { before, .. } = e {
                cut = Some((i, (**before).clone()));
            }
        }
        if self.done.is_empty() {
            cut = self.base.as_ref().map(|b| (0, (**b).clone()));
        }
        let (start, base) = cut?;
        let done = self.done.iter().skip(start).map(|e| e.command().clone()).collect();
        Some((base, done, undone))
    }
}
