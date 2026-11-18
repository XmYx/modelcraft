//! Tests of the pointer rig tools: picking joints, placing joints by clicking, painting weights.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use glam::Vec3;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{BindMethod, Command, RigCommand};

use crate::app::ModelApp;
use crate::args::Cli;
use crate::rig_tools::RigTool;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

/// A viewport rectangle for the helpers that take one (the tools only need it to be the same
/// rectangle for projecting and for picking).
fn rect() -> egui::Rect {
    egui::Rect::from_min_size(egui::pos2(300.0, 100.0), egui::vec2(900.0, 600.0))
}

fn screen(h: &Harness<'static, ModelApp>, p: Vec3) -> egui::Pos2 {
    crate::viewport::to_screen(&h.state().views[0].camera, rect(), p).unwrap()
}

fn joints(h: &Harness<'static, ModelApp>) -> Vec<ObjectId> {
    h.state().doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Joint { .. })).map(|o| o.id).collect()
}

fn at(h: &Harness<'static, ModelApp>, id: ObjectId) -> Vec3 {
    h.state().doc.scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

#[test]
fn joints_are_placed_by_clicking_and_picked_by_their_bones() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Place Joints by Clicking");
    assert!(matches!(h.state().rig_tool, RigTool::PlaceJoints));
    assert!(h.state().status.starts_with("Click in the viewport"), "{}", h.state().status);
    // Three clicks: a chain, each joint under the one before, where the clicks were.
    let clicks = [egui::pos2(600.0, 500.0), egui::pos2(700.0, 380.0), egui::pos2(820.0, 300.0)];
    for c in clicks {
        assert!(crate::rig_tools::place_joint(h.state_mut(), 0, rect(), c));
    }
    h.run();
    let j = joints(&h);
    assert_eq!(j.len(), 3);
    assert_eq!(h.state().doc.scene.get(j[2]).unwrap().parent, Some(j[1]));
    for (id, c) in j.iter().zip(clicks) {
        assert!(screen(&h, at(&h, *id)).distance(c) < 1.0, "the joint is under its click");
    }
    assert!(at(&h, j[0]).length() < 1e-3 || at(&h, j[0]).is_finite(), "the first joint lies on the plane through the origin");
    // One undo per joint; toggling the action again leaves the tool.
    palette(&mut h, "Undo");
    assert_eq!(joints(&h).len(), 2);
    palette(&mut h, "Redo");
    palette(&mut h, "Place Joints by Clicking");
    assert!(matches!(h.state().rig_tool, RigTool::None));
    // Picking: on a joint picks it, on a bone picks the joint the bone starts at, elsewhere nothing.
    let pick = |h: &Harness<'static, ModelApp>, p: egui::Pos2| crate::rig_tools::pick_joint(h.state(), 0, rect(), p);
    assert_eq!(pick(&h, clicks[1] + egui::vec2(3.0, -2.0)), Some(j[1]));
    let mid = clicks[1] + (clicks[2] - clicks[1]) * 0.5;
    assert_eq!(pick(&h, mid + egui::vec2(0.0, 2.0)), Some(j[1]), "the bone to the third joint belongs to the second");
    assert_eq!(pick(&h, egui::pos2(350.0, 150.0)), None);
    // Joints still read as tree items for those who do not use the pointer.
    h.get_by_role_and_label(Role::TreeItem, "Joint.2");
}

#[test]
fn the_weight_brush_paints_one_undo_step_per_stroke() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cylinder");
    let mesh = h.state().selection[0];
    // Without a skin the action says what it needs.
    palette(&mut h, "Paint Skin Weights");
    assert_eq!(h.state().status, "Select a skinned mesh (and the joint to paint for) first");
    let mut ids = Vec::new();
    for y in [-1.0, 0.0, 1.0] {
        let parent = ids.last().copied();
        let cmd = Command::Rig(RigCommand::AddJoint { parent, position: Some([0.0, y, 0.0]), name: None });
        assert!(h.state_mut().run(cmd));
        ids.push(h.state().selection[0]);
    }
    assert!(h.state_mut().run(Command::Rig(RigCommand::BindSkin {
        mesh,
        joints: ids.clone(),
        method: BindMethod::Distance,
        influences: 4,
        dual_quaternion: false
    })));
    // The tip joint and the mesh selected: the brush paints for the tip.
    h.state_mut().selection = vec![ids[2], mesh];
    palette(&mut h, "Paint Skin Weights");
    assert!(matches!(h.state().rig_tool, RigTool::PaintWeights { joint, .. } if joint == ids[2]));
    assert_eq!(h.state().selection, vec![mesh]);
    h.get_by_label("Weight Brush");
    let marks = crate::rig_tools::weight_lines(h.state()).len();
    assert!(marks > 0, "the tip's weight is shown");
    let weight_sum = |h: &Harness<'static, ModelApp>, joint: usize| {
        let skin = &h.state().doc.scene.skins_of(mesh)[0].1;
        (0..skin.weights.points()).map(|v| skin.weights.weight(v, joint as u16)).sum::<f32>()
    };
    let before = weight_sum(&h, 2);
    // Brush settings by number; the joint by its combo box.
    h.get_by_role_and_label(Role::SpinButton, "Brush strength").set_value_accesskit(1.0);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Brush radius").set_value_accesskit(0.8);
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Paint for joint");
    // A stroke down the side of the cylinder near its bottom, where the tip has little weight.
    let undo_steps = h.state().doc.undo.len();
    let side = |y: f32| screen(&h, Vec3::new(0.0, y, 0.0));
    let (a, b) = (side(-0.9), side(-0.3));
    let mut hits = 0;
    for i in 0..=8 {
        let p = a + (b - a) * (i as f32 / 8.0);
        hits += usize::from(crate::rig_tools::paint_at(h.state_mut(), 0, rect(), p, false));
    }
    assert!(hits >= 6, "the stroke runs over the mesh: {hits}");
    let during = weight_sum(&h, 2);
    assert!(during > before + 0.5, "the preview shows while the stroke is under way: {before} → {during}");
    assert_eq!(h.state().doc.undo.len(), undo_steps, "no undo steps until the stroke ends");
    // Leaving the tool ends the stroke: one step, the same weights, and one undo takes it back.
    palette(&mut h, "Paint Skin Weights");
    assert!(matches!(h.state().rig_tool, RigTool::None));
    assert_eq!(h.state().doc.undo.len(), undo_steps + 1);
    assert!((weight_sum(&h, 2) - during).abs() < 1e-3, "the committed stroke equals its preview");
    assert!(crate::rig_tools::weight_lines(h.state()).is_empty(), "no marks without the tool");
    palette(&mut h, "Undo");
    assert!((weight_sum(&h, 2) - before).abs() < 1e-4);
    // Taking weight away with the command key held.
    h.state_mut().selection = vec![ids[2], mesh];
    palette(&mut h, "Paint Skin Weights");
    let top = screen(&h, Vec3::new(0.0, 0.8, 0.0));
    assert!(crate::rig_tools::paint_at(h.state_mut(), 0, rect(), top, true));
    assert!(weight_sum(&h, 2) < before - 0.1);
    h.get_by_role_and_label(Role::Button, "Stop Painting").click();
    h.run();
    assert!(matches!(h.state().rig_tool, RigTool::None));
    assert_eq!(h.state().status, "Weight painting off");
}
