use glam::{Quat, Vec3};
use modelcraft_scene::constraint::{ConstraintKind, IkSolver, weight_path};
use modelcraft_scene::{ObjectId, Transform};

use crate::rig::RigCommand as R;
use crate::{AnimCommand, Command, Document};

fn joint(d: &mut Document, parent: Option<ObjectId>, at: [f32; 3], name: &str) -> ObjectId {
    d.execute(Command::Rig(R::AddJoint { parent, position: Some(at), name: Some(name.into()) })).unwrap().created[0]
}

fn group(d: &mut Document, at: [f32; 3], name: &str) -> ObjectId {
    let id = d.execute(Command::AddGroup { name: Some(name.into()) }).unwrap().created[0];
    move_to(d, id, at);
    id
}

fn move_to(d: &mut Document, id: ObjectId, at: [f32; 3]) {
    let t = Transform { translation: Vec3::from_array(at), ..d.scene.get(id).unwrap().transform };
    d.execute(Command::SetTransform { id, transform: t }).unwrap();
}

fn pos(d: &Document, id: ObjectId) -> Vec3 {
    d.scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 2e-3
}

fn run(d: &mut Document, json: &str) -> crate::Outcome {
    d.execute(serde_json::from_str(json).unwrap()).unwrap()
}

fn leg(d: &mut Document) -> [ObjectId; 3] {
    let hip = joint(d, None, [0.0, 2.0, 0.0], "Hip");
    let knee = joint(d, Some(hip), [0.0, 1.0, 0.1], "Knee");
    let foot = joint(d, Some(knee), [0.0, 0.0, 0.0], "Foot");
    [hip, knee, foot]
}

#[test]
fn ik_by_command_with_undo_and_matching() {
    let mut d = Document::new();
    let [hip, knee, foot] = leg(&mut d);
    // Without a goal one is made at the end joint, so nothing moves.
    let out = d.execute(Command::Rig(R::AddIk { end: foot, goal: None, pole: None, bones: 2, solver: IkSolver::Auto, name: None })).unwrap();
    let goal = out.created[0];
    assert_eq!(d.scene.get(goal).unwrap().name, "Foot goal");
    assert!(near(pos(&d, foot), Vec3::ZERO));
    let id = d.scene.constraints.list()[0].id;
    // Moving the goal with an ordinary command moves the foot in the same step.
    move_to(&mut d, goal, [0.5, 0.5, 0.0]);
    assert!(near(pos(&d, foot), Vec3::new(0.5, 0.5, 0.0)), "{:?}", pos(&d, foot));
    assert!(pos(&d, knee).z > 0.1, "the knee keeps bending forwards");
    d.execute(Command::Undo).unwrap();
    assert!(near(pos(&d, foot), Vec3::ZERO));
    d.execute(Command::Redo).unwrap();
    assert!(near(pos(&d, foot), Vec3::new(0.5, 0.5, 0.0)));
    // Match to FK: the blend goes to 0 and nothing jumps; the joints now own the pose.
    let before = [pos(&d, knee), pos(&d, foot)];
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"match_ik","id":{id},"to":"fk"}}"#));
    assert_eq!(d.scene.constraints.get(id).unwrap().weight, 0.0);
    assert!(near(pos(&d, knee), before[0]) && near(pos(&d, foot), before[1]));
    // Pose by FK, then match to IK: the goal jumps onto the foot and the blend is 1, no jump.
    let turn = Transform { rotation: Quat::from_rotation_x(0.6), ..d.scene.get(hip).unwrap().transform };
    d.execute(Command::SetTransform { id: hip, transform: turn }).unwrap();
    let posed = [pos(&d, knee), pos(&d, foot)];
    assert!(!near(posed[1], before[1]));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"match_ik","id":{id},"to":"ik"}}"#));
    assert_eq!(d.scene.constraints.get(id).unwrap().weight, 1.0);
    assert!(near(pos(&d, goal), posed[1]));
    assert!(near(pos(&d, knee), posed[0]) && near(pos(&d, foot), posed[1]), "{:?} {:?}", pos(&d, knee), posed[0]);
    // Settings, a pole, and the errors.
    let pole = group(&mut d, [0.0, 1.0, -5.0], "Pole");
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"pole":{},"solver":"fabrik","iterations":50,"name":"Leg IK"}}"#, pole.0));
    assert!(pos(&d, knee).z < posed[0].z, "the pole pulls the knee back");
    assert!(
        matches!(d.scene.constraints.get(id).unwrap().kind, ConstraintKind::Ik { solver: IkSolver::Fabrik, iterations: 50, pole: Some(p), .. } if p == pole)
    );
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"pole":0}}"#));
    assert!(matches!(d.scene.constraints.get(id).unwrap().kind, ConstraintKind::Ik { pole: None, .. }));
    for bad in [
        format!(r#"{{"cmd":"rig","op":"add_ik","end":{}}}"#, hip.0),
        format!(r#"{{"cmd":"rig","op":"add_ik","end":{},"goal":{}}}"#, foot.0, knee.0),
        r#"{"cmd":"rig","op":"add_ik","end":999}"#.to_string(),
        r#"{"cmd":"rig","op":"match_ik","id":999,"to":"fk"}"#.to_string(),
        format!(r#"{{"cmd":"rig","op":"match_ik","id":{id},"to":"sideways"}}"#),
        format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"source_weights":[1]}}"#),
        format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"weight":null,"pole":4242}}"#),
        r#"{"cmd":"rig","op":"remove_constraint","id":999}"#.to_string(),
    ] {
        let scene = d.scene.clone();
        assert!(d.execute(serde_json::from_str(&bad).unwrap()).is_err(), "{bad}");
        assert!(d.scene == scene, "a refused command changes nothing: {bad}");
    }
    // Removing the constraint gives the joints their own rotations back; deleting the goal of
    // a constraint removes the constraint.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"remove_constraint","id":{id}}}"#));
    assert!(d.scene.constraints.is_empty());
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.constraints.list().len(), 1);
    d.execute(Command::Delete { ids: vec![goal] }).unwrap();
    assert!(d.scene.constraints.list().is_empty());
}

