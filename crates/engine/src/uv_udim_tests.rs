//! UDIM layout export and UV transfer in a document.

use std::path::PathBuf;

use super::*;
use modelcraft_scene::{ObjectId, StepOp};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-engine-udim-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

#[test]
fn a_layout_over_tiles_exports_one_picture_per_tile() {
    let dir = tmp("tiles");
    let mut d = Document::new();
    let cube = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    // Six islands dealt over three tiles.
    d.execute(Command::AddStep {
        id: cube,
        op: StepOp::PackUvs {
            margin: 0.01,
            rotation: modelcraft_scene::Rotation::Fit,
            density: modelcraft_scene::Density::Uniform,
            tiles: 3,
            texel_density: None,
        },
    })
    .unwrap();
    for ext in ["svg", "png"] {
        let files = d.export_uv_layout_udim(&dir.join(format!("cube.{ext}")), None, 128).unwrap();
        let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [format!("cube.1001.{ext}"), format!("cube.1002.{ext}"), format!("cube.1003.{ext}")]);
        assert!(files.iter().all(|f| std::fs::metadata(f).map(|m| m.len() > 50).unwrap_or(false)));
    }
    // Every face is drawn in exactly one of the three pictures.
    let drawn: usize = ["1001", "1002", "1003"]
        .iter()
        .map(|n| std::fs::read_to_string(dir.join(format!("cube.{n}.svg"))).unwrap().lines().filter(|l| l.starts_with("<path")).count())
        .sum();
    assert_eq!(drawn, 6);
    // Bad extension and a missing object are errors; a mesh with nothing in a tile writes none.
    assert!(d.export_uv_layout_udim(&dir.join("cube.jpg"), None, 64).is_err());
    assert!(d.export_uv_layout_udim(&dir.join("x.svg"), Some(ObjectId(99)), 64).is_err());
}

#[test]
fn transfer_uvs_gives_one_object_the_layout_of_another_through_world_space() {
    let mut d = Document::new();
    let add = |d: &mut Document, kind: &str| d.execute(Command::AddPrimitive { kind: kind.into(), name: None, parent: None }).unwrap().created[0];
    let source = add(&mut d, "sphere");
    let target = add(&mut d, "sphere");
    d.execute(Command::AddStep {
        id: source,
        op: StepOp::Unwrap { seams: vec![], angle: None, pins: vec![], auto_cut: true, margin: 0.01, method: Default::default(), from_patches: false },
    })
    .unwrap();
    // Move the target away and scale it: the lookup is in world space.
    d.execute(Command::SetTransform {
        id: target,
        transform: modelcraft_scene::Transform { translation: glam::Vec3::new(10.0, 0.0, 0.0), scale: glam::Vec3::splat(2.0), ..Default::default() },
    })
    .unwrap();
    d.execute(Command::SetTransform {
        id: source,
        transform: modelcraft_scene::Transform { translation: glam::Vec3::new(10.0, 0.0, 0.0), scale: glam::Vec3::splat(2.0), ..Default::default() },
    })
    .unwrap();
    let before = d.undo.len();
    let out = d.execute(Command::TransferUvs { id: target, source, max_distance: None, per_island: true }).unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
    assert_eq!(d.undo.len(), before + 1, "one undo step");
    let (a, b) = (d.eval.evaluate(&d.scene, source).mesh, d.eval.evaluate(&d.scene, target).mesh);
    assert!(b.has_uvs());
    // Same shape in the same place: the same UVs at every corner.
    let worst = (0..a.corner_count()).map(|c| (a.corner_uvs()[c] - b.corner_uvs()[c]).length()).fold(0.0f32, f32::max);
    assert!(worst < 1e-3, "{worst}");
    // Undo and redo.
    d.execute(Command::Undo).unwrap();
    assert_ne!(d.eval.evaluate(&d.scene, target).mesh.corner_uvs(), b.corner_uvs());
    d.execute(Command::Redo).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, target).mesh.corner_uvs(), b.corner_uvs());
    // Refusals: itself, a missing source, a bad limit, a source without UVs.
    assert!(d.execute(Command::TransferUvs { id: target, source: target, max_distance: None, per_island: true }).is_err());
    assert!(d.execute(Command::TransferUvs { id: target, source: ObjectId(999), max_distance: None, per_island: true }).is_err());
    assert!(d.execute(Command::TransferUvs { id: target, source, max_distance: Some(-1.0), per_island: true }).is_err());
    let bare_id = d
        .execute(Command::AddMesh {
            name: Some("bare".into()),
            parent: None,
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            faces: vec![vec![0, 1, 2]],
        })
        .unwrap()
        .created[0];
    assert!(d.execute(Command::TransferUvs { id: target, source: bare_id, max_distance: None, per_island: true }).is_err());
    // A distance limit reports the corners it left alone.
    d.execute(Command::SetTransform {
        id: target,
        transform: modelcraft_scene::Transform { translation: glam::Vec3::new(50.0, 0.0, 0.0), ..Default::default() },
    })
    .unwrap();
    let out = d.execute(Command::TransferUvs { id: target, source, max_distance: Some(1.0), per_island: true }).unwrap();
    assert!(out.warnings.iter().any(|w| w.contains("further than")), "{:?}", out.warnings);
}
