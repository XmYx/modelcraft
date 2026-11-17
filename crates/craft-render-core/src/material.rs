//! Surface description. The parameter names follow the OpenPBR Surface specification
//! (Academy Software Foundation, v1.1); this slice evaluates its base, specular and metal lobes.

use glam::Vec3;

/// A surface material.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Diffuse reflectance of the dielectric base and reflectance at normal incidence of the metal.
    pub base_color: Vec3,
    pub base_weight: f32,
    /// 0 = dielectric, 1 = metal.
    pub metalness: f32,
    pub specular_weight: f32,
    pub specular_color: Vec3,
    /// Perceptual roughness of the specular lobe; the microfacet width is its square.
    pub specular_roughness: f32,
    /// Index of refraction of the dielectric base.
    pub specular_ior: f32,
    pub emission_color: Vec3,
    /// Emitted luminance scale (radiance = colour × luminance, in scene units).
    pub emission_luminance: f32,
    /// Emit from both faces instead of the front (counter-clockwise) face only.
    pub emission_two_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            base_color: Vec3::splat(0.8),
            base_weight: 1.0,
            metalness: 0.0,
            specular_weight: 1.0,
            specular_color: Vec3::ONE,
            specular_roughness: 0.3,
            specular_ior: 1.5,
            emission_color: Vec3::ONE,
            emission_luminance: 0.0,
            emission_two_sided: false,
        }
    }
}

fn unit(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { fallback }
}

fn color(c: Vec3) -> Vec3 {
    if c.is_finite() { c.clamp(Vec3::ZERO, Vec3::ONE) } else { Vec3::ZERO }
}

impl Material {
    /// A matte surface.
    pub fn diffuse(color: Vec3) -> Material {
        Material { base_color: color, specular_weight: 0.0, ..Material::default() }
    }
    /// A metal.
    pub fn metal(color: Vec3, roughness: f32) -> Material {
        Material { base_color: color, metalness: 1.0, specular_roughness: roughness, ..Material::default() }
    }
    /// A black surface that emits `radiance` from its front face.
    pub fn emitter(radiance: Vec3) -> Material {
        let scale = radiance.max_element().max(0.0);
        let color = if scale > 0.0 { radiance / scale } else { Vec3::ZERO };
        Material { base_color: Vec3::ZERO, specular_weight: 0.0, emission_color: color, emission_luminance: scale, ..Material::default() }
    }

    /// Clamp every parameter to its physical range; non-finite values become safe defaults.
    pub fn sanitized(&self) -> Material {
        Material {
            base_color: color(self.base_color),
            base_weight: unit(self.base_weight, 1.0),
            metalness: unit(self.metalness, 0.0),
            specular_weight: unit(self.specular_weight, 1.0),
            specular_color: color(self.specular_color),
            specular_roughness: unit(self.specular_roughness, 0.5),
            specular_ior: if self.specular_ior.is_finite() { self.specular_ior.clamp(1.0, 5.0) } else { 1.5 },
            emission_color: if self.emission_color.is_finite() { self.emission_color.max(Vec3::ZERO) } else { Vec3::ZERO },
            emission_luminance: if self.emission_luminance.is_finite() { self.emission_luminance.clamp(0.0, 1e9) } else { 0.0 },
            emission_two_sided: self.emission_two_sided,
        }
    }

    /// Emitted radiance (before the sidedness test).
    pub fn emission(&self) -> Vec3 {
        self.emission_color * self.emission_luminance
    }

    pub fn is_emissive(&self) -> bool {
        self.emission().max_element() > 0.0
    }
}
