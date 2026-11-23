//! Light and camera objects in the app: the Create entries, their attribute pages, their
//! viewport drawings and picking, and looking through a camera.
//!
//! Everything the pages and actions do is a `stage` command ([`modelcraft_engine::StageCommand`]),
//! so it is undoable, journaled and scriptable. Looking through a camera makes a view show the
//! camera's picture; moving that view moves the camera.

use egui::{Pos2, Rect, RichText};
use glam::{Mat4, Quat, Vec3};
use modelcraft_engine::scene::stage::{CameraData, LightData, LightKind, Placement};
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene};
use modelcraft_engine::{Command, StageCommand};
use modelcraft_render::{Camera, Projection};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name};
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageAction {
    AddLight(LightKind),
    AddCamera,
    /// A camera placed where the active view looks from.
    CameraFromView,
    /// The active view shows the selected camera's picture.
    LookThrough,
    /// The active view goes back to its own camera.
    StopLookingThrough,
    /// Renders look through the selected camera.
    UseForRender,
}

impl StageAction {
    pub const ALL: [StageAction; 10] = [
        StageAction::AddLight(LightKind::Point),
        StageAction::AddLight(LightKind::Spot),
        StageAction::AddLight(LightKind::Area),
        StageAction::AddLight(LightKind::Sun),
        StageAction::AddLight(LightKind::Dome),
        StageAction::AddCamera,
        StageAction::CameraFromView,
        StageAction::LookThrough,
        StageAction::StopLookingThrough,
        StageAction::UseForRender,
    ];

    pub fn label(self) -> String {
        match self {
            StageAction::AddLight(LightKind::Point) => tl!("Add Point Light"),
            StageAction::AddLight(LightKind::Spot) => tl!("Add Spot Light"),
            StageAction::AddLight(LightKind::Area) => tl!("Add Area Light"),
            StageAction::AddLight(LightKind::Sun) => tl!("Add Sun Light"),
            StageAction::AddLight(LightKind::Dome) => tl!("Add Dome Light"),
            StageAction::AddCamera => tl!("Add Camera"),
            StageAction::CameraFromView => tl!("Add Camera From View"),
            StageAction::LookThrough => tl!("Look Through Selected Camera"),
            StageAction::StopLookingThrough => tl!("Stop Looking Through Camera"),
            StageAction::UseForRender => tl!("Render Through Selected Camera"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            StageAction::AddLight(LightKind::Point) => "create.light_point",
            StageAction::AddLight(LightKind::Spot) => "create.light_spot",
            StageAction::AddLight(LightKind::Area) => "create.light_area",
            StageAction::AddLight(LightKind::Sun) => "create.light_sun",
            StageAction::AddLight(LightKind::Dome) => "create.light_dome",
            StageAction::AddCamera => "create.camera",
            StageAction::CameraFromView => "create.camera_from_view",
            StageAction::LookThrough => "view.look_through_camera",
            StageAction::StopLookingThrough => "view.stop_looking_through_camera",
            StageAction::UseForRender => "render.use_camera",
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            StageAction::LookThrough => Some("Cmd+Shift+0"),
            _ => None,
        }
    }

    /// The menu the action is listed under in the shortcuts editor.
    pub fn menu(self) -> &'static str {
        match self {
            StageAction::AddLight(_) | StageAction::AddCamera | StageAction::CameraFromView => "Create",
            _ => "View",
        }
    }
}

/// Which camera object each of the four views is looking through, and the camera it was given the
/// last time (so a change made by navigating can be told from the camera's own settings).
#[derive(Default)]
pub struct StageUi {
    looking: [Option<(ObjectId, Camera)>; 4],
}

impl StageUi {
    /// The camera object view `i` looks through.
    pub fn looking_through(&self, i: usize) -> Option<ObjectId> {
        self.looking.get(i).copied().flatten().map(|(id, _)| id)
    }
}

