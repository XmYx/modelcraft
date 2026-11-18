//! The scene a render reads: triangles in world space, materials, lights and a camera.

use craft_render_bvh::{Aabb, Hit, Ray, TriangleBvh};
use craft_texture::Image;
use glam::{Mat3A, Mat4, Vec2, Vec3, Vec3A};

use crate::camera::Camera;
use crate::hair::{CurveSet, FibreHit, HairParams, SharedCurves, leave_fibre};
pub use crate::light::{EmissiveTri, EnvMap, Light, LightSample, LightSampling, Lights};
use crate::material::Material;
use crate::math::Frame;
use crate::shapes::MeshData;

/// Most triangles a scene may hold.
pub const MAX_TRIANGLES: usize = 100_000_000;
/// Most materials and analytic lights a scene may hold.
pub const MAX_MATERIALS: usize = 1 << 20;
pub const MAX_LIGHTS: usize = 1 << 20;
/// Most textures a scene may hold.
pub const MAX_TEXTURES: usize = 4096;
/// Most curve sets (hair objects) a scene may hold.
pub const MAX_CURVE_SETS: usize = 256;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SceneError {
    #[error("the scene has more than {MAX_TRIANGLES} triangles")]
    TooManyTriangles,
    #[error("the scene has more than {MAX_MATERIALS} materials")]
    TooManyMaterials,
    #[error("the scene has more than {MAX_LIGHTS} lights")]
    TooManyLights,
    #[error("the scene has more than {MAX_TEXTURES} textures")]
    TooManyTextures,
    #[error("a mesh refers to vertex {0}, which it does not have")]
    BadIndex(u32),
    #[error("the scene has more than {MAX_CURVE_SETS} curve sets or {} curve segments", craft_render_bvh::curves::MAX_SEGMENTS)]
    TooManyCurves,
    #[error("the image size {0}×{1} is outside 1..={max}", max = crate::render::MAX_IMAGE_SIDE)]
    BadImageSize(u32, u32),
}

/// What the integrator needs at a hit.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    pub p: Vec3A,
    /// Geometric normal on the viewer's side.
    pub ng: Vec3A,
    /// Shading frame (its normal is on the viewer's side).
    pub frame: Frame,
    pub uv: Vec2,
    pub material: u32,
    /// Whether the viewer sees the front (counter-clockwise) face.
    pub front: bool,
}

