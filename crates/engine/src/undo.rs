//! Undo history: unlimited in count, bounded in memory. Scene changes store the scene before and
//! after (cheap: meshes and layers are shared); sculpt strokes store only the vertices they moved.

use std::collections::VecDeque;

use modelcraft_scene::{ObjectId, Scene};
use modelcraft_sculpt::StrokeUndo;

pub enum UndoEntry {
    Scene { label: String, before: Box<Scene>, after: Box<Scene>, bytes: usize },
    Sculpt { label: String, id: ObjectId, change: StrokeUndo },
}

impl UndoEntry {
    pub fn label(&self) -> &str {
        match self {
            UndoEntry::Scene { label, .. } | UndoEntry::Sculpt { label, .. } => label,
        }
    }
    pub fn bytes(&self) -> usize {
        match self {
            UndoEntry::Scene { bytes, .. } => *bytes,
            UndoEntry::Sculpt { change, .. } => change.heap_bytes() + 64,
        }
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
}

impl Default for UndoStack {
    fn default() -> Self {
        UndoStack { done: VecDeque::new(), undone: Vec::new(), bytes: 0, budget: DEFAULT_BUDGET, dropped: 0 }
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
    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
        self.bytes = 0;
    }
}
