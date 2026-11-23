//! The `stage` command: light and camera objects, and the camera renders look through
//! ([`modelcraft_scene::stage`]).
//!
//! `{"cmd":"stage","op":"add_light","kind":"spot"}`, `{"cmd":"stage","op":"set_camera","id":4,"camera":{"focal_length":85}}`

use glam::{Quat, Vec3};
use modelcraft_scene::stage::{CameraData, LightData, LightKind, RenderConfig};
use modelcraft_scene::{ObjectId, ObjectKind, Transform};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StageCommand {
    /// Add a light (`point`, `spot`, `area`, `sun`, `dome`), at `transform` or where that kind
    /// starts.
    AddLight {
        kind: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<ObjectId>,
        #[serde(default)]
        transform: Option<Transform>,
    },
    /// Change a light's settings.
    SetLight { id: ObjectId, light: LightData },
    /// Add a camera, at `transform` or in front of the origin looking at it.
    AddCamera {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<ObjectId>,
        #[serde(default)]
        transform: Option<Transform>,
    },
    /// Change a camera's lens, film, clipping and focus.
    SetCamera { id: ObjectId, camera: CameraData },
    /// Choose the camera renders look through (`null`: the active viewport's view).
    SetRenderCamera {
        #[serde(default)]
        id: Option<ObjectId>,
    },
    /// Change what renders of the project use: size, frames, format, passes, region, quality.
    SetRenderSettings { config: RenderConfig },
}

impl StageCommand {
    pub fn label(&self) -> String {
        match self {
            StageCommand::AddLight { kind, .. } => return format!("Add {kind} light"),
            StageCommand::SetLight { .. } => "Light settings",
            StageCommand::AddCamera { .. } => "Add camera",
            StageCommand::SetCamera { .. } => "Camera settings",
            StageCommand::SetRenderCamera { .. } => "Render camera",
            StageCommand::SetRenderSettings { .. } => "Render settings",
        }
        .into()
    }

    /// The objects the command changes, to check them for protection.
    pub(crate) fn targets(&self) -> Vec<ObjectId> {
        match self {
            StageCommand::SetLight { id, .. } | StageCommand::SetCamera { id, .. } => vec![*id],
            _ => vec![],
        }
    }
}

/// Where a new light or camera starts: lamps above the origin shining down, the sun slanted, the
/// camera back and up looking at the origin.
pub fn default_light_transform(kind: LightKind) -> Transform {
    let down = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    match kind {
        LightKind::Point => Transform { translation: Vec3::new(0.0, 3.0, 0.0), ..Transform::default() },
        LightKind::Spot | LightKind::Area => Transform { translation: Vec3::new(0.0, 3.0, 0.0), rotation: down, ..Transform::default() },
        LightKind::Sun => {
            Transform { translation: Vec3::new(0.0, 10.0, 0.0), rotation: Quat::from_rotation_y(0.6) * Quat::from_rotation_x(-1.0), ..Transform::default() }
        }
        LightKind::Dome => Transform::default(),
    }
}

pub fn default_camera_transform() -> Transform {
    let eye = Vec3::new(0.0, 1.5, 6.0);
    // Looking down -Z at the origin: turn about X by the angle the eye sits above it.
    let pitch = -(eye.y / eye.z).atan();
    Transform { translation: eye, rotation: Quat::from_rotation_x(pitch), ..Transform::default() }
}

fn scene_name(doc: &Document, name: &Option<String>, fallback: &str) -> String {
    let base = name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(fallback);
    doc.scene.unique_name(base)
}

pub fn apply(doc: &mut Document, cmd: &StageCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        StageCommand::AddLight { kind, name, parent, transform } => {
            let kind = LightKind::from_id(kind).ok_or_else(|| EngineError::Invalid(format!("no light kind {kind:?}: point, spot, area, sun or dome")))?;
            let name = scene_name(doc, name, kind.title());
            let id = doc.scene.add(name, ObjectKind::Light { light: LightData::of(kind) }, *parent)?;
            doc.scene.edit(id)?.transform = transform.unwrap_or_else(|| default_light_transform(kind)).sanitized();
            out.created.push(id);
        }
        StageCommand::SetLight { id, light } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Light { light: slot } => *slot = light.sanitized(),
            _ => return Err(EngineError::Invalid("that object is not a light".into())),
        },
        StageCommand::AddCamera { name, parent, transform } => {
            let name = scene_name(doc, name, "Camera");
            let id = doc.scene.add(name, ObjectKind::Camera { camera: CameraData::default() }, *parent)?;
            doc.scene.edit(id)?.transform = transform.unwrap_or_else(default_camera_transform).sanitized();
            out.created.push(id);
        }
        StageCommand::SetCamera { id, camera } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Camera { camera: slot } => *slot = camera.sanitized(),
            _ => return Err(EngineError::Invalid("that object is not a camera".into())),
        },
        StageCommand::SetRenderCamera { id } => {
            if let Some(id) = id
                && doc.scene.camera_data(*id).is_none()
            {
                return Err(EngineError::Invalid("renders can only look through a camera object".into()));
            }
            doc.scene.render.camera = *id;
        }
        StageCommand::SetRenderSettings { config } => {
            // The camera is chosen by its own command, so a settings change cannot name a missing one.
            let mut config = config.sanitized();
            config.camera = config.camera.filter(|c| doc.scene.camera_data(*c).is_some());
            doc.scene.render = config;
        }
    }
    Ok(())
}

/// Refuse an edit of a locked light or camera.
pub fn check_unlocked(doc: &Document, cmd: &StageCommand) -> Result<(), EngineError> {
    for id in cmd.targets() {
        if doc.scene.is_locked(id) {
            let name = doc.scene.get(id).map_or("an object", |o| o.name.as_str());
            return Err(EngineError::Invalid(format!("{name} is locked; unlock it first")));
        }
    }
    Ok(())
}

pub const COMMANDS: &[(&str, &str)] = &[
    ("add_light", "Add a light: kind (point, spot, area, sun, dome), name?, parent?, transform?; it shines along its own -Z"),
    ("set_light", "Change a light: id, light {kind, color [r,g,b], intensity, size, height, spread, softness, image?}"),
    ("add_camera", "Add a camera looking along its own -Z: name?, parent?, transform?"),
    (
        "set_camera",
        "Change a camera: id, camera {focal_length (mm), sensor_width, sensor_height, near, far, focus_distance, f_stop (0: no blur), orthographic, ortho_height}",
    ),
    ("set_render_camera", "Choose the camera renders look through: id? (none: the viewport's view)"),
    (
        "set_render_settings",
        "Change the project's render settings: config {camera?, width, height, samples, bounces, frame_start, frame_end, frame_step, format (png, exr), passes [albedo, normal, depth, motion, variance], region? [left, top, right, bottom], denoise, exposure, output}",
    ),
];
