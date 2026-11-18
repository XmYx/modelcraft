//! Recording what a user does, for macros: the commands that made undo steps (and the few that
//! did not but a replay needs), with the objects each created, so a script can name them.
//!
//! The document feeds a [`Recorder`] while one is running ([`crate::Document::start_recording`]).
//! The record follows the undo history: an Undo takes the last step back off the record and a
//! Redo puts it back, so what is recorded is what the user kept. The steps a script's
//! `undo_group` makes are recorded as one step.

use modelcraft_scene::ObjectId;

use crate::Command;

/// Most commands one recording keeps; later ones are dropped and the record says so.
pub const MAX_RECORDED_COMMANDS: usize = 20_000;

/// One command of a step and the objects it created (for later commands to refer to).
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedCommand {
    pub command: Command,
    pub created: Vec<ObjectId>,
}

/// One user-visible step: a command, or the commands of an undo group.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// The undo group's name (none for a single command).
    pub label: Option<String>,
    pub commands: Vec<RecordedCommand>,
    /// It made an undo step (Undo takes it back). Frame changes and starting or ending a sculpt
    /// session are recorded without one, because a replay needs them.
    pub undoable: bool,
}

/// The record of one recording.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    steps: Vec<Recorded>,
    redo: Vec<Recorded>,
    /// Commands of the undo group being run, until it closes.
    group: Vec<RecordedCommand>,
    /// The frame the record last noted (a frame change is recorded before a command that may
    /// depend on it).
    frame: f64,
    count: usize,
    /// Commands were dropped after [`MAX_RECORDED_COMMANDS`].
    pub truncated: bool,
    /// Nothing is recorded while paused (callbacks run).
    pub paused: bool,
}

impl Recorder {
    pub fn new(frame: f64) -> Self {
        Recorder { frame, ..Recorder::default() }
    }

    /// Steps recorded so far.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn steps(&self) -> &[Recorded] {
        &self.steps
    }

    pub fn into_steps(self) -> Vec<Recorded> {
        self.steps
    }

    /// The frame the record last noted.
    pub(crate) fn frame(&self) -> f64 {
        self.frame
    }

    pub(crate) fn note_frame(&mut self, frame: f64) {
        self.frame = frame;
    }

    fn room(&mut self, n: usize) -> bool {
        if self.count + n > MAX_RECORDED_COMMANDS {
            self.truncated = true;
            return false;
        }
        self.count += n;
        true
    }

    pub(crate) fn push(&mut self, step: Recorded) {
        if step.commands.is_empty() || !self.room(step.commands.len()) {
            return;
        }
        if step.undoable {
            self.redo.clear();
        }
        self.steps.push(step);
    }

    /// A command that ran inside an undo group, kept until the group closes.
    pub(crate) fn in_group(&mut self, command: Command, created: Vec<ObjectId>) {
        if self.group.len() < MAX_RECORDED_COMMANDS {
            self.group.push(RecordedCommand { command, created });
        } else {
            self.truncated = true;
        }
    }

    /// The outermost undo group closed: record it (or forget it when it was rolled back).
    pub(crate) fn close_group(&mut self, label: &str, keep: bool, undoable: bool) {
        let commands = std::mem::take(&mut self.group);
        if keep {
            self.push(Recorded { label: Some(label.to_string()), commands, undoable });
        }
    }

    /// Undo: the last step that made an undo step leaves the record (and can come back).
    pub(crate) fn undo(&mut self) {
        if let Some(i) = self.steps.iter().rposition(|s| s.undoable) {
            let step = self.steps.remove(i);
            self.count = self.count.saturating_sub(step.commands.len());
            self.redo.push(step);
        }
    }

    pub(crate) fn redo(&mut self) {
        if let Some(step) = self.redo.pop() {
            self.count += step.commands.len();
            self.steps.push(step);
        }
    }
}
