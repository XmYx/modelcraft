//! Rig tools that work in the viewport with the pointer: picking joints, placing joints by
//! clicking, and a brush that paints skin weights.
//!
//! Each has a path without the pointer: joints are also added with a key and placed by number,
//! selected in the tree, and weights are set by the `edit_weights` operations; the brush's
//! radius, strength and joint are fields in Properties.

use std::sync::Arc;

use egui::{Pos2, Rect};
use glam::Vec3;
use modelcraft_engine::scene::rig::Skin;
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene, pick};
use modelcraft_engine::{Command, RigCommand, WeightOp};

use crate::app::ModelApp;
use crate::rig_ui::RigAction;
use crate::theme;

/// How near (in points on screen) a click must be to a joint or to a bone to pick it.
const JOINT_REACH: f32 = 10.0;
const BONE_REACH: f32 = 6.0;
/// Most vertices the weight display draws a mark for (every n-th beyond that).
const MAX_MARKS: usize = 30_000;

/// A brush stroke in progress: what was painted so far and the scene to go back to.
pub struct Stroke {
    before: Box<Scene>,
    dabs: Vec<WeightOp>,
    last: Vec3,
}

/// The tool that has the viewport's pointer.
#[derive(Default)]
pub enum RigTool {
    #[default]
    None,
    /// Each click adds a joint under the active joint.
    PlaceJoints,
    /// Dragging over `mesh` paints the weight of `joint`.
    PaintWeights { mesh: ObjectId, joint: ObjectId, radius: f32, strength: f32, rest: Arc<Vec<Vec3>>, stroke: Option<Stroke> },
}

impl RigTool {
    pub fn is_on(&self) -> bool {
        !matches!(self, RigTool::None)
    }
}

fn is_joint(scene: &Scene, id: ObjectId) -> bool {
    matches!(scene.get(id).map(|o| &o.kind), Some(ObjectKind::Joint { .. }))
}

