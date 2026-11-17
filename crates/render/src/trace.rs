//! From the ModelCraft scene graph to the Craft render engine.
//!
//! [`export_scene`] evaluates every visible surface (meshes and smooth cages, not cage inputs,
//! groups or deformers: the same set the viewport draws), bakes world transforms, converts the
//! glTF metallic-roughness material and adds lighting, because the scene graph has no light
//! objects yet. The path tracer itself lives in the UI-free `craft-render-*` crates.

use craft_render_core::shapes::quad;
use craft_render_core::{Camera as TraceCamera, Material as TraceMaterial, MeshData, Scene as TraceScene, SceneBuilder, SceneError};
use glam::{Mat4, Vec3};
use modelcraft_mesh::PolyMesh;
use modelcraft_scene::{Evaluator, ObjectId, ObjectKind, Scene};

use crate::buffers::mesh_data;
use crate::camera::{Camera, Projection};

/// Lighting added on export.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lighting {
    /// A soft sky plus one large key light above and to the side, sized to the scene.
    Studio,
    /// Uniform light from every direction with this radiance.
    Sky(Vec3),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExportOptions {
    pub lighting: Lighting,
    /// Add a matte ground plane under the scene to catch shadows.
    pub ground: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { lighting: Lighting::Studio, ground: true }
    }
}

/// Triangles with auto-smoothed corner normals, as the viewport shades them.
pub fn trace_mesh(m: &PolyMesh) -> MeshData {
    let data = mesh_data(m);
    let positions: Vec<Vec3> = data.vertices.iter().map(|v| Vec3::from_array(v.pos)).collect();
    let mut indices = Vec::with_capacity(data.indices.len() / 3);
    let mut normals = Vec::with_capacity(data.indices.len() / 3);
    let (triangles, _) = data.indices.as_chunks::<3>();
    for t in triangles {
        let n = |i: u32| data.vertices.get(i as usize).map_or(Vec3::ZERO, |v| Vec3::from_array(v.normal));
        indices.push(*t);
        normals.push([n(t[0]), n(t[1]), n(t[2])]);
    }
    MeshData { positions, indices, normals: Some(normals), uvs: None }
}

/// The path tracer's camera for a viewport camera.
pub fn trace_camera(c: &Camera) -> TraceCamera {
    TraceCamera {
        eye: c.eye(),
        target: c.target,
        up: Vec3::Y,
        fov_y: c.fov_y,
        ortho_height: (c.projection == Projection::Orthographic).then(|| 2.0 * c.distance * (c.fov_y * 0.5).tan()),
        aperture_radius: 0.0,
        focus_distance: c.distance,
    }
}

/// The surfaces a render shows, in tree order.
pub fn renderable(scene: &Scene) -> Vec<ObjectId> {
    scene
        .tree_order()
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| scene.visible(*id) && !scene.is_cage_input(*id))
        .filter(|id| matches!(scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. })))
        .collect()
}

/// World-space bounds of everything [`renderable`], or `None` for an empty scene.
pub fn scene_bounds(scene: &Scene, eval: &mut Evaluator) -> Option<(Vec3, Vec3)> {
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for id in renderable(scene) {
        let mesh = eval.evaluate(scene, id).mesh;
        if mesh.vertex_count() == 0 {
            continue;
        }
        let (lo, hi) = mesh.bounds();
        let w = scene.world_matrix(id);
        for k in 0..8 {
            let corner = Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z });
            let p = w.transform_point3(corner);
            if p.is_finite() {
                bounds = Some(bounds.map_or((p, p), |(a, b)| (a.min(p), b.max(p))));
            }
        }
    }
    bounds
}

/// Build the scene the path tracer renders. Call `Document::sync_sculpt` first so an open sculpt
/// session is included.
pub fn export_scene(scene: &Scene, eval: &mut Evaluator, camera: &Camera, options: &ExportOptions) -> Result<TraceScene, SceneError> {
    Ok(export_builder(scene, eval, camera, options)?.build())
}

