//! From the ModelCraft scene graph to the Craft render engine.
//!
//! [`export_scene`] evaluates every visible surface (meshes and smooth cages, not cage inputs,
//! groups or deformers: the same set the viewport draws), bakes world transforms, converts the
//! glTF metallic-roughness material and adds lighting, because the scene graph has no light
//! objects yet. The path tracer itself lives in the UI-free `craft-render-*` crates.

use craft_render_core::shapes::quad;
use craft_render_core::sky::Sky;
use craft_render_core::texture::{Image, Kind, Limits, srgb_to_linear};
use craft_render_core::{Camera as TraceCamera, Material as TraceMaterial, MeshData, Scene as TraceScene, SceneBuilder, SceneError};
use glam::{Mat4, Vec2, Vec3};
use modelcraft_mesh::PolyMesh;
use modelcraft_scene::{Evaluator, ObjectId, ObjectKind, Scene};

use crate::buffers::mesh_data;
use crate::camera::{Camera, Projection};

/// Lighting added on export.
#[derive(Clone, Debug, PartialEq)]
pub enum Lighting {
    /// A soft sky plus one large key light above and to the side, sized to the scene.
    Studio,
    /// Uniform light from every direction with this radiance.
    Sky(Vec3),
    /// A physical daytime sky with the sun at `elevation` above the horizon and compass angle
    /// `azimuth` (radians).
    Daylight { elevation: f32, azimuth: f32 },
    /// An image of the surroundings (latitude–longitude, linear radiance), turned by `rotation`
    /// radians about the vertical axis.
    Image { image: Image, rotation: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub lighting: Lighting,
    /// Add a matte ground plane under the scene to catch shadows.
    pub ground: bool,
    /// Read the maps of linked MaterialCraft materials from disk.
    pub linked_materials: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { lighting: Lighting::Studio, ground: true, linked_materials: true }
    }
}

/// Triangles with auto-smoothed corner normals, as the viewport shades them, and the mesh's
/// texture coordinates (meshes too dense for per-corner data are exported without them).
pub fn trace_mesh(m: &PolyMesh) -> MeshData {
    let data = mesh_data(m);
    let positions: Vec<Vec3> = data.vertices.iter().map(|v| Vec3::from_array(v.pos)).collect();
    // `mesh_data` makes one vertex per face corner, in face order, unless the mesh is very dense.
    let corner_uvs: Option<Vec<Vec2>> = (m.has_uvs() && data.vertices.len() == (0..m.face_count() as u32).map(|f| m.face_len(f)).sum::<usize>())
        .then(|| (0..m.face_count() as u32).flat_map(|f| m.face_half_edges(f).map(|h| m.uv(h))).collect());
    let mut indices = Vec::with_capacity(data.indices.len() / 3);
    let mut normals = Vec::with_capacity(data.indices.len() / 3);
    let mut uvs = corner_uvs.as_ref().map(|_| Vec::with_capacity(data.indices.len() / 3));
    let (triangles, _) = data.indices.as_chunks::<3>();
    for t in triangles {
        let n = |i: u32| data.vertices.get(i as usize).map_or(Vec3::ZERO, |v| Vec3::from_array(v.normal));
        indices.push(*t);
        normals.push([n(t[0]), n(t[1]), n(t[2])]);
        if let (Some(out), Some(src)) = (&mut uvs, &corner_uvs) {
            // Image rows run downwards while `v` runs upwards.
            out.push(t.map(|i| src.get(i as usize).map_or(Vec2::ZERO, |uv| Vec2::new(uv.x, 1.0 - uv.y))));
        }
    }
    MeshData { positions, indices, normals: Some(normals), uvs }
}

/// Largest linked material file that is read.
const MAX_LINKED_BYTES: u64 = 256 << 20;

