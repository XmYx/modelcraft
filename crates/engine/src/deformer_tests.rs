use glam::{Quat, Vec3};
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::point_deform::PointDeformer;
use modelcraft_scene::{ObjectId, ObjectKind, StepOp, Transform};

use crate::rig::{BindMethod, RigCommand as R};
use crate::{AnimCommand, Command, Document};

fn run(d: &mut Document, json: &str) -> crate::Outcome {
    d.execute(serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"))).unwrap_or_else(|e| panic!("{json}: {e}"))
}

fn refused(d: &mut Document, json: &str) {
    let before = d.scene.clone();
    assert!(d.execute(serde_json::from_str(json).unwrap()).is_err(), "{json}");
    assert!(d.scene == before, "a refused command changes nothing: {json}");
}

fn cube(d: &mut Document, name: &str) -> ObjectId {
    let id = d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some(name.into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id, primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 2 } }).unwrap();
    id
}

fn points(d: &mut Document, id: ObjectId) -> Vec<Vec3> {
    let e = d.eval.evaluate(&d.scene, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    e.mesh.positions().to_vec()
}

fn top(points: &[Vec3]) -> f32 {
    points.iter().map(|p| p.y).fold(f32::MIN, f32::max)
}

/// A copy of `of` with its top face pulled up by `by`: a sculpted target.
fn target(d: &mut Document, of: ObjectId, name: &str, by: f32) -> ObjectId {
    let id = cube(d, name);
    let rest = points(d, of);
    let verts: Vec<u32> = rest.iter().enumerate().filter(|(_, p)| p.y > 0.9).map(|(i, _)| i as u32).collect();
    d.execute(Command::AddStep { id, op: StepOp::move_vertices(verts, Vec3::Y * by, Quat::IDENTITY, Vec3::ONE) }).unwrap();
    id
}

#[test]
fn blend_shapes_with_in_betweens_and_keyed_weights() {
    let mut d = Document::new();
    let face = cube(&mut d, "Face");
    let rest = points(&mut d, face);
    let smile = target(&mut d, face, "Smile", 1.0);
    let out = run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{}}}"#, face.0, smile.0));
    let shapes = out.created[0];
    assert_eq!(out.warnings, vec!["9 points move in the shape".to_string()], "only the top face is stored");
    assert_eq!(d.scene.get(shapes).unwrap().name, "Blend shapes");
    assert_eq!(points(&mut d, face), rest, "a new shape starts at weight 0");
    // The weight is an attribute: set it by command, read it by path, key it.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":0,"weight":0.5}}"#, face.0));
    assert!((top(&points(&mut d, face)) - 1.5).abs() < 1e-5);
    assert_eq!(d.scene.attr(shapes, "target.0.weight"), Some(0.5));
    assert!(modelcraft_scene::attr::list(d.scene.get(shapes).unwrap()).iter().any(|(p, v)| p == "target.0.weight" && *v == 0.5));
    // An in-between at 0.5 that goes down instead: the top dips before it rises.
    let dip = target(&mut d, face, "Dip", -0.5);
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{},"target":0,"at":0.5}}"#, face.0, dip.0));
    assert!((top(&points(&mut d, face)) - 0.5).abs() < 1e-5, "at 0.5 the in-between shows exactly");
    for (w, want) in [(0.25, 0.75), (0.75, 1.25), (1.0, 2.0), (0.0, 1.0)] {
        run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":0,"weight":{w}}}"#, face.0));
        assert!((top(&points(&mut d, face)) - want).abs() < 1e-5, "weight {w}");
    }
    // A second shape adds to the first.
    let wide = cube(&mut d, "Wide");
    d.execute(Command::SetPrimitive { id: wide, primitive: Primitive::Cube { size: Vec3::new(4.0, 2.0, 2.0), segments: 2 } }).unwrap();
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{},"name":"Wide"}}"#, face.0, wide.0));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":1,"weight":1}}"#, face.0));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":0,"weight":1}}"#, face.0));
    let both = points(&mut d, face);
    assert!((top(&both) - 2.0).abs() < 1e-5 && both.iter().any(|p| (p.x - 2.0).abs() < 1e-5));
    // Key the weights: the mesh changes with the frame and only then.
    d.execute(Command::Anim(AnimCommand::SetKey { id: shapes, path: "target.0.weight".into(), frame: Some(0.0), value: Some(0.0) })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetKey { id: shapes, path: "target.0.weight".into(), frame: Some(10.0), value: Some(1.0) })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetKey { id: shapes, path: "target.1.weight".into(), frame: Some(0.0), value: Some(1.0) })).unwrap();
    d.set_frame(5.0);
    assert!((top(&points(&mut d, face)) - 0.5).abs() < 0.05, "through the in-between: {}", top(&points(&mut d, face)));
    d.set_frame(10.0);
    assert!((top(&points(&mut d, face)) - 2.0).abs() < 1e-4);
    // Removing the first shape keeps the second one's curve on the second shape.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"remove_shape","mesh":{},"target":0}}"#, face.0));
    assert!(d.scene.anim.curve(shapes, "target.0.weight").is_some_and(|c| c.eval(0.0) == Some(1.0)), "Wide's curve moved up");
    assert!(d.scene.anim.curve(shapes, "target.1.weight").is_none());
    d.set_frame(0.0);
    assert!((top(&points(&mut d, face)) - 1.0).abs() < 1e-5);
    d.execute(Command::Undo).unwrap();
    d.set_frame(10.0);
    assert!((top(&points(&mut d, face)) - 2.0).abs() < 1e-4, "undo brings the shape and its curve back");
    // Removing one in-between leaves the shape.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"remove_shape","mesh":{},"target":0,"frame":0}}"#, face.0));
    d.set_frame(5.0);
    assert!(top(&points(&mut d, face)) > 1.2, "no dip any more");
    // Refusals.
    let ball = d.execute(Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }).unwrap().created[0];
    for bad in [
        format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{}}}"#, face.0, ball.0),
        format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{}}}"#, face.0, face.0),
        format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{},"target":9}}"#, face.0, smile.0),
        format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{},"target":0,"at":0}}"#, face.0, smile.0),
        format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{}}}"#, shapes.0, smile.0),
        format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":7,"weight":1}}"#, face.0),
        format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":0,"weight":1}}"#, ball.0),
        format!(r#"{{"cmd":"rig","op":"remove_shape","mesh":{},"target":7}}"#, face.0),
    ] {
        refused(&mut d, &bad);
    }
    // The base mesh changes its point count: the shapes say so instead of breaking it.
    d.execute(Command::SetPrimitive { id: face, primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 1 } }).unwrap();
    d.set_frame(10.0);
    let e = d.eval.evaluate(&d.scene, face);
    assert_eq!(e.errors.len(), 1, "{:?}", e.errors);
    assert!(e.errors[0].message.contains("make them again"));
}

