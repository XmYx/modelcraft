//! Cameras: perspective (pinhole or thin lens) and orthographic.

use craft_render_bvh::Ray;
use glam::{Vec2, Vec3, Vec3A};

use crate::math::concentric_disk;

/// A camera looking from `eye` at `target`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    /// Vertical field of view in radians (perspective).
    pub fov_y: f32,
    /// `Some(height)` makes the camera orthographic, seeing `height` world units vertically.
    pub ortho_height: Option<f32>,
    /// Lens radius in world units; zero is a pinhole (everything in focus).
    pub aperture_radius: f32,
    /// Distance from the eye to the plane in perfect focus.
    pub focus_distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            eye: Vec3::new(0.0, 1.0, 5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y: 40f32.to_radians(),
            ortho_height: None,
            aperture_radius: 0.0,
            focus_distance: 5.0,
        }
    }
}

/// A camera resolved for one image aspect ratio.
#[derive(Clone, Copy, Debug)]
pub struct PreparedCamera {
    origin: Vec3A,
    right: Vec3A,
    up: Vec3A,
    forward: Vec3A,
    half_w: f32,
    half_h: f32,
    ortho: bool,
    aperture: f32,
    focus: f32,
}

/// A [`PreparedCamera`] as plain numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraData {
    pub origin: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    /// Half the image plane's size at distance 1 (perspective) or in world units (orthographic).
    pub half_width: f32,
    pub half_height: f32,
    pub orthographic: bool,
    pub aperture_radius: f32,
    pub focus_distance: f32,
}

impl Camera {
    /// Resolve for `aspect` (width / height). Degenerate values are repaired, never rejected.
    pub fn prepare(&self, aspect: f32) -> PreparedCamera {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let eye = if self.eye.is_finite() { self.eye } else { Vec3::new(0.0, 0.0, 5.0) };
        let target = if self.target.is_finite() { self.target } else { Vec3::ZERO };
        let forward = (target - eye).try_normalize().unwrap_or(Vec3::NEG_Z);
        let up_hint = if self.up.is_finite() { self.up } else { Vec3::Y };
        let right = forward.cross(up_hint).try_normalize().unwrap_or_else(|| forward.any_orthonormal_vector());
        let up = right.cross(forward);
        let fov = if self.fov_y.is_finite() { self.fov_y.clamp(1e-3, 3.1) } else { 0.7 };
        let (half_h, ortho) = match self.ortho_height {
            Some(h) if h.is_finite() && h > 0.0 => (h * 0.5, true),
            _ => ((fov * 0.5).tan(), false),
        };
        let aperture = if self.aperture_radius.is_finite() { self.aperture_radius.max(0.0) } else { 0.0 };
        let focus = if self.focus_distance.is_finite() && self.focus_distance > 1e-6 { self.focus_distance } else { (target - eye).length().max(1e-3) };
        PreparedCamera {
            origin: eye.into(),
            right: right.into(),
            up: up.into(),
            forward: forward.into(),
            half_w: half_h * aspect,
            half_h,
            ortho,
            aperture,
            focus,
        }
    }
}

impl PreparedCamera {
    /// The ray through `ndc` (x and y in -1..1, y up). `lens` picks the point on the aperture.
    #[inline]
    pub fn ray(&self, ndc: Vec2, lens: Vec2) -> Ray {
        let (sx, sy) = (ndc.x * self.half_w, ndc.y * self.half_h);
        if self.ortho {
            let origin = self.origin + self.right * sx + self.up * sy;
            return Ray { origin, dir: self.forward, tmin: 0.0, tmax: f32::INFINITY };
        }
        let dir = self.forward + self.right * sx + self.up * sy;
        if self.aperture <= 0.0 {
            return Ray { origin: self.origin, dir: dir.normalize(), tmin: 0.0, tmax: f32::INFINITY };
        }
        // Thin lens: every ray through the lens meets the pinhole ray on the focus plane.
        let focus_point = self.origin + dir * self.focus;
        let l = concentric_disk(lens) * self.aperture;
        let origin = self.origin + self.right * l.x + self.up * l.y;
        Ray { origin, dir: (focus_point - origin).normalize(), tmin: 0.0, tmax: f32::INFINITY }
    }

