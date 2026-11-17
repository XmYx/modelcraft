//! The scene a render reads: triangles in world space, materials, lights and a camera.

use craft_render_bvh::{Aabb, Hit, Ray, TriangleBvh};
use glam::{Mat3A, Mat4, Vec2, Vec3, Vec3A};

use crate::camera::Camera;
use crate::material::Material;
use crate::math::{Frame, Rgb, luminance, uniform_sphere, uniform_triangle};
use crate::shapes::MeshData;

/// Most triangles a scene may hold.
pub const MAX_TRIANGLES: usize = 100_000_000;
/// Most materials and analytic lights a scene may hold.
pub const MAX_MATERIALS: usize = 1 << 20;
pub const MAX_LIGHTS: usize = 1 << 20;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SceneError {
    #[error("the scene has more than {MAX_TRIANGLES} triangles")]
    TooManyTriangles,
    #[error("the scene has more than {MAX_MATERIALS} materials")]
    TooManyMaterials,
    #[error("the scene has more than {MAX_LIGHTS} lights")]
    TooManyLights,
    #[error("a mesh refers to vertex {0}, which it does not have")]
    BadIndex(u32),
    #[error("the image size {0}×{1} is outside 1..={max}", max = crate::render::MAX_IMAGE_SIDE)]
    BadImageSize(u32, u32),
}

/// A light without area.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// Emits `intensity` (radiant intensity, W/sr) equally in every direction.
    Point { position: Vec3, intensity: Vec3 },
    /// Parallel light arriving from `direction` (pointing towards the light) with `irradiance`
    /// on a surface facing it.
    Directional { direction: Vec3, irradiance: Vec3 },
}

/// One triangle that emits light.
#[derive(Clone, Copy, Debug)]
pub struct EmissiveTri {
    pub tri: u32,
    pub area: f32,
    pub radiance: Vec3,
    pub two_sided: bool,
}

/// A direction towards a light, drawn for next-event estimation.
#[derive(Clone, Copy, Debug)]
pub struct LightSample {
    /// Unit direction from the shaded point towards the light.
    pub wi: Vec3A,
    pub distance: f32,
    pub radiance: Rgb,
    /// Density over solid angle (lights with area) or the probability of choosing the light
    /// (`delta` lights), selection included.
    pub pdf: f32,
    pub delta: bool,
}

/// Everything that emits, with the table used to choose one light per shading point.
#[derive(Clone, Debug, Default)]
pub struct Lights {
    pub emissive: Vec<EmissiveTri>,
    pub analytic: Vec<Light>,
    /// Radiance arriving from every direction that leaves the scene.
    pub environment: Vec3,
    /// Triangle index → index into `emissive` (`u32::MAX` for the rest).
    tri_slot: Vec<u32>,
    /// Running sum of selection probabilities: emissive triangles, analytic lights, environment.
    cdf: Vec<f32>,
}

const NO_SLOT: u32 = u32::MAX;

impl Lights {
    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    fn build(&mut self, tri_count: usize, scene_radius: f32) {
        let mut power = Vec::with_capacity(self.emissive.len() + self.analytic.len() + 1);
        self.tri_slot = vec![NO_SLOT; tri_count];
        for (slot, e) in self.emissive.iter().enumerate() {
            if let Some(s) = self.tri_slot.get_mut(e.tri as usize) {
                *s = slot as u32;
            }
            let sides = if e.two_sided { 2.0 } else { 1.0 };
            power.push(luminance(e.radiance.into()) * e.area * std::f32::consts::PI * sides);
        }
        let r2 = scene_radius * scene_radius;
        for l in &self.analytic {
            power.push(match l {
                Light::Point { intensity, .. } => luminance((*intensity).into()) * 4.0 * std::f32::consts::PI,
                Light::Directional { irradiance, .. } => luminance((*irradiance).into()) * std::f32::consts::PI * r2,
            });
        }
        if self.environment.max_element() > 0.0 {
            power.push(luminance(self.environment.into()) * 4.0 * std::f32::consts::PI * std::f32::consts::PI * r2);
        }
        let total: f64 = power.iter().map(|p| f64::from(p.max(0.0))).sum();
        self.cdf.clear();
        if total <= 0.0 || !total.is_finite() {
            // Lights that carry no measurable power are chosen uniformly.
            let n = power.len();
            self.cdf.extend((1..=n).map(|i| i as f32 / n as f32));
            return;
        }
        let mut acc = 0.0f64;
        for p in &power {
            acc += f64::from(p.max(0.0));
            self.cdf.push((acc / total) as f32);
        }
        if let Some(last) = self.cdf.last_mut() {
            *last = 1.0;
        }
    }

    /// The running sum of selection probabilities, in the order emissive triangles, analytic
    /// lights, environment.
    pub fn selection_cdf(&self) -> &[f32] {
        &self.cdf
    }

    /// The emissive slot of a triangle, if it emits.
    pub fn emissive_slot(&self, tri: u32) -> Option<u32> {
        self.tri_slot.get(tri as usize).copied().filter(|s| *s != NO_SLOT)
    }

    /// Probability with which entry `entry` of [`Lights::selection_cdf`] is chosen.
    #[inline]
    pub fn pmf(&self, entry: usize) -> f32 {
        let hi = self.cdf.get(entry).copied().unwrap_or(0.0);
        let lo = if entry == 0 { 0.0 } else { self.cdf.get(entry - 1).copied().unwrap_or(0.0) };
        hi - lo
    }

