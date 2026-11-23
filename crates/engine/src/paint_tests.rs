//! Vertex colours through the document: strokes as history steps, undo per stroke, exact
//! replay, and a glTF round trip.

use glam::Vec3;
use modelcraft_scene::{ObjectId, StepOp};

use super::*;

fn mesh(d: &mut Document, id: ObjectId) -> std::sync::Arc<modelcraft_mesh::PolyMesh> {
    d.eval.evaluate(&d.scene, id).mesh
}

fn stroke(points: Vec<Vec3>) -> StepOp {
    StepOp::PaintStroke { points, radius: 0.8, strength: 1.0, color: [1.0, 0.0, 0.0, 1.0], erase: false, falloff: modelcraft_sculpt::brush::Falloff::Linear }
}

fn sphere(d: &mut Document) -> ObjectId {
    d.execute(Command::AddPrimitive { kind: "sphere".into(), name: None, parent: None }).unwrap().created[0]
}

fn red_corners(m: &modelcraft_mesh::PolyMesh) -> usize {
    m.corner_colors().iter().filter(|c| c[1] < 0.99).count()
}

#[test]
fn each_stroke_is_one_undo_step_and_replays_exactly() {
    let mut d = Document::new();
    let id = sphere(&mut d);
    let before = d.undo.len();
    d.execute(Command::AddStep { id, op: stroke(vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.9, 0.3, 0.0)]) }).unwrap();
    d.execute(Command::AddStep { id, op: stroke(vec![Vec3::new(-1.0, 0.0, 0.0)]) }).unwrap();
    assert_eq!(d.undo.len(), before + 2);
    let both = mesh(&mut d, id);
    assert!(both.has_colors() && red_corners(&both) > 0);
    // Undo takes back the last stroke only.
    d.execute(Command::Undo).unwrap();
    let one = mesh(&mut d, id);
    assert!(red_corners(&one) > 0 && red_corners(&one) < red_corners(&both));
    d.execute(Command::Redo).unwrap();
    assert_eq!(mesh(&mut d, id).corner_colors(), both.corner_colors());
    // The journal replays the same strokes: a second document built from the same commands has the same colours.
    let mut e = Document::new();
    let id2 = sphere(&mut e);
    for c in [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.9, 0.3, 0.0)].chunks(2).map(<[Vec3]>::to_vec).chain([vec![Vec3::new(-1.0, 0.0, 0.0)]]) {
        e.execute(Command::AddStep { id: id2, op: stroke(c) }).unwrap();
    }
    assert_eq!(mesh(&mut e, id2).corner_colors(), both.corner_colors());
}

#[test]
fn a_fill_paints_the_listed_vertices_or_everything() {
    let mut d = Document::new();
    let id = d.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::AddStep { id, op: StepOp::FillColor { color: [0.0, 1.0, 0.0, 1.0], verts: vec![] } }).unwrap();
    assert!(mesh(&mut d, id).corner_colors().iter().all(|c| *c == [0.0, 1.0, 0.0, 1.0]));
    d.execute(Command::AddStep { id, op: StepOp::FillColor { color: [0.0, 0.0, 1.0, 1.0], verts: vec![0] } }).unwrap();
    let m = mesh(&mut d, id);
    assert!(m.corner_colors().contains(&[0.0, 0.0, 1.0, 1.0]) && m.corner_colors().contains(&[0.0, 1.0, 0.0, 1.0]));
    // A stale vertex is a step error, not a crash.
    let out = d.execute(Command::AddStep { id, op: StepOp::FillColor { color: [1.0; 4], verts: vec![9999] } }).unwrap();
    assert!(!out.warnings.is_empty());
}

#[test]
fn colours_survive_a_glb_round_trip() {
    let dir = std::env::temp_dir().join(format!("mc-paint-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut d = Document::new();
    let id = sphere(&mut d);
    d.execute(Command::AddStep { id, op: stroke(vec![Vec3::new(1.0, 0.0, 0.0)]) }).unwrap();
    let painted = mesh(&mut d, id);
    assert!(red_corners(&painted) > 0);
    let p = dir.join("painted.glb");
    d.export(&p).unwrap();
    let json = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
    assert!(json.contains("COLOR_0"), "the file has a colour attribute");
    let mut e = Document::new();
    let out = e.execute(Command::Import { path: p.display().to_string(), up: Default::default(), scale: None }).unwrap();
    let back = mesh(&mut e, out.created[0]);
    assert!(back.has_colors(), "the colours came back");
    // The colour at a vertex is the average of its corners, so compare by position.
    let at = |m: &modelcraft_mesh::PolyMesh, p: Vec3| {
        let h = (0..m.corner_count() as u32).min_by(|a, b| m.position(m.origin(*a)).distance(p).total_cmp(&m.position(m.origin(*b)).distance(p))).unwrap();
        m.color(h)
    };
    let (near, far) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
    assert!(at(&back, near)[1] < 0.2, "painted vertex stayed red");
    assert!(at(&back, far)[1] > 0.99, "far vertex stayed white");
    // An unpainted mesh writes no colour attribute.
    let mut f = Document::new();
    sphere(&mut f);
    let q = dir.join("plain.glb");
    f.export(&q).unwrap();
    assert!(!String::from_utf8_lossy(&std::fs::read(&q).unwrap()).contains("COLOR_0"));
    let _ = std::fs::remove_dir_all(dir);
}