    pub fn forward(&self) -> Vec3A {
        self.forward
    }

    /// The resolved camera as plain numbers (what a GPU implementation uploads).
    pub fn data(&self) -> CameraData {
        CameraData {
            origin: self.origin.to_array(),
            right: self.right.to_array(),
            up: self.up.to_array(),
            forward: self.forward.to_array(),
            half_width: self.half_w,
            half_height: self.half_h,
            orthographic: self.ortho,
            aperture_radius: self.aperture,
            focus_distance: self.focus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinhole_and_orthographic_rays() {
        let cam = Camera { eye: Vec3::new(0.0, 0.0, 4.0), target: Vec3::ZERO, fov_y: std::f32::consts::FRAC_PI_2, ..Camera::default() };
        let p = cam.prepare(2.0);
        let c = p.ray(Vec2::ZERO, Vec2::splat(0.5));
        assert!((c.dir - Vec3A::NEG_Z).length() < 1e-6);
        // 90° vertical field of view: the top edge is 45° up; the right edge is twice as far out.
        let top = p.ray(Vec2::new(0.0, 1.0), Vec2::ZERO);
        assert!((top.dir.y - top.dir.z.abs()).abs() < 1e-5);
        let right = p.ray(Vec2::new(1.0, 0.0), Vec2::ZERO);
        assert!((right.dir.x - 2.0 * right.dir.z.abs()).abs() < 1e-5);
        let o = Camera { ortho_height: Some(2.0), ..cam }.prepare(1.0);
        let r = o.ray(Vec2::new(1.0, -1.0), Vec2::ZERO);
        assert!((r.origin - Vec3A::new(1.0, -1.0, 4.0)).length() < 1e-5 && r.dir == Vec3A::NEG_Z);
    }

    #[test]
    fn thin_lens_rays_meet_on_the_focus_plane() {
        let cam = Camera { eye: Vec3::new(0.0, 0.0, 4.0), target: Vec3::ZERO, aperture_radius: 0.3, focus_distance: 4.0, ..Camera::default() };
        let p = cam.prepare(1.0);
        let ndc = Vec2::new(0.4, -0.3);
        let pinhole = Camera { aperture_radius: 0.0, ..cam }.prepare(1.0).ray(ndc, Vec2::ZERO);
        let on_plane = pinhole.at(4.0 / pinhole.dir.dot(Vec3A::NEG_Z));
        for lens in [Vec2::new(0.1, 0.9), Vec2::new(0.8, 0.2), Vec2::new(0.5, 0.5)] {
            let r = p.ray(ndc, lens);
            let t = (on_plane - r.origin).dot(Vec3A::NEG_Z) / r.dir.dot(Vec3A::NEG_Z);
            assert!((r.at(t) - on_plane).length() < 1e-4);
        }
    }

    #[test]
    fn hostile_cameras_are_repaired() {
        let cam = Camera {
            eye: Vec3::splat(f32::NAN),
            target: Vec3::ZERO,
            up: Vec3::ZERO,
            fov_y: f32::INFINITY,
            ortho_height: Some(-1.0),
            aperture_radius: f32::NAN,
            focus_distance: 0.0,
        };
        let r = cam.prepare(f32::NAN).ray(Vec2::new(0.3, 0.3), Vec2::splat(0.2));
        assert!(r.origin.is_finite() && r.dir.is_finite() && (r.dir.length() - 1.0).abs() < 1e-4);
        let same = Camera { eye: Vec3::ONE, target: Vec3::ONE, ..Camera::default() }.prepare(1.0).ray(Vec2::ZERO, Vec2::ZERO);
        assert!(same.dir.is_finite());
    }
}