pub fn act(app: &mut ModelApp, a: StageAction) {
    match a {
        StageAction::AddLight(kind) => {
            app.run(Command::Stage(StageCommand::AddLight { kind: kind.id().into(), name: None, parent: None, transform: None }));
        }
        StageAction::AddCamera => {
            app.run(Command::Stage(StageCommand::AddCamera { name: None, parent: None, transform: None }));
        }
        StageAction::CameraFromView => {
            let transform = view_transform(&app.views[app.active_view.min(3)].camera);
            app.run(Command::Stage(StageCommand::AddCamera { name: None, parent: None, transform: Some(transform) }));
        }
        StageAction::LookThrough => match selected_camera(app) {
            Some(id) => look_through(app, app.active_view.min(3), id),
            None => app.status = tl!("Select a camera first"),
        },
        StageAction::StopLookingThrough => {
            let i = app.active_view.min(3);
            if app.stage.looking[i].take().is_some() {
                app.status = tl!("Back to the view's own camera");
            }
        }
        StageAction::UseForRender => match selected_camera(app) {
            Some(id) => {
                app.run(Command::Stage(StageCommand::SetRenderCamera { id: Some(id) }));
            }
            None => app.status = tl!("Select a camera first"),
        },
    }
}

fn selected_camera(app: &ModelApp) -> Option<ObjectId> {
    app.active().filter(|id| app.doc.scene.camera_data(*id).is_some())
}

/// Make view `i` show camera `id`'s picture.
pub fn look_through(app: &mut ModelApp, i: usize, id: ObjectId) {
    let aspect = render_aspect(app);
    if let Some(cam) = camera_for(&app.doc.scene, id, aspect) {
        app.views[i].camera = cam;
        app.stage.looking[i] = Some((id, cam));
        app.status = crate::tlf!("Looking through {camera}", camera = app.doc.scene.get(id).map_or(String::new(), |o| o.name.clone()));
    }
}

/// Width over height of the pictures renders make.
fn render_aspect(app: &ModelApp) -> f32 {
    app.render_view.width.max(1) as f32 / app.render_view.height.max(1) as f32
}

/// The viewport's orbit camera that shows what camera object `id` sees. The orbit camera cannot
/// roll, so a rolled camera is shown level.
pub fn camera_for(scene: &Scene, id: ObjectId, aspect: f32) -> Option<Camera> {
    let (p, d) = scene.camera_view(id)?;
    let distance = d.focus_distance.clamp(0.01, 1e5);
    let back = -p.forward;
    let pitch = back.y.clamp(-1.0, 1.0).asin().clamp(-1.55, 1.55);
    let yaw = back.x.atan2(back.z);
    let (projection, fov_y) = if d.orthographic {
        // The orthographic view in the viewport sees `distance · tan(fov/2)` each way up.
        (Projection::Orthographic, 2.0 * (d.ortho_height * 0.5 / distance).atan())
    } else {
        (Projection::Perspective, d.fov_y(aspect))
    };
    Some(Camera { target: p.eye + p.forward * distance, yaw, pitch, distance, fov_y, projection })
}

/// The transform of an object that sits where the orbit camera does and looks the way it looks.
pub fn view_transform(cam: &Camera) -> modelcraft_engine::scene::Transform {
    let forward = -cam.back();
    let right = forward.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let up = right.cross(forward);
    modelcraft_engine::scene::Transform { translation: cam.eye(), rotation: Quat::from_mat3(&glam::Mat3::from_cols(right, up, -forward)), scale: Vec3::ONE }
}

// ---------------------------------------------------------------- looking through, per frame

/// Before a view is drawn: a view that looks through a camera takes the camera's current place
/// and lens (so it follows the camera's keys). A view whose camera is gone goes back to its own.
pub fn begin_view(app: &mut ModelApp, i: usize) {
    let Some((id, _)) = app.stage.looking.get(i).copied().flatten() else { return };
    let aspect = render_aspect(app);
    match camera_for(&app.doc.scene, id, aspect) {
        Some(cam) => {
            app.views[i].camera = cam;
            app.stage.looking[i] = Some((id, cam));
        }
        None => app.stage.looking[i] = None,
    }
}

