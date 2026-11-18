use glam::{Quat, Vec3};
use modelcraft_scene::attr;
use modelcraft_scene::organize::{DisplayMode, PresetParts, Tag, UserValue};
use modelcraft_scene::{ObjectId, ObjectKind, Transform};

use crate::{Command, Document};

fn run(d: &mut Document, json: &str) -> crate::Outcome {
    d.execute(serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"))).unwrap_or_else(|e| panic!("{json}: {e}"))
}

fn refused(d: &mut Document, json: &str) -> String {
    let before = d.scene.clone();
    let e = d.execute(serde_json::from_str(json).unwrap()).expect_err(json).to_string();
    assert!(d.scene == before, "a refused command changes nothing: {json}");
    e
}

fn cube(d: &mut Document, name: &str) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "cube".into(), name: Some(name.into()), parent: None }).unwrap().created[0]
}

fn move_to(d: &mut Document, id: ObjectId, at: Vec3) {
    d.execute(Command::SetTransform { id, transform: Transform { translation: at, ..Default::default() } }).unwrap();
}

fn verts(d: &mut Document, id: ObjectId) -> Vec<Vec3> {
    d.eval.evaluate(&d.scene, id).mesh.positions().to_vec()
}

// ---- tags ----

#[test]
fn a_protection_tag_refuses_the_channels_it_names_and_lets_others_through() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"protection","position":true,"rotation":false,"scale":false}}}}"#, a.0));
    let e = d.execute(Command::SetTransform { id: a, transform: Transform { translation: Vec3::X, ..Default::default() } }).unwrap_err().to_string();
    assert!(e.contains("position"), "{e}");
    assert_eq!(d.scene.get(a).unwrap().transform.translation, Vec3::ZERO);
    // Rotation is not protected.
    d.execute(Command::SetTransform { id: a, transform: Transform { rotation: Quat::from_rotation_y(0.5), ..Default::default() } }).unwrap();
    // Taking the tag off frees the position again, and one undo puts it back on.
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"remove_tag","ids":[{}],"kind":"protection"}}"#, a.0));
    move_to(&mut d, a, Vec3::X);
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.tag(a, modelcraft_scene::TagKind::Protection).is_some());
}

#[test]
fn a_render_tag_keeps_an_object_in_the_viewport_but_out_of_renders_and_exports() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let b = cube(&mut d, "B");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"render","visible":false}}}}"#, b.0));
    assert!(d.scene.visible(b) && !d.scene.renders(b) && d.scene.renders(a));
    let names: Vec<String> = d.export_items().into_iter().map(|i| i.name).collect();
    assert_eq!(names, ["A"], "exports follow what renders");
}

#[test]
fn a_display_tag_sets_one_objects_mode_and_replaces_its_own_kind() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    assert_eq!(d.scene.display_mode_of(a), DisplayMode::Auto);
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"display","mode":"wireframe"}}}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"display","mode":"bounding_box"}}}}"#, a.0));
    assert_eq!(d.scene.display_mode_of(a), DisplayMode::BoundingBox);
    assert_eq!(d.scene.get(a).unwrap().tags.len(), 1, "one tag of each kind");
}

#[test]
fn a_look_at_tag_keeps_facing_its_target_and_goes_when_the_target_does() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let t = cube(&mut d, "Target");
    move_to(&mut d, t, Vec3::new(5.0, 0.0, 0.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"look_at","target":{}}}}}"#, a.0, t.0));
    let facing = |d: &Document| d.scene.world_matrix(a).transform_vector3(Vec3::Z).normalize();
    assert!((facing(&d) - Vec3::X).length() < 1e-3, "{:?}", facing(&d));
    move_to(&mut d, t, Vec3::new(0.0, 0.0, -5.0));
    assert!((facing(&d) + Vec3::Z).length() < 1e-3, "follows the target: {:?}", facing(&d));
    refused(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"look_at","target":{}}}}}"#, a.0, a.0));
    d.execute(Command::Delete { ids: vec![t] }).unwrap();
    assert!(d.scene.tag(a, modelcraft_scene::TagKind::LookAt).is_none());
    assert!(d.scene.constraints.is_empty());
}

