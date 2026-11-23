//! The 3D viewport: navigation, picking, selection, the transform gizmo, sculpt strokes, GPU
//! drawing (CPU wireframe fallback without a GPU), overlays and the accessibility nodes.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Vec2, pos2};
use glam::{Mat4, Quat, Vec2 as GVec2, Vec3};
use modelcraft_engine::Command;
use modelcraft_engine::mesh::PolyMesh;
use modelcraft_engine::scene::pick::{self, Ray};
use modelcraft_engine::scene::{DisplayMode, Object, ObjectId, ObjectKind, Scene, Step, StepOp};
use modelcraft_engine::sculpt::{BrushKind, Dab};
use modelcraft_render::buffers::{self, LineVertex};
use modelcraft_render::{Camera, ViewPreset};

use crate::app::{ModelApp, NavPreset, SelectMode, Tool, Workspace};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Axis(usize),
    Free,
}

pub enum DragKind {
    Gizmo { tool: Tool, handle: Handle, pivot: Vec3, axes: [Vec3; 3], scene: Box<Scene> },
    Box,
}

pub struct Drag {
    pub view: usize,
    pub start: Pos2,
    pub kind: DragKind,
}

pub struct StrokeInput {
    pub view: usize,
    pub object: ObjectId,
    pub last: Option<Vec3>,
    /// Grab brush: the plane the pointer moves on (world point, normal) and its last point.
    pub grab: Option<(Vec3, Vec3, Vec3)>,
    /// Places the dabs along the pointer path for the brush's stroke mode.
    pub maker: modelcraft_engine::sculpt::DabMaker,
}

pub struct Bench {
    pub total: u32,
    pub frame: u32,
    pub orbit_ms: Vec<f32>,
    pub sculpt_ms: Vec<f32>,
    pub dab_ms: Vec<f32>,
    pub upload_ms: Vec<f32>,
    /// Playing the skinned benchmark character instead of sculpting ([`crate::rig_tools`]).
    pub skin: Option<crate::rig_tools::SkinBench>,
    pub(crate) last: Option<std::time::Instant>,
}

impl Bench {
    pub fn new(total: u32) -> Self {
        Bench { total: total.max(20), frame: 0, orbit_ms: Vec::new(), sculpt_ms: Vec::new(), dab_ms: Vec::new(), upload_ms: Vec::new(), skin: None, last: None }
    }
}

pub fn save_png(path: &std::path::Path, img: &egui::ColorImage) -> Result<(), String> {
    use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
    let [w, h] = img.size;
    let raw: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    let image = Image::from_raw(w as u32, h as u32, ChannelLayout::Rgba, SampleType::U8, raw).map_err(|e| e.to_string())?;
    let bytes = craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

pub(crate) fn to_screen(cam: &Camera, rect: Rect, p: Vec3) -> Option<Pos2> {
    let n = cam.project(p, rect.aspect_ratio())?;
    Some(pos2(rect.left() + (n.x * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - n.y * 0.5) * rect.height()))
}

pub(crate) fn ndc(rect: Rect, p: Pos2) -> GVec2 {
    GVec2::new((p.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (p.y - rect.top()) / rect.height() * 2.0)
}

pub(crate) fn world_ray(cam: &Camera, rect: Rect, p: Pos2) -> Ray {
    let (origin, dir) = cam.ray(ndc(rect, p), rect.aspect_ratio());
    Ray { origin, dir }
}

/// Objects drawn as surfaces: meshes, cages, clone generators and particle systems, not cage inputs, groups or deformers.
fn drawables(scene: &Scene) -> Vec<ObjectId> {
    scene
        .tree_order()
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| scene.visible(*id) && !scene.is_cage_input(*id))
        .filter(|id| {
            matches!(
                scene.get(*id).map(|o| &o.kind),
                Some(
                    ObjectKind::Mesh { .. }
                        | ObjectKind::SmoothCage { .. }
                        | ObjectKind::Array { .. }
                        | ObjectKind::Symmetry { .. }
                        | ObjectKind::Cloner { .. }
                        | ObjectKind::Instance { .. }
                )
            ) || scene.is_particles(*id)
        })
        .collect()
}

pub fn show(app: &mut ModelApp, ui: &mut egui::Ui) {
    let full = ui.available_rect_before_wrap();
    if let Some(g) = &mut app.gpu {
        g.begin_frame();
    }
    if let Some(s) = &mut app.doc.sculpt
        && let Some(g) = &mut app.gpu
    {
        g.sync_sculpt(s);
    }
    bench_tick(app);
    if app.quad {
        let half = full.size() * 0.5;
        for i in 0..4 {
            let min = full.min + egui::vec2(if i % 2 == 1 { half.x } else { 0.0 }, if i >= 2 { half.y } else { 0.0 });
            view_ui(app, ui, i, Rect::from_min_size(min, half).shrink(1.0));
        }
    } else {
        let i = app.active_view.min(3);
        view_ui(app, ui, i, full);
    }
}

fn bench_tick(app: &mut ModelApp) {
    if app.bench.as_ref().is_some_and(|b| b.skin.is_some()) {
        return crate::rig_tools::bench_tick(app);
    }
    let Some(b) = &mut app.bench else { return };
    let now = std::time::Instant::now();
    let dt = b.last.map(|t| now.duration_since(t).as_secs_f32() * 1000.0);
    b.last = Some(now);
    b.frame += 1;
    let half = b.total / 2;
    app.views[0].camera.orbit(0.012, 0.0);
    if b.frame > 10
        && let Some(dt) = dt
    {
        if b.frame <= half { b.orbit_ms.push(dt) } else { b.sculpt_ms.push(dt) }
    }
    if b.frame > half {
        // Sculpt while orbiting: one dab per frame along a ring on the surface.
        let t = b.frame as f32 * 0.05;
        if let Some(s) = &mut app.doc.sculpt {
            let r = s.mesh.positions.first().map(|p| p.length()).unwrap_or(1.0);
            let dir = Vec3::new(t.cos(), 0.3 * (t * 0.7).sin(), t.sin()).normalize();
            let hit = s.mesh.raycast(dir * r * 3.0, -dir).map(|h| h.1).unwrap_or(dir * r);
            let t0 = std::time::Instant::now();
            if !s.mesh.in_stroke() {
                let mut brush = modelcraft_engine::sculpt::BrushSettings::new(BrushKind::Clay, r * 0.12);
                brush.symmetry.x = true;
                s.mesh.begin_stroke(brush);
            }
            s.mesh.dab(Dab { center: hit, view_dir: -dir, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
            s.unsynced = true;
            b.dab_ms.push(t0.elapsed().as_secs_f32() * 1000.0);
        }
        if let Some(g) = &app.gpu {
            b.upload_ms.push(g.upload_ms);
        }
    }
    if b.frame >= b.total {
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
        let tris = app.doc.sculpt.as_ref().map(|s| s.mesh.triangle_count()).unwrap_or(0);
        let verts = app.doc.sculpt.as_ref().map(|s| s.mesh.vertex_count()).unwrap_or(0);
        let adapter = app.gpu.as_ref().map(|g| g.rs.adapter.get_info().name).unwrap_or_else(|| "none".into());
        let (mut o, mut s, mut d, mut u) = (b.orbit_ms.clone(), b.sculpt_ms.clone(), b.dab_ms.clone(), b.upload_ms.clone());
        println!(
            "{{\"adapter\":{adapter:?},\"triangles\":{tris},\"vertices\":{verts},\"orbit_frame\":{},\"sculpt_frame\":{},\"dab\":{},\"upload\":{}}}",
            stats(&mut o),
            stats(&mut s),
            stats(&mut d),
            stats(&mut u)
        );
        app.bench = None;
        std::process::exit(0);
    }
}

fn view_label(app: &ModelApp, i: usize) -> String {
    let v = &app.views[i];
    let objects = app.doc.scene.len();
    let sel = app.active().and_then(|id| app.doc.scene.get(id)).map(|o| o.name.clone());
    match sel {
        Some(name) => crate::tlf!("Viewport, {view}: {count} objects, {name} selected", view = preset_label(v.preset), count = objects, name = name),
        None => crate::tlf!("Viewport, {view}: {count} objects, nothing selected", view = preset_label(v.preset), count = objects),
    }
}

fn preset_label(p: ViewPreset) -> String {
    match p {
        ViewPreset::Perspective => tl!("Perspective"),
        ViewPreset::Front => tl!("Front"),
        ViewPreset::Right => tl!("Right"),
        ViewPreset::Top => tl!("Top"),
    }
}

fn view_ui(app: &mut ModelApp, ui: &mut egui::Ui, i: usize, rect: Rect) {
    let id = egui::Id::unique(("viewport", i));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let label = view_label(app, i);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, &label));
    publish_objects(app, ui, resp.id);
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
        app.active_view = i;
    }
    keyboard_nav(app, ui, &resp, i);
    navigate(app, ui, &resp, i, rect);
    if !crate::hair_ui::input(app, ui, &resp, i, rect) {
        match app.workspace {
            Workspace::Sculpt => {
                // The rectangle and lasso tools take the drag instead of the brush.
                if !crate::sculpt_groups_ui::viewport_input(app, ui, &resp, i, rect) {
                    sculpt_input(app, ui, &resp, i, rect);
                }
            }
            _ => model_input(app, ui, &resp, i, rect),
        }
    }
    render(app, ui, i, rect);
    overlays(app, ui, &resp, i, rect);
    crate::view_header::stats_overlay(app, ui, i, rect);
    crate::view_header::header(app, ui, i, rect);
}

