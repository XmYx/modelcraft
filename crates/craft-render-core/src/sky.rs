//! A physical daytime sky: sunlight scattered once by air molecules (Rayleigh) and haze (Mie)
//! in a spherical atmosphere, integrated numerically along each view ray (Nishita et al.,
//! "Display of the Earth Taking into Account Atmospheric Scattering", 1993). The result is baked
//! into a latitude–longitude image, which the renderer then importance-samples like any other
//! environment, plus a sun light attenuated by the same atmosphere.

use std::f32::consts::PI;

use craft_texture::{Image, Kind};
use glam::{Vec3, Vec3A};

use crate::light::Light;

const EARTH_RADIUS: f32 = 6_360e3;
const ATMOSPHERE_RADIUS: f32 = 6_420e3;
/// Scale heights of air and haze, metres.
const RAYLEIGH_HEIGHT: f32 = 7_994.0;
const MIE_HEIGHT: f32 = 1_200.0;
/// Scattering coefficients at sea level, per metre, for red, green and blue light.
const RAYLEIGH: Vec3A = Vec3A::new(5.8e-6, 13.5e-6, 33.1e-6);
const MIE: f32 = 21e-6;
const MIE_ANISOTROPY: f32 = 0.76;
/// Angular radius of the sun's disk, radians.
pub const SUN_ANGULAR_RADIUS: f32 = 0.004_65;

/// A sky to bake.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    /// Direction towards the sun (it need not be normalized; below the horizon is night).
    pub sun_direction: Vec3,
    /// Haze: 1 is a clear day, larger values are mistier.
    pub haze: f32,
    /// Irradiance of the sun above the atmosphere, in scene units.
    pub intensity: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Sky { sun_direction: Vec3::new(0.4, 0.6, 0.5), haze: 1.0, intensity: 4.0 }
    }
}

impl Sky {
    /// A sky with the sun `elevation` radians above the horizon at compass angle `azimuth`.
    pub fn from_angles(elevation: f32, azimuth: f32) -> Sky {
        let (e, a) = (if elevation.is_finite() { elevation } else { 0.8 }, if azimuth.is_finite() { azimuth } else { 0.0 });
        Sky { sun_direction: Vec3::new(e.cos() * a.cos(), e.sin(), e.cos() * a.sin()), ..Sky::default() }
    }

    fn sun(&self) -> Vec3A {
        let d = Vec3A::from(self.sun_direction);
        if d.is_finite() { d.normalize_or(Vec3A::Y) } else { Vec3A::Y }
    }

    fn haze(&self) -> f32 {
        if self.haze.is_finite() { self.haze.clamp(0.0, 50.0) } else { 1.0 }
    }

    fn intensity(&self) -> f32 {
        if self.intensity.is_finite() { self.intensity.clamp(0.0, 1e6) } else { 0.0 }
    }

    /// Optical depths (air, haze) from `origin` along `dir` to the top of the atmosphere, or
    /// `None` when the ray meets the ground first.
    fn depth_to_space(origin: Vec3A, dir: Vec3A, steps: u32) -> Option<(f32, f32)> {
        if sphere_exit(origin, dir, EARTH_RADIUS).is_some_and(|t| t > 0.0) && origin.dot(dir) < 0.0 {
            return None;
        }
        let far = sphere_exit(origin, dir, ATMOSPHERE_RADIUS)?;
        let ds = far / steps as f32;
        let (mut air, mut haze) = (0.0, 0.0);
        for i in 0..steps {
            let h = (origin + dir * ((i as f32 + 0.5) * ds)).length() - EARTH_RADIUS;
            if h < 0.0 {
                return None;
            }
            air += (-h / RAYLEIGH_HEIGHT).exp() * ds;
            haze += (-h / MIE_HEIGHT).exp() * ds;
        }
        Some((air, haze))
    }

    fn extinction(&self, air: f32, haze: f32) -> Vec3A {
        let tau = RAYLEIGH * air + Vec3A::splat(MIE * self.haze() * 1.1 * haze);
        Vec3A::new((-tau.x).exp(), (-tau.y).exp(), (-tau.z).exp())
    }