/// A scene ready to render. Build one with [`SceneBuilder`].
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub accel: TriangleBvh,
    /// World-space corners of every triangle.
    pub positions: Vec<[Vec3; 3]>,
    pub normals: Vec<[Vec3; 3]>,
    pub uvs: Vec<[Vec2; 3]>,
    pub tri_material: Vec<u32>,
    /// The object (in the order meshes were added) each triangle belongs to.
    pub tri_object: Vec<u32>,
    pub materials: Vec<Material>,
    /// Images the materials refer to (linear values).
    pub textures: Vec<Image>,
    /// Whether any material has cut-outs, so shadow rays must look at opacity.
    pub has_cutouts: bool,
    pub lights: Lights,
    pub camera: Camera,
    pub bounds: Aabb,
    /// Hair and fibres: round-cone curves with their own scattering model ([`crate::hair`]).
    pub curves: Vec<SharedCurves>,
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.positions.len()
    }

    /// Answer ray queries (and, on the GPU, trace) with the compressed 8-wide form of the
    /// hierarchy. `false`, and nothing changes, when the tree does not fit that form.
    pub fn enable_wide_bvh(&mut self) -> bool {
        self.accel.enable_wide()
    }

    /// Curve segments over all curve sets.
    pub fn curve_segment_count(&self) -> usize {
        self.curves.iter().map(|c| c.bvh.segment_count()).sum()
    }

    /// The nearest fibre the ray hits closer than `tmax`: the curve set it belongs to and the hit.
    #[inline]
    pub fn intersect_curves(&self, ray: &Ray, tmax: f32) -> Option<(usize, FibreHit)> {
        let mut best: Option<(usize, FibreHit)> = None;
        let mut limit = tmax;
        for (i, set) in self.curves.iter().enumerate() {
            if let Some(h) = set.intersect(ray, limit) {
                limit = h.t;
                best = Some((i, h));
            }
        }
        best
    }

    /// Whether anything blocks the segment from a fibre hit along `dir` up to `distance`.
    pub fn occluded_from_fibre(&self, hit: &FibreHit, dir: Vec3A, distance: f32) -> bool {
        let mut ray = leave_fibre(hit, dir, f32::INFINITY);
        if distance.is_finite() {
            let to = hit.p + dir * distance - ray.origin;
            let len = to.length();
            ray.tmax = len * (1.0 - 2e-4) - 1e-5;
            if ray.tmax <= 0.0 {
                return false;
            }
            ray.dir = to / len;
        }
        self.blocked(&ray)
    }

    /// Geometry and shading data at a hit of `ray`.
    #[inline]
    pub fn surface(&self, hit: &Hit, ray: &Ray) -> Option<Surface> {
        let i = hit.prim as usize;
        let t = self.positions.get(i)?;
        let (p0, p1, p2) = (Vec3A::from(t[0]), Vec3A::from(t[1]), Vec3A::from(t[2]));
        let w = 1.0 - hit.u - hit.v;
        let p = p0 * w + p1 * hit.u + p2 * hit.v;
        let mut ng = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        if ng == Vec3A::ZERO {
            return None;
        }
        let front = ng.dot(ray.dir) < 0.0;
        if !front {
            ng = -ng;
        }
        let mut ns = match self.normals.get(i) {
            Some(n) => (Vec3A::from(n[0]) * w + Vec3A::from(n[1]) * hit.u + Vec3A::from(n[2]) * hit.v).normalize_or(ng),
            None => ng,
        };
        if ns.dot(ng) < 0.0 {
            ns = -ns;
        }
        let uv = self.uvs.get(i).map_or(Vec2::ZERO, |t| t[0] * w + t[1] * hit.u + t[2] * hit.v);
        Some(Surface { p, ng, frame: Frame::new(ns), uv, material: self.tri_material.get(i).copied().unwrap_or(0), front })
    }

    /// A ray leaving `p` towards `dir`, started just off the surface (normal `ng`) so it cannot
    /// hit the triangle it leaves.
    #[inline]
    pub fn spawn(p: Vec3A, ng: Vec3A, dir: Vec3A, tmax: f32) -> Ray {
        let eps = 4e-5 * (1.0 + p.abs().max_element());
        let side = if dir.dot(ng) >= 0.0 { 1.0 } else { -1.0 };
        Ray { origin: p + ng * (eps * side), dir, tmin: 0.0, tmax }
    }

    /// Whether anything blocks the segment from `p` along `dir` up to `distance`. Cut-outs block
    /// in proportion to their opacity (a fixed pattern per point decides, so the answer is
    /// repeatable).
    #[inline]
    pub fn occluded(&self, p: Vec3A, ng: Vec3A, dir: Vec3A, distance: f32) -> bool {
        let mut ray = Scene::spawn(p, ng, dir, f32::INFINITY);
        if distance.is_finite() {
            // Aim from the offset origin at the very point on the light, and stop just short of
            // it: keeping the old direction would cross the light's own plane early at grazing
            // angles and count the light as its own shadow.
            let to = p + dir * distance - ray.origin;
            let len = to.length();
            ray.tmax = len * (1.0 - 2e-4) - 1e-5;
            if ray.tmax <= 0.0 {
                return false;
            }
            ray.dir = to / len;
        }
        self.blocked(&ray)
    }

    /// Whether triangles (cut-outs by opacity) or fibres lie inside the ray's interval.
    fn blocked(&self, ray: &Ray) -> bool {
        let triangles = if !self.has_cutouts {
            self.accel.occluded(ray)
        } else {
            self.accel.occluded_with(ray, |hit| {
                let opacity = self.opacity_at(hit);
                opacity >= 1.0
                    || (opacity > 0.0
                        && (crate::sampler::hash_combine(hit.prim, hit.u.to_bits() ^ hit.v.to_bits().rotate_left(16)) >> 8) as f32 / 16_777_216.0 < opacity)
            })
        };
        triangles || self.curves.iter().any(|c| c.occluded(ray))
    }

    /// Opacity of the surface at a hit.
    #[inline]
    pub fn opacity_at(&self, hit: &Hit) -> f32 {
        let i = hit.prim as usize;
        let Some(m) = self.tri_material.get(i).and_then(|m| self.materials.get(*m as usize)) else { return 1.0 };
        if !m.has_cutouts() {
            return 1.0;
        }
        let uv = self.uvs.get(i).map_or(Vec2::ZERO, |t| t[0] * (1.0 - hit.u - hit.v) + t[1] * hit.u + t[2] * hit.v);
        m.opacity(uv, &self.textures)
    }

    /// The directions in which the texture coordinates `u` and `v` grow across a triangle, or
    /// `None` when its texture coordinates are degenerate.
    #[inline]
    pub fn uv_axes(&self, tri: u32) -> Option<(Vec3A, Vec3A)> {
        let (p, t) = (self.positions.get(tri as usize)?, self.uvs.get(tri as usize)?);
        let (e1, e2) = (Vec3A::from(p[1] - p[0]), Vec3A::from(p[2] - p[0]));
        let (d1, d2) = (t[1] - t[0], t[2] - t[0]);
        let det = d1.x * d2.y - d1.y * d2.x;
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let dpdu = (e1 * d2.y - e2 * d1.y) / det;
        let dpdv = (e2 * d1.x - e1 * d2.x) / det;
        (dpdu.is_finite() && dpdv.is_finite() && dpdu.length_squared() > 0.0 && dpdv.length_squared() > 0.0).then_some((dpdu, dpdv))
    }

    pub fn material(&self, index: u32) -> Option<&Material> {
        self.materials.get(index as usize)
    }
}

