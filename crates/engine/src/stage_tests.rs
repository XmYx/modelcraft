use glam::Vec3;
use modelcraft_scene::stage::{CameraData, LightData, LightKind};
use modelcraft_scene::{ObjectKind, Transform};

use crate::anim::AnimCommand as A;
use crate::stage::StageCommand as S;
use crate::{Command, Document};

fn light(d: &mut Document, kind: &str) -> modelcraft_scene::ObjectId {
    d.execute(Command::Stage(S::AddLight { kind: kind.into(), name: None, parent: None, transform: None })).unwrap().created[0]
}

fn camera(d: &mut Document) -> modelcraft_scene::ObjectId {
    d.execute(Command::Stage(S::AddCamera { name: None, parent: None, transform: None })).unwrap().created[0]
}

#[test]
fn every_light_kind_is_added_named_and_undoable() {
    let mut d = Document::new();
    for kind in LightKind::ALL {
        let id = light(&mut d, kind.id());
        let o = d.scene.get(id).unwrap();
        assert!(matches!(&o.kind, ObjectKind::Light { light } if light.kind == kind));
        assert_eq!(o.kind.type_name(), kind.title());
    }
    assert_eq!(d.scene.lights().len(), 5);
    let names: std::collections::HashSet<_> = d.scene.objects().iter().map(|o| o.name.clone()).collect();
    assert_eq!(names.len(), 5, "names are unique");
    assert!(d.execute(Command::Stage(S::AddLight { kind: "laser".into(), name: None, parent: None, transform: None })).is_err());
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.lights().len(), 4);
}

#[test]
fn hostile_settings_are_clamped_and_the_wrong_kind_is_refused() {
    let mut d = Document::new();
    let l = light(&mut d, "spot");
    let hostile = LightData { intensity: f32::NAN, color: [9.0, -1.0, f32::INFINITY], spread: 1e9, softness: -3.0, ..LightData::of(LightKind::Spot) };
    d.execute(Command::Stage(S::SetLight { id: l, light: hostile })).unwrap();
    let ObjectKind::Light { light: got } = &d.scene.get(l).unwrap().kind else { panic!() };
    assert_eq!((got.intensity, got.color, got.spread, got.softness), (300.0, [1.0, 0.0, 1.0], 179.0, 0.0));
    let c = camera(&mut d);
    let bad = CameraData { focal_length: -5.0, sensor_height: f32::NAN, f_stop: 1e9, ..CameraData::default() };
    d.execute(Command::Stage(S::SetCamera { id: c, camera: bad })).unwrap();
    let cam = d.scene.camera_data(c).unwrap();
    assert_eq!((cam.focal_length, cam.sensor_height, cam.f_stop), (4.0, 24.0, 64.0));
    // A camera is not a light and the other way round.
    assert!(d.execute(Command::Stage(S::SetLight { id: c, light: LightData::default() })).is_err());
    assert!(d.execute(Command::Stage(S::SetCamera { id: l, camera: CameraData::default() })).is_err());
    assert!(d.execute(Command::Stage(S::SetRenderCamera { id: Some(l) })).is_err());
    // A locked light refuses edits.
    d.execute(Command::SetLocked { ids: vec![l], locked: true }).unwrap();
    assert!(d.execute(Command::Stage(S::SetLight { id: l, light: LightData::default() })).is_err());
}

#[test]
fn the_render_camera_is_chosen_kept_and_forgotten_with_its_object() {
    let mut d = Document::new();
    let a = camera(&mut d);
    let b = camera(&mut d);
    assert_eq!(d.scene.render_camera(), None);
    d.execute(Command::Stage(S::SetRenderCamera { id: Some(b) })).unwrap();
    assert_eq!(d.scene.render_camera(), Some(b));
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.render_camera(), None);
    d.execute(Command::Redo).unwrap();
    d.execute(Command::Delete { ids: vec![b] }).unwrap();
    assert_eq!(d.scene.render_camera(), None, "a deleted camera is no longer the render camera");
    assert_eq!(d.scene.cameras(), vec![a]);
}