#[test]
fn lattices_clusters_and_the_order_of_the_stack() {
    let mut d = Document::new();
    let mesh = d.execute(Command::AddPrimitive { kind: "cylinder".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id: mesh, primitive: Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 12, rings: 8, caps: true } }).unwrap();
    let rest = points(&mut d, mesh);
    // A lattice fits the mesh and changes nothing until a point moves.
    let lattice = run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_lattice","mesh":{},"divisions":[1,2,1]}}"#, mesh.0)).created[0];
    let t = d.scene.get(lattice).unwrap().transform;
    assert!((t.scale - Vec3::new(1.02, 4.08, 1.02)).length() < 1e-3 && t.translation.length() < 1e-5, "{t:?}");
    assert!(points(&mut d, mesh).iter().zip(&rest).all(|(a, b)| (*a - *b).length() < 1e-5));
    // 2×3×2 points: the middle ring is points 2..=5 (X fastest, then Y, then Z). Push it along X
    // by a quarter of the box: the middle of the cylinder bulges, the ends stay.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"points":[2,3,8,9],"translate":[0.5,0,0]}}"#, lattice.0));
    let bent = points(&mut d, mesh);
    let (mid, end): (Vec<_>, Vec<_>) = rest.iter().zip(&bent).partition(|(r, _)| r.y.abs() < 0.1);
    assert!(mid.iter().all(|(r, b)| (b.x - r.x - 0.255).abs() < 1e-3), "B₁(½) = ½ of the half-unit move, in a box 1.02 wide");
    assert!(
        end.iter().filter(|(r, _)| r.y.abs() > 1.99).all(|(r, b)| (**r - **b).length() < 0.02),
        "the ends barely move (the box is a little larger than the mesh)"
    );
    // Cell by cell the bulge is sharper; control points are attributes and can be keyed.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"linear":true}}"#, lattice.0));
    let sharp = points(&mut d, mesh);
    let i = rest.iter().position(|r| r.y.abs() < 0.1).unwrap();
    assert!((sharp[i].x - rest[i].x - 0.51).abs() < 1e-3);
    assert_eq!(d.scene.attr(lattice, "point.2.x"), Some(0.0));
    assert_eq!(d.scene.attr(lattice, "linear"), Some(1.0));
    d.execute(Command::Anim(AnimCommand::SetKey { id: lattice, path: "point.2.x".into(), frame: Some(0.0), value: Some(-0.5) })).unwrap();
    d.execute(Command::Anim(AnimCommand::SetKey { id: lattice, path: "point.2.x".into(), frame: Some(10.0), value: Some(1.0) })).unwrap();
    d.set_frame(10.0);
    assert_eq!(d.scene.attr(lattice, "point.2.x"), Some(1.0));
    assert_ne!(points(&mut d, mesh), sharp);
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"reset":true,"linear":false}}"#, lattice.0));
    d.execute(Command::Anim(AnimCommand::RemoveTrack { id: lattice, path: Some("point.2.x".into()) })).unwrap();
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"points":[2],"to":[-0.5,0,-0.5]}}"#, lattice.0));
    assert!(points(&mut d, mesh).iter().zip(&rest).all(|(a, b)| (*a - *b).length() < 1e-5), "back at rest");

    // A cluster with a soft radius at the top: moving its object lifts the top.
    let out = run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{},"center":[0,2,0],"radius":1.0,"name":"Top"}}"#, mesh.0));
    let cluster = out.created[0];
    assert!(out.warnings[0].ends_with("points in the cluster"), "{:?}", out.warnings);
    assert_eq!(d.scene.get(cluster).unwrap().transform.translation, Vec3::Y * 2.0);
    assert!(points(&mut d, mesh).iter().zip(&rest).all(|(a, b)| (*a - *b).length() < 1e-5), "a new cluster moves nothing");
    let lift = Transform { translation: Vec3::new(0.0, 3.0, 0.0), ..Transform::default() };
    d.execute(Command::SetTransform { id: cluster, transform: lift }).unwrap();
    let lifted = points(&mut d, mesh);
    assert!((top(&lifted) - 2.5).abs() < 1e-4, "the rim, halfway out, follows by half: {}", top(&lifted));
    let low = rest.iter().position(|r| r.y < -1.9).unwrap();
    assert_eq!(lifted[low], rest[low]);
    // Explicit points at full weight, and editing weights afterwards.
    let rim: Vec<u32> = rest.iter().enumerate().filter(|(_, p)| p.y > 1.99).map(|(i, _)| i as u32).collect();
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_cluster_weights","id":{},"points":{:?},"weight":1}}"#, cluster.0, rim));
    assert!((top(&points(&mut d, mesh)) - 3.0).abs() < 1e-5);
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_cluster_weights","id":{},"points":{:?},"weight":0}}"#, cluster.0, rim));
    assert!((top(&points(&mut d, mesh)) - 2.0).abs() < 1e-5, "weight 0 takes points out");
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_cluster_weights","id":{},"points":{:?},"weight":1}}"#, cluster.0, rim));

    // Order matters: the lattice pushes the −X side of the top inwards, the cluster turns the
    // top a quarter about Y. Squashed then turned is not turned then squashed.
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"linear":true,"points":[4,10],"translate":[0.5,0,0]}}"#, lattice.0));
    let turn = Transform { translation: Vec3::Y * 2.0, rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), ..Transform::default() };
    d.execute(Command::SetTransform { id: cluster, transform: turn }).unwrap();
    let lattice_first = points(&mut d, mesh);
    let out = run(&mut d, &format!(r#"{{"cmd":"rig","op":"move_in_stack","id":{},"delta":-1}}"#, cluster.0));
    assert!(out.warnings.is_empty());
    assert_eq!(d.scene.children(mesh), vec![cluster, lattice]);
    let cluster_first = points(&mut d, mesh);
    assert!(lattice_first.iter().zip(&cluster_first).any(|(a, b)| (*a - *b).length() > 0.05), "the order of the stack changes the result");
    assert!(run(&mut d, &format!(r#"{{"cmd":"rig","op":"move_in_stack","id":{},"delta":-5}}"#, cluster.0)).warnings[0].contains("already"));
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.children(mesh), vec![lattice, cluster]);
    assert_eq!(points(&mut d, mesh), lattice_first);
    // Hiding a deformer switches it off.
    d.execute(Command::SetVisible { id: lattice, visible: false }).unwrap();
    d.execute(Command::SetVisible { id: cluster, visible: false }).unwrap();
    assert_eq!(points(&mut d, mesh), rest);
    // Refusals.
    for bad in [
        format!(r#"{{"cmd":"rig","op":"add_lattice","mesh":{},"divisions":[0,2,2]}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"add_lattice","mesh":{},"divisions":[2,2,99]}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"add_lattice","mesh":{}}}"#, lattice.0),
        format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"points":[999],"translate":[1,0,0]}}"#, lattice.0),
        format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"points":[0],"translate":[1,0,0],"to":[0,0,0]}}"#, lattice.0),
        format!(r#"{{"cmd":"rig","op":"edit_lattice","id":{},"points":[0],"translate":[1,0,0]}}"#, cluster.0),
        format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{}}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{},"points":[99999]}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{},"points":[0],"weights":[1,1]}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{},"center":[50,50,50],"radius":0.1}}"#, mesh.0),
        format!(r#"{{"cmd":"rig","op":"set_cluster_weights","id":{},"points":[0],"weight":1}}"#, lattice.0),
        format!(r#"{{"cmd":"rig","op":"move_in_stack","id":{},"delta":1}}"#, mesh.0),
    ] {
        refused(&mut d, &bad);
    }
}