/// Collects meshes, materials and lights, then builds the acceleration structure.
#[derive(Clone, Debug, Default)]
pub struct SceneBuilder {
    scene: Scene,
    objects: u32,
}

impl SceneBuilder {
    pub fn new() -> SceneBuilder {
        SceneBuilder::default()
    }

    pub fn add_material(&mut self, m: Material) -> Result<u32, SceneError> {
        if self.scene.materials.len() >= MAX_MATERIALS {
            return Err(SceneError::TooManyMaterials);
        }
        self.scene.materials.push(m.sanitized());
        Ok(self.scene.materials.len() as u32 - 1)
    }

    /// Add an image for materials to refer to (linear values; decode sRGB first).
    pub fn add_texture(&mut self, image: Image) -> Result<u32, SceneError> {
        if self.scene.textures.len() >= MAX_TEXTURES {
            return Err(SceneError::TooManyTextures);
        }
        self.scene.textures.push(image);
        Ok(self.scene.textures.len() as u32 - 1)
    }

    /// Add a mesh placed by `transform`. Returns the object's index.
    pub fn add_mesh(&mut self, mesh: &MeshData, transform: Mat4, material: u32) -> Result<u32, SceneError> {
        let s = &mut self.scene;
        if s.positions.len().saturating_add(mesh.indices.len()) > MAX_TRIANGLES {
            return Err(SceneError::TooManyTriangles);
        }
        let transform = if transform.is_finite() { transform } else { Mat4::IDENTITY };
        let linear = Mat3A::from_mat4(transform);
        let det = linear.determinant();
        let normal_matrix = (det.is_finite() && det.abs() > 1e-20).then(|| linear.inverse().transpose());
        let normals = mesh.normals.as_ref().filter(|n| n.len() == mesh.indices.len());
        let uvs = mesh.uvs.as_ref().filter(|u| u.len() == mesh.indices.len());
        for (k, tri) in mesh.indices.iter().enumerate() {
            let mut corners = [Vec3::ZERO; 3];
            for (c, i) in corners.iter_mut().zip(tri) {
                *c = transform.transform_point3(*mesh.positions.get(*i as usize).ok_or(SceneError::BadIndex(*i))?);
            }
            let ng = (corners[1] - corners[0]).cross(corners[2] - corners[0]).normalize_or_zero();
            let ns = match (normals.and_then(|n| n.get(k)), normal_matrix) {
                (Some(n), Some(nm)) => n.map(|v| {
                    let w = Vec3::from(nm * Vec3A::from(v)).normalize_or_zero();
                    if w.is_finite() && w != Vec3::ZERO { w } else { ng }
                }),
                _ => [ng; 3],
            };
            s.positions.push(corners);
            s.normals.push(ns);
            s.uvs.push(uvs.and_then(|u| u.get(k)).copied().map_or([Vec2::ZERO; 3], |t| t.map(|v| if v.is_finite() { v } else { Vec2::ZERO })));
            s.tri_material.push(material);
            s.tri_object.push(self.objects);
        }
        self.objects += 1;
        Ok(self.objects - 1)
    }