#[test]
fn both_kinds_survive_a_save_and_open() {
    let mut d = Document::new();
    let l = light(&mut d, "area");
    let c = camera(&mut d);
    let want = LightData { color: [1.0, 0.5, 0.25], intensity: 12.5, size: 2.0, height: 0.5, ..LightData::of(LightKind::Area) };
    d.execute(Command::Stage(S::SetLight { id: l, light: want.clone() })).unwrap();
    let lens = CameraData { focal_length: 85.0, f_stop: 2.8, focus_distance: 4.0, ..CameraData::default() };
    d.execute(Command::Stage(S::SetCamera { id: c, camera: lens.clone() })).unwrap();
    d.execute(Command::Stage(S::SetRenderCamera { id: Some(c) })).unwrap();
    let bytes = modelcraft_formats::project::to_bytes(&d.scene).unwrap();
    let back = modelcraft_formats::project::from_bytes(&bytes).unwrap();
    assert!(matches!(&back.get(l).unwrap().kind, ObjectKind::Light { light } if *light == want));
    assert_eq!(back.camera_data(c), Some(&lens));
    assert_eq!(back.render_camera(), Some(c));
    assert_eq!(back.get(c).unwrap().transform, d.scene.get(c).unwrap().transform);
}

#[test]
fn every_number_is_keyable_and_a_camera_follows_its_keys() {
    let mut d = Document::new();
    let c = camera(&mut d);
    let l = light(&mut d, "point");
    for path in ["camera.focal_length", "camera.focus_distance", "camera.f_stop", "camera.sensor_width"] {
        assert!(d.scene.attr(c, path).is_some(), "{path} is an attribute");
    }
    for path in ["light.intensity", "light.color.0", "light.spread", "light.size"] {
        assert!(d.scene.attr(l, path).is_some(), "{path} is an attribute");
    }
    for (frame, value) in [(0.0, 24.0), (10.0, 100.0)] {
        d.execute(Command::Anim(A::SetKey { id: c, path: "camera.focal_length".into(), frame: Some(frame), value: Some(value) })).unwrap();
    }
    d.set_frame(10.0);
    assert_eq!(d.scene.camera_data(c).unwrap().focal_length, 100.0);
    d.set_frame(0.0);
    assert_eq!(d.scene.camera_data(c).unwrap().focal_length, 24.0);
    // A curve that asks for nonsense is held in range.
    d.execute(Command::Anim(A::SetKey { id: l, path: "light.spread".into(), frame: Some(0.0), value: Some(5000.0) })).unwrap();
    d.set_frame(0.0);
    let ObjectKind::Light { light } = &d.scene.get(l).unwrap().kind else { panic!() };
    assert_eq!(light.spread, 179.0);
    assert!(d.scene.set_attr(l, "light.intensity", f64::NAN).is_err());
}

#[test]
fn default_placements_face_the_scene() {
    let mut d = Document::new();
    let c = camera(&mut d);
    let (p, _) = d.scene.camera_view(c).unwrap();
    // The camera starts back and up, looking at the origin.
    let to_origin = (Vec3::ZERO - p.eye).normalize();
    assert!(p.forward.dot(to_origin) > 0.999, "{:?} vs {to_origin:?}", p.forward);
    let spot = light(&mut d, "spot");
    let m = d.scene.world_matrix(spot);
    assert!((m.transform_vector3(Vec3::NEG_Z) - Vec3::NEG_Y).length() < 1e-5, "a spot starts pointing down");
    let at = Transform { translation: Vec3::new(1.0, 2.0, 3.0), ..Transform::default() };
    let l = d.execute(Command::Stage(S::AddLight { kind: "point".into(), name: Some("Lamp".into()), parent: None, transform: Some(at) })).unwrap().created[0];
    assert_eq!((d.scene.get(l).unwrap().name.as_str(), d.scene.get(l).unwrap().transform.translation), ("Lamp", Vec3::new(1.0, 2.0, 3.0)));
}
