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
fn euler_filter_and_turntable() {
    let mut d = Document::new();
    let id = cube(&mut d);
    for (f, v) in [(0.0, 170.0), (10.0, -170.0)] {
        key(&mut d, id, "rotate.y", f, v);
    }
    let out = d.execute(Command::Anim(A::EulerFilter { ids: vec![id] })).unwrap();
    assert_eq!(out.warnings, vec!["1 keys changed".to_string()]);
    assert_eq!(d.scene.anim.curve(id, "rotate.y").unwrap().eval(10.0), Some(190.0));
    assert!(d.execute(Command::Anim(A::EulerFilter { ids: vec![] })).is_err());
    // A turntable replaces the spin axis: one turn over the range, repeating on both sides.
    d.execute(Command::Anim(A::Turntable { ids: vec![id], axis: "y".into(), turns: 1.0, start: Some(0.0), end: Some(48.0) })).unwrap();
    let c = d.scene.anim.curve(id, "rotate.y").unwrap();
    let base = c.eval(0.0).unwrap();
    assert!((base - 170.0).abs() < 1e-3, "the start pose keeps its plain angle, not a flipped triple: {base}");
    assert!((c.eval(12.0).unwrap() - base - 90.0).abs() < 1e-9);
    assert!((c.eval(60.0).unwrap() - base - 450.0).abs() < 1e-9, "keeps turning after the span");
    assert!((c.eval(-12.0).unwrap() - base + 90.0).abs() < 1e-9);
    assert_eq!(d.scene.anim.tracks().len(), 3, "all three angles are keyed");
    d.set_frame(12.0);
    let q = d.scene.get(id).unwrap().transform.rotation;
    let want = glam::Quat::from_rotation_y((base as f32 + 90.0).to_radians());
    assert!(q.dot(want).abs() > 0.9999, "a quarter turn the positive way: {q:?}");
    for bad in [
        A::Turntable { ids: vec![id], axis: "w".into(), turns: 1.0, start: None, end: None },
        A::Turntable { ids: vec![id], axis: "y".into(), turns: f64::NAN, start: None, end: None },
        A::Turntable { ids: vec![id], axis: "y".into(), turns: 1.0, start: Some(5.0), end: Some(5.0) },
        A::Turntable { ids: vec![ObjectId(9)], axis: "y".into(), turns: 1.0, start: None, end: None },
    ] {
        assert!(d.execute(Command::Anim(bad)).is_err());
    }
}

