//! Recorded macros: what the document records, the script written from it, and replaying it.

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{Command, Document, Recorded};
use modelcraft_script::macros::{MacroError, macro_script, record_commands};
use modelcraft_script::{Limits, Permissions, RunContext, RunReport, Session};
use proptest::prelude::*;
use serde_json::json;

fn cmd(v: serde_json::Value) -> Command {
    serde_json::from_value(v).unwrap_or_else(|e| panic!("not a command: {e}"))
}

fn add(d: &mut Document, kind: &str, name: &str) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: kind.into(), name: Some(name.into()), parent: None }).unwrap().created[0]
}

/// What a scene is, independent of object numbers: name, parent name, transform, face count.
fn shape(d: &mut Document) -> Vec<(String, Option<String>, [f32; 3], [f32; 3], usize)> {
    let objs: Vec<_> = d.scene.objects().to_vec();
    let mut v = Vec::new();
    for o in &objs {
        let parent = o.parent.and_then(|p| d.scene.get(p)).map(|p| p.name.clone());
        let faces = d.eval.evaluate(&d.scene, o.id).mesh.face_count();
        v.push((o.name.clone(), parent, o.transform.translation.to_array(), o.transform.scale.to_array(), faces));
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn run(doc: &mut Document, selection: Vec<ObjectId>, text: &str) -> RunReport {
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext { selection };
    s.run(doc, &mut ctx, "macro.rhai", text)
}

fn steps(d: &mut Document) -> Vec<Recorded> {
    d.stop_recording().expect("was recording").into_steps()
}

#[test]
fn what_the_user_does_is_recorded_and_replays_to_the_same_scene() {
    let mut d = Document::new();
    d.start_recording();
    let a = add(&mut d, "cube", "Base");
    let b = add(&mut d, "sphere", "Ball");
    d.execute(cmd(json!({"cmd":"set_transform","id":a.0,"transform":{"translation":[1.0,2.0,3.0],"rotation":[0.0,0.0,0.0,1.0],"scale":[2.0,2.0,2.0]}}))).unwrap();
    d.execute(Command::Reparent { id: b, parent: Some(a) }).unwrap();
    d.execute(Command::Rename { id: b, name: "Ball 2".into() }).unwrap();
    d.execute(cmd(json!({"cmd":"add_step","id":a.0,"op":{"op":"subdivide","levels":1}}))).unwrap();
    let recorded = steps(&mut d);
    assert_eq!(recorded.len(), 6);
    let text = macro_script("Build", &recorded, &[]).unwrap();
    assert!(text.contains("undo_group(\"Macro: Build\""), "{text}");
    assert!(text.contains("let r1 = cmd::add_primitive"), "created objects that later commands use are bound: {text}");
    assert!(text.contains("id: r1[0]"), "{text}");
    // Replay on an empty document (where the numbers may differ), and on one that already has objects.
    let mut fresh = Document::new();
    let r = run(&mut fresh, vec![], &text);
    assert!(r.ok(), "{:?}\n{text}", r.error);
    assert_eq!(shape(&mut fresh), shape(&mut d));
    let mut busy = Document::new();
    add(&mut busy, "cube", "Existing");
    add(&mut busy, "cube", "Existing 2");
    let r = run(&mut busy, vec![], &text);
    assert!(r.ok(), "{:?}", r.error);
    let mut expected = shape(&mut d);
    expected.push(("Existing".into(), None, [0.0; 3], [1.0; 3], 6));
    expected.push(("Existing 2".into(), None, [0.0; 3], [1.0; 3], 6));
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(shape(&mut busy), expected);
    // One replay is one undo step.
    let steps_before = busy.undo.len();
    run(&mut busy, vec![], &text);
    assert_eq!(busy.undo.len(), steps_before + 1);
    busy.execute(Command::Undo).unwrap();
    assert_eq!(busy.undo.len(), steps_before);
}

#[test]
fn undo_and_redo_while_recording_change_what_is_recorded() {
    let mut d = Document::new();
    d.start_recording();
    add(&mut d, "cube", "Keep");
    add(&mut d, "cube", "Oops");
    d.execute(Command::Undo).unwrap();
    add(&mut d, "sphere", "Instead");
    let r = steps(&mut d);
    let text = macro_script("m", &r, &[]).unwrap();
    assert!(text.contains("Keep") && text.contains("Instead") && !text.contains("Oops"), "{text}");
    // Redo puts an undone step back.
    let mut d = Document::new();
    d.start_recording();
    add(&mut d, "cube", "One");
    add(&mut d, "cube", "Two");
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.stop_recording().map(|r| r.len()), Some(1));
    let mut d = Document::new();
    d.start_recording();
    add(&mut d, "cube", "One");
    add(&mut d, "cube", "Two");
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Redo).unwrap();
    assert_eq!(steps(&mut d).len(), 2);
}