/// After the view's navigation: if the user moved a view that looks through a camera, the camera
/// moves with it. A drag is previewed and made one undo step when it ends.
pub fn end_view(app: &mut ModelApp, i: usize, dragging: bool) {
    let Some((id, given)) = app.stage.looking.get(i).copied().flatten() else { return };
    let now = app.views[i].camera;
    if now != given {
        let Some(o) = app.doc.scene.get(id) else { return };
        let old = o.transform;
        let world = view_transform(&now);
        let parent_world = o.parent.map_or(Mat4::IDENTITY, |p| app.doc.scene.world_matrix(p));
        let local = parent_world.inverse() * Mat4::from_rotation_translation(world.rotation, world.translation);
        let (_, rotation, translation) = local.to_scale_rotation_translation();
        let t = modelcraft_engine::scene::Transform { translation, rotation, scale: old.scale }.sanitized();
        if t.translation.is_finite() && t != old {
            // The lens follows the zoom: moving along the view changes where the camera is, not
            // its settings.
            if dragging {
                if app.prop_snapshot.is_none() {
                    app.prop_snapshot = Some(Box::new(app.doc.scene.clone()));
                }
                if let Ok(o) = app.doc.scene.edit(id) {
                    o.transform = t;
                }
            } else {
                if let Some(s) = app.prop_snapshot.take() {
                    app.doc.scene = *s;
                }
                app.run(Command::SetTransform { id, transform: t });
            }
        }
        app.stage.looking[i] = camera_for(&app.doc.scene, id, render_aspect(app)).map(|c| (id, c));
    } else if !dragging && let Some(s) = app.prop_snapshot.take() {
        // The drag ended: the preview becomes one undo step.
        let final_t = app.doc.scene.get(id).map(|o| o.transform);
        app.doc.scene = *s;
        if let Some(t) = final_t {
            app.run(Command::SetTransform { id, transform: t });
        }
    }
}

// ---------------------------------------------------------------- drawings

const LIGHT_COLOR: [f32; 4] = [1.0, 0.86, 0.38, 0.95];
const CAMERA_COLOR: [f32; 4] = [0.55, 0.8, 1.0, 0.95];
const SELECTED: [f32; 4] = [1.0, 0.62, 0.25, 1.0];

fn axes(world: Mat4) -> (Vec3, Vec3, Vec3, Vec3) {
    let p = Placement::of(world);
    let right = p.forward.cross(p.up).try_normalize().unwrap_or(Vec3::X);
    (world.transform_point3(Vec3::ZERO), right, p.up, p.forward)
}

fn circle(out: &mut Vec<(Vec3, Vec3)>, centre: Vec3, u: Vec3, v: Vec3, r: f32) {
    let n = 24;
    for k in 0..n {
        let (a, b) = (k as f32 / n as f32 * std::f32::consts::TAU, (k + 1) as f32 / n as f32 * std::f32::consts::TAU);
        out.push((centre + (u * a.cos() + v * a.sin()) * r, centre + (u * b.cos() + v * b.sin()) * r));
    }
}

/// How big the drawing of a light or camera is: a fixed size in the scene, a little larger than
/// the default lamp so it reads at the default view.
const GIZMO: f32 = 0.3;

