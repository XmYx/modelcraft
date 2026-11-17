use glam::Vec3;
use modelcraft_anim::{Extrapolation, Interp, Tangent};
use modelcraft_scene::{ObjectId, Transform};

use crate::anim::{AnimCommand as A, KeyRef, key_ref};
use crate::{Command, Document};

fn cube(d: &mut Document) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0]
}

fn key(d: &mut Document, id: ObjectId, path: &str, frame: f64, value: f64) {
    d.execute(Command::Anim(A::SetKey { id, path: path.into(), frame: Some(frame), value: Some(value) })).unwrap();
}

fn x(d: &Document, id: ObjectId) -> f32 {
    d.scene.get(id).unwrap().transform.translation.x
}

#[test]
fn keys_pose_the_scene_and_undo_restores_them() {
    let mut d = Document::new();
    let id = cube(&mut d);
    key(&mut d, id, "translate.x", 0.0, 0.0);
    key(&mut d, id, "translate.x", 10.0, 10.0);
    assert_eq!(d.undo.len(), 3);
    d.execute(Command::Anim(A::SetTime { frame: 10.0 })).unwrap();
    assert_eq!(d.undo.len(), 3, "going to a frame is not an undo step");
    assert_eq!(x(&d, id), 10.0);
    // Undo removes the second key; the frame stays where the user put it.
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.anim.frame, 10.0);
    assert_eq!(x(&d, id), 0.0, "one key left: the curve holds its value");
    d.execute(Command::Redo).unwrap();
    assert_eq!(x(&d, id), 10.0);
    d.set_frame(5.0);
    assert!((x(&d, id) - 5.0).abs() < 1e-5, "auto tangents with flat ends: symmetric ease");
}

#[test]
fn key_editing_commands() {
    let mut d = Document::new();
    let id = cube(&mut d);
    for (f, v) in [(0.0, 0.0), (10.0, 4.0), (20.0, 0.0)] {
        key(&mut d, id, "translate.y", f, v);
    }
    let k = |f: f64| key_ref(id, "translate.y", f);
    d.execute(Command::Anim(A::MoveKeys { keys: vec![k(10.0)], frames: 2.0, value: 1.0 })).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.y").unwrap().eval(12.0), Some(5.0));
    assert!(d.execute(Command::Anim(A::MoveKeys { keys: vec![k(10.0)], frames: 1.0, value: 0.0 })).is_err(), "no key there any more");
    d.execute(Command::Anim(A::ScaleKeys { keys: vec![k(0.0), k(12.0), k(20.0)], pivot: 0.0, time: 2.0, value_pivot: 0.0, value: 1.0 })).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.y").unwrap().time_range(), Some((0.0, 40.0)));
    d.execute(Command::Anim(A::EditKey {
        key: k(24.0),
        value: Some(8.0),
        interp: Some(Interp::Linear),
        tangent: None,
        in_slope: Some(0.5),
        out_slope: None,
        in_weight: Some(7.0),
        out_weight: None,
    }))
    .unwrap();
    let c = d.scene.anim.curve(id, "translate.y").unwrap();
    let edited = c.keys[1];
    assert_eq!(
        (edited.v, edited.interp, edited.tangent, edited.in_slope, edited.out_slope, edited.in_weight),
        (8.0, Interp::Linear, Tangent::Free, 0.5, 0.5, 1.0)
    );
    d.execute(Command::Anim(A::SetKeyShape { keys: vec![k(0.0), k(24.0)], interp: Some(Interp::Stepped), tangent: Some(Tangent::Flat) })).unwrap();
    d.execute(Command::Anim(A::SetExtrapolation { id, path: "translate.y".into(), pre: None, post: Some(Extrapolation::Cycle) })).unwrap();
    d.set_frame(41.0);
    assert_eq!(d.scene.get(id).unwrap().transform.translation.y, 0.0, "stepped, cycling");
    d.execute(Command::Anim(A::BakeTrack { id, path: "translate.y".into(), start: Some(0.0), end: Some(40.0), step: 1.0 })).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.y").unwrap().len(), 41);
    let out = d.execute(Command::Anim(A::SimplifyTrack { id, path: "translate.y".into(), tolerance: 0.0 })).unwrap();
    assert_eq!(out.warnings.len(), 1);
    d.execute(Command::Anim(A::RemoveKeys { keys: vec![k(0.0)] })).unwrap();
    d.execute(Command::Anim(A::RemoveTrack { id, path: None })).unwrap();
    assert!(d.scene.anim.is_empty());
    // Errors leave no trace.
    let n = d.undo.len();
    assert!(d.execute(Command::Anim(A::SetKey { id, path: "nope".into(), frame: None, value: None })).is_err());
    assert!(d.execute(Command::Anim(A::SetKey { id: ObjectId(77), path: "translate.x".into(), frame: None, value: None })).is_err());
    assert!(d.execute(Command::Anim(A::RemoveTrack { id, path: None })).is_err());
    assert!(d.execute(Command::Anim(A::SetTimeRange { start: Some(10.0), end: Some(1.0), fps: None })).is_err());
    assert!(d.execute(Command::Anim(A::SetTime { frame: f64::NAN })).is_err());
    assert!(d.execute(Command::Anim(A::KeyTransform { ids: vec![], frame: None, translate: true, rotate: true, scale: true })).is_err());
    assert_eq!(d.undo.len(), n);
    assert!(d.scene.anim.is_empty());
}