#[test]
fn constraints_by_command_keep_offsets_and_report_cycles() {
    let mut d = Document::new();
    let hand = group(&mut d, [1.0, 1.0, 0.0], "Hand");
    let prop = group(&mut d, [2.0, 1.0, 0.0], "Prop");
    let other = group(&mut d, [0.0, 5.0, 0.0], "Other");
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"parent","target":{},"sources":[{}]}}"#, prop.0, hand.0));
    assert!(near(pos(&d, prop), Vec3::new(2.0, 1.0, 0.0)), "the offset is kept");
    let turn = Transform { rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..d.scene.get(hand).unwrap().transform };
    d.execute(Command::SetTransform { id: hand, transform: turn }).unwrap();
    assert!(near(pos(&d, prop), Vec3::new(1.0, 2.0, 0.0)), "{:?}", pos(&d, prop));
    // Hand it over to the other object by weights.
    let id = d.scene.constraints.list()[0].id;
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"remove_constraint","id":{id}}}"#));
    let out = run(
        &mut d,
        &format!(
            r#"{{"cmd":"rig","op":"add_constraint","kind":"point","target":{},"sources":[{},{}],"weights":[1,1],"maintain_offset":false,"name":"Carry"}}"#,
            prop.0, hand.0, other.0
        ),
    );
    assert!(out.warnings.is_empty());
    assert!(near(pos(&d, prop), Vec3::new(0.5, 3.0, 0.0)));
    let id = d.scene.constraints.list()[0].id;
    assert_eq!(d.scene.constraints.get(id).unwrap().name, "Carry");
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"source_weights":[0,1]}}"#));
    assert!(near(pos(&d, prop), Vec3::new(0.0, 5.0, 0.0)));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_constraint","id":{id},"enabled":false}}"#));
    assert!(near(pos(&d, prop), Vec3::new(2.0, 1.0, 0.0)), "off: back where it was authored");
    // Aim, orient and scale parse and run.
    for kind in ["aim", "orient", "scale"] {
        let out =
            run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"{kind}","target":{},"sources":[{}],"aim":"x","up":"y"}}"#, other.0, hand.0));
        assert!(out.warnings.is_empty(), "{kind}: {:?}", out.warnings);
    }
    assert!(near(pos(&d, other), Vec3::new(0.0, 5.0, 0.0)));
    // A cycle is accepted (it can be broken later) but reported with the command.
    let out = run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"orient","target":{},"sources":[{}]}}"#, hand.0, other.0));
    assert!(out.warnings.iter().any(|w| w.contains("cycle")), "{:?}", out.warnings);
    for bad in [
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"glue","target":{},"sources":[{}]}}"#, prop.0, hand.0),
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"point","target":{},"sources":[]}}"#, prop.0),
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"point","target":{},"sources":[{}]}}"#, prop.0, prop.0),
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"point","target":{},"sources":[{}],"weights":[1,2]}}"#, prop.0, hand.0),
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"aim","target":{},"sources":[{}],"aim":"y","up":"y"}}"#, prop.0, hand.0),
        format!(r#"{{"cmd":"rig","op":"add_constraint","kind":"point","target":{},"sources":[777]}}"#, prop.0),
    ] {
        assert!(d.execute(serde_json::from_str(&bad).unwrap()).is_err(), "{bad}");
    }
}