/// The segments (world space) that draw light or camera `id`, and whether it is one of the two.
pub fn outline(scene: &Scene, id: ObjectId) -> Vec<(Vec3, Vec3)> {
    let Some(o) = scene.get(id) else { return Vec::new() };
    let world = scene.world_matrix(id);
    let (eye, right, up, forward) = axes(world);
    let mut out = Vec::new();
    match &o.kind {
        ObjectKind::Light { light } => {
            let light = light.sanitized();
            match light.kind {
                LightKind::Point => {
                    circle(&mut out, eye, right, up, GIZMO);
                    circle(&mut out, eye, right, forward, GIZMO);
                    circle(&mut out, eye, up, forward, GIZMO);
                }
                LightKind::Spot => {
                    let reach = 1.5;
                    let r = reach * (light.spread.to_radians() * 0.5).tan().min(20.0);
                    let c = eye + forward * reach;
                    circle(&mut out, c, right, up, r);
                    for s in [right, -right, up, -up] {
                        out.push((eye, c + s * r));
                    }
                    circle(&mut out, eye, right, up, GIZMO * 0.4);
                }
                LightKind::Area => {
                    let (w, h) = (world.transform_vector3(Vec3::X) * light.size * 0.5, world.transform_vector3(Vec3::Y) * light.height * 0.5);
                    let corners = [eye - w - h, eye + w - h, eye + w + h, eye - w + h];
                    for k in 0..4 {
                        out.push((corners[k], corners[(k + 1) % 4]));
                    }
                    out.push((eye, eye + forward * 0.5));
                    out.push((eye + forward * 0.5, eye + forward * 0.4 + right * 0.06));
                    out.push((eye + forward * 0.5, eye + forward * 0.4 - right * 0.06));
                }
                LightKind::Sun => {
                    circle(&mut out, eye, right, up, GIZMO);
                    for k in 0..8 {
                        let a = k as f32 / 8.0 * std::f32::consts::TAU;
                        let d = right * a.cos() + up * a.sin();
                        out.push((eye + d * GIZMO * 1.2, eye + d * GIZMO * 1.8));
                    }
                    out.push((eye, eye + forward * 2.5));
                }
                LightKind::Dome => {
                    let r = 1.0;
                    circle(&mut out, eye, Vec3::X, Vec3::Z, r);
                    circle(&mut out, eye, Vec3::X, Vec3::Y, r);
                    circle(&mut out, eye, Vec3::Z, Vec3::Y, r);
                }
            }
        }
        ObjectKind::Camera { camera } => {
            let c = camera.sanitized();
            let depth = 0.8;
            let (hw, hh) = if c.orthographic {
                let hh = c.ortho_height * 0.5;
                (hh * c.film_aspect(), hh)
            } else {
                let hw = depth * c.sensor_width * 0.5 / c.focal_length;
                (hw, hw / c.film_aspect())
            };
            let centre = eye + forward * depth;
            let corners = [centre - right * hw - up * hh, centre + right * hw - up * hh, centre + right * hw + up * hh, centre - right * hw + up * hh];
            for k in 0..4 {
                out.push((corners[k], corners[(k + 1) % 4]));
                out.push((if c.orthographic { corners[k] - forward * depth } else { eye }, corners[k]));
            }
            // A triangle on top marks which way is up.
            let top = centre + up * hh;
            out.push((corners[3] + up * hh * 0.05, top + up * hh * 0.35));
            out.push((corners[2] + up * hh * 0.05, top + up * hh * 0.35));
        }
        _ => {}
    }
    out
}

/// Lines for every visible light and camera, selected ones highlighted; the camera a view looks
/// through is not drawn in that view.
pub fn lines(app: &ModelApp, view: usize) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    let scene = &app.doc.scene;
    let through = app.stage.looking_through(view);
    let mut out = Vec::new();
    for o in scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Light { .. } | ObjectKind::Camera { .. })).take(2000) {
        if !scene.visible(o.id) || Some(o.id) == through {
            continue;
        }
        let selected = app.selection.contains(&o.id);
        let color = if selected {
            SELECTED
        } else if matches!(o.kind, ObjectKind::Camera { .. }) {
            if scene.render_camera() == Some(o.id) { [0.7, 0.95, 1.0, 1.0] } else { CAMERA_COLOR }
        } else {
            LIGHT_COLOR
        };
        out.extend(outline(scene, o.id).into_iter().map(|(a, b)| (a.to_array(), b.to_array(), color)));
    }
    out
}

