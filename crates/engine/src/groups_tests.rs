use glam::{Mat4, Vec2, Vec3};
use modelcraft_scene::{ObjectId, StepOp};
use modelcraft_sculpt::{BrushKind, BrushSettings, Dab, ScreenRegion, Stroke};

use crate::{Command, DetailOp, Document, GroupOp, MaskOp, RegionAction, VisibilityOp};

fn sculpting(kind: &str) -> (Document, ObjectId) {
    let mut d = Document::new();
    let id = d.execute(Command::AddPrimitive { kind: kind.into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::BeginSculpt { id }).unwrap();
    (d, id)
}

fn front() -> Mat4 {
    Mat4::orthographic_rh(-2.0, 2.0, -2.0, 2.0, 0.1, 20.0) * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y)
}

fn patches(d: &mut Document, id: ObjectId) -> Vec<u32> {
    let m = d.eval.evaluate(&d.scene, id).mesh;
    (0..m.face_count() as u32).map(|f| m.patch(f)).collect()
}

fn distinct(p: &[u32]) -> usize {
    let mut v = p.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len()
}

#[test]
fn patches_made_while_sculpting_reach_the_object_and_undo() {
    let (mut d, id) = sculpting("cube");
    let before = patches(&mut d, id);
    d.execute(Command::SculptGroups { id, op: GroupOp::ByNormal { angle: 30.0 } }).unwrap();
    let after = patches(&mut d, id);
    assert_eq!(distinct(&after), 6);
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.face_patches(), after, "the session follows without a rebuild");
    d.execute(Command::SculptGroups { id, op: GroupOp::ByPart }).unwrap();
    assert_eq!(distinct(&patches(&mut d, id)), 1);
    d.execute(Command::Undo).unwrap();
    assert_eq!(patches(&mut d, id), after);
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.face_patches(), after);
    d.execute(Command::Undo).unwrap();
    assert_eq!(patches(&mut d, id), before);
    d.execute(Command::Redo).unwrap();
    assert_eq!(patches(&mut d, id), after);
    // From the mask: nothing masked is refused and leaves no undo step.
    let steps = d.undo.len();
    assert!(d.execute(Command::SculptGroups { id, op: GroupOp::FromMask }).is_err());
    assert_eq!(d.undo.len(), steps);
    d.execute(Command::SculptMask { id, op: MaskOp::Patch { triangle: 0 } }).unwrap();
    d.execute(Command::SculptGroups { id, op: GroupOp::FromMask }).unwrap();
    assert_eq!(distinct(&patches(&mut d, id)), 6, "the masked face moved to a new patch, its old one is gone");
    // UV islands need UVs.
    let has_uvs = d.eval.evaluate(&d.scene, id).mesh.has_uvs();
    assert_eq!(d.execute(Command::SculptGroups { id, op: GroupOp::ByUvIsland }).is_ok(), has_uvs);
}

#[test]
fn hide_isolate_invert_show_and_their_undo() {
    let (mut d, id) = sculpting("cube");
    d.execute(Command::SculptGroups { id, op: GroupOp::ByNormal { angle: 30.0 } }).unwrap();
    let hidden = |d: &Document| d.sculpt.as_ref().unwrap().mesh.hidden_triangles().iter().filter(|h| **h).count();
    let tris = d.sculpt.as_ref().unwrap().mesh.triangle_count();
    let per_side = tris / 6;
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::HidePatch { triangle: 0 } }).unwrap();
    assert_eq!(hidden(&d), per_side);
    assert!(d.live_detail_blocked(id).is_some(), "live detail waits until everything shows");
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::Invert }).unwrap();
    assert_eq!(hidden(&d), tris - per_side);
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::ShowPatch { triangle: tris as u32 - 1 } }).unwrap();
    assert_eq!(hidden(&d), tris - 2 * per_side);
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert_eq!(hidden(&d), per_side);
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::ShowAll }).unwrap();
    assert_eq!(hidden(&d), 0);
    assert!(d.live_detail_blocked(id).is_none());
    let last = tris as u32 - 1;
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::Isolate { triangle: last } }).unwrap();
    assert_eq!(hidden(&d), tris - per_side);
    // Hiding the last visible patch is refused; a missing triangle too.
    assert!(d.execute(Command::SculptVisibility { id, op: VisibilityOp::HidePatch { triangle: last } }).is_err());
    assert!(d.execute(Command::SculptVisibility { id, op: VisibilityOp::HidePatch { triangle: 1 << 30 } }).is_err());
    assert_eq!(hidden(&d), tris - per_side);
    // The object itself keeps every face: hiding is a view state.
    assert_eq!(d.eval.evaluate(&d.scene, id).mesh.triangle_count(), tris);
    // Masked and unmasked.
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::ShowAll }).unwrap();
    d.execute(Command::SculptMask { id, op: MaskOp::Patch { triangle: 0 } }).unwrap();
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::HideMasked }).unwrap();
    assert_eq!(hidden(&d), per_side);
    d.execute(Command::Undo).unwrap();
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::HideUnmasked }).unwrap();
    assert_eq!(hidden(&d), tris - per_side);
}