/// One accessible node per object, so screen readers can hear what the viewport shows and what
/// is selected (the viewport itself is a drawn surface).
fn publish_objects(app: &ModelApp, ui: &egui::Ui, parent: egui::Id) {
    for o in app.doc.scene.objects().iter().take(500) {
        let nid = parent.with(("object", o.id.0));
        let selected = app.selection.contains(&o.id);
        ui.ctx().accesskit_node_builder(nid, |n| {
            n.set_role(egui::accesskit::Role::ListItem);
            n.set_label(o.name.clone());
            n.set_selected(selected);
        });
    }
}

/// Non-drag navigation: arrow keys orbit, Shift+arrows pan, +/- zoom while the viewport has focus.
fn keyboard_nav(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize) {
    if !resp.has_focus() {
        return;
    }
    let cam = &mut app.views[i].camera;
    ui.input(|inp| {
        let shift = inp.modifiers.shift;
        let step = 0.08;
        for (k, dx, dy) in
            [(egui::Key::ArrowLeft, -1.0, 0.0), (egui::Key::ArrowRight, 1.0, 0.0), (egui::Key::ArrowUp, 0.0, 1.0), (egui::Key::ArrowDown, 0.0, -1.0)]
        {
            if inp.key_pressed(k) {
                if shift { cam.pan(-dx * 0.05, dy * 0.05) } else { cam.orbit(dx * step, dy * step) }
            }
        }
        if inp.key_pressed(egui::Key::Plus) || inp.key_pressed(egui::Key::Equals) {
            cam.zoom(0.85);
        }
        if inp.key_pressed(egui::Key::Minus) {
            cam.zoom(1.0 / 0.85);
        }
    });
}

fn navigate(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) {
    let (scroll, alt, shift, primary, middle, secondary) = ui.input(|inp| {
        (
            inp.smooth_scroll_delta.y,
            inp.modifiers.alt,
            inp.modifiers.shift,
            inp.pointer.button_down(egui::PointerButton::Primary),
            inp.pointer.button_down(egui::PointerButton::Middle),
            inp.pointer.button_down(egui::PointerButton::Secondary),
        )
    });
    let cam = &mut app.views[i].camera;
    if resp.hovered() && scroll.abs() > 0.0 {
        cam.zoom((-scroll * 0.002).exp());
    }
    if !resp.dragged() {
        return;
    }
    let d = resp.drag_delta();
    let (orbit, pan, zoom) = match app.nav {
        NavPreset::AltMouse => (alt && primary, (alt && middle) || (middle && !alt), alt && secondary),
        NavPreset::MiddleMouse => (middle && !shift, middle && shift, false),
        NavPreset::RightMouse => (secondary && !shift, (secondary && shift) || middle, false),
    };
    if orbit {
        cam.orbit(d.x * 0.008, d.y * 0.008);
        app.views[i].preset = if cam.projection == modelcraft_render::Projection::Perspective { ViewPreset::Perspective } else { app.views[i].preset };
    } else if pan {
        cam.pan(d.x / rect.height(), d.y / rect.height());
    } else if zoom {
        cam.zoom((d.x - d.y).mul_add(-0.005, 0.0).exp());
    }
}

pub(crate) fn is_nav(app: &ModelApp, ui: &egui::Ui) -> bool {
    ui.input(|i| match app.nav {
        NavPreset::AltMouse => i.modifiers.alt,
        _ => false,
    })
}

// ---------------------------------------------------------------- modeling input