/// A light or camera under the click: the one whose drawing passes closest to it on screen.
pub fn pick(app: &ModelApp, view: usize, rect: Rect, at: Pos2) -> Option<ObjectId> {
    const REACH: f32 = 7.0;
    let scene = &app.doc.scene;
    let cam = app.views.get(view)?.camera;
    let through = app.stage.looking_through(view);
    let mut best: Option<(f32, ObjectId)> = None;
    for o in scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Light { .. } | ObjectKind::Camera { .. })).take(2000) {
        if !scene.visible(o.id) || o.template || Some(o.id) == through {
            continue;
        }
        for (a, b) in outline(scene, o.id) {
            let (Some(p), Some(q)) = (crate::viewport::to_screen(&cam, rect, a), crate::viewport::to_screen(&cam, rect, b)) else { continue };
            let ab = q - p;
            let t = if ab.length_sq() > 1e-6 { ((at - p).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
            let d = (p + ab * t).distance(at);
            if d <= REACH && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, o.id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// A small drawn badge for the tree and the attribute page.
pub fn badge(p: &egui::Painter, c: Pos2, kind: &ObjectKind) {
    let yellow = egui::Color32::from_rgb(250, 220, 100);
    let blue = egui::Color32::from_rgb(140, 200, 250);
    match kind {
        ObjectKind::Light { light } => match light.kind {
            LightKind::Spot => {
                p.add(egui::Shape::closed_line(
                    vec![c + egui::vec2(0.0, -5.0), c + egui::vec2(5.0, 5.0), c + egui::vec2(-5.0, 5.0)],
                    egui::Stroke::new(1.2, yellow),
                ));
            }
            LightKind::Area => {
                p.rect_stroke(egui::Rect::from_center_size(c, egui::vec2(10.0, 7.0)), 0.0, egui::Stroke::new(1.2, yellow), egui::StrokeKind::Inside);
            }
            LightKind::Sun => {
                p.circle_filled(c, 3.0, yellow);
                for k in 0..8 {
                    let a = k as f32 / 8.0 * std::f32::consts::TAU;
                    let d = egui::vec2(a.cos(), a.sin());
                    p.line_segment([c + d * 4.5, c + d * 6.0], egui::Stroke::new(1.0, yellow));
                }
            }
            LightKind::Dome => {
                p.circle_stroke(c, 5.0, egui::Stroke::new(1.2, yellow));
                p.line_segment([c + egui::vec2(-5.0, 0.0), c + egui::vec2(5.0, 0.0)], egui::Stroke::new(1.0, yellow));
            }
            LightKind::Point => {
                p.circle_filled(c, 3.0, yellow);
                p.circle_stroke(c, 5.0, egui::Stroke::new(1.0, yellow.gamma_multiply(0.6)));
            }
        },
        _ => {
            // A camera: a box with a lens.
            p.rect_stroke(
                egui::Rect::from_center_size(c + egui::vec2(-1.0, 0.0), egui::vec2(8.0, 7.0)),
                0.0,
                egui::Stroke::new(1.2, blue),
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::closed_line(vec![c + egui::vec2(3.0, 0.0), c + egui::vec2(6.0, -3.0), c + egui::vec2(6.0, 3.0)], egui::Stroke::new(1.2, blue)));
        }
    }
}

// ---------------------------------------------------------------- menus and pages

/// The View menu's camera entries.
pub fn view_menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    for a in StageAction::ALL.iter().skip(7) {
        menu_button(app, ui, *a);
    }
}

fn menu_button(app: &mut ModelApp, ui: &mut egui::Ui, a: StageAction) {
    let ctx = ui.ctx().clone();
    let b = egui::Button::new(crate::actions::Action::Stage(a).label()).shortcut_text(crate::actions::Action::Stage(a).shortcut_text(app));
    if ui.add(b).clicked() {
        app.act(&ctx, crate::actions::Action::Stage(a));
        ui.close();
    }
}

fn kind_label(k: LightKind) -> String {
    match k {
        LightKind::Point => tl!("Point"),
        LightKind::Spot => tl!("Spot"),
        LightKind::Area => tl!("Area"),
        LightKind::Sun => tl!("Sun"),
        LightKind::Dome => tl!("Dome"),
    }
}

/// The attribute page of a light.
pub fn light_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, light: &LightData) {
    let mut q = light.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    ui.horizontal(|ui| {
        ui.label(tl!("Kind"));
        let r = egui::ComboBox::from_id_salt(("light-kind", id.0)).selected_text(kind_label(q.kind)).show_ui(ui, |ui| {
            for k in LightKind::ALL {
                if ui.selectable_label(q.kind == k, kind_label(k)).clicked() && q.kind != k {
                    // A new kind starts from its own useful numbers, keeping the colour.
                    q = LightData { color: q.color, image: q.image.clone(), ..LightData::of(k) };
                    discrete = true;
                }
            }
        });
        set_name(ui, &r.response, tl!("Kind"));
    });
    ui.horizontal(|ui| {
        ui.label(tl!("Colour"));
        let r = ui.color_edit_button_rgb(&mut q.color);
        set_name(ui, &r, tl!("Colour"));
        drags.push(r);
    });
    let intensity_name = match q.kind {
        LightKind::Point | LightKind::Spot => tl!("Intensity (W/sr)"),
        LightKind::Sun => tl!("Irradiance"),
        LightKind::Area | LightKind::Dome => tl!("Radiance"),
    };
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.intensity, 0.1, 0.0..=crate::stage_ui::MAX_INTENSITY, intensity_name)));
    match q.kind {
        LightKind::Area => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut q.size, 0.01, 0.0..=1000.0, tl!("Width"))));
            ui.horizontal(|ui| drags.push(drag(ui, &mut q.height, 0.01, 0.0..=1000.0, tl!("Height"))));
        }
        LightKind::Sun => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut q.size, 0.05, 0.0..=40.0, tl!("Apparent size (degrees)"))));
        }
        LightKind::Spot => {
            ui.horizontal(|ui| drags.push(drag(ui, &mut q.spread, 0.5, 1.0..=179.0, tl!("Cone angle (degrees)"))));
            ui.horizontal(|ui| drags.push(drag(ui, &mut q.softness, 0.01, 0.0..=1.0, tl!("Edge softness"))));
        }
        LightKind::Dome => {
            ui.horizontal(|ui| {
                ui.label(tl!("Picture"));
                ui.label(RichText::new(q.image.as_deref().unwrap_or(&tl!("none"))).color(theme::TEXT_DIM));
                if ui.button(tl!("Choose Picture…")).clicked()
                    && let Some(path) = rfd::FileDialog::new().add_filter(tl!("Radiance HDR"), &["hdr"]).pick_file()
                {
                    q.image = Some(path.display().to_string());
                    discrete = true;
                }
                if ui.add_enabled(q.image.is_some(), egui::Button::new(tl!("Clear Picture"))).clicked() {
                    q.image = None;
                    discrete = true;
                }
            });
        }
        LightKind::Point => {}
    }
    let q = q.sanitized();
    if q != *light && discrete {
        app.run(Command::Stage(StageCommand::SetLight { id, light: q }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Light { light: q.clone() }, || Command::Stage(StageCommand::SetLight { id, light: q.clone() }), id);
    }
    ui.label(
        RichText::new(tl!("The light shines along its own blue (Z) axis, away from where it faces. Move and turn the object to aim it."))
            .color(theme::TEXT_DIM)
            .small(),
    );
}

