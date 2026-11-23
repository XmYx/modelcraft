//! Light and camera objects, and the render setup the project keeps.
//!
//! A light or camera is a transform with a few numbers. It shines or looks along the object's
//! own **-Z** axis with +Y up, as a camera does in glTF, so a light and a camera placed the same
//! way face the same way. Every number is reachable by [`crate::attr`] (`light.intensity`,
//! `camera.focal_length`, …), so each can be keyed.
//!
//! Units follow the path tracer's: positions are metres; a point or spot light has a radiant
//! intensity (W/sr), a sun an irradiance on a surface that faces it, and an area light or a dome
//! a radiance.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::{ObjectId, ObjectKind, Scene};

/// Largest value an intensity may take; larger numbers are clamped.
pub const MAX_INTENSITY: f32 = 1.0e7;
/// Largest light size (metres, or degrees for a sun) accepted.
pub const MAX_SIZE: f32 = 1000.0;

/// What a light looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightKind {
    /// Shines equally in every direction from a point.
    #[default]
    Point,
    /// A point light inside a cone, along -Z.
    Spot,
    /// A glowing rectangle that faces -Z; shadows soften with its size.
    Area,
    /// Parallel light from far away, travelling along -Z; `size` is its apparent width in degrees.
    Sun,
    /// The whole sky: a flat colour or a panorama picture, turned by the object's rotation about
    /// the vertical axis.
    Dome,
}

impl LightKind {
    pub const ALL: [LightKind; 5] = [LightKind::Point, LightKind::Spot, LightKind::Area, LightKind::Sun, LightKind::Dome];

    /// The name in commands and files.
    pub fn id(self) -> &'static str {
        match self {
            LightKind::Point => "point",
            LightKind::Spot => "spot",
            LightKind::Area => "area",
            LightKind::Sun => "sun",
            LightKind::Dome => "dome",
        }
    }

    pub fn from_id(id: &str) -> Option<LightKind> {
        LightKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// The English name.
    pub fn title(self) -> &'static str {
        match self {
            LightKind::Point => "Point light",
            LightKind::Spot => "Spot light",
            LightKind::Area => "Area light",
            LightKind::Sun => "Sun light",
            LightKind::Dome => "Dome light",
        }
    }
}

/// The settings of a light object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightData {
    pub kind: LightKind,
    /// Linear colour, 0..1 per channel.
    pub color: [f32; 3],
    /// Point, spot: radiant intensity (W/sr). Sun: irradiance on a surface facing it. Area, dome:
    /// radiance.
    pub intensity: f32,
    /// Area: width in metres. Sun: apparent width in degrees. Unused by point, spot and dome.
    pub size: f32,
    /// Area: height in metres.
    pub height: f32,
    /// Spot: the full angle of the cone in degrees.
    pub spread: f32,
    /// Spot: how much of the cone's edge fades out, 0 (hard) to 1.
    pub softness: f32,
    /// Dome: a Radiance HDR panorama to light the scene with, instead of the flat colour.
    pub image: Option<String>,
}

impl Default for LightData {
    fn default() -> Self {
        LightData::of(LightKind::Point)
    }
}

impl LightData {
    /// A light of `kind` with useful starting values.
    pub fn of(kind: LightKind) -> LightData {
        let (intensity, size, height) = match kind {
            LightKind::Point => (100.0, 0.0, 0.0),
            LightKind::Spot => (300.0, 0.0, 0.0),
            LightKind::Area => (8.0, 1.0, 1.0),
            LightKind::Sun => (3.0, 0.5, 0.0),
            LightKind::Dome => (1.0, 0.0, 0.0),
        };
        LightData { kind, color: [1.0; 3], intensity, size, height, spread: 45.0, softness: 0.2, image: None }
    }