fn edit_mesh(app: &mut ModelApp) -> Option<(ObjectId, Arc<PolyMesh>, Mat4)> {
    let id = app.edit_target()?;
    let m = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    Some((id, m, app.doc.scene.world_matrix(id)))
}

fn pivot(app: &mut ModelApp) -> Option<Vec3> {
    if app.mode == SelectMode::Object {
        let id = app.active()?;
        // The gizmo sits on the object's pivot.
        let pivot = app.doc.scene.get(id).map_or(Vec3::ZERO, |o| o.pivot);
        return Some(app.doc.scene.world_matrix(id).transform_point3(pivot));
    }
    let verts = app.component_vertices();
    let (_, m, w) = edit_mesh(app)?;
    if verts.is_empty() {
        return None;
    }
    let c = verts.iter().map(|v| m.position(*v)).sum::<Vec3>() / verts.len() as f32;
    Some(w.transform_point3(c))
}

fn gizmo_len(cam: &Camera, rect: Rect, p: Vec3) -> f32 {
    cam.world_per_pixel(p, rect.height()) * 90.0
}

fn hit_gizmo(app: &mut ModelApp, i: usize, rect: Rect, at: Pos2) -> Option<(Handle, Vec3, [Vec3; 3])> {
    if app.tool == Tool::Select {
        return None;
    }
    let p = pivot(app)?;
    let axes = crate::snap_ui::gizmo_axes(app);
    let cam = app.views[i].camera;
    let ps = to_screen(&cam, rect, p)?;
    let l = gizmo_len(&cam, rect, p);
    if ps.distance(at) < 10.0 && app.tool != Tool::Rotate {
        return Some((Handle::Free, p, axes));
    }
    let mut best: Option<(f32, Handle)> = None;
    for (k, ax) in axes.iter().enumerate() {
        let d = if app.tool == Tool::Rotate {
            ring(&cam, rect, p, *ax, l).windows(2).filter_map(|w| Some(seg_dist(at, *w.first()?, *w.get(1)?))).fold(f32::MAX, f32::min)
        } else {
            match to_screen(&cam, rect, p + *ax * l) {
                Some(e) => seg_dist(at, ps, e),
                None => f32::MAX,
            }
        };
        if d < 9.0 && best.is_none_or(|(b, _)| d < b) {
            best = Some((d, Handle::Axis(k)));
        }
    }
    best.map(|(_, h)| (h, p, axes))
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t).distance(p)
}

fn ring(cam: &Camera, rect: Rect, p: Vec3, axis: Vec3, r: f32) -> Vec<Pos2> {
    let u = axis.any_orthonormal_vector();
    let v = axis.cross(u);
    (0..=48)
        .filter_map(|k| {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            to_screen(cam, rect, p + (u * a.cos() + v * a.sin()) * r)
        })
        .collect()
}

/// The transform a gizmo drag stands for: (translate, rotate, scale) in world space.
/// Round a gizmo transform to the snapping steps (move in scene units, rotate in degrees, scale).
pub fn snapped(t: Vec3, r: Quat, s: Vec3, steps: (f32, f32, f32)) -> (Vec3, Quat, Vec3) {
    let q = |v: f32, step: f32| if step > 0.0 { (v / step).round() * step } else { v };
    let t = Vec3::new(q(t.x, steps.0), q(t.y, steps.0), q(t.z, steps.0));
    let (axis, angle) = r.to_axis_angle();
    let r = if angle.is_finite() && axis.is_finite() { Quat::from_axis_angle(axis, q(angle.to_degrees(), steps.1).to_radians()) } else { r };
    let s = s.map(|v| if (v - 1.0).abs() < 1e-6 { v } else { q(v, steps.2).max(steps.2.max(0.01)) });
    (t, r, s)
}

fn drag_transform(cam: &Camera, rect: Rect, tool: Tool, handle: Handle, pivot: Vec3, axes: &[Vec3; 3], start: Pos2, cur: Pos2) -> (Vec3, Quat, Vec3) {
    let d = cur - start;
    let l = gizmo_len(cam, rect, pivot);
    let ps = to_screen(cam, rect, pivot).unwrap_or(start);
    let along = |k: usize| -> f32 {
        let Some(e) = axes.get(k).and_then(|a| to_screen(cam, rect, pivot + *a * l)) else { return 0.0 };
        let s = e - ps;
        d.dot(s) / s.length_sq().max(1.0)
    };
    match (tool, handle) {
        (Tool::Move, Handle::Axis(k)) => (axes.get(k).copied().unwrap_or(Vec3::X) * along(k) * l, Quat::IDENTITY, Vec3::ONE),
        (Tool::Move, Handle::Free) => {
            let wpp = cam.world_per_pixel(pivot, rect.height());
            let fwd = -cam.back();
            let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
            let up = right.cross(fwd);
            ((right * d.x - up * d.y) * wpp, Quat::IDENTITY, Vec3::ONE)
        }
        (Tool::Scale, Handle::Axis(k)) => {
            let mut s = Vec3::ONE;
            s[k.min(2)] = (1.0 + along(k)).max(0.01);
            (Vec3::ZERO, Quat::IDENTITY, s)
        }
        (Tool::Scale, Handle::Free) => (Vec3::ZERO, Quat::IDENTITY, Vec3::splat((1.0 + (d.x - d.y) / 150.0).max(0.01))),
        (Tool::Rotate, Handle::Axis(k)) => {
            let a0 = (start - ps).angle();
            let a1 = (cur - ps).angle();
            let axis = axes.get(k).copied().unwrap_or(Vec3::Y);
            let facing = if axis.dot(cam.back()) >= 0.0 { -1.0 } else { 1.0 };
            (Vec3::ZERO, Quat::from_axis_angle(axis, (a1 - a0) * facing), Vec3::ONE)
        }
        _ => (Vec3::ZERO, Quat::IDENTITY, Vec3::ONE),
    }
}