    /// Radiance of the sky in direction `dir` (upper hemisphere), without the sun's disk.
    pub fn radiance(&self, dir: Vec3A) -> Vec3A {
        const VIEW_STEPS: u32 = 16;
        const LIGHT_STEPS: u32 = 8;
        let sun = self.sun();
        let origin = Vec3A::new(0.0, EARTH_RADIUS + 1.0, 0.0);
        let Some(far) = sphere_exit(origin, dir, ATMOSPHERE_RADIUS) else { return Vec3A::ZERO };
        let ds = far / VIEW_STEPS as f32;
        let mu = dir.dot(sun);
        let phase_air = 3.0 / (16.0 * PI) * (1.0 + mu * mu);
        let g = MIE_ANISOTROPY;
        let phase_haze = 3.0 / (8.0 * PI) * (1.0 - g * g) * (1.0 + mu * mu) / ((2.0 + g * g) * (1.0 + g * g - 2.0 * g * mu).max(1e-4).powf(1.5));
        let (mut air, mut haze) = (0.0, 0.0);
        let mut sum = Vec3A::ZERO;
        for i in 0..VIEW_STEPS {
            let p = origin + dir * ((i as f32 + 0.5) * ds);
            let h = (p.length() - EARTH_RADIUS).max(0.0);
            let (da, dh) = ((-h / RAYLEIGH_HEIGHT).exp() * ds, (-h / MIE_HEIGHT).exp() * ds);
            air += da;
            haze += dh;
            if let Some((la, lh)) = Sky::depth_to_space(p, sun, LIGHT_STEPS) {
                let through = self.extinction(air + la, haze + lh);
                sum += through * (RAYLEIGH * (da * phase_air) + Vec3A::splat(MIE * self.haze() * dh * phase_haze));
            }
        }
        let l = sum * self.intensity();
        if l.is_finite() { l.max(Vec3A::ZERO) } else { Vec3A::ZERO }
    }

    /// The sun as a light: its irradiance after the atmosphere, or `None` at night.
    pub fn sun_light(&self) -> Option<Light> {
        let sun = self.sun();
        let (air, haze) = Sky::depth_to_space(Vec3A::new(0.0, EARTH_RADIUS + 1.0, 0.0), sun, 32)?;
        let irradiance = self.extinction(air, haze) * self.intensity();
        (sun.y > 0.0 && irradiance.max_element() > 1e-6).then(|| Light::Sun {
            direction: sun.into(),
            irradiance: irradiance.into(),
            angular_radius: SUN_ANGULAR_RADIUS,
        })
    }

    /// The sky as a latitude–longitude image `width` pixels wide (clamped to 16..=2048). Below
    /// the horizon it shows a dim reflection of the sky above, standing in for the ground.
    pub fn bake(&self, width: u32) -> Image {
        let w = width.clamp(16, 2048);
        let h = w / 2;
        Image::generate(w, h, Kind::Color, |x, y| {
            let (theta, phi) = ((y as f32 + 0.5) / h as f32 * PI, ((x as f32 + 0.5) / w as f32 - 0.5) * 2.0 * PI);
            let dir = Vec3A::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            let c = if dir.y >= 0.0 { self.radiance(dir) } else { self.radiance(Vec3A::new(dir.x, -dir.y, dir.z)) * 0.15 };
            [c.x, c.y, c.z, 1.0]
        })
    }
}

/// Distance along a ray from inside a sphere (centred on the origin) to its surface.
fn sphere_exit(origin: Vec3A, dir: Vec3A, radius: f32) -> Option<f32> {
    let b = origin.dot(dir);
    let c = origin.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let t = -b + disc.sqrt();
    (t > 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_is_blue_at_noon_and_red_at_sunset() {
        let noon = Sky::from_angles(70f32.to_radians(), 0.0);
        let zenith = noon.radiance(Vec3A::Y);
        assert!(zenith.z > 1.5 * zenith.x && zenith.z > zenith.y, "zenith {zenith}");
        // Away from the sun the horizon is brighter and paler than the zenith.
        let horizon = noon.radiance(Vec3A::new(-0.999, 0.04, 0.0).normalize());
        assert!(horizon.y > zenith.y && horizon.x / horizon.z > zenith.x / zenith.z, "{horizon} vs {zenith}");
        let Some(Light::Sun { irradiance: day, .. }) = noon.sun_light() else { panic!("no sun at noon") };
        // Low sun: its light is dimmer and redder, and so is the sky around it.
        let dusk = Sky::from_angles(2f32.to_radians(), 0.0);
        let Some(Light::Sun { irradiance: evening, .. }) = dusk.sun_light() else { panic!("no sun at dusk") };
        assert!(evening.x < day.x && evening.x > 4.0 * evening.z && day.x < 2.0 * day.z, "day {day} evening {evening}");
        let glow = dusk.radiance(Vec3A::new(0.99, 0.08, 0.0).normalize());
        assert!(glow.x > glow.z, "glow {glow}");
        // Night: no sun light, and a dark sky.
        let night = Sky::from_angles(-30f32.to_radians(), 0.0);
        assert!(night.sun_light().is_none());
        assert!(night.radiance(Vec3A::Y).max_element() < 1e-3 * zenith.z);
    }

    #[test]
    fn baking_is_bounded_and_finite() {
        let img = Sky::default().bake(64);
        assert_eq!((img.width(), img.height()), (64, 32));
        assert!(img.data().iter().all(|v| v.is_finite() && *v >= 0.0));
        let hostile = Sky { sun_direction: Vec3::splat(f32::NAN), haze: f32::INFINITY, intensity: f32::NAN };
        assert!(hostile.bake(u32::MAX).data().iter().all(|v| v.is_finite()));
        assert!(hostile.sun_light().is_none());
    }
}
