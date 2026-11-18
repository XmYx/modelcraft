//! UDIM layout export from a document.

use super::*;
use modelcraft_scene::StepOp;

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