#[test]
fn removing_a_look_at_tag_removes_its_constraint() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let t = cube(&mut d, "T");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"look_at","target":{}}}}}"#, a.0, t.0));
    assert_eq!(d.scene.constraints.list().len(), 1);
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"remove_tag","ids":[{}],"kind":"look_at"}}"#, a.0));
    assert!(d.scene.constraints.is_empty());
    // Replacing the target does not pile constraints up.
    for _ in 0..3 {
        run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"look_at","target":{}}}}}"#, a.0, t.0));
    }
    assert_eq!(d.scene.constraints.list().len(), 1);
}

// ---- layers ----

#[test]
fn layers_hide_render_and_lock_their_objects_together() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let b = cube(&mut d, "B");
    let c = cube(&mut d, "C");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"add_layer","name":"Props","ids":[{},{}]}}"#, a.0, b.0));
    let layer = d.scene.layers[0].id;
    assert_eq!(d.scene.layer_of(a).map(|l| l.name.as_str()), Some("Props"));
    // Viewport-only.
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"edit_layer","id":{layer},"render":false}}"#));
    assert!(d.scene.visible(a) && !d.scene.renders(a) && !d.scene.renders(b) && d.scene.renders(c));
    // Hidden.
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"edit_layer","id":{layer},"visible":false,"render":true}}"#));
    assert!(!d.scene.visible(a) && !d.scene.renders(a) && d.scene.visible(c));
    // Locked: edits are refused for every object on it.
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"edit_layer","id":{layer},"visible":true,"locked":true}}"#));
    let e = d.execute(Command::Rename { id: b, name: "B2".into() }).unwrap_err().to_string();
    assert!(e.contains("locked"), "{e}");
    d.execute(Command::Rename { id: c, name: "C2".into() }).unwrap();
    // Removing the layer frees its objects.
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"remove_layer","id":{layer}}}"#));
    assert!(d.scene.layers.is_empty() && d.scene.get(a).unwrap().layer.is_none());
    d.execute(Command::Rename { id: b, name: "B2".into() }).unwrap();
}

#[test]
fn a_hidden_layer_hides_the_children_of_its_objects_and_layers_come_back_with_undo() {
    let mut d = Document::new();
    let g = d.execute(Command::AddGroup { name: Some("G".into()) }).unwrap().created[0];
    let child = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: Some(g) }).unwrap().created[0];
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"add_layer","name":"Back","ids":[{}]}}"#, g.0));
    run(&mut d, r#"{"cmd":"organize","op":"edit_layer","id":1,"visible":false}"#);
    assert!(!d.scene.visible(child));
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.visible(child));
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.layers.is_empty());
    refused(&mut d, r#"{"cmd":"organize","op":"assign_layer","ids":[1],"layer":9}"#);
    refused(&mut d, r#"{"cmd":"organize","op":"edit_layer","id":9,"visible":false}"#);
}

#[test]
fn layers_tags_and_user_attributes_survive_saving_and_hostile_files_are_repaired() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"add_layer","name":"L","ids":[{}]}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"display","mode":"flat"}}}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_user_attr","ids":[{}],"name":"mass","value":{{"type":"number","value":2.5}}}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_pivot","ids":[{}],"pivot":[0.0,1.0,0.0]}}"#, a.0));
    let json = serde_json::to_string(&d.scene).unwrap();
    let back: modelcraft_scene::Scene = serde_json::from_str(&json).unwrap();
    let back = back.repaired().unwrap();
    assert_eq!(back.get(a).unwrap().user.get("mass"), Some(&UserValue::Number(2.5)));
    assert_eq!(back.get(a).unwrap().pivot, Vec3::Y);
    assert_eq!(back.layers.len(), 1);
    assert_eq!(back.display_mode_of(a), DisplayMode::Flat);
    // Hostile: two tags of one kind, a missing layer, a bad attribute name, a self-instance.
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let o = &mut v["objects"][0];
    o["tags"] = serde_json::json!([{"type":"display","mode":"flat"},{"type":"display","mode":"wireframe"}]);
    o["layer"] = serde_json::json!(77);
    o["user"] = serde_json::json!({"a.b": {"type":"number","value":1.0}, "ok": {"type":"number","value":1.0}});
    let s: modelcraft_scene::Scene = serde_json::from_value(v).unwrap();
    let s = s.repaired().unwrap();
    let o = s.get(a).unwrap();
    assert_eq!(o.tags.len(), 1);
    assert_eq!(o.layer, None);
    assert_eq!(o.user.keys().collect::<Vec<_>>(), ["ok"]);
}

