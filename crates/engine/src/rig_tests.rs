use glam::{Quat, Vec3};
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{ObjectId, ObjectKind, Transform};

use crate::rig::{BindMethod, RigCommand as R, WeightOp};
use crate::{Command, Document};

fn joint(d: &mut Document, parent: Option<ObjectId>, at: [f32; 3], name: &str) -> ObjectId {
    d.execute(Command::Rig(R::AddJoint { parent, position: Some(at), name: Some(name.into()) })).unwrap().created[0]
}

fn pos(d: &Document, id: ObjectId) -> Vec3 {
    d.scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

/// A cylinder along Y from −2 to 2 and a three-joint chain up its middle.
fn arm(d: &mut Document) -> (ObjectId, [ObjectId; 3]) {
    let mesh = d.execute(Command::AddPrimitive { kind: "cylinder".into(), name: Some("Arm".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id: mesh, primitive: Primitive::Cylinder { radius: 0.4, height: 4.0, sides: 12, rings: 16, caps: true } }).unwrap();
    let a = joint(d, None, [0.0, -2.0, 0.0], "Shoulder");
    let b = joint(d, Some(a), [0.0, 0.0, 0.0], "Elbow");
    let c = joint(d, Some(b), [0.0, 2.0, 0.0], "Wrist");
    (mesh, [a, b, c])
}

#[test]
fn joints_are_placed_oriented_and_mirrored() {
    let mut d = Document::new();
    let hip = joint(&mut d, None, [1.0, 3.0, 0.0], "Hip_L");
    let knee = joint(&mut d, Some(hip), [1.2, 1.5, 0.3], "Knee_L");
    let foot = joint(&mut d, Some(knee), [1.0, 0.0, 0.0], "Foot_L");
    assert!((pos(&d, knee) - Vec3::new(1.2, 1.5, 0.3)).length() < 1e-5, "world position under a parent");
    let default = d.execute(Command::Rig(R::AddJoint { parent: Some(foot), position: None, name: None })).unwrap().created[0];
    assert_eq!(d.scene.get(default).unwrap().transform.translation, Vec3::Y);
    assert_eq!(d.scene.get(default).unwrap().name, "Joint");
    d.execute(Command::Delete { ids: vec![default] }).unwrap();
    // Orient: Y of each joint points at its child; nothing moves.
    let before = [pos(&d, hip), pos(&d, knee), pos(&d, foot)];
    let out = d.execute(Command::Rig(R::OrientJoints { ids: vec![hip], aim: "y".into(), up: "z".into(), children: true })).unwrap();
    assert_eq!(out.warnings, vec!["3 joints oriented".to_string()]);
    for (id, p) in [hip, knee, foot].into_iter().zip(before) {
        assert!((pos(&d, id) - p).length() < 1e-4, "joint {} stayed in place", id.0);
    }
    let aim = d.scene.world_matrix(hip).transform_vector3(Vec3::Y).normalize();
    assert!((aim - (before[1] - before[0]).normalize()).length() < 1e-4);
    let local = d.scene.get(knee).unwrap().transform.translation;
    assert!(local.x.abs() < 1e-4 && local.z.abs() < 1e-4 && local.y > 0.0, "the child sits on the bone axis: {local:?}");
    let end = d.scene.get(foot).unwrap().transform.rotation;
    assert!(end.dot(Quat::IDENTITY).abs() > 0.9999, "an end joint lines up with its parent");
    // Mirror across X with renaming.
    let made = d.execute(Command::Rig(R::MirrorJoints { id: hip, axis: "x".into(), find: "_L".into(), replace: "_R".into() })).unwrap().created;
    assert_eq!(made.len(), 3);
    let names: Vec<String> = made.iter().map(|id| d.scene.get(*id).unwrap().name.clone()).collect();
    assert_eq!(names, vec!["Hip_R", "Knee_R", "Foot_R"]);
    for (orig, copy) in [hip, knee, foot].into_iter().zip(&made) {
        let (a, b) = (pos(&d, orig), pos(&d, *copy));
        assert!((Vec3::new(-a.x, a.y, a.z) - b).length() < 1e-4);
    }
    assert_eq!(d.scene.get(made[1]).unwrap().parent, Some(made[0]));
    // Rotating a joint and its mirror by mirrored angles keeps the pose symmetric.
    let bend = |d: &mut Document, id: ObjectId, angle: f32| {
        let mut t = d.scene.get(id).unwrap().transform;
        t.rotation *= Quat::from_rotation_x(angle);
        d.execute(Command::SetTransform { id, transform: t }).unwrap();
    };
    bend(&mut d, hip, 0.6);
    bend(&mut d, made[0], 0.6);
    let (a, b) = (pos(&d, foot), pos(&d, made[2]));
    assert!((Vec3::new(-a.x, a.y, a.z) - b).length() < 1e-3, "{a:?} {b:?}");
    // Errors leave no trace.
    let n = d.undo.len();
    for bad in [
        R::AddJoint { parent: Some(ObjectId(99)), position: None, name: None },
        R::AddJoint { parent: None, position: Some([f32::NAN, 0.0, 0.0]), name: None },
        R::OrientJoints { ids: vec![hip], aim: "y".into(), up: "y".into(), children: false },
        R::OrientJoints { ids: vec![], aim: "y".into(), up: "z".into(), children: false },
        R::MirrorJoints { id: hip, axis: "q".into(), find: String::new(), replace: String::new() },
        R::MirrorJoints { id: ObjectId(99), axis: "x".into(), find: String::new(), replace: String::new() },
    ] {
        assert!(d.execute(Command::Rig(bad)).is_err());
    }
    assert_eq!(d.undo.len(), n);
}

#[test]
fn binding_posing_and_editing_weights() {
    let mut d = Document::new();
    let (mesh, [a, b, c]) = arm(&mut d);
    let rest = d.eval.evaluate(&d.scene, mesh).mesh;
    let skin =
        d.execute(Command::Rig(R::BindSkin { mesh, joints: vec![a, b, c], method: BindMethod::Heat, influences: 4, dual_quaternion: false })).unwrap().created
            [0];
    assert_eq!(d.scene.get(skin).unwrap().parent, Some(mesh));
    let weights = |d: &Document| match &d.scene.get(skin).unwrap().kind {
        ObjectKind::Skin { skin } => skin.weights.clone(),
        _ => panic!("not a skin"),
    };
    let w = weights(&d);
    assert_eq!(w.points(), rest.vertex_count());
    for (v, p) in rest.positions().iter().enumerate() {
        assert!((w.of(v).map(|(_, x)| x).sum::<f32>() - 1.0).abs() < 1e-5);
        if p.y < -1.5 {
            assert!(w.weight(v, 0) > 0.9, "the lower arm follows the shoulder: {} at {p:?}", w.weight(v, 0));
        } else if p.y > 1.5 {
            let upper = w.weight(v, 1) + w.weight(v, 2);
            assert!(upper > 0.9, "the upper arm follows the elbow and the wrist at its end: {upper} at {p:?}");
        }
    }
    // Bend the elbow: the wrist end swings, the shoulder end holds, and undo restores it.
    let bound = d.eval.evaluate(&d.scene, mesh).mesh;
    assert!(rest.positions().iter().zip(bound.positions()).all(|(p, q)| (*p - *q).length() < 1e-4));
    let mut t = d.scene.get(b).unwrap().transform;
    t.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    d.execute(Command::SetTransform { id: b, transform: t }).unwrap();
    let bent = d.eval.evaluate(&d.scene, mesh).mesh;
    let top = rest.positions().iter().position(|p| p.y > 1.99 && p.x > 0.39).unwrap() as u32;
    let bottom = rest.positions().iter().position(|p| p.y < -1.99).unwrap() as u32;
    assert!(bent.position(top).x < -1.5, "{:?}", bent.position(top));
    assert!((bent.position(bottom) - rest.position(bottom)).length() < 0.05);
    // Dual quaternions: switch, twist half a turn, the waist keeps its radius.
    d.execute(Command::Rig(R::SetSkin { mesh, dual_quaternion: true })).unwrap();
    t.rotation = Quat::from_rotation_y(std::f32::consts::PI);
    d.execute(Command::SetTransform { id: b, transform: t }).unwrap();
    let radius = |d: &mut Document| {
        d.eval.evaluate(&d.scene, mesh).mesh.positions().iter().filter(|p| p.y.abs() < 1.9).map(|p| (p.x * p.x + p.z * p.z).sqrt()).fold(f32::MAX, f32::min)
    };
    assert!(radius(&mut d) > 0.38, "{}", radius(&mut d));
    d.execute(Command::Rig(R::SetSkin { mesh, dual_quaternion: false })).unwrap();
    assert!(radius(&mut d) < 0.3, "linear blending pinches the twist: {}", radius(&mut d));
    // Weight tools.
    let out = d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Prune { threshold: 0.2 } })).unwrap();
    assert!(out.warnings[0].ends_with("weights dropped"));
    d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Limit { max: 1 } })).unwrap();
    assert!((0..rest.vertex_count()).all(|v| weights(&d).of(v).count() == 1));
    d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Set { points: vec![top], joint: a, weight: 0.25 } })).unwrap();
    assert_eq!(weights(&d).weight(top as usize, 0), 0.25);
    let out =
        d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Paint { joint: c, center: [0.0, 2.0, 0.0], radius: 0.8, strength: 1.0 } })).unwrap();
    assert!(out.warnings[0].ends_with("points painted"));
    let w = weights(&d);
    let cap = rest.positions().iter().position(|p| p.y > 1.99 && p.x.abs() < 1e-3 && p.z.abs() < 1e-3);
    if let Some(v) = cap {
        assert!(w.weight(v, 2) > 0.99, "the centre of the dab is fully painted");
    }
    assert!(w.weight(top as usize, 2) > 0.3, "the rim of the cap got some: {}", w.weight(top as usize, 2));
    assert_eq!(w.weight(bottom as usize, 2), 0.0, "outside the radius nothing changes");
    d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Normalize })).unwrap();
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert_eq!(weights(&d).weight(top as usize, 2), 0.0, "undo takes the dab back");
    // Errors.
    let n = d.undo.len();
    let cube = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    for bad in [
        R::BindSkin { mesh, joints: vec![a], method: BindMethod::Heat, influences: 4, dual_quaternion: false },
        R::BindSkin { mesh: cube, joints: vec![a], method: BindMethod::Heat, influences: 4, dual_quaternion: false },
        R::UnbindSkin { mesh: cube },
        R::SetSkin { mesh: ObjectId(99), dual_quaternion: true },
        R::EditWeights { mesh, edit: WeightOp::Set { points: vec![0], joint: cube, weight: 1.0 } },
        R::EditWeights { mesh, edit: WeightOp::Paint { joint: a, center: [0.0; 3], radius: 0.0, strength: 1.0 } },
        R::EditWeights { mesh, edit: WeightOp::Mirror { axis: "w".into(), find: String::new(), replace: String::new(), tolerance: 0.01 } },
    ] {
        assert!(d.execute(Command::Rig(bad)).is_err());
    }
    assert_eq!(d.undo.len(), n + 1);
    d.execute(Command::Rig(R::UnbindSkin { mesh })).unwrap();
    for bad in [
        R::BindSkin { mesh, joints: vec![], method: BindMethod::Distance, influences: 4, dual_quaternion: false },
        R::BindSkin { mesh, joints: vec![a, a], method: BindMethod::Distance, influences: 4, dual_quaternion: false },
        R::BindSkin { mesh, joints: vec![mesh], method: BindMethod::Distance, influences: 4, dual_quaternion: false },
    ] {
        assert!(d.execute(Command::Rig(bad)).is_err());
    }
    d.execute(Command::Rig(R::BindSkin { mesh, joints: vec![a, b, c], method: BindMethod::Distance, influences: 2, dual_quaternion: true })).unwrap();
}