    /// The settings with every number put in its range; NaN and infinity become the defaults.
    pub fn sanitized(&self) -> LightData {
        let d = LightData::of(self.kind);
        let num = |v: f32, lo: f32, hi: f32, fallback: f32| if v.is_finite() { v.clamp(lo, hi) } else { fallback };
        LightData {
            kind: self.kind,
            color: self.color.map(|c| num(c, 0.0, 1.0, 1.0)),
            intensity: num(self.intensity, 0.0, MAX_INTENSITY, d.intensity),
            size: num(self.size, 0.0, MAX_SIZE, d.size),
            height: num(self.height, 0.0, MAX_SIZE, d.height),
            spread: num(self.spread, 1.0, 179.0, d.spread),
            softness: num(self.softness, 0.0, 1.0, d.softness),
            image: self.image.clone().filter(|p| p.len() <= 4096),
        }
    }

    /// `color × intensity`, the strength the renderer is given.
    pub fn power(&self) -> Vec3 {
        let s = self.sanitized();
        Vec3::from_array(s.color) * s.intensity
    }
}

/// The settings of a camera object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraData {
    /// Millimetres.
    pub focal_length: f32,
    /// Width of the film (millimetres); the lens's view across it is `2·atan(width / 2f)`.
    pub sensor_width: f32,
    /// Height of the film (millimetres); sets the frame shape shown while looking through.
    pub sensor_height: f32,
    /// Nearest and farthest distance drawn in the viewport (metres).
    pub near: f32,
    pub far: f32,
    /// Distance to the plane that is sharp (metres).
    pub focus_distance: f32,
    /// The lens opening as an f-number; 0 is a pinhole with no blur.
    pub f_stop: f32,
    /// A flat (parallel) view that sees `ortho_height` metres vertically.
    pub orthographic: bool,
    pub ortho_height: f32,
}

impl Default for CameraData {
    fn default() -> Self {
        CameraData {
            focal_length: 50.0,
            sensor_width: 36.0,
            sensor_height: 24.0,
            near: 0.1,
            far: 1000.0,
            focus_distance: 5.0,
            f_stop: 0.0,
            orthographic: false,
            ortho_height: 5.0,
        }
    }
}

impl CameraData {
    /// The settings with every number put in its range.
    pub fn sanitized(&self) -> CameraData {
        let d = CameraData::default();
        let num = |v: f32, lo: f32, hi: f32, fallback: f32| if v.is_finite() { v.clamp(lo, hi) } else { fallback };
        let near = num(self.near, 1e-4, 1e5, d.near);
        CameraData {
            focal_length: num(self.focal_length, 4.0, 2000.0, d.focal_length),
            sensor_width: num(self.sensor_width, 1.0, 200.0, d.sensor_width),
            sensor_height: num(self.sensor_height, 1.0, 200.0, d.sensor_height),
            near,
            far: num(self.far, near * 1.001, 1e7, d.far.max(near * 2.0)),
            focus_distance: num(self.focus_distance, 1e-3, 1e6, d.focus_distance),
            f_stop: if self.f_stop.is_finite() && self.f_stop > 0.0 { self.f_stop.clamp(0.5, 64.0) } else { 0.0 },
            orthographic: self.orthographic,
            ortho_height: num(self.ortho_height, 1e-3, 1e6, d.ortho_height),
        }
    }

    /// Width over height of the film.
    pub fn film_aspect(&self) -> f32 {
        let s = self.sanitized();
        s.sensor_width / s.sensor_height
    }

    /// The vertical field of view (radians) in a picture of `aspect` (width / height). The film's
    /// width is fitted across the picture, so a wider picture shows the same width and less height.
    pub fn fov_y(&self, aspect: f32) -> f32 {
        let s = self.sanitized();
        let aspect = if aspect.is_finite() && aspect > 1e-3 { aspect } else { 1.0 };
        let half_width = (s.sensor_width * 0.5 / s.focal_length).atan();
        2.0 * (half_width.tan() / aspect).atan()
    }