#[test]
fn auto_key_turns_edits_into_keys() {
    let mut d = Document::new();
    let id = cube(&mut d);
    d.execute(Command::Anim(A::KeyTransform { ids: vec![id], frame: None, translate: true, rotate: false, scale: false })).unwrap();
    assert_eq!(d.scene.anim.tracks().len(), 3);
    d.execute(Command::Anim(A::SetAutoKey { on: true })).unwrap();
    d.set_frame(24.0);
    let t = Transform { translation: Vec3::new(4.0, 0.0, 0.0), ..Default::default() };
    let n = d.undo.len();
    d.execute(Command::SetTransform { id, transform: t }).unwrap();
    assert_eq!(d.undo.len(), n + 1, "the edit and its key are one undo step");
    assert_eq!(d.scene.anim.curve(id, "translate.x").unwrap().len(), 2);
    d.set_frame(0.0);
    assert_eq!(x(&d, id), 0.0);
    d.set_frame(24.0);
    assert_eq!(x(&d, id), 4.0);
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.x").unwrap().len(), 1);
    // Without auto-key an edit lasts until the frame changes.
    d.execute(Command::Anim(A::SetAutoKey { on: false })).unwrap();
    d.execute(Command::SetTransform { id, transform: t }).unwrap();
    assert_eq!(x(&d, id), 4.0);
    d.set_frame(1.0);
    assert_eq!(x(&d, id), 0.0);
}

#[test]
fn commands_are_json_and_round_trip() {
    let c: Command = serde_json::from_str(r#"{"cmd":"anim","op":"set_key","id":1,"path":"translate.x","frame":12,"value":2.5}"#).unwrap();
    assert_eq!(c, Command::Anim(A::SetKey { id: ObjectId(1), path: "translate.x".into(), frame: Some(12.0), value: Some(2.5) }));
    let c: Command = serde_json::from_str(r#"{"cmd":"anim","op":"key_transform","ids":[1]}"#).unwrap();
    assert_eq!(c, Command::Anim(A::KeyTransform { ids: vec![ObjectId(1)], frame: None, translate: true, rotate: true, scale: true }));
    let c: Command = serde_json::from_str(r#"{"cmd":"anim","op":"edit_key","id":1,"path":"scale.x","frame":3,"tangent":"flat"}"#).unwrap();
    assert!(matches!(c, Command::Anim(A::EditKey { key: KeyRef { frame, .. }, tangent: Some(Tangent::Flat), .. }) if frame == 3.0));
    for c in [
        Command::Anim(A::SetTime { frame: 4.0 }),
        Command::Anim(A::MoveKeys { keys: vec![key_ref(ObjectId(2), "visible", 1.0)], frames: 1.0, value: 0.0 }),
        Command::Anim(A::SetTimeRange { start: Some(1.0), end: None, fps: Some(30.0) }),
        Command::Undo,
        Command::AddGroup { name: None },
    ] {
        let j = serde_json::to_string(&c).unwrap();
        assert!(j.starts_with(r#"{"cmd":""#), "{j}");
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c);
    }
    // Errors name what is wrong.
    let e = serde_json::from_str::<Command>(r#"{"cmd":"no_such_command"}"#).unwrap_err().to_string();
    assert!(e.contains("unknown variant `no_such_command`"), "{e}");
    let e = serde_json::from_str::<Command>(r#"{"cmd":"anim","op":"set_key","id":1}"#).unwrap_err().to_string();
    assert!(e.contains("missing field `path`"), "{e}");
    let e = serde_json::from_str::<Command>(r#"{"cmd":"anim","op":"wobble"}"#).unwrap_err().to_string();
    assert!(e.contains("unknown variant `wobble`"), "{e}");
    assert_eq!(crate::anim::COMMANDS.len(), 14);
    // Every listed name parses as a command tag.
    for (name, _) in crate::anim::COMMANDS {
        let e = serde_json::from_str::<A>(&format!(r#"{{"op":"{name}"}}"#)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(!e.contains("unknown variant"), "{name}: {e}");
    }
}

#[test]
fn animation_survives_save_and_crash_recovery() {
    let dir = std::env::temp_dir().join(format!("mc-anim-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("a.modelcraft");
    let mut d = Document::new();
    let id = cube(&mut d);
    key(&mut d, id, "translate.x", 0.0, 0.0);
    d.execute(Command::Anim(A::SetTimeRange { start: Some(0.0), end: Some(48.0), fps: Some(30.0) })).unwrap();
    d.save_as(&p).unwrap();
    // Unsaved: scrub (not journaled by itself), then key the current frame and auto-key an edit.
    d.set_frame(30.0);
    d.execute(Command::Anim(A::SetKey { id, path: "translate.x".into(), frame: None, value: Some(6.0) })).unwrap();
    d.execute(Command::Anim(A::SetAutoKey { on: true })).unwrap();
    d.set_frame(40.0);
    d.execute(Command::SetTransform { id, transform: Transform { translation: Vec3::new(9.0, 1.0, 0.0), ..Default::default() } }).unwrap();
    let want = d.scene.anim.clone();
    drop(d);
    let mut r = Document::open(&p).unwrap();
    assert_eq!(r.scene.anim.time.fps, 30.0);
    assert_eq!(r.scene.anim.curve(id, "translate.x").unwrap().len(), 1);
    assert!(r.pending_recovery > 0);
    r.recover().unwrap();
    assert_eq!(r.scene.anim, want, "the journal replays keys at the frames they were set");
    r.save_as(&p).unwrap();
    let again = Document::open(&p).unwrap();
    assert_eq!(again.scene.anim, want);
    let _ = std::fs::remove_dir_all(dir);
}
