//! Orbit camera. Y is up. Angles in radians.

use glam::{Mat4, Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}

/// Fixed views for the four-view layout and the numeric keypad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPreset {
    Perspective,
    Front,
    Right,
    Top,
}

impl ViewPreset {
    pub fn label(self) -> &'static str {
        match self {
            ViewPreset::Perspective => "Perspective",
            ViewPreset::Front => "Front",
            ViewPreset::Right => "Right",
            ViewPreset::Top => "Top",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view (perspective).
    pub fov_y: f32,
    pub projection: Projection,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { target: Vec3::ZERO, yaw: 0.6, pitch: 0.45, distance: 8.0, fov_y: 40f32.to_radians(), projection: Projection::Perspective }
    }
}

impl Camera {
    pub fn preset(p: ViewPreset) -> Camera {
        let mut c = Camera::default();
        match p {
            ViewPreset::Perspective => {}
            ViewPreset::Front => {
                c.yaw = 0.0;
                c.pitch = 0.0;
                c.projection = Projection::Orthographic;
            }
            ViewPreset::Right => {
                c.yaw = std::f32::consts::FRAC_PI_2;
                c.pitch = 0.0;
                c.projection = Projection::Orthographic;
            }
            ViewPreset::Top => {
                c.yaw = 0.0;
                c.pitch = std::f32::consts::FRAC_PI_2 - 1e-3;
                c.projection = Projection::Orthographic;
            }
        }
        c
    }

    /// Direction from the target to the eye.
    pub fn back(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.sin(), self.pitch.sin(), self.pitch.cos() * self.yaw.cos())
    }
    pub fn eye(&self) -> Vec3 {
        self.target + self.back() * self.distance
    }
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }
    fn near_far(&self) -> (f32, f32) {
        let d = self.distance.max(1e-3);
        (d * 0.01, d * 100.0 + 100.0)
    }
    pub fn proj(&self, aspect: f32) -> Mat4 {
        let aspect = if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 };
        let (n, f) = self.near_far();
        match self.projection {
            Projection::Perspective => Mat4::perspective_rh(self.fov_y, aspect, n, f),
            Projection::Orthographic => {
                let h = self.distance * (self.fov_y * 0.5).tan();
                Mat4::orthographic_rh(-h * aspect, h * aspect, -h, h, -f, f)
            }
        }
    }
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx;
        self.pitch = (self.pitch + dy).clamp(-1.55, 1.55);
        if self.projection == Projection::Orthographic {
            self.projection = Projection::Perspective;
        }
    }
    /// Pan by a fraction of the view height.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let fwd = -self.back();
        let right = fwd.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(fwd);
        let scale = self.distance * (self.fov_y * 0.5).tan() * 2.0;
        self.target += (-right * dx + up * dy) * scale;
    }
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            self.distance = (self.distance * factor).clamp(0.01, 1e5);
        }
    }
    /// Frame a bounding box.
    pub fn frame(&mut self, lo: Vec3, hi: Vec3) {
        if !(lo.is_finite() && hi.is_finite()) || hi.cmplt(lo).any() {
            return;
        }
        self.target = (lo + hi) * 0.5;
        let r = ((hi - lo).length() * 0.5).max(0.05);
        self.distance = r / (self.fov_y * 0.5).sin() * 1.1;
    }

    /// World-space ray through a point in normalised device coordinates (-1..1, y up).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let near = inv.project_point3(ndc.extend(0.0));
        let far = inv.project_point3(ndc.extend(1.0));
        let origin = if self.projection == Projection::Perspective { self.eye() } else { near - (far - near).normalize_or_zero() * self.distance * 100.0 };
        (origin, (far - near).normalize_or(-self.back()))
    }

    /// World point to normalised device coordinates; `None` behind the camera.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<Vec2> {
        let c = self.view_proj(aspect) * p.extend(1.0);
        (c.w > 1e-6).then(|| Vec2::new(c.x / c.w, c.y / c.w))
    }

    /// World units per pixel at a point (for constant-size gizmos).
    pub fn world_per_pixel(&self, p: Vec3, viewport_height: f32) -> f32 {
        let h = viewport_height.max(1.0);
        match self.projection {
            Projection::Perspective => (p - self.eye()).length() * (self.fov_y * 0.5).tan() * 2.0 / h,
            Projection::Orthographic => self.distance * (self.fov_y * 0.5).tan() * 2.0 / h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_go_through_projected_points() {
        for p in [ViewPreset::Perspective, ViewPreset::Front, ViewPreset::Top] {
            let c = Camera::preset(p);
            let target = Vec3::new(0.3, 0.2, -0.1);
            let ndc = c.project(target, 1.5).unwrap();
            let (o, d) = c.ray(ndc, 1.5);
            let closest = o + d * (target - o).dot(d);
            assert!(closest.distance(target) < 1e-2, "{p:?}: {closest} vs {target}");
        }
    }

    #[test]
    fn navigation_is_bounded() {
        let mut c = Camera::default();
        c.orbit(0.0, 100.0);
        assert!(c.pitch <= 1.55);
        c.zoom(f32::NAN);
        c.zoom(1e-9);
        assert!(c.distance >= 0.01);
        c.frame(Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!(c.distance > 1.0 && c.target == Vec3::ZERO);
        c.pan(0.1, 0.0);
        assert!(c.target != Vec3::ZERO);
    }
}