#[test]
fn regions_mask_and_hide_and_strokes_skip_what_is_hidden() {
    let (mut d, id) = sculpting("round_cube");
    let right = ScreenRegion::rect(front(), Vec2::new(0.013, -1.0), Vec2::new(1.0, 1.0));
    d.execute(Command::SculptRegion { id, region: right.clone(), action: RegionAction::Mask }).unwrap();
    {
        let m = &d.sculpt.as_ref().unwrap().mesh;
        for (p, k) in m.positions.iter().zip(&m.mask) {
            assert_eq!(*k > 0.5, p.x > 0.026);
        }
    }
    d.execute(Command::SculptRegion { id, region: right.clone(), action: RegionAction::Unmask }).unwrap();
    assert!(d.sculpt.as_ref().unwrap().mesh.mask.iter().all(|m| *m == 0.0));
    d.execute(Command::Undo).unwrap();
    assert!(d.sculpt.as_ref().unwrap().mesh.mask.contains(&1.0));
    d.execute(Command::Undo).unwrap();
    d.execute(Command::SculptRegion { id, region: right.clone(), action: RegionAction::Hide }).unwrap();
    let before = d.sculpt.as_ref().unwrap().mesh.positions.clone();
    let brush = BrushSettings::new(BrushKind::Draw, 0.5);
    let dab = Dab { center: Vec3::X, view_dir: -Vec3::X, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 };
    d.execute(Command::SculptStroke { id, stroke: Stroke { brush, dabs: vec![dab] } }).unwrap();
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.positions, before, "the hidden side does not move");
    d.execute(Command::SculptRegion { id, region: right.clone(), action: RegionAction::Isolate }).unwrap_err();
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::ShowAll }).unwrap();
    d.execute(Command::SculptRegion { id, region: right, action: RegionAction::Isolate }).unwrap();
    let m = &d.sculpt.as_ref().unwrap().mesh;
    assert!(m.any_hidden());
    // Bad regions are refused.
    let bad = ScreenRegion { view_proj: front(), points: vec![Vec2::ZERO], front_only: false, eye: Vec3::ZERO };
    assert!(d.execute(Command::SculptRegion { id, region: bad.clone(), action: RegionAction::Mask }).is_err());
    assert!(d.execute(Command::SculptRegion { id, region: bad, action: RegionAction::Hide }).is_err());
}

#[test]
fn patches_and_hidden_faces_follow_detail_levels() {
    let (mut d, id) = sculpting("cube");
    d.execute(Command::SculptGroups { id, op: GroupOp::ByNormal { angle: 30.0 } }).unwrap();
    d.execute(Command::SculptVisibility { id, op: VisibilityOp::HidePatch { triangle: 0 } }).unwrap();
    let coarse = patches(&mut d, id);
    let hidden = |d: &Document| d.sculpt.as_ref().unwrap().mesh.hidden_triangles().iter().filter(|h| **h).count();
    let h0 = hidden(&d);
    d.execute(Command::SculptDetail { id, op: DetailOp::Up }).unwrap();
    let fine = patches(&mut d, id);
    assert_eq!(fine.len(), coarse.len() * 4);
    assert_eq!(distinct(&fine), 6);
    for p in 1..=6u32 {
        assert_eq!(fine.iter().filter(|x| **x == p).count(), 4 * coarse.iter().filter(|x| **x == p).count());
    }
    assert_eq!(hidden(&d), h0 * 4);
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.face_patches(), fine);
    d.execute(Command::SculptDetail { id, op: DetailOp::Down }).unwrap();
    assert_eq!(patches(&mut d, id), coarse);
    assert_eq!(hidden(&d), h0);
}

#[test]
fn clip_trim_and_extract() {
    let (mut d, id) = sculpting("round_cube");
    d.execute(Command::SculptClip { id, point: Vec3::new(0.0, 0.2, 0.0), normal: Vec3::Y }).unwrap();
    assert!(d.sculpt.as_ref().unwrap().mesh.positions.iter().all(|p| p.y <= 0.2 + 1e-5));
    d.execute(Command::Undo).unwrap();
    assert!(d.sculpt.as_ref().unwrap().mesh.positions.iter().any(|p| p.y > 0.4));
    assert!(d.execute(Command::SculptClip { id, point: Vec3::ZERO, normal: Vec3::ZERO }).is_err());
    // A shell from the mask is a new object beside the sculpt.
    assert!(d.execute(Command::SculptExtract { id, thickness: 0.05 }).is_err(), "nothing masked");
    let top = ScreenRegion::rect(front(), Vec2::new(-1.0, 0.1), Vec2::new(1.0, 1.0));
    d.execute(Command::SculptRegion { id, region: top, action: RegionAction::Mask }).unwrap();
    let shell = d.execute(Command::SculptExtract { id, thickness: 0.05 }).unwrap().created[0];
    let m = d.eval.evaluate(&d.scene, shell).mesh;
    assert!(m.face_count() > 0 && (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e)));
    assert!(d.sculpt.is_some(), "still sculpting the original");
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.get(shell).is_none());
    // Trim is a history step.
    d.execute(Command::EndSculpt).unwrap();
    d.execute(Command::AddStep { id, op: StepOp::TrimPlane { point: Vec3::ZERO, normal: Vec3::X, cap: true } }).unwrap();
    let e = d.eval.evaluate(&d.scene, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert!(e.mesh.positions().iter().all(|p| p.x <= 1e-5));
    d.execute(Command::BeginSculpt { id }).unwrap();
    assert!(d.sculpt.as_ref().unwrap().mesh.positions.iter().all(|p| p.x <= 1e-5));
}