#[test]
fn driven_keys_expressions_and_baking() {
    let mut d = Document::new();
    let lever = group(&mut d, [0.0, 0.0, 0.0], "Lever");
    let door = group(&mut d, [0.0, 0.0, 0.0], "Door");
    // Two driven keys set from the current values, the usual way: pose both, key, pose, key.
    let key = format!(r#"{{"cmd":"rig","op":"set_driven_key","driver":{},"driver_path":"translate.y","target":{},"path":"rotate.y"}}"#, lever.0, door.0);
    run(&mut d, &key);
    move_to(&mut d, lever, [0.0, 2.0, 0.0]);
    // Posing the driven attribute by number, then keying it at the new driver value.
    run(
        &mut d,
        &format!(r#"{{"cmd":"rig","op":"set_driven_key","driver":{},"driver_path":"translate.y","target":{},"path":"rotate.y","value":90}}"#, lever.0, door.0),
    );
    assert_eq!(d.scene.constraints.list().len(), 1, "the second key goes into the same curve");
    assert!((d.scene.attr(door, "rotate.y").unwrap() - 90.0).abs() < 1e-3);
    move_to(&mut d, lever, [0.0, 1.0, 0.0]);
    let mid = d.scene.attr(door, "rotate.y").unwrap();
    assert!(mid > 20.0 && mid < 70.0, "between the keys: {mid}");
    // An expression over the frame and another object.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"translate.x","expr":"frame * 0.5 + @{}.translate.y"}}"#, door.0, lever.0));
    d.execute(Command::Anim(AnimCommand::SetTime { frame: 4.0 })).unwrap();
    assert_eq!(d.scene.attr(door, "translate.x"), Some(3.0));
    // Replacing and refusing.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"translate.x","expr":"frame"}}"#, door.0));
    assert_eq!(d.scene.constraints.list().len(), 2);
    assert_eq!(d.scene.attr(door, "translate.x"), Some(4.0));
    for bad in [
        format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"translate.x","expr":"frame +"}}"#, door.0),
        format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"no.such","expr":"1"}}"#, door.0),
        format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"scale.x","expr":""}}"#, door.0),
        format!(r#"{{"cmd":"rig","op":"set_driven_key","driver":{},"driver_path":"nope","target":{},"path":"rotate.y"}}"#, lever.0, door.0),
        format!(r#"{{"cmd":"rig","op":"bake_constraints","objects":[{}],"start":5,"end":1}}"#, door.0),
        r#"{"cmd":"rig","op":"bake_constraints","objects":[]}"#.to_string(),
    ] {
        assert!(d.execute(serde_json::from_str(&bad).unwrap()).is_err(), "{bad}");
    }
    // Bake frames 0..10: the door then moves by its own keys with the constraints off.
    let out = run(&mut d, &format!(r#"{{"cmd":"rig","op":"bake_constraints","objects":[{}],"start":0,"end":10}}"#, door.0));
    assert!(out.warnings.iter().any(|w| w == "2 constraints switched off"), "{:?}", out.warnings);
    assert!(d.scene.constraints.list().iter().all(|c| !c.enabled));
    assert_eq!(d.scene.anim.frame, 4.0, "the current frame is put back");
    assert_eq!(d.scene.anim.curve(door, "translate.x").unwrap().len(), 11);
    d.set_frame(7.0);
    assert_eq!(d.scene.attr(door, "translate.x"), Some(7.0));
    assert!((d.scene.attr(door, "rotate.y").unwrap() - mid).abs() < 1e-3);
    // One undo step takes the whole bake back.
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.anim.curve(door, "translate.x").is_none());
    assert!(d.scene.constraints.list().iter().all(|c| c.enabled));
    // An empty formula removes the expression.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_expression","target":{},"path":"translate.x","expr":""}}"#, door.0));
    assert_eq!(d.scene.constraints.list().len(), 1);
}

#[test]
fn rigs_with_constraints_survive_save_load_and_crash_recovery() {
    let dir = std::env::temp_dir().join(format!("modelcraft-constraints-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rig.modelcraft");
    let mut d = Document::new();
    let [_, knee, foot] = leg(&mut d);
    let goal = d.execute(Command::Rig(R::AddIk { end: foot, goal: None, pole: None, bones: 2, solver: IkSolver::Auto, name: None })).unwrap().created[0];
    let id = d.scene.constraints.list()[0].id;
    d.save_as(&path).unwrap();
    // Unsaved work after the save: move the goal, key the blend.
    move_to(&mut d, goal, [0.4, 0.3, 0.2]);
    d.execute(Command::Anim(AnimCommand::SetKey { id: foot, path: weight_path(id), frame: Some(0.0), value: Some(1.0) })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetKey { id: foot, path: weight_path(id), frame: Some(10.0), value: Some(0.0) })).unwrap();
    d.sync_journal();
    let want = (pos(&d, knee), pos(&d, foot), d.scene.clone());
    assert!(near(want.1, Vec3::new(0.4, 0.3, 0.2)));
    drop(d);
    let mut back = Document::open(&path).unwrap();
    assert!(near(pos(&back, foot), Vec3::ZERO), "the saved pose");
    assert!(back.pending_recovery > 0);
    back.recover().unwrap();
    assert!(near(pos(&back, knee), want.0) && near(pos(&back, foot), want.1));
    assert!(back.scene.constraints == want.2.constraints);
    back.set_frame(10.0);
    assert!(!near(pos(&back, foot), want.1), "the keyed blend is off at frame 10");
    back.save().unwrap();
    let mut again = Document::open(&path).unwrap();
    again.set_frame(0.0);
    assert!(near(pos(&again, foot), want.1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn formulas_on_any_attribute_survive_save_undo_and_crash_recovery() {
    let dir = std::env::temp_dir().join(format!("modelcraft-formulas-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.modelcraft");
    let mut d = Document::new();
    let lever = group(&mut d, [2.0, 0.0, 0.0], "Lever");
    let ball = d.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Ball".into()), parent: None }).unwrap().created[0];
    let set = |d: &mut Document, path: &str, expr: &str| {
        d.execute(Command::Rig(R::SetExpression { target: ball, path: path.into(), expr: expr.into() })).unwrap();
    };
    // A primitive parameter and a transform channel, one reading the other object by name.
    set(&mut d, "primitive.radius", "0.25 + @Lever.translate.x / 4");
    assert_eq!(d.scene.attr(ball, "primitive.radius"), Some(0.75));
    d.save_as(&path).unwrap();
    set(&mut d, "translate.y", "self.primitive.radius * 2 + frame");
    d.execute(Command::Anim(AnimCommand::SetTime { frame: 3.0 })).unwrap();
    assert_eq!(d.scene.attr(ball, "translate.y"), Some(4.5));
    // Undo takes the second formula back (the frame is not an undo step); redo brings it again.
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.constraints.formula(ball, "translate.y").is_none());
    assert!(d.scene.constraints.formula(ball, "primitive.radius").is_some());
    d.execute(Command::Redo).unwrap();
    assert!(d.scene.constraints.formula(ball, "translate.y").is_some());
    assert_eq!(d.scene.attr(ball, "translate.y"), Some(4.5));
    d.sync_journal();
    let want = d.scene.constraints.clone();
    drop(d);
    // The file has the first formula; the journal brings back the second.
    let mut back = Document::open(&path).unwrap();
    assert!(back.scene.constraints.formula(ball, "primitive.radius").is_some());
    assert!(back.scene.constraints.formula(ball, "translate.y").is_none());
    back.recover().unwrap();
    assert!(back.scene.constraints == want);
    back.set_frame(3.0);
    assert_eq!(back.scene.attr(ball, "translate.y"), Some(4.5));
    move_to(&mut back, lever, [6.0, 0.0, 0.0]);
    assert_eq!(back.scene.attr(ball, "primitive.radius"), Some(1.75), "the formula follows its driver");
    assert_eq!(back.scene.attr(ball, "translate.y"), Some(6.5), "and the formula reading it runs after it");
    let _ = std::fs::remove_dir_all(&dir);
}