fn world_position(scene: &Scene, id: ObjectId) -> Vec3 {
    scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

/// The mesh as its skin was bound: evaluated with the skin switched off.
fn rest_points(app: &mut ModelApp, mesh: ObjectId, skin: ObjectId) -> Vec<Vec3> {
    let mut scene = app.doc.scene.clone();
    if let Ok(o) = scene.edit(skin) {
        o.visible = false;
    }
    app.doc.eval.evaluate(&scene, mesh).mesh.positions().to_vec()
}

fn skin_of(scene: &Scene, mesh: ObjectId) -> Option<(ObjectId, Arc<Skin>)> {
    scene.skins_of(mesh).into_iter().next()
}

pub fn act(app: &mut ModelApp, a: RigAction) {
    match a {
        RigAction::PlaceJoints => {
            if matches!(app.rig_tool, RigTool::PlaceJoints) {
                app.rig_tool = RigTool::None;
                app.status = tl!("Joint placing off");
            } else {
                app.rig_tool = RigTool::PlaceJoints;
                app.status = tl!("Click in the viewport to place joints; each is a child of the one before. Escape stops.");
            }
        }
        RigAction::PaintWeights => {
            if matches!(app.rig_tool, RigTool::PaintWeights { .. }) {
                stop(app);
                app.status = tl!("Weight painting off");
                return;
            }
            let scene = &app.doc.scene;
            let mesh = app.selection.iter().copied().find(|id| skin_of(scene, *id).is_some());
            let Some((mesh, (skin_id, skin))) = mesh.and_then(|m| skin_of(scene, m).map(|s| (m, s))) else {
                app.status = tl!("Select a skinned mesh (and the joint to paint for) first");
                return;
            };
            // The selected joint of this skin, else its first joint.
            let joint = app.selection.iter().copied().find(|j| skin.joints.contains(j)).or_else(|| skin.joints.first().copied());
            let Some(joint) = joint else {
                app.status = tl!("The skin has no joints");
                return;
            };
            let rest = Arc::new(rest_points(app, mesh, skin_id));
            // A brush about a tenth of the mesh's size.
            let (lo, hi) = rest.iter().fold((Vec3::MAX, Vec3::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
            let radius = if lo.cmple(hi).all() { ((hi - lo).length() * 0.1).max(1e-3) } else { 0.25 };
            let name = app.doc.scene.get(joint).map(|o| o.name.clone()).unwrap_or_default();
            app.rig_tool = RigTool::PaintWeights { mesh, joint, radius, strength: 0.25, rest, stroke: None };
            app.selection = vec![mesh];
            app.status = crate::tlf!("Painting weights for {joint}: drag on the mesh, hold the command key to take weight away. Escape stops.", joint = name);
        }
        _ => {}
    }
}

/// Leave the tool, finishing a stroke that is under way.
fn stop(app: &mut ModelApp) {
    finish_stroke(app);
    app.rig_tool = RigTool::None;
}

/// End the stroke: the previewed weights are taken back and the dabs run as one command, so the
/// stroke is one undo step and is in the journal.
fn finish_stroke(app: &mut ModelApp) {
    let RigTool::PaintWeights { mesh, stroke, .. } = &mut app.rig_tool else { return };
    let Some(s) = stroke.take() else { return };
    let mesh = *mesh;
    app.doc.scene = *s.before;
    if s.dabs.is_empty() {
        return;
    }
    let n = s.dabs.len();
    let commands = s.dabs.into_iter().map(|edit| Command::Rig(RigCommand::EditWeights { mesh, edit })).collect();
    if app.run(Command::Batch { label: Some("Paint weights".into()), commands }) {
        app.status = crate::tlf!("{count} dabs painted", count = n);
    }
}

/// The joint under a click: the nearest joint within reach on screen, else the joint whose bone
/// (the line to a child joint) passes within reach.
pub fn pick_joint(app: &ModelApp, view: usize, rect: Rect, at: Pos2) -> Option<ObjectId> {
    let scene = &app.doc.scene;
    let cam = app.views.get(view)?.camera;
    let on_screen = |id: ObjectId| crate::viewport::to_screen(&cam, rect, world_position(scene, id));
    let mut best: Option<(f32, ObjectId)> = None;
    let mut bone: Option<(f32, ObjectId)> = None;
    for o in scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Joint { .. }) && scene.visible(o.id)).take(20_000) {
        let Some(here) = on_screen(o.id) else { continue };
        let d = here.distance(at);
        if d <= JOINT_REACH && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, o.id));
        }
        // The bone from the parent joint belongs to the parent: clicking it picks the parent.
        if let Some(p) = o.parent.filter(|p| is_joint(scene, *p))
            && let Some(head) = on_screen(p)
        {
            let d = segment_distance(at, head, here);
            if d <= BONE_REACH && bone.is_none_or(|(b, _)| d < b) {
                bone = Some((d, p));
            }
        }
    }
    best.or(bone).map(|(_, id)| id)
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 1e-6 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// Where a click lands for a new joint: on the plane through `through` that faces the camera.
fn place_point(app: &ModelApp, view: usize, rect: Rect, at: Pos2, through: Vec3) -> Option<Vec3> {
    let cam = app.views.get(view)?.camera;
    let ray = crate::viewport::world_ray(&cam, rect, at);
    let facing = crate::viewport::world_ray(&cam, rect, rect.center()).dir.normalize_or_zero();
    let along = ray.dir.dot(facing);
    if along.abs() < 1e-6 {
        return None;
    }
    let t = (through - ray.origin).dot(facing) / along;
    let p = ray.origin + ray.dir * t;
    (t.is_finite() && p.is_finite()).then_some(p)
}

/// Add a joint at a viewport position (the click of the placing tool).
pub fn place_joint(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2) -> bool {
    let parent = app.active().filter(|id| is_joint(&app.doc.scene, *id));
    let through = parent.map_or(Vec3::ZERO, |p| world_position(&app.doc.scene, p));
    let Some(p) = place_point(app, view, rect, at, through) else { return false };
    app.run(Command::Rig(RigCommand::AddJoint { parent, position: Some(p.to_array()), name: None }))
}