#[test]
fn the_journal_replays_patches_visibility_and_regions() {
    let dir = std::env::temp_dir().join(format!("mc-engine-groups-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let journal = dir.join("groups.journal");
    let _ = std::fs::remove_file(&journal);
    let mut d = Document::new();
    d.attach_journal(&journal).unwrap();
    let id = d.execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::BeginSculpt { id }).unwrap();
    d.execute(Command::SculptGroups { id, op: GroupOp::ByNormal { angle: 12.0 } }).unwrap();
    let region = ScreenRegion {
        view_proj: front(),
        points: vec![Vec2::new(-0.3, -0.3), Vec2::new(0.4, -0.2), Vec2::new(0.0, 0.5)],
        front_only: true,
        eye: Vec3::new(0.0, 0.0, 5.0),
    };
    d.execute(Command::SculptRegion { id, region: region.clone(), action: RegionAction::Mask }).unwrap();
    d.execute(Command::SculptMask { id, op: MaskOp::Occlusion }).unwrap();
    d.execute(Command::SculptRegion { id, region, action: RegionAction::Hide }).unwrap();
    d.execute(Command::SculptClip { id, point: Vec3::new(0.0, -0.3, 0.0), normal: -Vec3::Y }).unwrap();
    d.sync_sculpt();
    d.sync_journal();
    let mut r = Document::new();
    assert!(r.recover_from(&journal).unwrap() > 0);
    r.sync_sculpt();
    let (a, b) = (d.sculpt_layer(id).unwrap(), r.sculpt_layer(id).unwrap());
    assert_eq!(a.positions, b.positions);
    assert_eq!(a.mask, b.mask);
    assert_eq!(a.patches, b.patches);
    assert_eq!(a.hidden, b.hidden);
    assert!(!a.hidden.is_empty() && !a.patches.is_empty());
    // And the layer survives serialization.
    let json = serde_json::to_string(a).unwrap();
    let back: modelcraft_scene::SculptLayer = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, a);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_commands_read_as_documented_json() {
    let id = ObjectId(1);
    let region = ScreenRegion { view_proj: front(), points: vec![Vec2::ZERO, Vec2::X, Vec2::Y], front_only: true, eye: Vec3::Z };
    let cmds = vec![
        Command::SculptGroups { id, op: GroupOp::ByNormal { angle: 25.0 } },
        Command::SculptGroups { id, op: GroupOp::ByUvIsland },
        Command::SculptVisibility { id, op: VisibilityOp::Isolate { triangle: 3 } },
        Command::SculptVisibility { id, op: VisibilityOp::ShowAll },
        Command::SculptRegion { id, region, action: RegionAction::Isolate },
        Command::SculptClip { id, point: Vec3::ZERO, normal: Vec3::Y },
        Command::SculptExtract { id, thickness: 0.1 },
        Command::SculptMask { id, op: MaskOp::Occlusion },
        Command::AddStep { id, op: StepOp::TrimPlane { point: Vec3::ZERO, normal: Vec3::X, cap: true } },
    ];
    for c in cmds {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c, "{j}");
    }
    // The forms docs/cli.md gives.
    for j in [
        r#"{"cmd":"sculpt_groups","id":1,"op":{"by_normal":{"angle":30}}}"#,
        r#"{"cmd":"sculpt_groups","id":1,"op":"from_mask"}"#,
        r#"{"cmd":"sculpt_visibility","id":1,"op":{"hide_patch":{"triangle":0}}}"#,
        r#"{"cmd":"sculpt_visibility","id":1,"op":"invert"}"#,
        r#"{"cmd":"sculpt_region","id":1,"region":{"view_proj":[1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1],"points":[[0,0],[1,0],[0,1]]},"action":"mask"}"#,
        r#"{"cmd":"sculpt_clip","id":1,"point":[0,0.2,0],"normal":[0,1,0]}"#,
        r#"{"cmd":"sculpt_extract","id":1,"thickness":0.05}"#,
        r#"{"cmd":"sculpt_mask","id":1,"op":"occlusion"}"#,
        r#"{"cmd":"add_step","id":1,"op":{"op":"trim_plane","point":[0,0,0],"normal":[1,0,0],"cap":true}}"#,
    ] {
        assert!(serde_json::from_str::<Command>(j).is_ok(), "{j}");
    }
}
