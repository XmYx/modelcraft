use glam::{Quat, Vec3};
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::constraint::IkSolver;
use modelcraft_scene::point_deform::PointDeformer;
use modelcraft_scene::{ObjectId, ObjectKind, StepOp};

use crate::rig::{BindMethod, RigCommand as R};
use crate::{AnimCommand as A, Command, Document};

fn world_points(d: &mut Document, id: ObjectId) -> Vec<Vec3> {
    let e = d.eval.evaluate(&d.scene, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    let w = d.scene.world_matrix(id);
    e.mesh.positions().iter().map(|p| w.transform_point3(*p)).collect()
}

fn key(d: &mut Document, id: ObjectId, path: &str, frame: f64, value: f64) {
    d.execute(Command::Anim(A::SetKey { id, path: path.into(), frame: Some(frame), value: Some(value) })).unwrap();
}

/// The largest distance from a point of `a` to the nearest point of `b`, and back.
fn apart(a: &[Vec3], b: &[Vec3]) -> f32 {
    let one_way = |x: &[Vec3], y: &[Vec3]| x.iter().map(|p| y.iter().map(|q| p.distance(*q)).fold(f32::MAX, f32::min)).fold(0.0, f32::max);
    one_way(a, b).max(one_way(b, a))
}

/// A skinned, shaped, animated arm: three joints, a bound cylinder, a blend shape, curves on
/// the joints and on the shape's weight, and a prop that follows an IK goal.
fn character(d: &mut Document) -> ObjectId {
    let arm = d.execute(Command::AddPrimitive { kind: "cylinder".into(), name: Some("Arm".into()), parent: None }).unwrap().created[0];
    let shape = Primitive::Cylinder { radius: 0.4, height: 4.0, sides: 10, rings: 8, caps: true };
    d.execute(Command::SetPrimitive { id: arm, primitive: shape }).unwrap();
    let mut joints = Vec::new();
    for (i, y) in [-2.0, 0.0, 2.0].into_iter().enumerate() {
        let parent = joints.last().copied();
        joints.push(d.execute(Command::Rig(R::AddJoint { parent, position: Some([0.0, y, 0.0]), name: Some(format!("J{i}")) })).unwrap().created[0]);
    }
    d.execute(Command::Rig(R::BindSkin { mesh: arm, joints: joints.clone(), method: BindMethod::Heat, influences: 4, dual_quaternion: false })).unwrap();
    // A fat copy as a blend shape.
    let fat = d.execute(Command::AddPrimitive { kind: "cylinder".into(), name: Some("Fat".into()), parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id: fat, primitive: Primitive::Cylinder { radius: 0.7, height: 4.0, sides: 10, rings: 8, caps: true } }).unwrap();
    let shapes = d.execute(Command::Rig(R::AddShape { mesh: arm, from: fat, name: Some("Fat".into()), target: None, at: 1.0 })).unwrap().created[0];
    d.execute(Command::Delete { ids: vec![fat] }).unwrap();
    d.execute(Command::Anim(A::SetTimeRange { start: Some(0.0), end: Some(12.0), fps: Some(24.0) })).unwrap();
    key(d, joints[1], "rotate.z", 0.0, 0.0);
    key(d, joints[1], "rotate.z", 12.0, 110.0);
    key(d, joints[0], "rotate.y", 0.0, -40.0);
    key(d, joints[0], "rotate.y", 12.0, 200.0);
    key(d, joints[0], "translate.x", 0.0, 0.0);
    key(d, joints[0], "translate.x", 6.0, 1.5);
    key(d, shapes, "target.0.weight", 0.0, 0.0);
    key(d, shapes, "target.0.weight", 12.0, 1.0);
    arm
}

#[test]
fn skins_shapes_and_clips_round_trip_through_glb() {
    let dir = std::env::temp_dir().join(format!("modelcraft-gltf-rig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("character.glb");
    let mut d = Document::new();
    let arm = character(&mut d);
    let warnings = d.export_report(&path, Default::default()).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let bytes = std::fs::read(&path).unwrap();
    assert!(modelcraft_formats::gltf_rig::file_has_rig(&bytes));
    // What the file holds, read with the shared reader.
    let doc = craft_gltf_anim::read(&bytes, &mut |_| None, &craft_gltf_anim::Limits::default()).unwrap();
    assert_eq!((doc.skins.len(), doc.skins[0].joints.len(), doc.animations.len()), (1, 3, 1));
    let mesh = &doc.meshes[0];
    assert_eq!((mesh.targets.len(), mesh.targets[0].name.as_str()), (1, "Fat"));
    assert!(mesh.joints.len() == mesh.positions.len() && !mesh.uvs.is_empty());
    assert!(mesh.weights.iter().all(|w| (w.iter().sum::<f32>() - 1.0).abs() < 1e-4), "weights sum to one");
    let paths: Vec<&str> = doc.animations[0].channels.iter().map(|c| c.path.id()).collect();
    assert_eq!(paths.iter().filter(|p| **p == "rotation").count(), 2, "{paths:?}");
    assert_eq!(paths.iter().filter(|p| **p == "translation").count(), 1);
    assert_eq!(paths.iter().filter(|p| **p == "weights").count(), 1);
    assert_eq!(doc.animations[0].channels[0].times.len(), 13, "one key per frame of the range");
    assert!((doc.animations[0].duration() - 0.5).abs() < 1e-6, "12 frames at 24 per second");

    // Into a new document: the same joints, a skin, a blend shape, curves.
    let mut back = Document::new();
    back.execute(Command::Anim(A::SetTimeRange { start: Some(0.0), end: Some(12.0), fps: Some(24.0) })).unwrap();
    let out = back.execute(Command::Import { path: path.display().to_string(), up: Default::default(), scale: None }).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    let named = |d: &Document, name: &str| d.scene.objects().iter().find(|o| o.name == name).map(|o| o.id).unwrap_or_else(|| panic!("no {name}"));
    let arm2 = named(&back, "Arm");
    assert_eq!(back.scene.get(named(&back, "character")).map(|o| o.parent), Some(None), "everything sits under one group named after the file");
    for j in ["J0", "J1", "J2"] {
        assert!(matches!(back.scene.get(named(&back, j)).unwrap().kind, ObjectKind::Joint { .. }));
    }
    assert_eq!(back.scene.get(named(&back, "J2")).unwrap().parent, Some(named(&back, "J1")));
    assert_eq!(back.scene.skins_of(arm2).len(), 1);
    let kids: Vec<&str> = back.scene.children(arm2).iter().map(|c| back.scene.get(*c).unwrap().kind.type_name()).collect();
    assert_eq!(kids, vec!["Blend shapes", "Skin"], "shapes before the skin");
    let shapes2 = back.scene.children(arm2)[0];
    assert!(
        matches!(&back.scene.get(shapes2).unwrap().kind, ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } } if targets[0].name == "Fat")
    );
    assert_eq!(back.scene.anim.curve(shapes2, "target.0.weight").map(|c| c.len()), Some(13));
    assert_eq!(back.scene.anim.curve(named(&back, "J0"), "translate.x").map(|c| c.len()), Some(13));
    // The same number of points as the original (UV seams welded back), and the same deformed
    // shape at every frame: skin, shape and curves all took part.
    for frame in [0.0, 3.0, 6.0, 9.0, 12.0] {
        d.set_frame(frame);
        back.set_frame(frame);
        let (a, b) = (world_points(&mut d, arm), world_points(&mut back, arm2));
        assert_eq!(a.len(), b.len());
        assert!(apart(&a, &b) < 2e-3, "frame {frame}: {} apart", apart(&a, &b));
    }
    d.set_frame(0.0);
    let (first, _) = (world_points(&mut d, arm), d.set_frame(12.0));
    assert!(apart(&first, &world_points(&mut d, arm)) > 0.5, "and the animation does move it");
    // Importing again and exporting that gives the same clip: a second generation loses nothing more.
    let again = dir.join("again.glb");
    back.export_report(&again, Default::default()).unwrap();
    let doc2 = craft_gltf_anim::read(&std::fs::read(&again).unwrap(), &mut |_| None, &craft_gltf_anim::Limits::default()).unwrap();
    assert_eq!(doc2.meshes[0].positions.len(), mesh.positions.len());
    assert_eq!(doc2.animations[0].channels.len(), doc.animations[0].channels.len());
    for (x, y) in doc2.animations[0].channels.iter().zip(&doc.animations[0].channels) {
        assert_eq!(x.path, y.path);
        assert!(x.values.iter().zip(&y.values).all(|(p, q)| (p - q).abs() < 2e-3 || (p + q).abs() < 2e-3), "{:?}", x.path);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn constraints_are_baked_into_the_clip_and_limits_are_said() {
    let dir = std::env::temp_dir().join(format!("modelcraft-gltf-ik-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ik.glb");
    let mut d = Document::new();
    let arm = character(&mut d);
    let joints: Vec<ObjectId> = d.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Joint { .. })).map(|o| o.id).collect();
    // IK on top of the curves, with an animated goal; plus what glTF cannot hold.
    let goal = d.execute(Command::Rig(R::AddIk { end: joints[2], goal: None, pole: None, bones: 2, solver: IkSolver::Auto, name: None })).unwrap().created[0];
    key(&mut d, goal, "translate.x", 0.0, 0.0);
    key(&mut d, goal, "translate.x", 12.0, 2.5);
    d.execute(Command::Rig(R::AddLattice { mesh: arm, divisions: [2, 2, 2], linear: false })).unwrap();
    let wide = d.execute(Command::AddPrimitive { kind: "cylinder".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::SetPrimitive { id: wide, primitive: Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 10, rings: 8, caps: true } }).unwrap();
    d.execute(Command::Rig(R::AddShape { mesh: arm, from: wide, name: None, target: Some(0), at: 0.5 })).unwrap();
    d.execute(Command::Delete { ids: vec![wide] }).unwrap();
    let warnings = d.export_report(&path, Default::default()).unwrap();
    assert!(warnings.iter().any(|w| w.contains("lattices and clusters")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("in-between")), "{warnings:?}");
    let mut back = Document::new();
    back.execute(Command::Anim(A::SetTimeRange { start: Some(0.0), end: Some(12.0), fps: Some(24.0) })).unwrap();
    back.execute(Command::Import { path: path.display().to_string(), up: Default::default(), scale: None }).unwrap();
    assert!(back.scene.constraints.is_empty(), "the file holds the result, not the constraint");
    // The joints end up where the IK put them, frame by frame.
    for frame in [0.0, 4.0, 8.0, 12.0] {
        d.set_frame(frame);
        back.set_frame(frame);
        for (i, j) in joints.iter().enumerate() {
            let there = back.scene.objects().iter().find(|o| o.name == format!("J{i}")).map(|o| o.id).unwrap();
            let (a, b) = (d.scene.world_matrix(*j).transform_point3(Vec3::ZERO), back.scene.world_matrix(there).transform_point3(Vec3::ZERO));
            assert!(a.distance(b) < 2e-3, "frame {frame}, joint {i}: {a:?} {b:?}");
        }
    }
    // A scene without rig or animation still goes through the static writer, and a static file
    // through the static reader (one mesh object, no group of joints).
    let mut plain = Document::new();
    let cube = plain.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    plain.execute(Command::AddStep { id: cube, op: StepOp::move_vertices(vec![0], Vec3::X, Quat::IDENTITY, Vec3::ONE) }).unwrap();
    let still = dir.join("still.glb");
    assert!(plain.export_report(&still, Default::default()).unwrap().is_empty());
    assert!(!modelcraft_formats::gltf_rig::file_has_rig(&std::fs::read(&still).unwrap()));
    // Garbage with a glTF name fails with a message and changes nothing.
    let junk = dir.join("junk.glb");
    std::fs::write(&junk, b"glTF\x02\0\0\0 not really").unwrap();
    let before = back.scene.clone();
    assert!(back.execute(Command::Import { path: junk.display().to_string(), up: Default::default(), scale: None }).is_err());
    assert!(back.scene == before);
    let _ = std::fs::remove_dir_all(&dir);
}