    /// The lens radius in metres: `f / (2·N)`, zero for a pinhole.
    pub fn aperture_radius(&self) -> f32 {
        let s = self.sanitized();
        if s.f_stop > 0.0 { s.focal_length * 1e-3 / (2.0 * s.f_stop) } else { 0.0 }
    }
}

/// An image a render can write besides the picture itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pass {
    /// The lit picture (always written).
    Beauty,
    /// The surface's base colour, unlit.
    Albedo,
    /// The shading normal in world space.
    Normal,
    /// Distance from the camera.
    Depth,
    /// How far each pixel moved since the last frame, in pixels.
    Motion,
    /// How noisy each pixel still is.
    Variance,
}

impl Pass {
    pub const ALL: [Pass; 6] = [Pass::Beauty, Pass::Albedo, Pass::Normal, Pass::Depth, Pass::Motion, Pass::Variance];

    pub fn id(self) -> &'static str {
        match self {
            Pass::Beauty => "beauty",
            Pass::Albedo => "albedo",
            Pass::Normal => "normal",
            Pass::Depth => "depth",
            Pass::Motion => "motion",
            Pass::Variance => "variance",
        }
    }

    pub fn from_id(id: &str) -> Option<Pass> {
        Pass::ALL.into_iter().find(|p| p.id() == id)
    }

    pub fn title(self) -> &'static str {
        match self {
            Pass::Beauty => "Picture",
            Pass::Albedo => "Colour",
            Pass::Normal => "Normal",
            Pass::Depth => "Depth",
            Pass::Motion => "Motion",
            Pass::Variance => "Noise",
        }
    }
}

/// The file type a render is saved as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// 8-bit sRGB, the picture only.
    #[default]
    Png,
    /// 32-bit linear radiance, with the extra passes as layers.
    Exr,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Png => "png",
            OutputFormat::Exr => "exr",
        }
    }
}

/// Largest picture side a project may ask for.
pub const MAX_SIDE: u32 = 8192;
/// Most frames one render queue holds.
pub const MAX_FRAMES: u32 = 100_000;

/// What a render of the project uses, saved with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
    /// The camera object renders look through; `None` renders the active viewport's view.
    pub camera: Option<ObjectId>,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// How many times light may bounce.
    pub bounces: u32,
    /// The frames a queue renders: first, last (both included) and the step between them.
    pub frame_start: i32,
    pub frame_end: i32,
    pub frame_step: u32,
    pub format: OutputFormat,
    /// What an EXR holds besides the picture.
    pub passes: Vec<Pass>,
    /// The part of the picture to keep, as fractions of its size: left, top, right, bottom.
    pub region: Option<[f32; 4]>,
    /// Filter the noise out of finished pictures.
    pub denoise: bool,
    /// Brightness of 8-bit pictures, in stops.
    pub exposure: f32,
    /// Where a queue writes; frame numbers go before the extension (or in place of `#` signs).
    pub output: String,
}

impl Default for RenderConfig {
    fn default() -> Self {
        RenderConfig {
            camera: None,
            width: 960,
            height: 540,
            samples: 256,
            bounces: 12,
            frame_start: 0,
            frame_end: 0,
            frame_step: 1,
            format: OutputFormat::Png,
            passes: Vec::new(),
            region: None,
            denoise: false,
            exposure: 0.0,
            output: String::new(),
        }
    }
}

impl RenderConfig {
    pub fn is_default(&self) -> bool {
        *self == RenderConfig::default()
    }