    /// Density over solid angle with which [`Lights::sample`] reaches a point on emissive
    /// triangle `tri`, seen at `distance` under the angle whose cosine is `cos_light`.
    #[inline]
    pub fn pdf_triangle(&self, tri: u32, distance: f32, cos_light: f32) -> f32 {
        let Some(slot) = self.tri_slot.get(tri as usize).copied().filter(|s| *s != NO_SLOT) else { return 0.0 };
        let Some(e) = self.emissive.get(slot as usize) else { return 0.0 };
        let cos = cos_light.abs();
        if cos <= 0.0 || e.area <= 0.0 {
            return 0.0;
        }
        self.pmf(slot as usize) * distance * distance / (e.area * cos)
    }

    /// Density with which [`Lights::sample`] picks a direction that leaves the scene.
    #[inline]
    pub fn pdf_environment(&self) -> f32 {
        if self.environment.max_element() > 0.0 { self.pmf(self.cdf.len().saturating_sub(1)) / (4.0 * std::f32::consts::PI) } else { 0.0 }
    }

    /// Radiance an emissive triangle sends towards a viewer on its front (`front`) or back side.
    #[inline]
    pub fn emitted(&self, tri: u32, front: bool) -> Option<Rgb> {
        let slot = self.tri_slot.get(tri as usize).copied().filter(|s| *s != NO_SLOT)?;
        let e = self.emissive.get(slot as usize)?;
        (front || e.two_sided).then(|| e.radiance.into())
    }

    /// Choose a light with `pick` and a point on it with `u`, as seen from `p`.
    #[inline]
    pub fn sample(&self, positions: &[[Vec3; 3]], p: Vec3A, pick: f32, u: Vec2) -> Option<LightSample> {
        if self.cdf.is_empty() {
            return None;
        }
        let entry = self.cdf.partition_point(|c| *c <= pick).min(self.cdf.len() - 1);
        let pmf = self.pmf(entry);
        if pmf <= 0.0 {
            return None;
        }
        if let Some(e) = self.emissive.get(entry) {
            let t = positions.get(e.tri as usize)?;
            let (p0, p1, p2) = (Vec3A::from(t[0]), Vec3A::from(t[1]), Vec3A::from(t[2]));
            let (b1, b2) = uniform_triangle(u);
            let q = p0 + (p1 - p0) * b1 + (p2 - p0) * b2;
            let d = q - p;
            let dist2 = d.length_squared();
            if dist2 <= 0.0 {
                return None;
            }
            let distance = dist2.sqrt();
            let wi = d / distance;
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            let cos = -wi.dot(n);
            if cos == 0.0 || (cos < 0.0 && !e.two_sided) || e.area <= 0.0 {
                return None;
            }
            return Some(LightSample { wi, distance, radiance: e.radiance.into(), pdf: pmf * dist2 / (e.area * cos.abs()), delta: false });
        }
        if let Some(l) = self.analytic.get(entry - self.emissive.len()) {
            return match *l {
                Light::Point { position, intensity } => {
                    let d = Vec3A::from(position) - p;
                    let dist2 = d.length_squared();
                    (dist2 > 0.0).then(|| LightSample {
                        wi: d / dist2.sqrt(),
                        distance: dist2.sqrt(),
                        radiance: Vec3A::from(intensity) / dist2,
                        pdf: pmf,
                        delta: true,
                    })
                }
                Light::Directional { direction, irradiance } => {
                    let wi = Vec3A::from(direction).normalize_or_zero();
                    (wi != Vec3A::ZERO).then(|| LightSample { wi, distance: f32::INFINITY, radiance: irradiance.into(), pdf: pmf, delta: true })
                }
            };
        }
        let wi = uniform_sphere(u);
        Some(LightSample { wi, distance: f32::INFINITY, radiance: self.environment.into(), pdf: pmf / (4.0 * std::f32::consts::PI), delta: false })
    }
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
    pub lights: Lights,
    pub camera: Camera,
    pub bounds: Aabb,
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.positions.len()
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

    /// Whether anything blocks the segment from `p` along `dir` up to `distance`.
    #[inline]
    pub fn occluded(&self, p: Vec3A, ng: Vec3A, dir: Vec3A, distance: f32) -> bool {
        let tmax = if distance.is_finite() { distance * (1.0 - 2e-4) - 1e-5 } else { f32::INFINITY };
        if tmax <= 0.0 {
            return false;
        }
        self.accel.occluded(&Scene::spawn(p, ng, dir, tmax))
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

    pub fn add_light(&mut self, light: Light) -> Result<(), SceneError> {
        if self.scene.lights.analytic.len() >= MAX_LIGHTS {
            return Err(SceneError::TooManyLights);
        }
        let ok = match light {
            Light::Point { position, intensity } => position.is_finite() && intensity.is_finite() && intensity.max_element() > 0.0,
            Light::Directional { direction, irradiance } => {
                direction.is_finite() && direction.length_squared() > 0.0 && irradiance.is_finite() && irradiance.max_element() > 0.0
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
        s.accel = TriangleBvh::build(&s.positions);
        s.bounds = s.accel.bvh.bounds();
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
        s.lights.build(s.positions.len(), radius);
        self.scene
    }
}