/// Apply a gizmo transform to a scene: objects' transforms or a component step.
fn apply_drag(app: &ModelApp, scene: &mut Scene, t: Vec3, r: Quat, s: Vec3, pivot: Vec3, commit: bool) -> Vec<Command> {
    let mut cmds = Vec::new();
    if app.mode == SelectMode::Object {
        for id in &app.selection {
            let Some(o) = scene.get(*id) else { continue };
            let mut tr = o.transform;
            tr.translation = pivot + r * (tr.translation - pivot) + t;
            tr.rotation = (r * tr.rotation).normalize();
            tr.scale *= s;
            if commit {
                cmds.push(Command::SetTransform { id: *id, transform: tr });
            } else if let Ok(o) = scene.edit(*id) {
                o.transform = tr;
            }
        }
        return cmds;
    }
    let Some(id) = app.edit_target() else { return cmds };
    let w = scene.world_matrix(id);
    let inv = w.inverse();
    let (_, wr, _) = w.to_scale_rotation_translation();
    let local_t = inv.transform_vector3(t);
    let local_r = wr.inverse() * r * wr;
    let verts = app.component_vertices_cached.clone();
    let op = StepOp::MoveVertices { verts, translate: local_t, rotate: local_r, scale: s };
    if commit {
        cmds.push(Command::AddStep { id, op });
    } else if let Ok(o) = scene.edit(id)
        && let ObjectKind::Mesh { steps, .. } = &mut o.kind
    {
        steps.push(Step { id: u64::MAX, enabled: true, op });
    }
    cmds
}

fn model_input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) {
    // A rig tool (placing joints, painting weights) takes the pointer while it is on.
    if !is_nav(app, ui) && crate::rig_tools::input(app, ui, resp, i, rect) {
        return;
    }
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let shift = ui.input(|inp| inp.modifiers.shift);
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !is_nav(app, ui)
        && let Some(at) = resp.interact_pointer_pos()
    {
        let start = at - resp.drag_delta();
        app.component_vertices_cached = app.component_vertices();
        let kind = match hit_gizmo(app, i, rect, start) {
            Some((handle, pivot, axes)) => DragKind::Gizmo { tool: app.tool, handle, pivot, axes, scene: Box::new(app.doc.scene.clone()) },
            None => DragKind::Box,
        };
        app.drag = Some(Drag { view: i, start, kind });
    }
    let dragging = app.drag.as_ref().is_some_and(|d| d.view == i);
    if dragging
        && resp.dragged_by(egui::PointerButton::Primary)
        && let (Some(cur), Some(Drag { start, kind: DragKind::Gizmo { tool, handle, pivot, axes, scene }, .. })) = (pointer, &app.drag)
    {
        {
            let cam = app.views[i].camera;
            let (tool, handle, pivot, axes, start) = (*tool, *handle, *pivot, *axes, *start);
            let base = (**scene).clone();
            let (t, r, s) = drag_transform(&cam, rect, tool, handle, pivot, &axes, start, cur);
            let (t, r, s) = if snap_on(app, ui) { snapped(t, r, s, app.snap_steps) } else { (t, r, s) };
            let (t, r, s) = (geometry_snap_move(app, &cam, rect, tool, handle, pivot, &axes, cur, t), r, s);
            let mut preview = base;
            apply_drag(app, &mut preview, t, r, s, pivot, false);
            app.doc.scene = preview;
        }
    }
    if dragging
        && resp.drag_stopped()
        && let Some(drag) = app.drag.take()
    {
        {
            let cur = pointer.unwrap_or(drag.start);
            match drag.kind {
                DragKind::Gizmo { tool, handle, pivot, axes, scene } => {
                    let cam = app.views[i].camera;
                    let (t, r, s) = drag_transform(&cam, rect, tool, handle, pivot, &axes, drag.start, cur);
                    let (t, r, s) = if snap_on(app, ui) { snapped(t, r, s, app.snap_steps) } else { (t, r, s) };
                    let t = geometry_snap_move(app, &cam, rect, tool, handle, pivot, &axes, cur, t);
                    app.snaps.hint = None;
                    app.doc.scene = *scene;
                    let mut base = app.doc.scene.clone();
                    let cmds = apply_drag(app, &mut base, t, r, s, pivot, true);
                    for c in cmds {
                        app.run(c);
                    }
                }
                DragKind::Box => box_select(app, i, rect, Rect::from_two_pos(drag.start, cur), shift),
            }
        }
    }
    if resp.clicked_by(egui::PointerButton::Primary)
        && let Some(at) = resp.interact_pointer_pos()
    {
        let cam = app.views[i].camera;
        if !crate::snap_ui::measure_click(app, &cam, rect, at) {
            click_select(app, i, rect, at, shift);
        }
    }
    // Double-click an edge: select its loop.
    if resp.double_clicked() && app.mode == SelectMode::Edge && !app.components.is_empty() {
        app.act(ui.ctx(), crate::actions::Action::SelectLoop);
    }
}

/// A move with geometry snapping: the translation that puts the pivot on the vertex, edge point or
/// surface point under the pointer (along the dragged axis for an axis handle), when one is in
/// range; the plain translation `t` otherwise. The dragged objects are not snapped to.
#[allow(clippy::too_many_arguments)]
pub(crate) fn geometry_snap_move(
    app: &mut ModelApp,
    cam: &Camera,
    rect: Rect,
    tool: Tool,
    handle: Handle,
    pivot: Vec3,
    axes: &[Vec3; 3],
    cur: Pos2,
    t: Vec3,
) -> Vec3 {
    app.snaps.hint = None;
    if tool != Tool::Move || !app.snaps.targets.any() {
        return t;
    }
    let skip: Vec<ObjectId> = if app.mode == SelectMode::Object { app.selection.clone() } else { app.edit_target().into_iter().collect() };
    let Some(hit) = crate::snap_ui::geometry_snap(app, app.snaps.targets, &skip, cam, rect, cur) else { return t };
    app.snaps.hint = Some(hit);
    let d = hit.point - pivot;
    match handle {
        Handle::Axis(k) => axes.get(k).map_or(t, |a| *a * d.dot(*a)),
        Handle::Free => d,
    }
}

/// Snapping is on when the toggle is on, or for this drag while Ctrl is held (and the reverse).
fn snap_on(app: &ModelApp, ui: &egui::Ui) -> bool {
    app.snap != ui.input(|i| i.modifiers.command)
}