    /// The settings with every number put in its range.
    pub fn sanitized(&self) -> RenderConfig {
        let d = RenderConfig::default();
        let mut passes: Vec<Pass> = Vec::new();
        for p in &self.passes {
            if *p != Pass::Beauty && !passes.contains(p) {
                passes.push(*p);
            }
        }
        let region = self.region.and_then(|r| {
            let r = r.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
            (r[2] > r[0] && r[3] > r[1]).then_some(r)
        });
        let (a, b) = (self.frame_start.clamp(-1_000_000, 1_000_000), self.frame_end.clamp(-1_000_000, 1_000_000));
        RenderConfig {
            camera: self.camera,
            width: self.width.clamp(16, MAX_SIDE),
            height: self.height.clamp(16, MAX_SIDE),
            samples: self.samples.clamp(1, 1 << 20),
            bounces: self.bounces.clamp(1, 64),
            frame_start: a,
            frame_end: b.max(a),
            frame_step: self.frame_step.clamp(1, 10_000),
            format: self.format,
            passes,
            region,
            denoise: self.denoise,
            exposure: if self.exposure.is_finite() { self.exposure.clamp(-16.0, 16.0) } else { d.exposure },
            output: self.output.chars().take(4096).collect(),
        }
    }

    /// The frames a queue renders, in order, at most [`MAX_FRAMES`].
    pub fn frames(&self) -> Vec<i32> {
        let c = self.sanitized();
        (c.frame_start..=c.frame_end).step_by(c.frame_step as usize).take(MAX_FRAMES as usize).collect()
    }

    /// The size of the picture after the region is cut out.
    pub fn output_size(&self) -> [u32; 2] {
        let c = self.sanitized();
        match c.region {
            Some([l, t, r, b]) => [(((r - l) * c.width as f32).round() as u32).max(1), (((b - t) * c.height as f32).round() as u32).max(1)],
            None => [c.width, c.height],
        }
    }
}

/// A camera's place in the world: where it is, which way it looks and which way is up. Scale and
/// shear are ignored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub eye: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

impl Placement {
    /// Read the placement off a world matrix; a degenerate matrix looks down -Z from its origin.
    pub fn of(world: Mat4) -> Placement {
        let eye = world.transform_point3(Vec3::ZERO);
        let forward = world.transform_vector3(Vec3::NEG_Z).try_normalize().unwrap_or(Vec3::NEG_Z);
        let up = world.transform_vector3(Vec3::Y).try_normalize().unwrap_or(Vec3::Y);
        let up = (up - forward * up.dot(forward)).try_normalize().unwrap_or_else(|| forward.any_orthonormal_vector());
        Placement { eye: if eye.is_finite() { eye } else { Vec3::ZERO }, forward, up }
    }
}

