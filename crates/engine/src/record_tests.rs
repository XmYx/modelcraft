use crate::record::{MAX_RECORDED_COMMANDS, Recorded, RecordedCommand, Recorder};
use crate::{Command, Document};

fn step(name: &str, undoable: bool) -> Recorded {
    Recorded { label: None, commands: vec![RecordedCommand { command: Command::AddGroup { name: Some(name.into()) }, created: Vec::new() }], undoable }
}

#[test]
fn a_recording_stops_growing_at_its_cap_and_says_so() {
    let mut r = Recorder::new(0.0);
    for i in 0..MAX_RECORDED_COMMANDS + 50 {
        r.push(step(&i.to_string(), true));
    }
    assert_eq!(r.len(), MAX_RECORDED_COMMANDS);
    assert!(r.truncated);
    // Undo makes room again.
    r.undo();
    assert_eq!(r.len(), MAX_RECORDED_COMMANDS - 1);
    r.push(step("again", true));
    assert_eq!(r.len(), MAX_RECORDED_COMMANDS);
}

#[test]
fn undo_skips_steps_that_made_no_undo_step_and_a_new_step_clears_redo() {
    let mut r = Recorder::new(0.0);
    r.push(step("a", true));
    r.push(step("frame", false));
    r.undo();
    assert_eq!(r.len(), 1, "the frame change stays, the undoable step left");
    r.redo();
    assert_eq!(r.len(), 2);
    r.undo();
    r.push(step("b", true));
    r.redo();
    assert_eq!(r.len(), 2, "a new step cleared what could be redone");
    // Empty steps are never kept.
    let mut e = Recorder::new(0.0);
    e.push(Recorded { label: None, commands: Vec::new(), undoable: true });
    assert!(e.is_empty());
}

#[test]
fn a_group_closes_into_one_step_and_a_rollback_into_none() {
    let mut d = Document::new();
    d.start_recording();
    d.begin_group("outer");
    d.begin_group("inner");
    d.execute(Command::AddGroup { name: Some("x".into()) }).unwrap();
    d.end_group(false);
    d.execute(Command::AddGroup { name: Some("y".into()) }).unwrap();
    d.end_group(false);
    assert_eq!(d.recording_steps(), 1, "only the outermost group is a step");
    d.begin_group("failed");
    d.execute(Command::AddGroup { name: Some("z".into()) }).unwrap();
    d.end_group(true);
    assert_eq!(d.recording_steps(), 1);
    let steps = d.stop_recording().map(Recorder::into_steps).unwrap_or_default();
    assert_eq!(steps[0].commands.len(), 2);
}

#[test]
fn failed_commands_and_unchanged_state_are_not_recorded() {
    let mut d = Document::new();
    d.start_recording();
    assert!(d.execute(Command::Delete { ids: vec![crate::scene::ObjectId(99)] }).is_err());
    assert!(d.execute(Command::EndSculpt).is_ok());
    assert_eq!(d.recording_steps(), 1, "ending a sculpt session is kept: a replay needs it; the failed delete is not");
}