/// One brush sample at a viewport position: find the point of the posed mesh under the pointer
/// and paint around where that point was bound. Returns whether the mesh was hit.
pub fn paint_at(app: &mut ModelApp, view: usize, rect: Rect, at: Pos2, subtract: bool) -> bool {
    let RigTool::PaintWeights { mesh, joint, radius, strength, rest, .. } = &app.rig_tool else { return false };
    let (mesh, joint, radius, strength, rest) = (*mesh, *joint, *radius, if subtract { -*strength } else { *strength }, rest.clone());
    let Some(cam) = app.views.get(view).map(|v| v.camera) else { return false };
    let world = app.doc.scene.world_matrix(mesh);
    let ray = crate::viewport::world_ray(&cam, rect, at).transformed(world.inverse());
    let posed = app.doc.eval.evaluate(&app.doc.scene, mesh).mesh;
    let Some(hit) = pick::pick_face(&posed, &ray) else { return false };
    // The vertex of the hit face nearest the hit: its bound position is the dab's centre.
    let nearest = posed.face_vertices(hit.face).min_by(|a, b| posed.position(*a).distance(hit.point).total_cmp(&posed.position(*b).distance(hit.point)));
    let Some(center) = nearest.and_then(|v| rest.get(v as usize)).copied() else { return false };
    let Some((skin_id, skin)) = skin_of(&app.doc.scene, mesh) else { return false };
    let Some(j) = skin.joints.iter().position(|x| *x == joint) else { return false };
    let RigTool::PaintWeights { stroke, .. } = &mut app.rig_tool else { return false };
    let s = stroke.get_or_insert_with(|| Stroke { before: Box::new(app.doc.scene.clone()), dabs: Vec::new(), last: Vec3::splat(f32::MAX) });
    // Dabs a quarter of the radius apart: even coverage however fast the pointer moves.
    if !s.dabs.is_empty() && s.last.distance(center) < radius * 0.25 {
        return true;
    }
    s.last = center;
    s.dabs.push(WeightOp::Paint { joint, center: center.to_array(), radius, strength });
    // Preview: the same dab on the scene, without an undo step.
    let mut painted = (*skin).clone();
    modelcraft_engine::rig::paint_weights(&mut painted.weights, j as u16, &rest, center, radius, strength);
    if let Ok(o) = app.doc.scene.edit(skin_id) {
        o.kind = ObjectKind::Skin { skin: Arc::new(painted) };
    }
    true
}

/// Pointer input while a rig tool is on. Returns `true` when the tool took it.
pub fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, view: usize, rect: Rect) -> bool {
    if !app.rig_tool.is_on() {
        return false;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) || resp.clicked_by(egui::PointerButton::Secondary) {
        stop(app);
        app.status = tl!("Tool off");
        return true;
    }
    // The tool's objects may be gone (undo, delete).
    if let RigTool::PaintWeights { mesh, .. } = &app.rig_tool
        && skin_of(&app.doc.scene, *mesh).is_none()
    {
        app.rig_tool = RigTool::None;
        return false;
    }
    match &app.rig_tool {
        RigTool::PlaceJoints => {
            if resp.clicked_by(egui::PointerButton::Primary)
                && let Some(at) = resp.interact_pointer_pos()
            {
                place_joint(app, view, rect, at);
            }
        }
        RigTool::PaintWeights { .. } => {
            let subtract = ui.input(|i| i.modifiers.command);
            let down = resp.dragged_by(egui::PointerButton::Primary)
                || resp.drag_started_by(egui::PointerButton::Primary)
                || resp.clicked_by(egui::PointerButton::Primary);
            if down && let Some(at) = resp.interact_pointer_pos() {
                paint_at(app, view, rect, at, subtract);
            }
            if resp.drag_stopped() || resp.clicked_by(egui::PointerButton::Primary) {
                finish_stroke(app);
            }
            // The brush: a ring at the pointer.
            if let Some(at) = resp.hover_pos() {
                ui.painter_at(rect).circle_stroke(at, 14.0, egui::Stroke::new(1.5, if subtract { theme::AXIS_Z } else { theme::ACCENT }));
            }
        }
        RigTool::None => {}
    }
    true
}