/// Read the maps of a linked MaterialCraft material (`.mcmat`) into `builder` and point `material`
/// at them. Anything unreadable is skipped: the plain parameters still render.
fn apply_linked(path: &str, builder: &mut SceneBuilder, material: &mut TraceMaterial) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_LINKED_BYTES {
        return Err(format!("{size} bytes is more than the {MAX_LINKED_BYTES} read for a linked material"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let limits = craft_mcmat::Limits {
        min_version: 1,
        max_version: u32::MAX,
        max_entries: 256,
        max_maps: 64,
        max_manifest_bytes: 4 << 20,
        max_entry_bytes: 128 << 20,
        max_total_bytes: 512 << 20,
    };
    let mut archive = craft_mcmat::Material::open(&bytes, limits).map_err(|e| e.to_string())?;
    let maps = archive.manifest.maps.clone();
    for map in maps {
        let slot = match map.usage.as_str() {
            "baseColor" => &mut material.base_color_texture,
            "roughness" => &mut material.roughness_texture,
            "metallic" => &mut material.metalness_texture,
            "normal" => &mut material.normal_texture,
            "opacity" => &mut material.opacity_texture,
            "height" => &mut material.bump_texture,
            _ => continue,
        };
        let Ok(encoded) = archive.read(&map.file) else { continue };
        let Ok(decoded) = craft_image_codecs::decode(&encoded) else { continue };
        let integer = matches!(decoded.sample_type(), craft_image_codecs::SampleType::U8 | craft_image_codecs::SampleType::U16);
        let rgba = decoded.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::F32);
        let Some(mut values) = rgba.to_f32_samples() else { continue };
        if map.usage == "baseColor" && integer {
            for (i, c) in values.iter_mut().enumerate() {
                if i % 4 != 3 {
                    *c = srgb_to_linear(*c);
                }
            }
        }
        let Ok(image) = Image::from_data(rgba.width(), rgba.height(), Kind::Color, values, &Limits::default()) else { continue };
        let Ok(id) = builder.add_texture(image) else { break };
        *slot = Some(id);
        match map.usage.as_str() {
            // A map replaces the flat value it stands for.
            "baseColor" => material.base_color = Vec3::ONE,
            "roughness" => material.specular_roughness = 1.0,
            "metallic" => material.base_metalness = 1.0,
            "height" => material.bump_scale = 0.02,
            _ => {}
        }
    }
    Ok(())
}

/// Largest panorama file that is read.
const MAX_PANORAMA_BYTES: u64 = 1 << 30;

/// Read a Radiance HDR panorama as an environment image.
pub fn load_panorama(path: &str) -> Result<Image, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.len();
    if size > MAX_PANORAMA_BYTES {
        return Err(format!("{path}: {size} bytes is more than the {MAX_PANORAMA_BYTES} read for a panorama"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    if !craft_hdr::sniff(&bytes) {
        return Err(format!("{path}: not a Radiance HDR (.hdr) picture"));
    }
    let picture = craft_hdr::decode_rgba(&bytes, &craft_hdr::Limits::default()).map_err(|e| format!("{path}: {e}"))?;
    Image::from_data(picture.width, picture.height, Kind::Color, picture.rgba, &Limits::default()).map_err(|e| format!("{path}: {e}"))
}

/// The path tracer's material for a scene material (without its linked maps).
pub fn trace_material(m: &modelcraft_scene::Material) -> TraceMaterial {
    TraceMaterial {
        base_color: Vec3::new(m.base_color[0], m.base_color[1], m.base_color[2]),
        base_metalness: m.metallic,
        specular_roughness: m.roughness,
        specular_ior: m.ior,
        transmission_weight: m.transmission,
        coat_weight: m.coat,
        coat_roughness: m.coat_roughness,
        fuzz_weight: m.sheen,
        emission_color: Vec3::from_array(m.emission),
        emission_luminance: m.emission_strength,
        geometry_opacity: m.base_color[3],
        ..TraceMaterial::default()
    }
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
        .filter(|id| scene.renders(*id) && !scene.is_cage_input(*id))
        .filter(|id| matches!(scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. } | ObjectKind::Instance { .. })))
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
        let mut converted = trace_material(&object.material);
        if options.linked_materials
            && let Some(path) = &object.material.linked
            && let Err(e) = apply_linked(path, &mut b, &mut converted)
        {
            log::warn!("linked material {path}: {e}; rendering its plain parameters");
        }
        let material = b.add_material(converted)?;
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
    match &options.lighting {
        Lighting::Sky(radiance) => b.set_environment(*radiance),
        Lighting::Daylight { elevation, azimuth } => {
            let sky = Sky::from_angles(*elevation, *azimuth);
            b.set_environment_map(sky.bake(256), 0.0);
            if let Some(sun) = sky.sun_light() {
                b.add_light(sun)?;
            }
        }
        Lighting::Image { image, rotation } => b.set_environment_map(image.clone(), *rotation),
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
        s.edit(cube).unwrap().material = Material { base_color: [0.9, 0.1, 0.1, 1.0], metallic: 0.0, roughness: 0.5, ..Material::default() };
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
        let sky = export_scene(
            &cube_scene(),
            &mut eval,
            &Camera::default(),
            &ExportOptions { lighting: Lighting::Sky(Vec3::ONE), ground: false, ..ExportOptions::default() },
        )
        .unwrap();
        assert_eq!((sky.triangle_count(), sky.lights.emissive.len()), (12, 0));
    }

    #[test]
    fn material_layers_uvs_and_linked_maps_reach_the_render() {
        use modelcraft_mesh::{PolyMesh, Polygon};
        let mut s = Scene::new();
        // A unit square with texture coordinates, facing +z.
        let quad = PolyMesh::build(
            vec![Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(-1.0, 1.0, 0.0)],
            vec![Polygon::with_uvs(vec![0, 1, 2, 3], vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)])],
        )
        .unwrap()
        .0;
        let id = s.add("Card", ObjectKind::Mesh { source: Source::Mesh { mesh: Arc::new(quad) }, steps: vec![] }, None).unwrap();

        // A linked material whose base-colour map is red on its left half and green on its right.
        let dir = std::env::temp_dir().join(format!("modelcraft-mcmat-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut px = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                px.extend_from_slice(if x < 4 { &[255u8, 0, 0, 255] } else { &[0u8, 255, 0, 255] });
            }
        }
        let image = craft_image_codecs::Image::from_raw(8, 4, craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::U8, px).unwrap();
        let png = craft_image_codecs::encode(&image, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
        let manifest = craft_mcmat::Manifest {
            format: craft_mcmat::FORMAT.into(),
            version: 1,
            name: "Two tone".into(),
            graph: String::new(),
            size: [8, 4],
            maps: vec![craft_mcmat::MapEntry { usage: "baseColor".into(), file: "maps/base.png".into(), extra: Default::default() }],
            parameters: vec![],
            inputs: vec![],
            presets: vec![],
            source: "source.json".into(),
            extra: Default::default(),
        };
        let archive = craft_mcmat::write(&manifest, &[("maps/base.png".to_string(), png)], b"{}").unwrap();
        let path = dir.join("two-tone.mcmat");
        std::fs::write(&path, archive).unwrap();

        s.edit(id).unwrap().material = Material {
            linked: Some(path.display().to_string()),
            coat: 0.5,
            sheen: 0.25,
            transmission: 0.0,
            ior: 1.33,
            emission_strength: 0.0,
            ..Material::default()
        };
        let mut eval = Evaluator::new();
        let cam = Camera { yaw: 0.0, pitch: 0.0, distance: 6.0, ..Camera::default() };
        let options = ExportOptions { lighting: Lighting::Sky(Vec3::ONE), ground: false, ..ExportOptions::default() };
        let t = export_scene(&s, &mut eval, &cam, &options).unwrap();
        assert_eq!(t.textures.len(), 1);
        let m = &t.materials[0];
        assert_eq!((m.coat_weight, m.fuzz_weight, m.specular_ior, m.base_color_texture), (0.5, 0.25, 1.33, Some(0)));
        let film = render_image(Arc::new(t), RenderSettings { width: 32, height: 32, ..RenderSettings::default() }, 64).unwrap();
        let img = film.mean();
        let (left, right) = (img[16 * 32 + 11], img[16 * 32 + 20]);
        assert!(left[0] > 3.0 * left[1] && right[1] > 3.0 * right[0], "left {left:?} right {right:?}");

        // An emissive material becomes a light; a missing link falls back to the plain colour.
        s.edit(id).unwrap().material = Material { linked: Some(dir.join("gone.mcmat").display().to_string()), emission_strength: 5.0, ..Material::default() };
        let t = export_scene(&s, &mut eval, &cam, &options).unwrap();
        assert_eq!((t.textures.len(), t.lights.emissive.len()), (0, 2));
        // With links turned off nothing is read.
        s.edit(id).unwrap().material = Material { linked: Some(path.display().to_string()), ..Material::default() };
        let t = export_scene(&s, &mut eval, &cam, &ExportOptions { linked_materials: false, ..options.clone() }).unwrap();
        assert!(t.textures.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn daylight_and_image_lighting_export() {
        let mut eval = Evaluator::new();
        let scene = cube_scene();
        let cam = Camera::default();
        let day =
            export_scene(&scene, &mut eval, &cam, &ExportOptions { lighting: Lighting::Daylight { elevation: 0.9, azimuth: 0.5 }, ..ExportOptions::default() })
                .unwrap();
        assert!(day.lights.env_map.is_some());
        assert!(matches!(day.lights.analytic.as_slice(), [craft_render_core::Light::Sun { .. }]));
        let film = render_image(Arc::new(day), RenderSettings { width: 32, height: 32, ..RenderSettings::default() }, 32).unwrap();
        let m = mean_rgb(&film.mean());
        assert!(m[1] > 0.05 && m[1] < 5.0, "{m:?}");
        // After sunset there is no sun light, and the export still works.
        let night = export_scene(
            &scene,
            &mut eval,
            &cam,
            &ExportOptions { lighting: Lighting::Daylight { elevation: -0.5, azimuth: 0.0 }, ..ExportOptions::default() },
        )
        .unwrap();
        assert!(night.lights.analytic.is_empty());
        let image = Image::filled(16, 8, Kind::Color, [0.5, 0.5, 0.5, 1.0]);
        let lit =
            export_scene(&scene, &mut eval, &cam, &ExportOptions { lighting: Lighting::Image { image, rotation: 1.0 }, ..ExportOptions::default() }).unwrap();
        assert!(lit.lights.env_map.is_some() && lit.lights.emissive.is_empty());
    }
}