impl Scene {
    /// The visible light objects in tree order.
    pub fn lights(&self) -> Vec<ObjectId> {
        self.tree_order()
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| self.renders(*id) && matches!(self.get(*id).map(|o| &o.kind), Some(ObjectKind::Light { .. })))
            .collect()
    }

    /// Every camera object in tree order.
    pub fn cameras(&self) -> Vec<ObjectId> {
        self.tree_order().into_iter().map(|(id, _)| id).filter(|id| matches!(self.get(*id).map(|o| &o.kind), Some(ObjectKind::Camera { .. }))).collect()
    }

    /// The settings of camera `id`, if it is a camera.
    pub fn camera_data(&self, id: ObjectId) -> Option<&CameraData> {
        match self.get(id).map(|o| &o.kind) {
            Some(ObjectKind::Camera { camera }) => Some(camera),
            _ => None,
        }
    }

    /// The camera renders look through: the chosen one if it still is a camera, else none.
    pub fn render_camera(&self) -> Option<ObjectId> {
        self.render.camera.filter(|id| self.camera_data(*id).is_some())
    }

    /// Where camera `id` is and what it settings it has, at the current frame.
    pub fn camera_view(&self, id: ObjectId) -> Option<(Placement, CameraData)> {
        let data = self.camera_data(id)?.sanitized();
        Some((Placement::of(self.world_matrix(id)), data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_values_are_clamped() {
        let l = LightData {
            kind: LightKind::Spot,
            color: [f32::NAN, 5.0, -1.0],
            intensity: f32::INFINITY,
            size: -3.0,
            height: 1e30,
            spread: 0.0,
            softness: 9.0,
            image: None,
        };
        let s = l.sanitized();
        assert_eq!(s.color, [1.0, 1.0, 0.0]);
        assert_eq!((s.intensity, s.size, s.height, s.spread, s.softness), (300.0, 0.0, MAX_SIZE, 1.0, 1.0));
        let c = CameraData {
            focal_length: f32::NAN,
            sensor_width: -1.0,
            near: 0.0,
            far: -5.0,
            focus_distance: 0.0,
            f_stop: f32::NAN,
            ortho_height: 0.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!((c.focal_length, c.sensor_width, c.f_stop), (50.0, 1.0, 0.0));
        assert!(c.near > 0.0 && c.far > c.near && c.focus_distance > 0.0 && c.ortho_height > 0.0);
        assert!(CameraData { focal_length: 1e9, ..Default::default() }.fov_y(1.0).is_finite());
    }

    #[test]
    fn the_lens_sets_the_field_of_view_and_the_blur() {
        let c = CameraData::default();
        // 36 mm across at 50 mm: 2·atan(0.36) = 39.6° wide; in a 3:2 picture 2·atan(0.24) tall.
        assert!((c.fov_y(1.5) - 2.0 * (0.24f32).atan()).abs() < 1e-6);
        assert!((c.film_aspect() - 1.5).abs() < 1e-6);
        let longer = CameraData { focal_length: 100.0, ..c.clone() };
        assert!(longer.fov_y(1.5) < c.fov_y(1.5));
        assert_eq!(c.aperture_radius(), 0.0);
        let open = CameraData { f_stop: 2.0, ..c };
        assert!((open.aperture_radius() - 0.0125).abs() < 1e-7, "{}", open.aperture_radius());
    }

    #[test]
    fn render_settings_are_clamped_and_list_their_frames() {
        let c = RenderConfig {
            width: 5,
            height: 99_999,
            samples: 0,
            bounces: 1000,
            frame_start: 5,
            frame_end: 2,
            frame_step: 0,
            exposure: f32::NAN,
            region: Some([0.5, 0.5, 0.4, 0.9]),
            ..Default::default()
        }
        .sanitized();
        assert_eq!((c.width, c.height, c.samples, c.bounces, c.frame_step, c.exposure), (16, MAX_SIDE, 1, 64, 1, 0.0));
        assert_eq!((c.frame_start, c.frame_end, c.region), (5, 5, None), "an empty range is one frame and an upside-down region is none");
        let q = RenderConfig { frame_start: 1, frame_end: 10, frame_step: 3, ..Default::default() };
        assert_eq!(q.frames(), vec![1, 4, 7, 10]);
        let huge = RenderConfig { frame_start: -1_000_000, frame_end: 1_000_000, ..Default::default() };
        assert_eq!(huge.frames().len(), MAX_FRAMES as usize, "a queue is bounded");
        let r = RenderConfig { width: 200, height: 100, region: Some([0.25, 0.0, 0.75, 0.5]), ..Default::default() };
        assert_eq!(r.output_size(), [100, 50]);
        let p = RenderConfig { passes: vec![Pass::Beauty, Pass::Depth, Pass::Depth, Pass::Normal], ..Default::default() }.sanitized();
        assert_eq!(p.passes, vec![Pass::Depth, Pass::Normal]);
        assert!(RenderConfig::default().is_default() && !q.is_default());
    }

    #[test]
    fn a_placement_reads_the_world_matrix() {
        let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)) * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2) * Mat4::from_scale(Vec3::splat(4.0));
        let p = Placement::of(m);
        assert!((p.eye - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5);
        // Turned a quarter about Y, -Z becomes -X.
        assert!((p.forward - Vec3::NEG_X).length() < 1e-5, "{:?}", p.forward);
        assert!((p.up - Vec3::Y).length() < 1e-5 && p.up.dot(p.forward).abs() < 1e-5);
        let bad = Placement::of(Mat4::ZERO);
        assert_eq!((bad.forward, bad.eye), (Vec3::NEG_Z, Vec3::ZERO));
    }
}