/// The weight brush's settings in Properties while it is on.
pub fn properties(app: &mut ModelApp, ui: &mut egui::Ui) {
    let RigTool::PaintWeights { mesh, joint, radius, strength, .. } = &app.rig_tool else { return };
    let (mesh, mut joint, mut radius, mut strength) = (*mesh, *joint, *radius, *strength);
    let Some((_, skin)) = skin_of(&app.doc.scene, mesh) else { return };
    let name = |id: ObjectId| app.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
    let mut stop_now = false;
    egui::CollapsingHeader::new(tl!("Weight Brush")).id_salt("weight-brush").default_open(true).show(ui, |ui| {
        ui.horizontal(|ui| {
            let label = ui.label(tl!("Paint for joint"));
            egui::ComboBox::from_id_salt("paint-joint")
                .selected_text(name(joint))
                .show_ui(ui, |ui| {
                    for j in &skin.joints {
                        ui.selectable_value(&mut joint, *j, name(*j));
                    }
                })
                .response
                .labelled_by(label.id);
        });
        ui.horizontal(|ui| {
            crate::panels::drag(ui, &mut radius, 0.005, 1e-4..=1e4, tl!("Brush radius"));
        });
        ui.horizontal(|ui| {
            crate::panels::drag(ui, &mut strength, 0.01, 0.01..=1.0, tl!("Brush strength"));
        });
        ui.label(
            egui::RichText::new(tl!("Marks on the mesh show the joint's weight: none where it is 0, cool to warm up to 1.")).color(theme::TEXT_DIM).small(),
        );
        stop_now = ui.button(tl!("Stop Painting")).clicked();
    });
    if let RigTool::PaintWeights { joint: j, radius: r, strength: s, .. } = &mut app.rig_tool {
        (*j, *r, *s) = (joint, radius.max(1e-4), strength.clamp(0.01, 1.0));
    }
    if stop_now {
        stop(app);
        app.status = tl!("Weight painting off");
    }
}