// ---- user attributes and presets ----

#[test]
fn user_attributes_are_named_values_that_numbers_can_be_keyed_through() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let b = cube(&mut d, "B");
    for (name, value) in [("mass", r#"{"type":"number","value":3.0}"#), ("armed", r#"{"type":"switch","value":true}"#), ("label", r#"{"type":"text","value":"hello"}"#), ("offset", r#"{"type":"vector","value":[1.0,2.0,3.0]}"#)] {
        run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_user_attr","ids":[{},{}],"name":"{name}","value":{value}}}"#, a.0, b.0));
    }
    let o = d.scene.get(a).unwrap();
    assert_eq!(attr::get(o, "user.mass"), Some(3.0));
    assert_eq!(attr::get(o, "user.armed"), Some(1.0));
    assert_eq!(attr::get(o, "user.offset.y"), Some(2.0));
    assert_eq!(attr::get(o, "user.label"), None, "text is not a number");
    let listed: Vec<String> = attr::list(o).into_iter().map(|(p, _)| p).collect();
    assert!(listed.contains(&"user.mass".to_string()) && listed.contains(&"user.offset.z".to_string()) && !listed.contains(&"user.label".to_string()));
    // A number is keyable like any other attribute, and the curve drives it.
    d.execute(Command::Anim(crate::AnimCommand::SetKey { id: a, path: "user.mass".into(), frame: Some(0.0), value: Some(1.0) })).unwrap();
    d.execute(Command::Anim(crate::AnimCommand::SetKey { id: a, path: "user.mass".into(), frame: Some(10.0), value: Some(11.0) })).unwrap();
    d.execute(Command::Anim(crate::AnimCommand::SetTime { frame: 5.0 })).unwrap();
    assert_eq!(attr::get(d.scene.get(a).unwrap(), "user.mass"), Some(6.0));
    // Bad names, too many attributes and unknown removals are refused without changes.
    for bad in ["a.b", "", "a/b", &"n".repeat(65)] {
        let cmd = Command::Organize(crate::OrganizeCommand::SetUserAttr { ids: vec![a], name: bad.into(), value: UserValue::Number(1.0) });
        assert!(d.execute(cmd).is_err(), "{bad:?}");
    }
    refused(&mut d, &format!(r#"{{"cmd":"organize","op":"remove_user_attr","ids":[{}],"name":"nope"}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"remove_user_attr","ids":[{}],"name":"mass"}}"#, a.0));
    assert!(!d.scene.get(a).unwrap().user.contains_key("mass") && d.scene.get(b).unwrap().user.contains_key("mass"));
    for i in 0..70 {
        let _ = d.execute(Command::Organize(crate::OrganizeCommand::SetUserAttr { ids: vec![b], name: format!("n{i}"), value: UserValue::Switch(true) }));
    }
    assert!(d.scene.get(b).unwrap().user.len() <= modelcraft_scene::organize::MAX_USER_ATTRS);
}

#[test]
fn a_preset_carries_the_chosen_parts_to_other_objects() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let b = cube(&mut d, "B");
    d.execute(Command::SetMaterial { id: a, material: modelcraft_scene::Material { base_color: [1.0, 0.0, 0.0, 1.0], metallic: 1.0, ..Default::default() } }).unwrap();
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_user_attr","ids":[{}],"name":"cost","value":{{"type":"number","value":9.0}}}}"#, a.0));
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_tag","ids":[{}],"tag":{{"type":"render","visible":false}}}}"#, a.0));
    move_to(&mut d, a, Vec3::new(4.0, 0.0, 0.0));
    d.execute(Command::Organize(crate::OrganizeCommand::SavePreset { from: a, name: "Red".into(), parts: None })).unwrap();
    assert_eq!(d.scene.presets.len(), 1);
    assert!(d.scene.presets[0].transform.is_none(), "the default parts leave the placement out");
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"apply_preset","index":0,"ids":[{}]}}"#, b.0));
    let o = d.scene.get(b).unwrap();
    assert_eq!(o.material.metallic, 1.0);
    assert_eq!(o.user.get("cost"), Some(&UserValue::Number(9.0)));
    assert!(matches!(o.tags.first(), Some(Tag::Render { visible: false })));
    assert_eq!(o.transform.translation, Vec3::ZERO, "the placement stays");
    // Saving under the same name replaces it; with the transform part it moves objects.
    d.execute(Command::Organize(crate::OrganizeCommand::SavePreset { from: a, name: "Red".into(), parts: Some(PresetParts { transform: true, ..Default::default() }) })).unwrap();
    assert_eq!(d.scene.presets.len(), 1);
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"apply_preset","index":0,"ids":[{}]}}"#, b.0));
    assert_eq!(d.scene.get(b).unwrap().transform.translation.x, 4.0);
    refused(&mut d, r#"{"cmd":"organize","op":"apply_preset","index":5,"ids":[1]}"#);
    run(&mut d, r#"{"cmd":"organize","op":"remove_preset","index":0}"#);
    assert!(d.scene.presets.is_empty());
}

// ---- pivots ----

#[test]
fn moving_the_pivot_keeps_the_object_in_place_and_rotation_turns_about_it() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    let before = verts(&mut d, a).iter().map(|p| d.scene.world_matrix(a).transform_point3(*p)).collect::<Vec<_>>();
    // A rotated, scaled, moved object: the keep-in-place compensation has to include all three.
    d.execute(Command::SetTransform { id: a, transform: Transform { translation: Vec3::new(1.0, 2.0, 3.0), rotation: Quat::from_rotation_y(0.7), scale: Vec3::new(2.0, 1.0, 0.5) } }).unwrap();
    let world = |d: &mut Document| -> Vec<Vec3> {
        let m = d.scene.world_matrix(a);
        verts(d, a).iter().map(|p| m.transform_point3(*p)).collect()
    };
    let placed = world(&mut d);
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"set_pivot","ids":[{}],"pivot":[0.5,-1.0,0.25]}}"#, a.0));
    for (x, y) in world(&mut d).iter().zip(&placed) {
        assert!((*x - *y).length() < 1e-4, "keep_in_place: {x:?} vs {y:?}");
    }
    assert_ne!(d.scene.get(a).unwrap().transform.translation, Vec3::new(1.0, 2.0, 3.0), "the translation compensates");
    // The pivot is the fixed point of a rotation: its world position does not move.
    let o = d.scene.get(a).unwrap();
    let pivot_world = d.scene.world_matrix(a).transform_point3(o.pivot);
    assert!((pivot_world - o.transform.translation).length() < 1e-4, "translation places the pivot");
    d.execute(Command::SetTransform { id: a, transform: Transform { rotation: Quat::from_rotation_z(1.2), ..d.scene.get(a).unwrap().transform } }).unwrap();
    let after = d.scene.world_matrix(a).transform_point3(d.scene.get(a).unwrap().pivot);
    assert!((after - pivot_world).length() < 1e-4, "rotation about the pivot leaves the pivot where it was");
    // Without keep_in_place the object moves with the pivot.
    let _ = before;
    let mut e = Document::new();
    let b = cube(&mut e, "B");
    let first = e.scene.world_matrix(b).transform_point3(verts(&mut e, b)[0]);
    run(&mut e, &format!(r#"{{"cmd":"organize","op":"set_pivot","ids":[{}],"pivot":[1.0,0.0,0.0],"keep_in_place":false}}"#, b.0));
    let moved = e.scene.world_matrix(b).transform_point3(verts(&mut e, b)[0]);
    assert!(((first - moved) - Vec3::X).length() < 1e-5, "{first:?} {moved:?}");
}

#[test]
fn the_pivot_goes_to_the_bounding_box_centre_and_exports_carry_it_in_the_transform() {
    let mut d = Document::new();
    let a = cube(&mut d, "A");
    // Pull one face out so the centre is not the origin.
    let top: Vec<u32> = verts(&mut d, a).iter().enumerate().filter(|(_, p)| p.y > 0.0).map(|(i, _)| i as u32).collect();
    d.execute(Command::AddStep { id: a, op: modelcraft_scene::StepOp::MoveVertices { verts: top, translate: Vec3::Y * 2.0, rotate: Quat::IDENTITY, scale: Vec3::ONE } }).unwrap();
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"pivot_to_center","ids":[{}]}}"#, a.0));
    let o = d.scene.get(a).unwrap();
    assert!((o.pivot - Vec3::Y).length() < 1e-4, "{:?}", o.pivot);
    // Exchange formats have no pivot: the item's transform alone gives the same placement.
    let items = d.export_items();
    let t = items[0].transform;
    let pivoted = d.scene.get(a).unwrap().local_matrix();
    assert!((t.matrix() - pivoted).abs_diff_eq(glam::Mat4::ZERO, 1e-5));
    // A group has nothing to centre on.
    let g = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    refused(&mut d, &format!(r#"{{"cmd":"organize","op":"pivot_to_center","ids":[{}]}}"#, g.0));
}

// ---- instances ----

#[test]
fn an_instance_shares_the_masters_mesh_and_follows_its_edits() {
    let mut d = Document::new();
    let m = cube(&mut d, "Master");
    let id = run(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, m.0)).created[0];
    assert!(matches!(d.scene.get(id).unwrap().kind, ObjectKind::Instance { of } if of == m));
    assert_eq!(d.scene.get(id).unwrap().kind.type_name(), "Instance");
    let a = d.eval.evaluate(&d.scene, m).mesh;
    let b = d.eval.evaluate(&d.scene, id).mesh;
    assert!(std::sync::Arc::ptr_eq(&a, &b), "the same allocation, not a copy");
    // An instance of an instance points at the master.
    let id2 = run(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, id.0)).created[0];
    assert!(matches!(d.scene.get(id2).unwrap().kind, ObjectKind::Instance { of } if of == m));
    // Editing the master changes what the instances show; moving an instance does not touch it.
    d.execute(Command::AddStep { id: m, op: modelcraft_scene::StepOp::Subdivide { levels: 1 } }).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, id).mesh.face_count(), d.eval.evaluate(&d.scene, m).mesh.face_count());
    assert!(d.eval.evaluate(&d.scene, id2).mesh.face_count() > 6);
    move_to(&mut d, id, Vec3::new(3.0, 0.0, 0.0));
    assert_eq!(d.scene.get(m).unwrap().transform.translation, Vec3::ZERO);
    // Only geometry can be instanced, and an instance can be duplicated.
    let g = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    refused(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, g.0));
    let copy = d.execute(Command::Duplicate { id }).unwrap().created[0];
    assert!(matches!(d.scene.get(copy).unwrap().kind, ObjectKind::Instance { of } if of == m));
}