/// [`export_scene`] without the last step: the returned builder still has to build its
/// acceleration structure, which a caller can do on another thread.
pub fn export_builder(scene: &Scene, eval: &mut Evaluator, camera: &Camera, options: &ExportOptions) -> Result<SceneBuilder, SceneError> {
    let mut b = SceneBuilder::new();
    for id in renderable(scene) {
        let Some(object) = scene.get(id) else { continue };
        let mesh = eval.evaluate(scene, id).mesh;
        if mesh.face_count() == 0 {
            continue;
        }
        let m = &object.material;
        let material = b.add_material(TraceMaterial {
            base_color: Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]),
            metalness: m.metallic,
            specular_roughness: m.roughness,
            ..TraceMaterial::default()
        })?;
        b.add_mesh(&trace_mesh(&mesh), scene.world_matrix(id), material)?;
    }
    let (lo, hi) = scene_bounds(scene, eval).unwrap_or((Vec3::splat(-1.0), Vec3::splat(1.0)));
    let centre = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(1e-3);
    if options.ground && b.triangle_count() > 0 {
        let ground = b.add_material(TraceMaterial { base_color: Vec3::splat(0.45), specular_roughness: 0.7, ..TraceMaterial::default() })?;
        let s = radius * 40.0;
        // A hair below the lowest point so resting faces do not coincide with it.
        let y = lo.y - radius * 1e-4;
        b.add_mesh(&quad(Vec3::new(centre.x - s, y, centre.z - s), Vec3::Z * (2.0 * s), Vec3::X * (2.0 * s)), Mat4::IDENTITY, ground)?;
    }
    match options.lighting {
        Lighting::Sky(radiance) => b.set_environment(radiance),
        Lighting::Studio => {
            b.set_environment(Vec3::new(0.32, 0.36, 0.42));
            // The key: a square of side 1.5 radii, 3 radii away, up and to the camera's left,
            // facing the scene. Its radiance gives a matte surface facing it about 2.5 units of
            // irradiance whatever the scene's size.
            let back = camera.back();
            let side = back.cross(Vec3::Y).normalize_or(Vec3::X);
            let dir = (back * 0.5 - side * 0.7 + Vec3::Y * 0.9).normalize_or(Vec3::Y);
            let pos = centre + dir * radius * 3.0;
            let u = dir.cross(Vec3::Y).normalize_or(Vec3::X);
            let v = dir.cross(u);
            let half = radius * 0.75;
            let solid_angle = (2.0 * half) * (2.0 * half) / (radius * 3.0).powi(2);
            let key = b.add_material(TraceMaterial::emitter(Vec3::new(1.0, 0.96, 0.9) * (2.5 / solid_angle)))?;
            // `u × v` is `dir`, so the front face would look away from the scene; swap the edges.
            b.add_mesh(&quad(pos - u * half - v * half, v * (2.0 * half), u * (2.0 * half)), Mat4::IDENTITY, key)?;
        }
    }
    b.set_camera(trace_camera(camera));
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_render_core::{RenderSettings, mean_rgb, render_image};
    use modelcraft_scene::{Material, Primitive, Source};
    use std::sync::Arc;

    fn cube_scene() -> Scene {
        let mut s = Scene::new();
        let cube = s
            .add(
                "Cube",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 1 } }, steps: vec![] },
                None,
            )
            .unwrap();
        s.edit(cube).unwrap().material = Material { base_color: [0.9, 0.1, 0.1, 1.0], metallic: 0.0, roughness: 0.5, linked: None };
        let hidden = s
            .add(
                "Hidden",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(50.0), segments: 1 } }, steps: vec![] },
                None,
            )
            .unwrap();
        s.edit(hidden).unwrap().visible = false;
        s.add("Group", ObjectKind::Group, None).unwrap();
        s
    }

    #[test]
    fn exports_visible_surfaces_with_materials_ground_and_light() {
        let scene = cube_scene();
        let mut eval = Evaluator::new();
        let mut cam = Camera::default();
        let (lo, hi) = scene_bounds(&scene, &mut eval).unwrap();
        assert!((lo - Vec3::splat(-1.0)).length() < 1e-5 && (hi - Vec3::splat(1.0)).length() < 1e-5);
        cam.frame(lo, hi);
        let t = export_scene(&scene, &mut eval, &cam, &ExportOptions::default()).unwrap();
        // 12 cube triangles, 2 ground, 2 key light; the hidden cube and the group add nothing.
        assert_eq!(t.triangle_count(), 16);
        assert_eq!(t.lights.emissive.len(), 2);
        assert_eq!(t.materials.len(), 3);
        // The key light faces the scene: its front side sees the cube's centre.
        let e = t.lights.emissive[0];
        let p = t.positions[e.tri as usize];
        assert!((p[1] - p[0]).cross(p[2] - p[0]).dot(Vec3::ZERO - p[0]) > 0.0);

        let film = render_image(Arc::new(t), RenderSettings { width: 48, height: 48, ..RenderSettings::default() }, 32).unwrap();
        let img = film.mean();
        // The cube is red and lit; the image is not black and not blown out.
        let c = img[24 * 48 + 24];
        assert!(c[0] > 2.0 * c[1] && c[0] > 0.05, "{c:?}");
        let mean = mean_rgb(&img);
        assert!(mean[1] > 0.05 && mean[1] < 2.0, "{mean:?}");
    }

    #[test]
    fn empty_scenes_and_orthographic_views_export() {
        let mut eval = Evaluator::new();
        let empty = export_scene(&Scene::new(), &mut eval, &Camera::default(), &ExportOptions::default()).unwrap();
        // Only the key light.
        assert_eq!(empty.triangle_count(), 2);
        assert!(scene_bounds(&Scene::new(), &mut eval).is_none());
        let ortho = trace_camera(&Camera::preset(crate::camera::ViewPreset::Front));
        assert!(ortho.ortho_height.is_some_and(|h| h > 0.0));
        let sky = export_scene(&cube_scene(), &mut eval, &Camera::default(), &ExportOptions { lighting: Lighting::Sky(Vec3::ONE), ground: false }).unwrap();
        assert_eq!((sky.triangle_count(), sky.lights.emissive.len()), (12, 0));
    }
}