/// The painted joint's weight on the mesh, as a short mark along each vertex's normal: no mark
/// at 0, then from a cool to a warm colour up to 1 (length grows with the weight too, so the
/// display does not rest on colour alone).
pub fn weight_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let RigTool::PaintWeights { mesh, joint, rest, .. } = &app.rig_tool else { return Vec::new() };
    let scene = &app.doc.scene;
    let Some((_, skin)) = skin_of(scene, *mesh) else { return Vec::new() };
    let Some(j) = skin.joints.iter().position(|x| x == joint) else { return Vec::new() };
    if skin.weights.points() != rest.len() || rest.is_empty() {
        return Vec::new();
    }
    // Posed positions: the same skinning the evaluator does, from the rest points.
    let world = scene.world_matrix(*mesh);
    let to_mesh = world.inverse();
    let matrices: Vec<glam::Mat4> = skin
        .joints
        .iter()
        .zip(&skin.inverse_bind)
        .map(|(id, ib)| to_mesh * scene.world_matrix(*id) * *ib)
        .map(|m| if m.is_finite() { m } else { glam::Mat4::IDENTITY })
        .collect();
    let mut posed = Vec::new();
    if skin.dual_quaternion {
        modelcraft_engine::animation::skin::dual_quaternion(rest, &skin.weights, &matrices, &mut posed);
    } else {
        modelcraft_engine::animation::skin::linear_blend(rest, &skin.weights, &matrices, &mut posed);
    }
    let (lo, hi) = rest.iter().fold((Vec3::MAX, Vec3::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
    let size = (hi - lo).length().max(1e-3) * 0.012;
    let middle = (lo + hi) * 0.5;
    let every = rest.len().div_ceil(MAX_MARKS).max(1);
    let mut out = Vec::new();
    for v in (0..rest.len()).step_by(every) {
        let w = skin.weights.weight(v, j as u16);
        if w <= 0.01 {
            continue;
        }
        let (Some(p), Some(r)) = (posed.get(v), rest.get(v)) else { continue };
        // Outwards from the middle of the mesh: near enough to the normal for a mark.
        let out_dir = (*r - middle).normalize_or(Vec3::Y);
        let a = world.transform_point3(*p);
        let b = world.transform_point3(*p + out_dir * size * (0.5 + 1.5 * w));
        out.push((a.to_array(), b.to_array(), [0.2 + 0.8 * w, 0.75 - 0.45 * w, 1.0 - 0.9 * w, 1.0]));
    }
    out
}

// ---------------------------------------------------------------- playback benchmark

/// The skinned-character playback benchmark (`--bench-skin`): what each frame cost.
#[derive(Default)]
pub struct SkinBench {
    mesh: Option<ObjectId>,
    vertices: usize,
    joints: usize,
    dual_quaternion: bool,
    pose_ms: Vec<f32>,
    eval_ms: Vec<f32>,
}

/// Build the benchmark character of `docs/performance.md` and start playing it.
pub fn start_bench(app: &mut ModelApp, frames: u32, dual_quaternion: bool) {
    let frames = frames.max(20);
    let built = modelcraft_engine::rig::bench_character(&mut app.doc, 100_000, 60, 120, modelcraft_engine::BindMethod::Heat);
    let Ok((mesh, _, joints)) = built else {
        eprintln!("--bench-skin: the character could not be built");
        return;
    };
    if dual_quaternion {
        app.run(Command::Rig(RigCommand::SetSkin { mesh, dual_quaternion: true }));
    }
    app.selection.clear();
    let vertices = app.doc.eval.evaluate(&app.doc.scene, mesh).mesh.vertex_count();
    // Stand back far enough to see the whole tube.
    if let Some(v) = app.views.get_mut(0) {
        v.camera.target = Vec3::ZERO;
        v.camera.distance = joints.len() as f32 * 1.6;
    }
    let mut bench = crate::viewport::Bench::new(frames);
    bench.skin = Some(SkinBench { mesh: Some(mesh), vertices, joints: joints.len(), dual_quaternion, ..SkinBench::default() });
    app.bench = Some(bench);
}

/// One frame of the benchmark: go to the next frame, evaluate the mesh (the viewport then draws
/// and uploads that result), and at the end print the timings and leave.
pub fn bench_tick(app: &mut ModelApp) {
    let Some(b) = &mut app.bench else { return };
    let now = std::time::Instant::now();
    let dt = b.last.map(|t| now.duration_since(t).as_secs_f32() * 1000.0);
    b.last = Some(now);
    b.frame += 1;
    let (frame, total) = (b.frame, b.total);
    // The first frames build caches and pipelines.
    if frame > 10
        && let Some(dt) = dt
    {
        b.orbit_ms.push(dt);
    }
    let Some(mesh) = b.skin.as_ref().and_then(|s| s.mesh) else { return };
    let span = app.doc.scene.anim.time.span().max(1.0);
    let t0 = std::time::Instant::now();
    app.doc.set_frame((frame as f64) % (span + 1.0));
    let pose = t0.elapsed().as_secs_f32() * 1000.0;
    let t1 = std::time::Instant::now();
    let _ = app.doc.eval.evaluate(&app.doc.scene, mesh);
    let eval = t1.elapsed().as_secs_f32() * 1000.0;
    let upload = app.gpu.as_ref().map(|g| g.upload_ms);
    let Some(b) = &mut app.bench else { return };
    if frame > 10 {
        if let Some(s) = &mut b.skin {
            s.pose_ms.push(pose);
            s.eval_ms.push(eval);
        }
        b.upload_ms.extend(upload);
    }
    if frame < total {
        return;
    }
    let stats = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        let mean = if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
        let pct = |q: f32| v.get(((v.len() as f32 - 1.0) * q).round().max(0.0) as usize).copied().unwrap_or(0.0);
        format!(
            "{{\"mean_ms\":{mean:.3},\"p50_ms\":{:.3},\"p95_ms\":{:.3},\"fps_mean\":{:.1}}}",
            pct(0.5),
            pct(0.95),
            if mean > 0.0 { 1000.0 / mean } else { 0.0 }
        )
    };
    let adapter = app.gpu.as_ref().map(|g| g.rs.adapter.get_info().name).unwrap_or_else(|| "none".into());
    let (mut f, mut u) = (b.orbit_ms.clone(), b.upload_ms.clone());
    let s = b.skin.take().unwrap_or_default();
    let (mut p, mut e) = (s.pose_ms, s.eval_ms);
    println!(
        "{{\"adapter\":{adapter:?},\"vertices\":{},\"joints\":{},\"skinning\":{:?},\"threads\":{},\"frame\":{},\"pose\":{},\"evaluate\":{},\"upload\":{}}}",
        s.vertices,
        s.joints,
        if s.dual_quaternion { "dual quaternion" } else { "linear blend" },
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        stats(&mut f),
        stats(&mut p),
        stats(&mut e),
        stats(&mut u)
    );
    app.bench = None;
    std::process::exit(0);
}