fn click_select(app: &mut ModelApp, i: usize, rect: Rect, at: Pos2, add: bool) {
    let cam = app.views[i].camera;
    let ray = world_ray(&cam, rect, at);
    if app.mode != SelectMode::Object {
        let Some((_, m, w)) = edit_mesh(app) else { return };
        let r = ray.transformed(w.inverse());
        let hit = match app.mode {
            SelectMode::Vertex => pick::pick_vertex(&m, &r),
            SelectMode::Edge => pick::pick_edge(&m, &r),
            SelectMode::Face => pick::pick_face(&m, &r).map(|h| h.face),
            SelectMode::Object => None,
        };
        match hit {
            Some(c) if add => {
                if !app.components.remove(&c) {
                    app.components.insert(c);
                }
            }
            Some(c) => app.components = [c].into_iter().collect(),
            None if !add => app.components.clear(),
            None => {}
        }
        return;
    }
    // Joints are picked first, by their bones on screen: they sit inside the meshes they move.
    if let Some(joint) = crate::rig_tools::pick_joint(app, i, rect, at) {
        if add {
            match app.selection.iter().position(|s| *s == joint) {
                Some(p) => {
                    app.selection.remove(p);
                }
                None => app.selection.push(joint),
            }
        } else {
            app.select_only(joint);
        }
        return;
    }
    // A curve under the pointer is picked before the surfaces: splines have no surface to hit.
    if let Some(spline) = crate::spline_ui::pick_curve(app, i, rect, at) {
        if add {
            match app.selection.iter().position(|s| *s == spline) {
                Some(p) => {
                    app.selection.remove(p);
                }
                None => app.selection.push(spline),
            }
        } else {
            app.select_only(spline);
        }
        return;
    }
    let mut best: Option<(f32, ObjectId)> = None;
    for id in drawables(&app.doc.scene) {
        if app.doc.scene.get(id).is_some_and(|o| o.template) {
            continue;
        }
        let w = app.doc.scene.world_matrix(id);
        let r = ray.transformed(w.inverse());
        let dist = match &app.doc.sculpt {
            Some(s) if s.id == id => s.mesh.raycast(r.origin, r.dir).map(|h| w.transform_vector3(r.dir * h.0).length()),
            _ => {
                let e = app.doc.eval.evaluate(&app.doc.scene, id);
                match e.instances {
                    Some(inst) => pick_instances(&inst, &ray, w),
                    None => pick::pick_face(&e.mesh, &r).map(|h| w.transform_vector3(r.dir * h.t).length()),
                }
            }
        };
        if let Some(dist) = dist
            && best.is_none_or(|(b, _)| dist < b)
        {
            best = Some((dist, id));
        }
    }
    match best {
        Some((_, id)) if add => {
            if let Some(p) = app.selection.iter().position(|s| *s == id) {
                app.selection.remove(p);
            } else {
                app.selection.push(id);
            }
        }
        Some((_, id)) => app.select_only(id),
        None if !add => app.selection.clear(),
        None => {}
    }
}

/// The nearest distance at which a world ray hits any clone of a generator (each clone is tested
/// against the source's bounding box first, so a thousand clones cost a thousand box tests).
fn pick_instances(inst: &modelcraft_engine::scene::Instances, ray: &Ray, world: Mat4) -> Option<f32> {
    let (lo, hi) = inst.source.bounds();
    let mut best: Option<f32> = None;
    for c in &inst.clones.instances {
        let m = world * c.matrix;
        if m.determinant().abs() < 1e-12 {
            continue;
        }
        let r = ray.transformed(m.inverse());
        if !pick::ray_aabb(&r, lo, hi) {
            continue;
        }
        if let Some(h) = pick::pick_face(&inst.source, &r) {
            let d = m.transform_vector3(r.dir * h.t).length();
            if best.is_none_or(|b| d < b) {
                best = Some(d);
            }
        }
    }
    best
}

fn box_select(app: &mut ModelApp, i: usize, rect: Rect, sel: Rect, add: bool) {
    let cam = app.views[i].camera;
    if !add {
        if app.mode == SelectMode::Object { app.selection.clear() } else { app.components.clear() }
    }
    if app.mode == SelectMode::Object {
        for id in drawables(&app.doc.scene) {
            if app.doc.scene.get(id).is_some_and(|o| o.template) {
                continue;
            }
            let e = app.doc.eval.evaluate(&app.doc.scene, id);
            let (lo, hi) = match &e.instances {
                // A clone generator is as wide as its clones; with none there is nothing to select.
                Some(inst) if inst.clones.is_empty() => continue,
                Some(inst) => inst.clones.instances.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), c| {
                    let p = c.matrix.w_axis.truncate();
                    (a.min(p), b.max(p))
                }),
                None => e.mesh.bounds(),
            };
            let c = app.doc.scene.world_matrix(id).transform_point3((lo + hi) * 0.5);
            if to_screen(&cam, rect, c).is_some_and(|p| sel.contains(p)) && !app.selection.contains(&id) {
                app.selection.push(id);
            }
        }
        return;
    }
    let Some((_, m, w)) = edit_mesh(app) else { return };
    let inside = |p: Vec3| to_screen(&cam, rect, w.transform_point3(p)).is_some_and(|s| sel.contains(s));
    let budget = 4_000_000usize;
    match app.mode {
        SelectMode::Vertex => app.components.extend((0..m.vertex_count().min(budget) as u32).filter(|v| inside(m.position(*v)))),
        SelectMode::Edge => app.components.extend((0..m.edge_count().min(budget) as u32).filter(|e| {
            let (a, b) = m.edge_vertices(*e);
            inside((m.position(a) + m.position(b)) * 0.5)
        })),
        SelectMode::Face => app.components.extend((0..m.face_count().min(budget) as u32).filter(|f| inside(m.face_center(*f)))),
        SelectMode::Object => {}
    }
}

// ---------------------------------------------------------------- sculpt input

fn pressure(ui: &egui::Ui) -> f32 {
    ui.input(|i| {
        i.raw.events.iter().rev().find_map(|e| match e {
            egui::Event::Touch { force: Some(f), .. } => Some(*f),
            _ => None,
        })
    })
    .filter(|f| f.is_finite() && *f > 0.0)
    .unwrap_or(1.0)
}

fn sculpt_hit(app: &ModelApp, cam: &Camera, rect: Rect, at: Pos2) -> Option<(Vec3, Vec3, Mat4)> {
    let s = app.doc.sculpt.as_ref()?;
    let w = app.doc.scene.world_matrix(s.id);
    let inv = w.inverse();
    let ray = world_ray(cam, rect, at).transformed(inv);
    let (_, p, _) = s.mesh.raycast(ray.origin, ray.dir)?;
    Some((p, ray.dir.normalize_or(Vec3::NEG_Z), w))
}