#[test]
fn objects_that_were_selected_become_the_selection_of_the_replay() {
    let mut d = Document::new();
    let a = add(&mut d, "cube", "Mover");
    let b = add(&mut d, "cube", "Other");
    d.start_recording();
    d.execute(Command::Rename { id: a, name: "Moved".into() }).unwrap();
    d.execute(Command::SetVisible { id: b, visible: false }).unwrap();
    d.execute(Command::Rename { id: ObjectId(a.0 + 100), name: "x".into() }).unwrap_err();
    let rec = steps(&mut d);
    let text = macro_script("rename", &rec, &[b, a]).unwrap();
    assert!(text.contains("sel[1]") && text.contains("sel[0]"), "{text}");
    assert!(text.contains("select 2 object(s) first"), "{text}");
    // A scene whose objects have other numbers: the selection says which is which.
    let mut other = Document::new();
    add(&mut other, "sphere", "Pad");
    let x = add(&mut other, "cube", "X");
    let y = add(&mut other, "cube", "Y");
    let r = run(&mut other, vec![y, x], &text);
    assert!(r.ok(), "{:?}", r.error);
    let names: Vec<_> = other.scene.objects().iter().map(|o| (o.name.clone(), o.visible)).collect();
    assert!(names.contains(&("Moved".into(), true)) && names.contains(&("Y".into(), false)), "{names:?}");
    // Without the selection the macro says so instead of touching something else.
    let mut empty = Document::new();
    add(&mut empty, "cube", "Z");
    let r = run(&mut empty, vec![], &text);
    assert!(r.error.as_ref().is_some_and(|e| e.message.contains("select 2 object(s) first")), "{:?}", r.error);
    assert_eq!(empty.scene.objects()[0].name, "Z");
}

#[test]
fn an_undo_group_is_one_recorded_step_and_a_rolled_back_one_is_none() {
    let mut d = Document::new();
    d.start_recording();
    d.begin_group("Two cubes");
    add(&mut d, "cube", "G1");
    add(&mut d, "cube", "G2");
    d.end_group(false);
    d.begin_group("Never");
    add(&mut d, "cube", "Gone");
    d.end_group(true);
    let r = steps(&mut d);
    assert_eq!(r.len(), 1, "{r:?}");
    assert_eq!(r[0].label.as_deref(), Some("Two cubes"));
    assert_eq!(r[0].commands.len(), 2);
    let text = macro_script("g", &r, &[]).unwrap();
    assert!(text.contains("// Two cubes") && !text.contains("Gone"), "{text}");
    // A script that ran while recording is recorded as its commands, replayable.
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut e = Document::new();
    e.start_recording();
    let rep = s.run(&mut e, &mut RunContext::default(), "x.rhai", "undo_group(\"Row\", || { for i in 0..3 { move_to(add_primitive(\"cube\", \"c\" + i), i, 0, 0); } });");
    assert!(rep.ok(), "{:?}", rep.error);
    let r = steps(&mut e);
    assert_eq!(r.len(), 1);
    let text = macro_script("row", &r, &[]).unwrap();
    let mut again = Document::new();
    assert!(run(&mut again, vec![], &text).ok());
    assert_eq!(shape(&mut again), shape(&mut e));
}

#[test]
fn nothing_is_recorded_while_paused_or_when_not_recording() {
    let mut d = Document::new();
    add(&mut d, "cube", "before");
    assert!(!d.is_recording());
    d.start_recording();
    assert!(d.is_recording());
    d.pause_recording(true);
    add(&mut d, "cube", "callback");
    d.pause_recording(false);
    add(&mut d, "cube", "user");
    assert_eq!(d.recording_steps(), 1);
    let r = steps(&mut d);
    assert!(macro_script("m", &r, &[]).unwrap().contains("user"));
    assert!(!d.is_recording());
    assert!(d.stop_recording().is_none());
    assert_eq!(macro_script("m", &[], &[]), Err(MacroError::Empty));
}

#[test]
fn a_frame_change_is_recorded_before_the_command_that_may_depend_on_it() {
    let mut d = Document::new();
    let a = add(&mut d, "cube", "Key me");
    d.start_recording();
    d.set_frame(12.0);
    d.execute(cmd(json!({"cmd":"anim","op":"set_key","id":a.0,"path":"translate.x","frame":12.0,"value":3.0}))).unwrap();
    let r = steps(&mut d);
    assert_eq!(r.len(), 2, "{r:?}");
    let text = macro_script("key", &r, &[a]).unwrap();
    assert!(text.contains("op: \"set_time\"") && text.contains("op: \"set_key\"") && text.contains("sel[0]"), "{text}");
    let mut other = Document::new();
    let o = add(&mut other, "cube", "Key me");
    let rep = run(&mut other, vec![o], &text);
    assert!(rep.ok(), "{:?}\n{text}", rep.error);
    assert_eq!(other.scene.anim.frame, 12.0);
    assert!(!other.scene.anim.is_empty());
}