#[test]
fn mirrored_weights_animation_and_files() {
    let mut d = Document::new();
    // A bar along X with a joint on each side and one in the middle.
    let mesh = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id: mesh, primitive: Primitive::Cube { size: Vec3::new(4.0, 0.5, 0.5), segments: 8 } }).unwrap();
    let mid = joint(&mut d, None, [0.0, 0.0, 0.0], "Mid");
    let left = joint(&mut d, Some(mid), [2.0, 0.0, 0.0], "Arm_L");
    let right = joint(&mut d, Some(mid), [-2.0, 0.0, 0.0], "Arm_R");
    let skin = d
        .execute(Command::Rig(R::BindSkin { mesh, joints: vec![mid, left, right], method: BindMethod::Distance, influences: 3, dual_quaternion: false }))
        .unwrap()
        .created[0];
    let rest = d.eval.evaluate(&d.scene, mesh).mesh;
    let weights = |d: &Document| match &d.scene.get(skin).unwrap().kind {
        ObjectKind::Skin { skin } => skin.weights.clone(),
        _ => panic!("not a skin"),
    };
    // Paint the left tip fully to the left arm, then mirror: the right tip belongs to the right arm.
    d.execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Paint { joint: left, center: [2.0, 0.0, 0.0], radius: 1.0, strength: 1.0 } })).unwrap();
    let out = d
        .execute(Command::Rig(R::EditWeights { mesh, edit: WeightOp::Mirror { axis: "x".into(), find: "_L".into(), replace: "_R".into(), tolerance: 0.01 } }))
        .unwrap();
    assert!(out.warnings[0].ends_with("points mirrored"));
    let w = weights(&d);
    for (v, p) in rest.positions().iter().enumerate().filter(|(_, p)| p.x < -1e-3) {
        let twin = rest.positions().iter().position(|q| (Vec3::new(-q.x, q.y, q.z) - *p).length() < 1e-4).unwrap();
        assert!((w.weight(v, 2) - w.weight(twin, 1)).abs() < 1e-6 && (w.weight(v, 0) - w.weight(twin, 0)).abs() < 1e-6);
    }
    // Key the left arm's rotation: the skinned mesh animates, bakes to a point cache and survives a save.
    for (frame, deg) in [(0.0, 0.0), (10.0, 60.0)] {
        d.execute(Command::Anim(crate::AnimCommand::SetKey { id: left, path: "rotate.z".into(), frame: Some(frame), value: Some(deg) })).unwrap();
    }
    let cache = d.bake_points(mesh, 0.0, 10.0, 5.0, false).unwrap();
    assert_eq!(cache.frames(), 3);
    assert_ne!(cache.frame(0), cache.frame(2));
    d.set_frame(10.0);
    let posed = d.eval.evaluate(&d.scene, mesh).mesh;
    let dir = std::env::temp_dir().join(format!("mc-rig-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join("rig.modelcraft");
    d.save_as(&p).unwrap();
    let mut r = Document::open(&p).unwrap();
    assert_eq!(r.scene.get(skin).map(|o| &o.kind), d.scene.get(skin).map(|o| &o.kind));
    let again = r.eval.evaluate(&r.scene, mesh).mesh;
    assert!(posed.positions().iter().zip(again.positions()).all(|(a, b)| (*a - *b).length() < 1e-5));
    let _ = std::fs::remove_dir_all(dir);
    // JSON.
    let c: Command = serde_json::from_str(r#"{"cmd":"rig","op":"edit_weights","mesh":1,"weights":"prune","threshold":0.1}"#).unwrap();
    assert_eq!(c, Command::Rig(R::EditWeights { mesh: ObjectId(1), edit: WeightOp::Prune { threshold: 0.1 } }));
    let c: Command = serde_json::from_str(r#"{"cmd":"rig","op":"bind_skin","mesh":1,"joints":[2,3]}"#).unwrap();
    assert_eq!(
        c,
        Command::Rig(R::BindSkin {
            mesh: ObjectId(1),
            joints: vec![ObjectId(2), ObjectId(3)],
            method: BindMethod::Heat,
            influences: 4,
            dual_quaternion: false
        })
    );
    for c in [
        Command::Rig(R::AddJoint { parent: None, position: Some([1.0, 2.0, 3.0]), name: None }),
        Command::Rig(R::EditWeights { mesh: ObjectId(4), edit: WeightOp::Normalize }),
    ] {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c, "{j}");
    }
    assert_eq!(crate::rig::COMMANDS.len(), 15);
    let _ = Transform::default();
}