#[test]
fn shapes_run_before_the_skin_and_everything_survives_a_save() {
    let dir = std::env::temp_dir().join(format!("modelcraft-deformers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stack.modelcraft");
    let mut d = Document::new();
    let arm = cube(&mut d, "Arm");
    let a = d.execute(Command::Rig(R::AddJoint { parent: None, position: Some([0.0, -1.0, 0.0]), name: None })).unwrap().created[0];
    let b = d.execute(Command::Rig(R::AddJoint { parent: Some(a), position: Some([0.0, 1.0, 0.0]), name: None })).unwrap().created[0];
    d.execute(Command::Rig(R::BindSkin { mesh: arm, joints: vec![a, b], method: BindMethod::Distance, influences: 2, dual_quaternion: false })).unwrap();
    // A shape added after the skin still goes first in the stack: it is a change of the rest
    // shape, and the skin then carries it along.
    let bulge = target(&mut d, arm, "Bulge", 1.0);
    let shapes = run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_shape","mesh":{},"from":{}}}"#, arm.0, bulge.0)).created[0];
    assert_eq!(d.scene.children(arm).first(), Some(&shapes));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"set_shape","mesh":{},"target":0,"weight":1}}"#, arm.0));
    let turn = Transform { translation: Vec3::new(0.0, -1.0, 0.0), rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..Transform::default() };
    d.execute(Command::SetTransform { id: a, transform: turn }).unwrap();
    let posed = points(&mut d, arm);
    let far = posed.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    assert!((far + 3.0).abs() < 1e-3, "the bulged top (y = 2, one above the joint at −1… 3 away) swings to −X: {far}");
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_lattice","mesh":{}}}"#, arm.0));
    run(&mut d, &format!(r#"{{"cmd":"rig","op":"add_cluster","mesh":{},"points":[0,1]}}"#, arm.0));
    let want = points(&mut d, arm);
    d.save_as(&path).unwrap();
    let mut back = Document::open(&path).unwrap();
    assert_eq!(points(&mut back, arm), want);
    assert!(back.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::PointDeformer { .. })).count() == 3);
    // Exports skip the deformer objects themselves.
    assert!(back.export_items().iter().all(|i| !["Blend shapes", "Lattice", "Cluster"].contains(&i.name.as_str())));
    // Hostile deformers are repaired on load.
    let json = r#"{"objects":[{"id":1,"name":"M","kind":{"type":"mesh","source":{"type":"primitive","primitive":{"kind":"cube","size":[1,1,1],"segments":1}},"steps":[]}},
      {"id":2,"name":"L","parent":1,"kind":{"type":"point_deformer","deformer":{"kind":"lattice","divisions":[0,900,2],"points":[[0,0,0]]}}},
      {"id":3,"name":"C","parent":1,"kind":{"type":"point_deformer","deformer":{"kind":"cluster","points":[0,1,99],"weights":[5],"bind":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1]}}},
      {"id":4,"name":"B","parent":1,"kind":{"type":"point_deformer","deformer":{"kind":"blend_shapes","targets":[{"name":"t","weight":1e30,"frames":[{"at":-1,"indices":[0],"deltas":[[1,1,1]]},{"at":1,"indices":[0,1],"deltas":[[1,1,1]]},{"at":1,"indices":[77],"deltas":[[1,1,1]]}]}]}}}]}"#;
    let scene = serde_json::from_str::<modelcraft_scene::Scene>(json).unwrap().repaired().unwrap();
    let ObjectKind::PointDeformer { deformer: PointDeformer::Lattice { divisions, points: p, .. } } = &scene.get(ObjectId(2)).unwrap().kind else { panic!() };
    assert_eq!((*divisions, p.len()), ([1, 8, 2], 2 * 9 * 3));
    let ObjectKind::PointDeformer { deformer: PointDeformer::Cluster { weights, .. } } = &scene.get(ObjectId(3)).unwrap().kind else { panic!() };
    assert_eq!(**weights, vec![1.0, 1.0, 1.0]);
    let ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } } = &scene.get(ObjectId(4)).unwrap().kind else { panic!() };
    assert_eq!((targets[0].weight, targets[0].frames.len()), (10.0, 1));
    let mut doc = Document::from_scene(scene);
    let e = doc.eval.evaluate(&doc.scene, ObjectId(1));
    assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
    assert_eq!(e.errors.len(), 2, "the stale shape and the stale cluster say so: {:?}", e.errors);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn formula_deformers_and_clone_modifiers_follow_the_frame_by_command() {
    let mut d = Document::new();
    let id = cube(&mut d, "Box");
    let def = run(&mut d, &format!(r#"{{"cmd":"add_deformer","kind":"formula","target":{}}}"#, id.0)).created[0];
    // Lift every point by the time in seconds along the deformer's Y axis.
    run(&mut d, &format!(r#"{{"cmd":"set_deformer","id":{},"deformer":{{"kind":"formula","expr":"time","along":"y","amount":1}}}}"#, def.0));
    d.scene.anim.time.fps = 24.0;
    d.set_frame(0.0);
    assert!((top(&points(&mut d, id)) - 1.0).abs() < 1e-5);
    d.set_frame(48.0);
    assert!((top(&points(&mut d, id)) - 3.0).abs() < 1e-5, "two seconds later the evaluation follows");
    // A formula that reads what a deformer does not know is refused only at evaluation: the
    // mesh stays and the error says why.
    run(&mut d, &format!(r#"{{"cmd":"set_deformer","id":{},"deformer":{{"kind":"formula","expr":"frame + nothing"}}}}"#, def.0));
    let e = d.eval.evaluate(&d.scene, id);
    assert!(e.errors.iter().any(|e| e.message.contains("nothing")), "{:?}", e.errors);
    assert!((e.mesh.positions().iter().map(|p| p.y).fold(f32::MIN, f32::max) - 1.0).abs() < 1e-5);
    // Undo goes back to the working formula.
    d.execute(Command::Undo).unwrap();
    assert!((top(&points(&mut d, id)) - 3.0).abs() < 1e-5);

    // A clone modifier in formula mode: clone i rises by i * time * 0.5.
    let clones = d.execute(Command::AddCloner { kind: "linear".into(), input: None }).unwrap().created[0];
    let m = d.execute(Command::AddCloneModifier { kind: "formula".into(), cloner: clones }).unwrap().created[0];
    run(&mut d, &format!(r#"{{"cmd":"set_clone_modifier","id":{},"modifier":{{"mode":"formula","expr":"i * time","position":[0,0.5,0]}}}}"#, m.0));
    let heights = |d: &mut Document| -> Vec<f32> {
        let e = d.eval.evaluate(&d.scene, clones);
        e.instances.as_ref().map(|i| i.clones.instances.iter().map(|c| c.matrix.w_axis.y).collect()).unwrap_or_default()
    };
    d.set_frame(24.0);
    let h1 = heights(&mut d);
    assert!(h1.len() >= 3 && (h1[2] - h1[0] - 1.0).abs() < 1e-4, "{h1:?}");
    d.set_frame(72.0);
    let h3 = heights(&mut d);
    assert!((h3[2] - h3[0] - 3.0).abs() < 1e-4, "{h3:?}");
}
