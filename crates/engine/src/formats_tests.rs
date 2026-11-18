//! 3MF and USD through the document: export a scene, import the file into a new one, and compare
//! where the geometry ended up.

use glam::Vec3;

use crate::{Command, Document};

fn bounds(d: &mut Document) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::MAX);
    let mut hi = Vec3::splat(f32::MIN);
    let ids: Vec<_> = d.scene.objects().iter().map(|o| o.id).collect();
    for id in ids {
        let m = d.scene.world_matrix(id);
        for p in d.eval.evaluate(&d.scene, id).mesh.positions() {
            let w = m.transform_point3(*p);
            lo = lo.min(w);
            hi = hi.max(w);
        }
    }
    (lo, hi)
}

#[test]
fn a_scene_exported_as_3mf_or_usda_comes_back_where_it_was() {
    let dir = std::env::temp_dir().join(format!("mc-formats-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Document::new();
    let a = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::AddPrimitive { kind: "cone".into(), name: None, parent: Some(a) }).unwrap();
    d.execute(Command::SetTransform {
        id: a,
        transform: modelcraft_scene::Transform { translation: Vec3::new(1.0, 2.0, 3.0), rotation: glam::Quat::from_rotation_y(0.7), scale: Vec3::ONE },
    })
    .unwrap();
    let want = bounds(&mut d);
    for ext in ["3mf", "usda"] {
        let p = dir.join(format!("out.{ext}"));
        d.export(&p).unwrap();
        let mut e = Document::new();
        e.execute(Command::Import { path: p.display().to_string(), up: Default::default(), scale: None }).unwrap();
        let got = bounds(&mut e);
        assert!((got.0 - want.0).length() < 1e-3 && (got.1 - want.1).length() < 1e-3, "{ext}: {got:?} vs {want:?}");
    }
    // A file that is not what its name says is an error, not a scene.
    let bad = dir.join("bad.usda");
    std::fs::write(&bad, b"not usd").unwrap();
    assert!(Document::new().execute(Command::Import { path: bad.display().to_string(), up: Default::default(), scale: None }).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