fn sculpt_input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) {
    if app.doc.sculpt.is_none() {
        return;
    }
    let cam = app.views[i].camera;
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let (ctrl, shift) = ui.input(|inp| (inp.modifiers.command, inp.modifiers.shift));
    let nav = is_nav(app, ui);
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !nav
        && let Some(at) = pointer
        && let Some((hit, dir, w)) = sculpt_hit(app, &cam, rect, at - resp.drag_delta())
    {
        let mut brush = app.brush.clone();
        if shift {
            brush = modelcraft_engine::sculpt::BrushSettings { kind: BrushKind::Smooth, strength: 0.5, ..brush };
        }
        if ctrl {
            brush.invert = !brush.invert;
        }
        let scale = w.to_scale_rotation_translation().0.abs().max_element().max(1e-6);
        brush.radius = app.brush_px * cam.world_per_pixel(w.transform_point3(hit), rect.height()) / scale;
        app.brush.radius = brush.radius;
        // The app keeps the live-detail size as a share of the brush radius.
        brush.live_detail = crate::sculpt_ui::live_detail_edge(app, brush.radius);
        let object = app.doc.sculpt.as_ref().map(|s| s.id).unwrap_or_default();
        let mut maker = modelcraft_engine::sculpt::DabMaker::new(&brush);
        app.doc.stroke_begin(brush);
        let world_hit = w.transform_point3(hit);
        for d in maker.push(hit, dir, pressure(ui)).dabs {
            app.doc.stroke_dab(d);
        }
        app.stroke = Some(StrokeInput { view: i, object, last: Some(hit), grab: Some((world_hit, cam.back(), world_hit)), maker });
    }
    let active = app.stroke.as_ref().is_some_and(|s| s.view == i);
    if active
        && resp.dragged_by(egui::PointerButton::Primary)
        && let Some(at) = pointer
    {
        let p = pressure(ui);
        let kind = app.doc.sculpt.as_ref().map(|_| app.brush.kind);
        if kind.is_some_and(BrushKind::drags) {
            // Move on the plane through the grab point facing the camera.
            let ray = world_ray(&cam, rect, at);
            if let Some(st) = &mut app.stroke
                && let Some((origin, n, last)) = st.grab
            {
                let denom = ray.dir.dot(n);
                if denom.abs() > 1e-6 {
                    let t = (origin - ray.origin).dot(n) / denom;
                    let now = ray.origin + ray.dir * t;
                    let w = app.doc.scene.world_matrix(st.object);
                    let delta = w.inverse().transform_vector3(now - last);
                    st.grab = Some((origin, n, now));
                    let mut center = st.last.unwrap_or(Vec3::ZERO);
                    if kind.is_some_and(BrushKind::travels) {
                        // A pull travels with what it moves, picking up new surface on the way.
                        center += delta;
                        st.last = Some(center);
                    }
                    app.doc.stroke_dab(Dab { center, view_dir: -n, pressure: p, grab_delta: delta, scale: 1.0 });
                }
            }
        } else if let Some((hit, dir, _)) = sculpt_hit(app, &cam, rect, at) {
            let out = app.stroke.as_mut().map(|st| st.maker.push(hit, dir, p)).unwrap_or_default();
            if out.rewind {
                app.doc.stroke_rewind();
            }
            for d in out.dabs {
                app.doc.stroke_dab(d);
            }
        }
    }
    if active && (resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down())) {
        app.doc.stroke_end();
        app.stroke = None;
    }
}

// ---------------------------------------------------------------- drawing