#[test]
fn a_journal_or_list_of_commands_becomes_a_macro_by_replaying_it() {
    let list = vec![
        cmd(json!({"cmd":"add_primitive","kind":"cube","name":"A"})),
        cmd(json!({"cmd":"add_primitive","kind":"cube","name":"B"})),
        cmd(json!({"cmd":"reparent","id":2,"parent":1})),
        cmd(json!({"cmd":"set_visible","id":2,"visible":false})),
    ];
    let rec = record_commands(&list).unwrap();
    assert_eq!(rec.len(), 4);
    let text = macro_script("list", &rec, &[]).unwrap();
    let mut d = Document::new();
    add(&mut d, "sphere", "Pad");
    let r = run(&mut d, vec![], &text);
    assert!(r.ok(), "{:?}\n{text}", r.error);
    let b = d.scene.objects().iter().find(|o| o.name == "B").expect("B");
    assert!(!b.visible);
    assert_eq!(d.scene.get(b.parent.expect("parent")).map(|p| p.name.as_str()), Some("A"));
    // A command that fails says which one.
    let bad = vec![cmd(json!({"cmd":"rename","id":99,"name":"x"}))];
    assert!(record_commands(&bad).unwrap_err().contains("command 1"));
}

#[test]
fn awkward_text_and_numbers_survive_the_script() {
    let mut d = Document::new();
    d.start_recording();
    let a = add(&mut d, "cube", "He said \"hi\"\\ \n tab\t and ünï © 日本");
    d.execute(cmd(json!({"cmd":"set_transform","id":a.0,"transform":{"translation":[1e-7,-0.0,123456789.5],"rotation":[0.0,0.0,0.0,1.0],"scale":[1e-12,1.0,3.0]}}))).unwrap();
    let r = steps(&mut d);
    let text = macro_script("odd \"name\"\nwith newline", &r, &[]).unwrap();
    assert!(text.lines().count() > 3);
    let mut again = Document::new();
    let rep = run(&mut again, vec![], &text);
    assert!(rep.ok(), "{:?}\n{text}", rep.error);
    let (x, y) = (&d.scene.objects()[0], &again.scene.objects()[0]);
    assert_eq!(x.name, y.name);
    assert!((x.transform.translation.x - y.transform.translation.x).abs() < 1e-9, "{} vs {}", x.transform.translation.x, y.transform.translation.x);
    assert_eq!(x.transform.translation.z, y.transform.translation.z);
}

#[test]
fn a_huge_recording_is_refused_not_written() {
    let mut d = Document::new();
    d.start_recording();
    add(&mut d, "cube", &"n".repeat(60_000));
    let r = steps(&mut d);
    // Fine at its own size; a list of thousands of such steps is over the limit.
    let big: Vec<Recorded> = (0..200).flat_map(|_| r.clone()).collect();
    assert!(macro_script("big", &r, &[]).is_ok());
    assert!(matches!(macro_script("big", &big, &[]), Err(MacroError::TooLarge)));
}

proptest! {
    /// Whatever a user does, replaying the recorded macro on an empty scene gives the same scene.
    #[test]
    fn replaying_a_recording_reproduces_the_scene(ops in proptest::collection::vec((0u8..7, 0u8..5, -5.0f32..5.0), 1..25)) {
        let mut d = Document::new();
        d.start_recording();
        let mut ids: Vec<ObjectId> = Vec::new();
        for (i, (kind, which, x)) in ops.iter().enumerate() {
            let pick = ids.get(usize::from(*which) % ids.len().max(1)).copied();
            match (kind, pick) {
                (0, _) => { ids.push(add(&mut d, ["cube", "sphere", "cylinder"][usize::from(*which) % 3], &format!("o{i}"))); }
                (1, Some(id)) => { let _ = d.execute(Command::Rename { id, name: format!("n{i}") }); }
                (2, Some(id)) => { let _ = d.execute(cmd(json!({"cmd":"set_transform","id":id.0,"transform":{"translation":[x, 0.5, -x],"rotation":[0.0,0.0,0.0,1.0],"scale":[1.0,1.0,1.0]}}))); }
                (3, Some(id)) => { let _ = d.execute(Command::SetVisible { id, visible: *x > 0.0 }); }
                (4, _) => { let _ = d.execute(Command::Undo); }
                (5, _) => { let _ = d.execute(Command::Redo); }
                (6, Some(id)) => { ids.push(d.execute(Command::Duplicate { id }).map(|o| o.created[0]).unwrap_or(id)); }
                _ => {}
            }
            ids.retain(|id| d.scene.get(*id).is_some());
        }
        let rec = steps(&mut d);
        if rec.is_empty() { return Ok(()); }
        let text = macro_script("p", &rec, &[]).unwrap();
        let mut again = Document::new();
        let rep = run(&mut again, vec![], &text);
        prop_assert!(rep.ok(), "{:?}\n{}", rep.error, text);
        prop_assert_eq!(shape(&mut again), shape(&mut d));
    }
}
