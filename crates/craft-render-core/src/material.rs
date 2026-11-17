//! Surface description. The parameters follow the OpenPBR Surface specification (Academy
//! Software Foundation, v1.1): a fuzz layer over a coat over a base that mixes metal with a
//! dielectric, which in turn is opaque (diffuse) or transmissive.

use craft_texture::Image;
use glam::{Vec2, Vec3, Vec3A};

/// An index into [`crate::Scene::textures`].
pub type TextureId = u32;

/// A surface material. Colours are linear; textures multiply the value they belong to.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Diffuse reflectance of the dielectric base and reflectance at normal incidence of the metal.
    pub base_color: Vec3,
    pub base_weight: f32,
    /// Roughness of the diffuse base (0 is Lambert; higher flattens it as Oren–Nayar does).
    pub base_diffuse_roughness: f32,
    /// 0 = dielectric, 1 = metal.
    pub base_metalness: f32,
    pub specular_weight: f32,
    /// Tints the dielectric reflection; for metal, the colour towards grazing angles.
    pub specular_color: Vec3,
    /// Perceptual roughness of the specular lobe; the microfacet width is its square.
    pub specular_roughness: f32,
    /// 0 is isotropic; 1 stretches highlights fully along the tangent.
    pub specular_roughness_anisotropy: f32,
    /// Index of refraction of the dielectric base.
    pub specular_ior: f32,
    /// 0 = opaque (diffuse) base, 1 = light passes through the dielectric base.
    pub transmission_weight: f32,
    pub transmission_color: Vec3,
    /// Distance at which light inside has `transmission_color`; 0 tints at the surface instead.
    pub transmission_depth: f32,
    pub coat_weight: f32,
    pub coat_color: Vec3,
    pub coat_roughness: f32,
    pub coat_ior: f32,
    /// A layer of fibres standing on the surface (cloth, dust, peach fuzz).
    pub fuzz_weight: f32,
    pub fuzz_color: Vec3,
    pub fuzz_roughness: f32,
    /// A thin transparent film on the dielectric base that colours its reflection by interference.
    pub thin_film_weight: f32,
    /// Film thickness in micrometres.
    pub thin_film_thickness: f32,
    pub thin_film_ior: f32,
    pub emission_color: Vec3,
    /// Emitted luminance scale (radiance = colour × luminance, in scene units).
    pub emission_luminance: f32,
    /// Emit from both faces instead of the front (counter-clockwise) face only.
    pub emission_two_sided: bool,
    /// 1 is solid; lower values let rays pass straight through (cut-outs).
    pub geometry_opacity: f32,

    pub base_color_texture: Option<TextureId>,
    /// Multiplies the roughness (reads the green channel, as glTF packs it).
    pub roughness_texture: Option<TextureId>,
    /// Multiplies the metalness (reads the blue channel, as glTF packs it).
    pub metalness_texture: Option<TextureId>,
    /// Tangent-space normal map (red and green are x and y in 0..1, blue is z).
    pub normal_texture: Option<TextureId>,
    pub normal_scale: f32,
    /// Multiplies the opacity (reads alpha for colour images, the value for grey ones).
    pub opacity_texture: Option<TextureId>,
    /// Height map for bump mapping (grey value), in units of `bump_scale`.
    pub bump_texture: Option<TextureId>,
    pub bump_scale: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material {
            base_color: Vec3::splat(0.8),
            base_weight: 1.0,
            base_diffuse_roughness: 0.0,
            base_metalness: 0.0,
            specular_weight: 1.0,
            specular_color: Vec3::ONE,
            specular_roughness: 0.3,
            specular_roughness_anisotropy: 0.0,
            specular_ior: 1.5,
            transmission_weight: 0.0,
            transmission_color: Vec3::ONE,
            transmission_depth: 0.0,
            coat_weight: 0.0,
            coat_color: Vec3::ONE,
            coat_roughness: 0.0,
            coat_ior: 1.6,
            fuzz_weight: 0.0,
            fuzz_color: Vec3::ONE,
            fuzz_roughness: 0.5,
            thin_film_weight: 0.0,
            thin_film_thickness: 0.5,
            thin_film_ior: 1.4,
            emission_color: Vec3::ONE,
            emission_luminance: 0.0,
            emission_two_sided: false,
            geometry_opacity: 1.0,
            base_color_texture: None,
            roughness_texture: None,
            metalness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            opacity_texture: None,
            bump_texture: None,
            bump_scale: 1.0,
        }
    }
}