fn render(app: &mut ModelApp, ui: &egui::Ui, i: usize, rect: Rect) {
    let cam = app.views[i].camera;
    let aspect = rect.aspect_ratio();
    if i == app.active_view && aspect.is_finite() && aspect > 0.0 {
        app.uv.view_aspect = aspect;
    }
    let ids = drawables(&app.doc.scene);
    let session = app.doc.sculpt.as_ref().map(|s| s.id);
    let sculpting = app.workspace == Workspace::Sculpt;
    let paint = app.workspace == Workspace::Paint;
    // Evaluate first (the GPU borrow below must not overlap the document's).
    let mut items: Vec<Item> = Vec::new();
    for id in &ids {
        let w = app.doc.scene.world_matrix(*id);
        if Some(*id) == session {
            items.push((*id, None, w, WHITE));
        } else {
            let e = app.doc.eval.evaluate(&app.doc.scene, *id);
            match e.instances {
                // Clones share one mesh (and so one GPU upload); each gets its own matrix and tint.
                Some(inst) => items.extend(inst.clones.instances.iter().map(|c| (*id, Some(inst.source.clone()), w * c.matrix, c.color))),
                None => items.push((*id, Some(e.mesh), w, WHITE)),
            }
        }
    }
    let patches = crate::view_header::patches_of(app, i);
    let mut lines = scene_lines(app, i);
    let mut overlay = face_overlay(app);
    let extras = crate::display_modes::plan(&app.doc.scene, crate::view_header::display_of(app, i), &|id| app.selection.contains(&id), &items);
    lines.extend(extras.lines);
    overlay.extend(extras.overlay);
    let modes = extras.modes;
    let Some(g) = &mut app.gpu else {
        cpu_view(app, ui, i, rect, &items, &lines);
        return;
    };
    let ppp = ui.ctx().pixels_per_point();
    let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
    let mut specs = Vec::new();
    for (n, (id, m, w, tint)) in items.iter().enumerate() {
        let Some(o) = app.doc.scene.get(*id) else { continue };
        let mode = modes.get(n).copied().unwrap_or_default();
        if !crate::display_modes::is_solid(mode) {
            continue;
        }
        let key = if mode == DisplayMode::Flat { m.as_ref().map(|m| g.mesh_flat(m)) } else { m.as_ref().map(|m| g.mesh(m)) };
        let base = material_source(&app.doc.scene, o).material.base_color;
        specs.push(crate::gpu::DrawSpec {
            key,
            model: *w,
            color: [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2], base[3] * tint[3]],
            selected: app.selection.contains(id) && !sculpting,
            show_mask: Some(*id) == session,
            patch_tint: patches || paint,
            uv_checker: app.uv.viewport_checker && m.as_ref().is_some_and(|m| m.has_uvs()),
            roughness: o.material.roughness,
        });
    }
    let hair = crate::hair_ui::hair_specs(&mut app.doc, &app.groom, &app.selection, g);
    if let Some(tex) = g.draw(i, size, cam.view_proj(aspect), cam.eye(), theme::VIEWPORT_BG, &specs, &lines, &overlay, &hair) {
        ui.painter_at(rect).image(tex, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// One drawn mesh: its object, the mesh (`None`: the sculpt mesh), its world matrix and a colour tint.
type Item = (ObjectId, Option<Arc<PolyMesh>>, Mat4, [f32; 4]);
const WHITE: [f32; 4] = [1.0; 4];

/// The object whose material colours `o`: a clone generator or particle system is coloured by its source.
fn material_source<'a>(scene: &'a Scene, o: &'a Object) -> &'a Object {
    match &o.kind {
        ObjectKind::Cloner { .. } => scene.generator_input(o.id).and_then(|c| scene.get(c)).map_or(o, |c| material_source(scene, c)),
        ObjectKind::Sim { .. } if scene.is_particles(o.id) => scene.generator_input(o.id).and_then(|c| scene.get(c)).map_or(o, |c| material_source(scene, c)),
        _ => o,
    }
}

fn line(a: Vec3, b: Vec3, c: [f32; 4]) -> [LineVertex; 2] {
    [LineVertex { pos: a.to_array(), color: c }, LineVertex { pos: b.to_array(), color: c }]
}

/// Grid, wireframes of selected objects and cages, selected edges.
fn scene_lines(app: &mut ModelApp, i: usize) -> Vec<LineVertex> {
    let mut out = buffers::grid(20, 0.5, [0.24, 0.25, 0.27, 1.0], [0.34, 0.35, 0.38, 1.0]);
    if app.views[i].preset == ViewPreset::Front || app.views[i].preset == ViewPreset::Right || !crate::view_header::grid_of(app, i) {
        out.clear();
    }
    for (a, b, color) in crate::rig_ui::bone_lines(app) {
        out.extend(line(a.into(), b.into(), color));
    }
    for (a, b, color) in crate::anim_ui::path_lines(app) {
        out.extend(line(a.into(), b.into(), color));
    }
    for (a, b, color) in crate::hair_ui::guide_lines(app) {
        out.extend(line(a.into(), b.into(), color));
    }
    for (a, b, color) in crate::spline_ui::curve_lines(app) {
        out.extend(line(a.into(), b.into(), color));
    }
    if app.workspace == Workspace::Sculpt {
        return out;
    }
    let wire = [0.55, 0.58, 0.62, 0.55];
    let cage = [0.62, 0.66, 0.72, 0.8];
    let sel_wire = [1.0, 0.62, 0.25, 0.9];
    let objects: Vec<ObjectId> = app.doc.scene.objects().iter().map(|o| o.id).collect();
    for id in objects {
        let is_cage_input = app.doc.scene.is_cage_input(id);
        let selected =
            app.selection.contains(&id) || (is_cage_input && app.doc.scene.get(id).and_then(|o| o.parent).is_some_and(|p| app.selection.contains(&p)));
        if !(is_cage_input || (selected && app.show_wire)) || !app.doc.scene.visible(id) {
            continue;
        }
        let m = if is_cage_input { app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh } else { app.doc.eval.evaluate(&app.doc.scene, id).mesh };
        let w = app.doc.scene.world_matrix(id);
        let color = if is_cage_input { cage } else { wire };
        if m.edge_count() > 400_000 {
            continue;
        }
        for e in 0..m.edge_count() as u32 {
            let (a, b) = m.edge_vertices(e);
            out.extend(line(w.transform_point3(m.position(a)), w.transform_point3(m.position(b)), color));
        }
    }
    // Outline of the selected falloff fields (and of every field of a selected deformer).
    let sel_fields: Vec<ObjectId> = app
        .doc
        .scene
        .objects()
        .iter()
        .filter(|o| matches!(o.kind, ObjectKind::Field { .. }) && app.doc.scene.visible(o.id))
        .filter(|o| app.selection.contains(&o.id) || o.parent.is_some_and(|p| app.selection.contains(&p)))
        .map(|o| o.id)
        .collect();
    for f in sel_fields {
        for (a, b) in crate::field_ui::outline(&app.doc.scene, f) {
            out.extend(line(a, b, [0.45, 0.85, 0.8, 0.9]));
        }
    }
    if app.mode == SelectMode::Edge
        && let Some((_, m, w)) = edit_mesh(app)
    {
        for e in &app.components {
            let (a, b) = m.edge_vertices(*e);
            out.extend(line(w.transform_point3(m.position(a)), w.transform_point3(m.position(b)), sel_wire));
        }
    }
    out.extend(crate::uv_ui::seam_lines(app));
    out
}

fn face_overlay(app: &mut ModelApp) -> Vec<LineVertex> {
    let mut out = Vec::new();
    if app.mode != SelectMode::Face || app.components.is_empty() {
        return out;
    }
    let Some((_, m, w)) = edit_mesh(app) else { return out };
    let c = [1.0, 0.55, 0.15, 0.38];
    for f in app.components.iter().take(2_000_000) {
        let vs: Vec<Vec3> = m.face_vertices(*f).map(|v| w.transform_point3(m.position(v))).collect();
        if let Some(&a) = vs.first() {
            for k in 1..vs.len().saturating_sub(1) {
                for p in [a, vs.get(k).copied().unwrap_or(a), vs.get(k + 1).copied().unwrap_or(a)] {
                    out.push(LineVertex { pos: p.to_array(), color: c });
                }
            }
        }
    }
    out
}

/// No GPU: a wireframe drawn with the UI painter (bounded), so the app stays usable and testable.
fn cpu_view(app: &ModelApp, ui: &egui::Ui, i: usize, rect: Rect, items: &[Item], lines: &[LineVertex]) {
    let cam = app.views[i].camera;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(29, 31, 34));
    let to = |p: [f32; 3]| to_screen(&cam, rect, Vec3::from(p));
    for pair in lines.chunks(2).take(50_000) {
        if let [a, b] = pair
            && let (Some(p), Some(q)) = (to(a.pos), to(b.pos))
        {
            let c =
                Color32::from_rgba_unmultiplied((a.color[0] * 255.0) as u8, (a.color[1] * 255.0) as u8, (a.color[2] * 255.0) as u8, (a.color[3] * 255.0) as u8);
            painter.line_segment([p, q], Stroke::new(1.0, c));
        }
    }
    let mut budget = 120_000usize;
    for (id, m, w, _) in items {
        let Some(m) = m else { continue };
        let sel = app.selection.contains(id);
        let c = if sel { theme::SELECT } else { theme::TEXT_DIM };
        for e in 0..(m.edge_count() as u32).min(30_000).min(budget as u32) {
            budget = budget.saturating_sub(1);
            let (a, b) = m.edge_vertices(e);
            if let (Some(p), Some(q)) = (to_screen(&cam, rect, w.transform_point3(m.position(a))), to_screen(&cam, rect, w.transform_point3(m.position(b)))) {
                painter.line_segment([p, q], Stroke::new(1.0, c));
            }
        }
    }
    painter.text(
        rect.right_bottom() - egui::vec2(8.0, 8.0),
        egui::Align2::RIGHT_BOTTOM,
        tl!("Wireframe preview (no GPU)"),
        egui::FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
}

fn overlays(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) {
    crate::hair_ui::overlay(app, ui, rect);
    let painter = ui.painter_at(rect);
    let cam = app.views[i].camera;
    crate::snap_ui::overlay(app, &painter, &cam, rect);
    // Focus/active outline (the view's name is in its header).
    let active = app.active_view == i;
    if (app.quad && active) || resp.has_focus() {
        painter.rect_stroke(
            rect.shrink(0.5),
            0.0,
            Stroke::new(1.0, if resp.has_focus() { theme::ACCENT } else { Color32::from_rgb(70, 74, 80) }),
            egui::StrokeKind::Inside,
        );
    }
    if crate::view_header::axes_of(app, i) {
        axis_triad(&painter, &cam, rect);
    }
    // Component points.
    if app.workspace.models()
        && app.mode == SelectMode::Vertex
        && let Some((_, m, w)) = edit_mesh(app)
    {
        let n = m.vertex_count().min(100_000);
        for v in 0..n as u32 {
            if let Some(p) = to_screen(&cam, rect, w.transform_point3(m.position(v))) {
                let sel = app.components.contains(&v);
                painter.circle_filled(p, if sel { 3.5 } else { 2.2 }, if sel { theme::SELECT } else { Color32::from_rgb(150, 160, 175) });
            }
        }
    }
    // Box selection.
    if let (Some(Drag { kind: DragKind::Box, start, view }), Some(cur)) = (&app.drag, resp.interact_pointer_pos())
        && *view == i
    {
        let r = Rect::from_two_pos(*start, cur);
        painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(232, 138, 60, 28));
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, theme::ACCENT), egui::StrokeKind::Inside);
    }
    if app.workspace.models() {
        gizmo(app, &painter, i, rect);
    }
    crate::sculpt_groups_ui::overlay(app, &painter, i);
    if app.workspace == Workspace::Sculpt
        && app.sculpt_groups.tool == crate::sculpt_groups_ui::RegionTool::Brush
        && let Some(at) = resp.hover_pos()
    {
        let col = if app.brush.kind == BrushKind::Mask { Color32::from_rgb(110, 160, 255) } else { theme::ACCENT };
        let on_surface = sculpt_hit(app, &cam, rect, at).is_some();
        painter.circle_stroke(at, app.brush_px, Stroke::new(if on_surface { 1.5 } else { 1.0 }, if on_surface { col } else { theme::TEXT_DIM }));
        painter.circle_filled(at, 1.5, col);
        if app.brush.symmetry.x {
            painter.text(at + egui::vec2(app.brush_px + 4.0, -8.0), egui::Align2::LEFT_TOP, "X", egui::FontId::proportional(10.0), col);
        }
    }
    if app.doc.sculpt.is_none() && app.workspace == Workspace::Sculpt {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, tl!("Select a mesh to sculpt"), egui::FontId::proportional(14.0), theme::TEXT_DIM);
    }
}