    /// Add hair or fibres: `points` holds `points_per_strand` points per strand back to back (world
    /// space) and `radii` the radius at each point. Returns the curve set's index.
    pub fn add_curves(&mut self, points: &[Vec3], radii: &[f32], points_per_strand: usize, params: HairParams) -> Result<usize, SceneError> {
        let s = &mut self.scene;
        let segments = points.len().saturating_sub(points.len() / points_per_strand.max(1));
        if s.curves.len() >= MAX_CURVE_SETS || s.curve_segment_count().saturating_add(segments) > craft_render_bvh::curves::MAX_SEGMENTS {
            return Err(SceneError::TooManyCurves);
        }
        s.curves.push(std::sync::Arc::new(CurveSet::new(points, radii, points_per_strand, params)));
        Ok(s.curves.len() - 1)
    }

    pub fn add_light(&mut self, light: Light) -> Result<(), SceneError> {
        if self.scene.lights.analytic.len() >= MAX_LIGHTS {
            return Err(SceneError::TooManyLights);
        }
        let good = |v: Vec3| v.is_finite() && v.max_element() > 0.0;
        let ok = match light {
            Light::Point { position, intensity } => position.is_finite() && good(intensity),
            Light::Spot { position, direction, intensity, cone_angle, blend } => {
                position.is_finite()
                    && direction.is_finite()
                    && direction.length_squared() > 0.0
                    && good(intensity)
                    && cone_angle.is_finite()
                    && cone_angle > 0.0
                    && blend.is_finite()
            }
            Light::Directional { direction, irradiance } => direction.is_finite() && direction.length_squared() > 0.0 && good(irradiance),
            Light::Sun { direction, irradiance, angular_radius } => {
                direction.is_finite() && direction.length_squared() > 0.0 && good(irradiance) && angular_radius.is_finite()
            }
        };
        if ok {
            self.scene.lights.analytic.push(light);
        }
        Ok(())
    }

    /// Constant radiance arriving from outside the scene.
    pub fn set_environment(&mut self, radiance: Vec3) {
        self.scene.lights.environment = if radiance.is_finite() { radiance.max(Vec3::ZERO) } else { Vec3::ZERO };
    }

    /// Light the scene with an image of the surroundings (latitude–longitude layout, linear
    /// radiance), turned by `rotation` radians about the vertical axis. Replaces the constant
    /// environment.
    pub fn set_environment_map(&mut self, image: Image, rotation: f32) {
        self.scene.lights.env_map = EnvMap::new(image, rotation, true);
    }

    /// [`SceneBuilder::set_environment_map`] with the plain sampling table (brightness, not
    /// brightness above the mean), which is also correct for light sampling on its own.
    pub fn set_environment_map_uncompensated(&mut self, image: Image, rotation: f32) {
        self.scene.lights.env_map = EnvMap::new(image, rotation, false);
    }

    pub fn set_camera(&mut self, camera: Camera) {
        self.scene.camera = camera;
    }

    pub fn triangle_count(&self) -> usize {
        self.scene.positions.len()
    }

    /// Build the acceleration structure and the light table.
    pub fn build(mut self) -> Scene {
        let s = &mut self.scene;
        if s.materials.is_empty() {
            s.materials.push(Material::default().sanitized());
        }
        let last = s.materials.len() as u32 - 1;
        for m in &mut s.tri_material {
            *m = (*m).min(last);
        }
        s.has_cutouts = s.materials.iter().any(Material::has_cutouts);
        s.accel = TriangleBvh::build(&s.positions);
        s.bounds = s.accel.bvh.bounds();
        for c in &s.curves {
            s.bounds = s.bounds.union(c.bounds());
        }
        s.lights.emissive.clear();
        for (i, t) in s.positions.iter().enumerate() {
            let Some(m) = s.materials.get(s.tri_material[i] as usize) else { continue };
            if !m.is_emissive() {
                continue;
            }
            let area = 0.5 * (t[1] - t[0]).cross(t[2] - t[0]).length();
            if area.is_finite() && area > 0.0 {
                s.lights.emissive.push(EmissiveTri { tri: i as u32, area, radiance: m.emission(), two_sided: m.emission_two_sided });
            }
        }
        let radius = if s.bounds.is_empty() { 1.0 } else { ((s.bounds.max - s.bounds.min).length() * 0.5).max(1e-3) };
        s.lights.build(&s.positions, radius);
        self.scene
    }
}