fn unit(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { fallback }
}

fn color(c: Vec3) -> Vec3 {
    if c.is_finite() { c.clamp(Vec3::ZERO, Vec3::ONE) } else { Vec3::ZERO }
}

fn ior(v: f32, fallback: f32) -> f32 {
    if v.is_finite() { v.clamp(1.0, 5.0) } else { fallback }
}

/// The numbers the scattering model reads at one point, after textures.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceParams {
    pub base: Vec3A,
    pub diffuse_roughness: f32,
    pub metalness: f32,
    pub specular_weight: f32,
    pub specular_color: Vec3A,
    pub roughness: f32,
    pub anisotropy: f32,
    pub ior: f32,
    pub transmission: f32,
    pub transmission_tint: Vec3A,
    pub coat: f32,
    pub coat_color: Vec3A,
    pub coat_roughness: f32,
    pub coat_ior: f32,
    pub fuzz: f32,
    pub fuzz_color: Vec3A,
    pub fuzz_roughness: f32,
    pub thin_film: f32,
    pub thin_film_thickness: f32,
    pub thin_film_ior: f32,
}

impl Material {
    /// A matte surface.
    pub fn diffuse(color: Vec3) -> Material {
        Material { base_color: color, specular_weight: 0.0, ..Material::default() }
    }
    /// A metal.
    pub fn metal(color: Vec3, roughness: f32) -> Material {
        Material { base_color: color, base_metalness: 1.0, specular_roughness: roughness, ..Material::default() }
    }
    /// Clear glass (or any dielectric you can see through).
    pub fn glass(ior: f32, roughness: f32) -> Material {
        Material { base_color: Vec3::ONE, transmission_weight: 1.0, specular_ior: ior, specular_roughness: roughness, ..Material::default() }
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
            base_diffuse_roughness: unit(self.base_diffuse_roughness, 0.0),
            base_metalness: unit(self.base_metalness, 0.0),
            specular_weight: unit(self.specular_weight, 1.0),
            specular_color: color(self.specular_color),
            specular_roughness: unit(self.specular_roughness, 0.5),
            specular_roughness_anisotropy: unit(self.specular_roughness_anisotropy, 0.0),
            specular_ior: ior(self.specular_ior, 1.5),
            transmission_weight: unit(self.transmission_weight, 0.0),
            transmission_color: color(self.transmission_color),
            transmission_depth: if self.transmission_depth.is_finite() { self.transmission_depth.clamp(0.0, 1e9) } else { 0.0 },
            coat_weight: unit(self.coat_weight, 0.0),
            coat_color: color(self.coat_color),
            coat_roughness: unit(self.coat_roughness, 0.0),
            coat_ior: ior(self.coat_ior, 1.6),
            fuzz_weight: unit(self.fuzz_weight, 0.0),
            fuzz_color: color(self.fuzz_color),
            fuzz_roughness: unit(self.fuzz_roughness, 0.5),
            thin_film_weight: unit(self.thin_film_weight, 0.0),
            thin_film_thickness: if self.thin_film_thickness.is_finite() { self.thin_film_thickness.clamp(0.0, 100.0) } else { 0.5 },
            thin_film_ior: ior(self.thin_film_ior, 1.4),
            emission_color: if self.emission_color.is_finite() { self.emission_color.max(Vec3::ZERO) } else { Vec3::ZERO },
            emission_luminance: if self.emission_luminance.is_finite() { self.emission_luminance.clamp(0.0, 1e9) } else { 0.0 },
            emission_two_sided: self.emission_two_sided,
            geometry_opacity: unit(self.geometry_opacity, 1.0),
            normal_scale: if self.normal_scale.is_finite() { self.normal_scale.clamp(-16.0, 16.0) } else { 1.0 },
            bump_scale: if self.bump_scale.is_finite() { self.bump_scale.clamp(-1e3, 1e3) } else { 1.0 },
            ..self.clone()
        }
    }

    /// Emitted radiance (before the sidedness test).
    pub fn emission(&self) -> Vec3 {
        self.emission_color * self.emission_luminance
    }

    pub fn is_emissive(&self) -> bool {
        self.emission().max_element() > 0.0
    }

    /// Whether light can pass through the surface into the object.
    pub fn is_transmissive(&self) -> bool {
        self.transmission_weight > 0.0 && self.base_metalness < 1.0
    }

    /// Whether the surface has cut-outs.
    pub fn has_cutouts(&self) -> bool {
        self.geometry_opacity < 1.0 || self.opacity_texture.is_some()
    }

    /// Absorption per unit length inside the object (Beer–Lambert), zero when the tint applies
    /// at the surface.
    pub fn absorption(&self) -> Vec3A {
        if self.transmission_depth <= 0.0 {
            return Vec3A::ZERO;
        }
        let c = Vec3A::from(self.transmission_color).max(Vec3A::splat(1e-6));
        Vec3A::new(-c.x.ln(), -c.y.ln(), -c.z.ln()) / self.transmission_depth
    }

    /// Opacity at a texture coordinate.
    #[inline]
    pub fn opacity(&self, uv: Vec2, textures: &[Image]) -> f32 {
        match self.opacity_texture.and_then(|t| textures.get(t as usize)) {
            Some(img) => {
                let s = img.sample(uv.x, uv.y, true);
                self.geometry_opacity * if img.is_gray() { s[0] } else { s[3] }.clamp(0.0, 1.0)
            }
            None => self.geometry_opacity,
        }
    }

    /// The parameters at a texture coordinate. The material must be [`Material::sanitized`].
    #[inline]
    pub fn params(&self, uv: Vec2, textures: &[Image]) -> SurfaceParams {
        let tex = |id: Option<TextureId>| id.and_then(|t| textures.get(t as usize)).map(|img| img.sample(uv.x, uv.y, true));
        let mut base = Vec3A::from(self.base_color) * self.base_weight;
        if let Some(t) = tex(self.base_color_texture) {
            base *= Vec3A::new(t[0], t[1], t[2]).clamp(Vec3A::ZERO, Vec3A::ONE);
        }
        let roughness = tex(self.roughness_texture).map_or(self.specular_roughness, |t| (self.specular_roughness * t[1]).clamp(0.0, 1.0));
        let metalness = tex(self.metalness_texture).map_or(self.base_metalness, |t| (self.base_metalness * t[2]).clamp(0.0, 1.0));
        SurfaceParams {
            base,
            diffuse_roughness: self.base_diffuse_roughness,
            metalness,
            specular_weight: self.specular_weight,
            specular_color: self.specular_color.into(),
            roughness,
            anisotropy: self.specular_roughness_anisotropy,
            ior: self.specular_ior,
            transmission: self.transmission_weight,
            transmission_tint: if self.transmission_depth > 0.0 { Vec3A::ONE } else { self.transmission_color.into() },
            coat: self.coat_weight,
            coat_color: self.coat_color.into(),
            coat_roughness: self.coat_roughness,
            coat_ior: self.coat_ior,
            fuzz: self.fuzz_weight,
            fuzz_color: self.fuzz_color.into(),
            fuzz_roughness: self.fuzz_roughness,
            thin_film: self.thin_film_weight,
            thin_film_thickness: self.thin_film_thickness,
            thin_film_ior: self.thin_film_ior,
        }
    }
}