fn axis_triad(painter: &egui::Painter, cam: &Camera, rect: Rect) {
    let o = rect.left_bottom() + egui::vec2(34.0, -34.0);
    let view = cam.view();
    for (ax, c, n) in [(Vec3::X, theme::AXIS_X, "X"), (Vec3::Y, theme::AXIS_Y, "Y"), (Vec3::Z, theme::AXIS_Z, "Z")] {
        let v = view.transform_vector3(ax);
        let e = o + egui::vec2(v.x, -v.y) * 22.0;
        painter.line_segment([o, e], Stroke::new(2.0, c));
        painter.text(e, egui::Align2::CENTER_CENTER, n, egui::FontId::proportional(10.0), c);
    }
}

fn gizmo(app: &mut ModelApp, painter: &egui::Painter, i: usize, rect: Rect) {
    if app.tool == Tool::Select {
        return;
    }
    let Some(p) = pivot(app) else { return };
    let cam = app.views[i].camera;
    let Some(ps) = to_screen(&cam, rect, p) else { return };
    let l = gizmo_len(&cam, rect, p);
    let active = match &app.drag {
        Some(Drag { kind: DragKind::Gizmo { handle, .. }, .. }) => Some(*handle),
        _ => None,
    };
    let cols = [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z];
    let axes = match &app.drag {
        Some(Drag { kind: DragKind::Gizmo { axes, .. }, .. }) => *axes,
        _ => crate::snap_ui::gizmo_axes(app),
    };
    for (k, ax) in axes.iter().enumerate() {
        let c = if active == Some(Handle::Axis(k)) { Color32::from_rgb(255, 220, 90) } else { cols.get(k).copied().unwrap_or(Color32::WHITE) };
        match app.tool {
            Tool::Rotate => {
                let pts = ring(&cam, rect, p, *ax, l);
                painter.add(egui::Shape::line(pts, Stroke::new(2.0, c)));
            }
            _ => {
                let Some(e) = to_screen(&cam, rect, p + *ax * l) else { continue };
                painter.line_segment([ps, e], Stroke::new(2.0, c));
                let dir = (e - ps).normalized();
                if app.tool == Tool::Move {
                    let side = Vec2::new(-dir.y, dir.x) * 5.0;
                    painter.add(egui::Shape::convex_polygon(vec![e + dir * 10.0, e + side, e - side], c, Stroke::NONE));
                } else {
                    painter.rect_filled(Rect::from_center_size(e, egui::vec2(8.0, 8.0)), 1.0, c);
                }
            }
        }
    }
    if app.tool != Tool::Rotate {
        let c = if active == Some(Handle::Free) { Color32::from_rgb(255, 220, 90) } else { Color32::from_rgb(230, 230, 230) };
        painter.rect_stroke(Rect::from_center_size(ps, egui::vec2(12.0, 12.0)), 1.0, Stroke::new(1.5, c), egui::StrokeKind::Inside);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapping_rounds_each_part() {
        let (t, r, s) = snapped(Vec3::new(0.3, -0.62, 0.0), Quat::from_rotation_y(20f32.to_radians()), Vec3::new(1.0, 1.26, 1.0), (0.25, 15.0, 0.1));
        assert!((t - Vec3::new(0.25, -0.5, 0.0)).length() < 1e-5);
        assert!((r.to_axis_angle().1.to_degrees() - 15.0).abs() < 1e-3);
        assert!((s.y - 1.3).abs() < 1e-5 && s.x == 1.0);
    }
}