#[test]
fn make_unique_and_deleting_the_master_turn_instances_into_real_meshes() {
    let mut d = Document::new();
    let m = cube(&mut d, "Master");
    let i1 = run(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, m.0)).created[0];
    let i2 = run(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, m.0)).created[0];
    run(&mut d, &format!(r#"{{"cmd":"organize","op":"make_unique","ids":[{}]}}"#, i1.0));
    assert!(matches!(d.scene.get(i1).unwrap().kind, ObjectKind::Mesh { .. }));
    d.execute(Command::AddStep { id: m, op: modelcraft_scene::StepOp::Subdivide { levels: 1 } }).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, i1).mesh.face_count(), 6, "no longer follows the master");
    refused(&mut d, &format!(r#"{{"cmd":"organize","op":"make_unique","ids":[{}]}}"#, m.0));
    // Deleting the master keeps what the remaining instance showed.
    let faces = d.eval.evaluate(&d.scene, m).mesh.face_count();
    d.execute(Command::Delete { ids: vec![m] }).unwrap();
    assert!(matches!(d.scene.get(i2).unwrap().kind, ObjectKind::Mesh { .. }));
    assert_eq!(d.eval.evaluate(&d.scene, i2).mesh.face_count(), faces);
    // And one undo brings the master and the instance back.
    d.execute(Command::Undo).unwrap();
    assert!(matches!(d.scene.get(i2).unwrap().kind, ObjectKind::Instance { .. }));
}

#[test]
fn a_cycle_of_instances_is_an_empty_group_not_a_hang() {
    let mut d = Document::new();
    let m = cube(&mut d, "M");
    let i = run(&mut d, &format!(r#"{{"cmd":"organize","op":"instance","ids":[{}]}}"#, m.0)).created[0];
    let json = serde_json::to_string(&d.scene).unwrap().replace(&format!(r#""of":{}"#, m.0), &format!(r#""of":{}"#, i.0));
    let s: modelcraft_scene::Scene = serde_json::from_str(&json).unwrap();
    let mut s = s.repaired().unwrap();
    let mut ev = modelcraft_scene::Evaluator::new();
    let e = ev.evaluate(&s, i);
    assert_eq!(e.mesh.vertex_count(), 0);
    let _ = s.edit(i);
    assert!(ev.evaluate(&s, i).mesh.vertex_count() == 0);
}

#[test]
fn every_organize_command_round_trips_through_json_and_is_listed() {
    let cmds = vec![
        r#"{"cmd":"organize","op":"set_tag","ids":[1],"tag":{"type":"look_at","target":2}}"#,
        r#"{"cmd":"organize","op":"remove_tag","ids":[1],"kind":"display"}"#,
        r#"{"cmd":"organize","op":"add_layer","name":"L"}"#,
        r#"{"cmd":"organize","op":"edit_layer","id":1,"name":"M","color":[1.0,0.0,0.0],"visible":false}"#,
        r#"{"cmd":"organize","op":"remove_layer","id":1}"#,
        r#"{"cmd":"organize","op":"assign_layer","ids":[1],"layer":null}"#,
        r#"{"cmd":"organize","op":"set_user_attr","ids":[1],"name":"c","value":{"type":"colour","value":[0.1,0.2,0.3]}}"#,
        r#"{"cmd":"organize","op":"remove_user_attr","ids":[1],"name":"c"}"#,
        r#"{"cmd":"organize","op":"save_preset","from":1,"name":"P"}"#,
        r#"{"cmd":"organize","op":"apply_preset","index":0,"ids":[1]}"#,
        r#"{"cmd":"organize","op":"remove_preset","index":0}"#,
        r#"{"cmd":"organize","op":"set_pivot","ids":[1],"pivot":[0.0,0.0,0.0]}"#,
        r#"{"cmd":"organize","op":"pivot_to_center","ids":[1]}"#,
        r#"{"cmd":"organize","op":"instance","ids":[1]}"#,
        r#"{"cmd":"organize","op":"make_unique","ids":[1]}"#,
    ];
    assert_eq!(cmds.len(), crate::organize::COMMANDS.len());
    for (json, (op, _)) in cmds.iter().zip(crate::organize::COMMANDS) {
        let c: Command = serde_json::from_str(json).unwrap_or_else(|e| panic!("{json}: {e}"));
        assert!(json.contains(&format!("\"op\":\"{op}\"")), "{op}");
        let back: Command = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert!(!c.label().is_empty());
    }
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    fn v3() -> impl Strategy<Value = Vec3> {
        (-50.0f32..50.0, -50.0f32..50.0, -50.0f32..50.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    proptest! {
        /// Whatever the placement, folding the pivot into the transform gives the same matrix, and
        /// moving the pivot while keeping the object in place never moves a point.
        #[test]
        fn the_pivot_never_moves_the_object_unless_asked(t in v3(), s in (0.1f32..4.0, 0.1f32..4.0, 0.1f32..4.0), axis in v3(), angle in -3.0f32..3.0, p0 in v3(), p1 in v3(), x in v3()) {
            let mut d = Document::new();
            let a = cube(&mut d, "A");
            let rotation = Quat::from_axis_angle(axis.try_normalize().unwrap_or(Vec3::Y), angle);
            d.execute(Command::SetTransform { id: a, transform: Transform { translation: t, rotation, scale: Vec3::new(s.0, s.1, s.2) } }).unwrap();
            d.execute(Command::Organize(crate::OrganizeCommand::SetPivot { ids: vec![a], pivot: p0.to_array(), keep_in_place: true })).unwrap();
            let at = |d: &Document| d.scene.world_matrix(a).transform_point3(x);
            let first = at(&d);
            let folded = d.scene.get(a).unwrap().effective_transform().matrix().transform_point3(x);
            prop_assert!((first - folded).length() < 1e-2 * (1.0 + first.length()));
            d.execute(Command::Organize(crate::OrganizeCommand::SetPivot { ids: vec![a], pivot: p1.to_array(), keep_in_place: true })).unwrap();
            let second = at(&d);
            prop_assert!((first - second).length() < 1e-2 * (1.0 + first.length()), "{first:?} {second:?}");
        }
    }
}