/// Largest intensity the page offers.
pub const MAX_INTENSITY: f32 = modelcraft_engine::scene::stage::MAX_INTENSITY;

/// The attribute page of a camera.
pub fn camera_fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, camera: &CameraData) {
    let mut q = camera.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.focal_length, 0.5, 4.0..=2000.0, tl!("Focal length (mm)"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.sensor_width, 0.1, 1.0..=200.0, tl!("Sensor width (mm)"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.sensor_height, 0.1, 1.0..=200.0, tl!("Sensor height (mm)"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.near, 0.01, 0.0001..=100_000.0, tl!("Near clip"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.far, 1.0, 0.001..=10_000_000.0, tl!("Far clip"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.focus_distance, 0.02, 0.001..=1_000_000.0, tl!("Focus distance"))));
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.f_stop, 0.05, 0.0..=64.0, tl!("Aperture (f-stop, 0 for no blur)"))));
    let r = ui.checkbox(&mut q.orthographic, tl!("Orthographic"));
    discrete |= r.changed();
    if q.orthographic {
        ui.horizontal(|ui| drags.push(drag(ui, &mut q.ortho_height, 0.05, 0.001..=1_000_000.0, tl!("Orthographic height"))));
    }
    let q = q.sanitized();
    if q != *camera && discrete {
        app.run(Command::Stage(StageCommand::SetCamera { id, camera: q }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Camera { camera: q.clone() }, || Command::Stage(StageCommand::SetCamera { id, camera: q.clone() }), id);
    }
    ui.horizontal_wrapped(|ui| {
        let looking = app.stage.looking_through(app.active_view.min(3)) == Some(id);
        let label = if looking { tl!("Stop Looking Through") } else { tl!("Look Through") };
        if ui.button(label).clicked() {
            act(app, if looking { StageAction::StopLookingThrough } else { StageAction::LookThrough });
        }
        let used = app.doc.scene.render_camera() == Some(id);
        let r = ui.add_enabled(!used, egui::Button::new(tl!("Render Through This Camera")));
        if r.clicked() {
            app.run(Command::Stage(StageCommand::SetRenderCamera { id: Some(id) }));
        }
        if used {
            ui.label(RichText::new(tl!("Renders look through this camera.")).color(theme::TEXT_DIM).small());
        } else if app.doc.scene.render_camera().is_some() && ui.button(tl!("Render From the View Instead")).clicked() {
            app.run(Command::Stage(StageCommand::SetRenderCamera { id: None }));
        }
    });
    ui.label(
        RichText::new(tl!("The camera looks along its own Z axis, toward the negative end. The picture is as wide as the sensor."))
            .color(theme::TEXT_DIM)
            .small(),
    );
}