#[test]
fn point_cache_bakes_animated_deformation() {
    let mut d = Document::new();
    let id = cube(&mut d);
    let bend = d.execute(Command::AddDeformer { kind: "bend".into(), target: id }).unwrap().created[0];
    key(&mut d, bend, "deformer.angle", 0.0, 0.0);
    key(&mut d, bend, "deformer.angle", 10.0, 90.0);
    key(&mut d, id, "translate.x", 0.0, 0.0);
    key(&mut d, id, "translate.x", 10.0, 5.0);
    d.set_frame(3.0);
    let local = d.bake_points(id, 0.0, 10.0, 5.0, false).unwrap();
    assert_eq!((local.frames(), local.vertices, local.fps, local.step), (3, 8, 24.0, 5.0));
    assert_eq!(d.scene.anim.frame, 3.0, "the frame is restored");
    assert_ne!(local.frame(0), local.frame(2), "the bend deforms the points");
    let world = d.bake_points(id, 0.0, 10.0, 5.0, true).unwrap();
    let shift = world.frame(2).unwrap()[0] - local.frame(2).unwrap()[0];
    assert!((shift - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-4, "{shift:?}");
    let back = modelcraft_formats::pointcache::PointCache::from_bytes(&world.to_bytes().unwrap()).unwrap();
    assert_eq!(back, world);
    // A vertex count that changes is an error, as are bad spans.
    key(&mut d, id, "primitive.segments", 0.0, 1.0);
    key(&mut d, id, "primitive.segments", 10.0, 4.0);
    assert!(d.bake_points(id, 0.0, 10.0, 1.0, false).is_err());
    assert_eq!(d.scene.anim.frame, 3.0);
    assert!(d.bake_points(id, 5.0, 1.0, 1.0, false).is_err());
    assert!(d.bake_points(id, 0.0, 1.0, 0.0, false).is_err());
    assert!(d.bake_points(ObjectId(99), 0.0, 1.0, 1.0, false).is_err());
}

#[test]
fn layers_adjust_the_animation_below_without_changing_it() {
    use modelcraft_scene::LayerMode;
    let mut d = Document::new();
    let id = cube(&mut d);
    key(&mut d, id, "translate.x", 0.0, 0.0);
    key(&mut d, id, "translate.x", 10.0, 10.0);
    d.execute(Command::Anim(A::EditKey {
        key: key_ref(id, "translate.x", 0.0),
        value: None,
        interp: Some(Interp::Linear),
        tangent: None,
        in_slope: None,
        out_slope: None,
        in_weight: None,
        out_weight: None,
    }))
    .unwrap();
    let base = d.scene.anim.curve(id, "translate.x").unwrap().clone();
    // An adding layer: a constant offset of 2, then faded and muted.
    d.execute(Command::Anim(A::AddLayer { name: Some("Offset".into()), mode: LayerMode::Add })).unwrap();
    let layer = d.scene.anim.active_layer();
    assert_eq!((layer, d.scene.anim.layers()[0].name.as_str()), (1, "Offset"));
    assert!(d.scene.anim.curve(id, "translate.x").is_none(), "the layer starts empty");
    key(&mut d, id, "translate.x", 0.0, 2.0);
    d.set_frame(5.0);
    assert_eq!(x(&d, id), 7.0);
    d.execute(Command::Anim(A::SetLayer { layer, name: None, weight: Some(0.5), mute: None, mode: None, solo: None, lock: None })).unwrap();
    assert_eq!(x(&d, id), 6.0);
    d.execute(Command::Anim(A::SetLayer { layer, name: None, weight: None, mute: Some(true), mode: None, solo: None, lock: None })).unwrap();
    assert_eq!(x(&d, id), 5.0);
    d.execute(Command::Anim(A::SetLayer {
        layer,
        name: Some("Lift".into()),
        weight: Some(9.0),
        mute: Some(false),
        mode: Some(LayerMode::Override),
        solo: None,
        lock: None,
    }))
    .unwrap();
    assert_eq!(x(&d, id), 2.0, "an overriding layer at full weight replaces the base");
    // Keying the current pose in a layer makes the result equal it: move to 8 at frame 5.
    d.execute(Command::Anim(A::SetLayer { layer, name: None, weight: Some(0.5), mute: None, mode: Some(LayerMode::Add), solo: None, lock: None })).unwrap();
    d.execute(Command::Anim(A::SetAutoKey { on: true })).unwrap();
    d.execute(Command::SetTransform { id, transform: Transform { translation: Vec3::new(8.0, 0.0, 0.0), ..Default::default() } }).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.x").unwrap().eval(5.0), Some(6.0), "(8 − 5) / 0.5");
    d.set_frame(0.0);
    d.set_frame(5.0);
    assert_eq!(x(&d, id), 8.0);
    // A new attribute keyed in a layer gets a base key first, so the layer has something below.
    key(&mut d, id, "scale.y", 5.0, 0.5);
    assert_eq!(d.scene.anim.track_in(id, "scale.y", 0).map(|t| t.curve.len()), Some(1));
    assert_eq!(d.scene.get(id).unwrap().transform.scale.y, 1.25, "1 + 0.5 × 0.5");
    // The base is untouched; the base layer shows it again when made active.
    d.execute(Command::Anim(A::SetActiveLayer { layer: 0 })).unwrap();
    assert_eq!(d.scene.anim.curve(id, "translate.x").unwrap(), &base);
    // Flatten bakes the result into the base and removes the layers; the motion stays.
    let before: Vec<f64> = (0..=10).map(|f| d.scene.anim.value_at(id, "translate.x", f as f64).unwrap()).collect();
    d.execute(Command::Anim(A::FlattenLayers { step: 1.0 })).unwrap();
    assert!(d.scene.anim.layers().is_empty());
    let after: Vec<f64> = (0..=10).map(|f| d.scene.anim.value_at(id, "translate.x", f as f64).unwrap()).collect();
    assert!(before.iter().zip(&after).all(|(a, b)| (a - b).abs() < 1e-9), "{before:?} {after:?}");
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.anim.layers().len(), 1);
    // Errors.
    for bad in [
        A::RemoveLayer { layer: 42 },
        A::SetActiveLayer { layer: 42 },
        A::SetLayer { layer: 42, name: None, weight: None, mute: None, mode: None, solo: None, lock: None },
        A::SetLayer { layer, name: None, weight: Some(f64::NAN), mute: None, mode: None, solo: None, lock: None },
        A::FlattenLayers { step: 0.0 },
    ] {
        assert!(d.execute(Command::Anim(bad)).is_err());
    }
    d.execute(Command::Anim(A::RemoveLayer { layer })).unwrap();
    assert_eq!(d.scene.anim.active_layer(), 0);
    assert!(d.execute(Command::Anim(A::FlattenLayers { step: 1.0 })).is_err());
    // Layers survive a save.
    d.execute(Command::Anim(A::AddLayer { name: None, mode: LayerMode::Override })).unwrap();
    key(&mut d, id, "translate.x", 3.0, 1.0);
    let dir = std::env::temp_dir().join(format!("mc-layers-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("l.modelcraft");
    d.save_as(&p).unwrap();
    assert_eq!(Document::open(&p).unwrap().scene.anim, d.scene.anim);
    let _ = std::fs::remove_dir_all(dir);
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
    assert_eq!(crate::anim::COMMANDS.len(), 25);
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

#[test]
fn layers_solo_lock_keyed_weights_and_order() {
    let mut d = Document::new();
    let id = cube(&mut d);
    key(&mut d, id, "translate.x", 0.0, 1.0);
    let add = |d: &mut Document, mode| {
        d.execute(Command::Anim(A::AddLayer { name: None, mode })).unwrap();
        d.scene.anim.active_layer()
    };
    let a = add(&mut d, modelcraft_scene::LayerMode::Add);
    key(&mut d, id, "translate.x", 0.0, 10.0);
    let b = add(&mut d, modelcraft_scene::LayerMode::Add);
    key(&mut d, id, "translate.x", 0.0, 100.0);
    let x = |d: &mut Document, frame: f64| {
        d.set_frame(frame);
        d.scene.attr(id, "translate.x").unwrap()
    };
    assert_eq!(x(&mut d, 0.0), 111.0);
    let set = |d: &mut Document, json: &str| d.execute(serde_json::from_str(json).unwrap());
    // Solo: only that layer plays over the base.
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{a},"solo":true}}"#)).unwrap();
    assert_eq!(x(&mut d, 0.0), 11.0);
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{a},"solo":false}}"#)).unwrap();
    // A keyed weight fades the layer over time; clearing the keys gives the plain weight back.
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"key_layer_weight","layer":{b},"frame":0,"value":0}}"#)).unwrap();
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"key_layer_weight","layer":{b},"frame":10,"value":1}}"#)).unwrap();
    assert_eq!(x(&mut d, 0.0), 11.0);
    assert_eq!(x(&mut d, 10.0), 111.0);
    let half = x(&mut d, 5.0);
    assert!(half > 40.0 && half < 80.0, "{half}");
    // Keying a pose in a faded layer solves for the weight at that frame.
    d.set_frame(5.0);
    d.execute(Command::Anim(A::SetKey { id, path: "translate.x".into(), frame: None, value: None })).unwrap();
    assert!((x(&mut d, 5.0) - half).abs() < 1e-6);
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"key_layer_weight","layer":{b},"clear":true}}"#)).unwrap();
    assert!(d.scene.anim.layer(b).unwrap().weight_curve.is_none());
    // Lock: keys in the layer cannot be set, moved or removed, by command or by auto-key.
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{b},"lock":true}}"#)).unwrap();
    let before = d.scene.anim.clone();
    for bad in [
        format!(r#"{{"cmd":"anim","op":"set_key","id":{},"path":"translate.x","frame":3,"value":5}}"#, id.0),
        format!(r#"{{"cmd":"anim","op":"remove_track","id":{},"path":"translate.x"}}"#, id.0),
        format!(r#"{{"cmd":"anim","op":"key_transform","ids":[{}]}}"#, id.0),
    ] {
        let r = set(&mut d, &bad);
        assert!(r.is_err() || d.scene.anim == before, "{bad}");
        assert!(d.scene.anim == before, "{bad}");
    }
    d.execute(Command::Anim(A::SetAutoKey { on: true })).unwrap();
    d.execute(Command::SetTransform { id, transform: Transform { translation: Vec3::new(7.0, 0.0, 0.0), ..Default::default() } }).unwrap();
    assert_eq!(d.scene.anim.tracks(), before.tracks(), "auto-key does not write into a locked layer");
    d.execute(Command::Anim(A::SetAutoKey { on: false })).unwrap();
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{b},"lock":false}}"#)).unwrap();
    // Order: with a replacing layer the order decides the result.
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{a},"mode":"override"}}"#)).unwrap();
    d.scene.anim.clone_from(&{
        let mut an = d.scene.anim.clone();
        an.layer_mut(b).unwrap().weight = 1.0;
        an
    });
    let top_adds = x(&mut d, 0.0);
    assert_eq!(top_adds, 110.0, "a replaces the base with 10, then b adds 100");
    let out = set(&mut d, &format!(r#"{{"cmd":"anim","op":"move_layer","layer":{a},"delta":1}}"#)).unwrap();
    assert!(out.warnings.is_empty());
    assert_eq!(x(&mut d, 0.0), 10.0, "b adds first, then a replaces everything below");
    assert!(!set(&mut d, &format!(r#"{{"cmd":"anim","op":"move_layer","layer":{a},"delta":1}}"#)).unwrap().warnings.is_empty());
    assert!(set(&mut d, r#"{"cmd":"anim","op":"move_layer","layer":99,"delta":1}"#).is_err());
    assert!(set(&mut d, r#"{"cmd":"anim","op":"key_layer_weight","layer":99}"#).is_err());
    // Saved and read back.
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"set_layer","layer":{a},"solo":true,"lock":true}}"#)).unwrap();
    set(&mut d, &format!(r#"{{"cmd":"anim","op":"key_layer_weight","layer":{a},"frame":2,"value":0.5}}"#)).unwrap();
    let json = serde_json::to_string(&d.scene).unwrap();
    let back = serde_json::from_str::<modelcraft_scene::Scene>(&json).unwrap().repaired().unwrap();
    assert_eq!(back.anim, d.scene.anim);
}

#[test]
fn keys_are_pasted_with_their_shape_at_an_offset_or_onto_another_object() {
    use crate::anim::PastedKey;
    use modelcraft_anim::{Interp, Key};
    let mut d = Document::new();
    let a = cube(&mut d);
    let b = cube(&mut d);
    key(&mut d, a, "translate.x", 0.0, 1.0);
    key(&mut d, a, "translate.x", 10.0, 5.0);
    key(&mut d, a, "rotate.y", 10.0, 90.0);
    let copied: Vec<PastedKey> = ["translate.x", "rotate.y"]
        .iter()
        .flat_map(|p| d.scene.anim.curve(a, p).unwrap().keys.iter().map(|k| PastedKey { id: a, path: p.to_string(), key: *k }).collect::<Vec<_>>())
        .collect();
    assert_eq!(copied.len(), 3);
    // Twenty frames later on the same object.
    let out = d.execute(Command::Anim(A::PasteKeys { keys: copied.clone(), offset: 20.0, to: None })).unwrap();
    assert_eq!(out.warnings, vec!["3 keys pasted".to_string()]);
    let c = d.scene.anim.curve(a, "translate.x").unwrap();
    assert_eq!(c.keys.iter().map(|k| (k.t, k.v)).collect::<Vec<_>>(), vec![(0.0, 1.0), (10.0, 5.0), (20.0, 1.0), (30.0, 5.0)]);
    // A stepped key keeps its interpolation through the paste.
    let stepped = PastedKey { id: a, path: "translate.x".into(), key: Key::new(40.0, 9.0).with_interp(Interp::Stepped) };
    d.execute(Command::Anim(A::PasteKeys { keys: vec![stepped], offset: 0.0, to: None })).unwrap();
    assert_eq!(d.scene.anim.curve(a, "translate.x").unwrap().keys.last().map(|k| k.interp), Some(Interp::Stepped));
    // Onto another object: one undo step; pasting one angle keys all three there.
    d.execute(Command::Anim(A::PasteKeys { keys: copied.clone(), offset: 0.0, to: Some(b) })).unwrap();
    assert_eq!(d.scene.anim.curve(b, "translate.x").map(|c| c.len()), Some(2));
    assert!(d.scene.anim.curve(b, "rotate.x").is_some() && d.scene.anim.curve(b, "rotate.z").is_some());
    d.set_frame(10.0);
    assert!((d.scene.attr(b, "rotate.y").unwrap() - 90.0).abs() < 1e-3);
    d.execute(Command::Undo).unwrap();
    assert!(!d.scene.anim.is_animated(b));
    // Attributes the target does not have are skipped and counted; nothing fitting is an error.
    let odd = PastedKey { id: a, path: "primitive.nonsense".into(), key: Key::new(0.0, 1.0) };
    let mut mixed = copied.clone();
    mixed.push(odd.clone());
    assert_eq!(
        d.execute(Command::Anim(A::PasteKeys { keys: mixed, offset: 0.0, to: Some(b) })).unwrap().warnings,
        vec!["3 keys pasted, 1 skipped".to_string()]
    );
    assert!(d.execute(Command::Anim(A::PasteKeys { keys: vec![odd], offset: 0.0, to: Some(b) })).is_err());
    assert!(d.execute(Command::Anim(A::PasteKeys { keys: vec![], offset: 0.0, to: None })).is_err());
    assert!(d.execute(Command::Anim(A::PasteKeys { keys: copied, offset: f64::NAN, to: None })).is_err());
}

#[test]
fn any_attribute_is_set_by_path_without_a_key() {
    let mut d = Document::new();
    let id = cube(&mut d);
    d.execute(Command::Anim(A::SetAttr { id, path: "material.roughness".into(), value: 0.25 })).unwrap();
    assert_eq!(d.scene.attr(id, "material.roughness"), Some(0.25));
    assert!(d.scene.anim.is_empty(), "nothing was keyed");
    d.execute(Command::Undo).unwrap();
    assert_ne!(d.scene.attr(id, "material.roughness"), Some(0.25));
    assert!(d.execute(Command::Anim(A::SetAttr { id, path: "no.such".into(), value: 1.0 })).is_err());
    assert!(d.execute(Command::Anim(A::SetAttr { id: modelcraft_scene::ObjectId(999), path: "visible".into(), value: 1.0 })).is_err());
    assert!(d.execute(Command::Anim(A::SetAttr { id, path: "translate.x".into(), value: f64::NAN })).is_err());
}
