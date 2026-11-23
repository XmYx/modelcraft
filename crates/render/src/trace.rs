//! From the ModelCraft scene graph to the Craft render engine.
//!
//! [`export_scene`] evaluates every visible surface (meshes and smooth cages, not cage inputs,
//! groups or deformers: the same set the viewport draws), bakes world transforms, converts the
//! glTF metallic-roughness material and adds lighting: the scene's own light objects, or a studio
//! rig when it has none. The camera is the scene's render camera object, else the viewport's.
//! The path tracer itself lives in the UI-free `craft-render-*` crates.

use craft_render_core::shapes::quad;
use craft_render_core::sky::Sky;
use craft_render_core::texture::{Image, Kind, Limits, srgb_to_linear};
use craft_render_core::{Camera as TraceCamera, Light as TraceLight, Material as TraceMaterial, MeshData, Scene as TraceScene, SceneBuilder, SceneError};
use glam::{Mat4, Vec2, Vec3};
use modelcraft_mesh::PolyMesh;
use modelcraft_scene::stage::{LightKind, Placement};
use modelcraft_scene::{Evaluator, ObjectId, ObjectKind, Scene};
use std::sync::Arc;

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
    /// Read the maps of linked MaterialCraft materials and dome pictures from disk.
    pub linked_materials: bool,
    /// Width over height of the picture, which a camera object's lens is fitted to.
    pub aspect: f32,
    /// Look through the scene's render camera when it has one; otherwise (or when false) use the
    /// viewport camera given to the export.
    pub scene_camera: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions { lighting: Lighting::Studio, ground: true, linked_materials: true, aspect: 16.0 / 9.0, scene_camera: true }
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

/// Largest picture file a graph's image node reads.
const MAX_GRAPH_IMAGE_BYTES: u64 = 256 << 20;
/// Side of the maps a material graph is evaluated into for a render.
pub const GRAPH_MAP_SIZE: u32 = 512;

/// Reads the pictures a material graph's image nodes name, once each, as linear colour.
#[derive(Default)]
pub struct FileImages {
    cache: std::sync::Mutex<std::collections::HashMap<String, Option<Arc<modelcraft_matgraph::Picture>>>>,
}

impl modelcraft_matgraph::Resolver for FileImages {
    fn picture(&self, path: &str) -> Option<Arc<modelcraft_matgraph::Picture>> {
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        cache
            .entry(path.to_string())
            .or_insert_with(|| {
                let size = std::fs::metadata(path).ok()?.len();
                if size > MAX_GRAPH_IMAGE_BYTES {
                    return None;
                }
                let decoded = craft_image_codecs::decode(&std::fs::read(path).ok()?).ok()?;
                let integer = matches!(decoded.sample_type(), craft_image_codecs::SampleType::U8 | craft_image_codecs::SampleType::U16);
                let rgba = decoded.convert(craft_image_codecs::ChannelLayout::Rgba, craft_image_codecs::SampleType::F32);
                let values = rgba.to_f32_samples()?;
                let rgb = values
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| if integer { [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2])] } else { [p[0], p[1], p[2]] })
                    .collect::<Vec<_>>();
                (rgb.len() == rgba.width() as usize * rgba.height() as usize)
                    .then(|| Arc::new(modelcraft_matgraph::Picture { width: rgba.width(), height: rgba.height(), rgb }))
            })
            .clone()
    }
}

/// Compiled graphs kept between renders, most recent last.
const GRAPH_CACHE_LEN: usize = 16;
static GRAPH_CACHE: std::sync::Mutex<Vec<(u64, Arc<modelcraft_matgraph::Compiled>)>> = std::sync::Mutex::new(Vec::new());

/// What a compiled graph depends on: the graph itself, the map size, whether pictures are read,
/// and the size and time of each picture file it names.
fn graph_key(graph: &modelcraft_matgraph::Graph, size: u32, read_files: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{graph:?}").hash(&mut h);
    (size, read_files).hash(&mut h);
    for n in &graph.nodes {
        if let modelcraft_matgraph::NodeKind::Image { path } = &n.kind
            && read_files
            && let Ok(m) = std::fs::metadata(path)
        {
            (m.len(), m.modified().ok()).hash(&mut h);
        }
    }
    h.finish()
}

/// A graph compiled at `size`, from the cache when nothing it depends on changed, so restarting a
/// render after an unrelated edit does not evaluate every graph again.
pub fn compile_graph(
    graph: &modelcraft_matgraph::Graph,
    size: u32,
    read_files: bool,
) -> Result<Arc<modelcraft_matgraph::Compiled>, modelcraft_matgraph::GraphError> {
    let key = graph_key(graph, size, read_files);
    {
        let mut cache = GRAPH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(i) = cache.iter().position(|(k, _)| *k == key) {
            let hit = cache.remove(i);
            let compiled = hit.1.clone();
            cache.push(hit);
            return Ok(compiled);
        }
    }
    let compiled = Arc::new(if read_files { graph.compile(size, &FileImages::default())? } else { graph.compile(size, &modelcraft_matgraph::NoImages)? });
    let mut cache = GRAPH_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if cache.len() >= GRAPH_CACHE_LEN {
        cache.remove(0);
    }
    cache.push((key, compiled.clone()));
    Ok(compiled)
}

/// An image of a graph's map for the path tracer.
fn graph_image(map: &modelcraft_matgraph::Map) -> Option<Image> {
    let values: Vec<f32> = map.rgb.iter().flat_map(|p| [p[0], p[1], p[2], 1.0]).collect();
    Image::from_data(map.width, map.height, Kind::Color, values, &Limits::default()).ok()
}

/// Evaluate a material graph and point `material` at its maps: a linked input becomes a texture
/// that replaces the flat number it stands for, as a linked material's maps do. Emission has no
/// map in the engine, so a linked emission is its mean colour. A graph that cannot be compiled is
/// reported and the flat material renders.
fn apply_graph(graph: &modelcraft_matgraph::Graph, read_files: bool, builder: &mut SceneBuilder, material: &mut TraceMaterial) -> Result<(), String> {
    let compiled = compile_graph(graph, GRAPH_MAP_SIZE, read_files).map_err(|e| e.to_string())?;
    material.base_color = Vec3::from_array(compiled.flat_base_color);
    material.base_metalness = compiled.flat_metallic;
    material.specular_roughness = compiled.flat_roughness;
    let mut emission = Vec3::from_array(compiled.flat_emission);
    if let Some(id) = compiled.base_color.as_ref().and_then(graph_image).and_then(|i| builder.add_texture(i).ok()) {
        material.base_color_texture = Some(id);
        material.base_color = Vec3::ONE;
    }
    if let Some(id) = compiled.metallic.as_ref().and_then(graph_image).and_then(|i| builder.add_texture(i).ok()) {
        material.metalness_texture = Some(id);
        material.base_metalness = 1.0;
    }
    if let Some(id) = compiled.roughness.as_ref().and_then(graph_image).and_then(|i| builder.add_texture(i).ok()) {
        material.roughness_texture = Some(id);
        material.specular_roughness = 1.0;
    }
    if let Some(id) = compiled.normal.as_ref().and_then(graph_image).and_then(|i| builder.add_texture(i).ok()) {
        material.normal_texture = Some(id);
    }
    if let Some(map) = &compiled.emission {
        let n = map.rgb.len().max(1) as f32;
        emission = map.rgb.iter().fold(Vec3::ZERO, |a, p| a + Vec3::from_array(*p)) / n;
    }
    // The colour is radiance: its largest part is the strength, the rest is the hue.
    let strength = emission.max_element().max(0.0);
    material.emission_color = if strength > 0.0 { emission / strength } else { Vec3::ZERO };
    material.emission_luminance = strength;
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

/// The path tracer's camera for a camera object, in a picture of `aspect` (width / height).
pub fn trace_camera_object(scene: &Scene, id: ObjectId, aspect: f32) -> Option<TraceCamera> {
    let (p, d) = scene.camera_view(id)?;
    Some(TraceCamera {
        eye: p.eye,
        target: p.eye + p.forward * d.focus_distance,
        up: p.up,
        fov_y: d.fov_y(aspect),
        ortho_height: d.orthographic.then_some(d.ortho_height),
        aperture_radius: d.aperture_radius(),
        focus_distance: d.focus_distance,
    })
}

/// The surfaces a render shows, in tree order.
pub fn renderable(scene: &Scene) -> Vec<ObjectId> {
    scene
        .tree_order()
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| scene.renders(*id) && !scene.is_cage_input(*id))
        .filter(|id| {
            matches!(
                scene.get(*id).map(|o| &o.kind),
                Some(
                    ObjectKind::Mesh { .. }
                        | ObjectKind::SmoothCage { .. }
                        | ObjectKind::Array { .. }
                        | ObjectKind::Symmetry { .. }
                        | ObjectKind::Generator { .. }
                        | ObjectKind::SplineGenerator { .. }
                        | ObjectKind::Instance { .. }
                )
            )
        })
        .collect()
}

/// The generators a render shows as instances of one mesh: clone generators and particle systems.
pub fn instanced(scene: &Scene) -> Vec<ObjectId> {
    scene
        .tree_order()
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| scene.visible(*id) && !scene.is_cage_input(*id))
        .filter(|id| matches!(scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Cloner { .. })) || scene.is_particles(*id))
        .collect()
}

/// World-space bounds of everything [`renderable`] and [`instanced`], or `None` for an empty scene.
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
    for id in instanced(scene) {
        let Some(inst) = eval.evaluate(scene, id).instances else { continue };
        if inst.source.vertex_count() == 0 {
            continue;
        }
        let (lo, hi) = inst.source.bounds();
        let w = scene.world_matrix(id);
        for c in &inst.clones.instances {
            let m = w * c.matrix;
            for k in 0..8 {
                let corner = Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z });
                let p = m.transform_point3(corner);
                if p.is_finite() {
                    bounds = Some(bounds.map_or((p, p), |(a, b)| (a.min(p), b.max(p))));
                }
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
        if let Some(graph) = &object.material.graph
            && let Err(e) = apply_graph(graph, options.linked_materials, &mut b, &mut converted)
        {
            log::warn!("material graph of {}: {e}; rendering its flat material", object.name);
        }
        let material = b.add_material(converted)?;
        b.add_mesh(&trace_mesh(&mesh), scene.world_matrix(id), material)?;
    }
    // Clones and particles: the one source mesh placed once per instance.
    for id in instanced(scene) {
        let Some(object) = scene.get(id) else { continue };
        let Some(inst) = eval.evaluate(scene, id).instances else { continue };
        if inst.source.face_count() == 0 || inst.clones.instances.is_empty() {
            continue;
        }
        // The source object's own material, else the generator's.
        let owner = inst.source_object.and_then(|s| scene.get(s)).unwrap_or(object);
        let material = b.add_material(trace_material(&owner.material))?;
        let data = trace_mesh(&inst.source);
        let world = scene.world_matrix(id);
        for c in &inst.clones.instances {
            b.add_mesh(&data, world * c.matrix, material)?;
        }
    }
    crate::hair_trace::add_hair(&mut b, scene, eval)?;
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
    let scene_lights = scene.lights();
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
        // The scene has lights of its own: they are all the light there is.
        Lighting::Studio if !scene_lights.is_empty() => b.set_environment(Vec3::ZERO),
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
    add_scene_lights(&mut b, scene, &scene_lights, options)?;
    let from_object = options.scene_camera.then(|| scene.render_camera()).flatten().and_then(|id| trace_camera_object(scene, id, options.aspect));
    b.set_camera(from_object.unwrap_or_else(|| trace_camera(camera)));
    Ok(b)
}

/// Scale an environment picture by `power`.
fn scaled(image: &Image, power: Vec3) -> Image {
    if power == Vec3::ONE {
        return image.clone();
    }
    image.try_map(Kind::Color, &Limits::default(), |p| [p[0] * power.x, p[1] * power.y, p[2] * power.z, p[3]]).unwrap_or_else(|_| image.clone())
}

/// The scene's light objects as engine lights: points, spots and suns as analytic lights, an area
/// light as a glowing rectangle, a dome as the environment.
fn add_scene_lights(b: &mut SceneBuilder, scene: &Scene, lights: &[ObjectId], options: &ExportOptions) -> Result<(), SceneError> {
    for id in lights {
        let Some(ObjectKind::Light { light }) = scene.get(*id).map(|o| &o.kind) else { continue };
        let light = light.sanitized();
        let power = light.power();
        let world = scene.world_matrix(*id);
        let place = Placement::of(world);
        match light.kind {
            LightKind::Point => b.add_light(TraceLight::Point { position: place.eye, intensity: power })?,
            LightKind::Spot => b.add_light(TraceLight::Spot {
                position: place.eye,
                direction: place.forward,
                intensity: power,
                cone_angle: (light.spread * 0.5).to_radians(),
                blend: light.softness,
            })?,
            LightKind::Sun => b.add_light(TraceLight::Sun { direction: -place.forward, irradiance: power, angular_radius: (light.size * 0.5).to_radians() })?,
            LightKind::Area => {
                if power.max_element() <= 0.0 || light.size <= 0.0 || light.height <= 0.0 {
                    continue;
                }
                // The rectangle faces -Z: `e1 × e2` is its front.
                let (hw, hh) = (light.size * 0.5, light.height * 0.5);
                let material = b.add_material(TraceMaterial::emitter(power))?;
                b.add_mesh(&quad(Vec3::new(-hw, -hh, 0.0), Vec3::Y * (2.0 * hh), Vec3::X * (2.0 * hw)), world, material)?;
            }
            LightKind::Dome => {
                let picture = light.image.as_deref().filter(|_| options.linked_materials).and_then(|path| match load_panorama(path) {
                    Ok(image) => Some(image),
                    Err(e) => {
                        log::warn!("dome light {path}: {e}; using its colour");
                        None
                    }
                });
                match picture {
                    Some(image) => {
                        // The picture turns with the object about the vertical axis.
                        let x = world.transform_vector3(Vec3::X);
                        b.set_environment_map(scaled(&image, Vec3::from_array(light.color) * light.intensity), x.z.atan2(x.x));
                    }
                    None => b.set_environment(power),
                }
            }
        }
    }
    Ok(())
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
    fn particles_and_clones_render_as_instances_of_their_source() {
        use modelcraft_sim::{FrameCache, SimKind, SimObject, particles::ParticleSettings};
        let mut s = Scene::new();
        let emitter = s.add("Emitter", ObjectKind::Group, None).unwrap();
        let mut cache = FrameCache::new(24.0, 1.0, 1.0, 4);
        cache.push(&[Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), modelcraft_sim::DEAD, Vec3::new(0.0, 3.0, 0.0)], usize::MAX).unwrap();
        let mut sim = SimObject::new(SimKind::Particles(ParticleSettings { size: 0.2, size_variation: 0.0, ..Default::default() }));
        sim.cache = Some(Arc::new(cache));
        let id = s.add("Spray", ObjectKind::Sim { sim: Arc::new(sim) }, Some(emitter)).unwrap();
        s.edit(id).unwrap().material = Material { base_color: [0.1, 0.9, 0.1, 1.0], ..Material::default() };
        s.set_frame(1.0);
        let mut eval = Evaluator::new();
        let (lo, hi) = scene_bounds(&s, &mut eval).unwrap();
        assert!(hi.y > 3.1 && lo.y < -0.1 && hi.x > 2.1, "the bounds cover the particles: {lo:?} {hi:?}");
        let options = ExportOptions { lighting: Lighting::Sky(Vec3::ONE), ground: false, ..ExportOptions::default() };
        let t = export_scene(&s, &mut eval, &Camera::default(), &options).unwrap();
        let per = trace_mesh(&modelcraft_scene::sim::particle_sphere()).indices.len();
        assert_eq!(t.triangle_count(), 3 * per, "three live particles, each a sphere");
        assert_eq!(t.materials.len(), 1);
        // Hidden, they are gone.
        s.edit(id).unwrap().visible = false;
        assert_eq!(export_scene(&s, &mut eval, &Camera::default(), &options).unwrap().triangle_count(), 0);
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

    /// A big plane with a grey matte material, a spot light 2 m above its middle shining down and a
    /// camera 5 m above looking straight down through a long lens.
    fn spot_scene(intensity: f32) -> Scene {
        use modelcraft_scene::Transform;
        use modelcraft_scene::stage::{CameraData, LightData, LightKind};
        let mut s = Scene::new();
        let plane = s
            .add(
                "Floor",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Plane { width: 20.0, depth: 20.0, segments: 1 } }, steps: vec![] },
                None,
            )
            .unwrap();
        s.edit(plane).unwrap().material = Material { base_color: [0.5, 0.5, 0.5, 1.0], metallic: 0.0, roughness: 1.0, ..Material::default() };
        let down = glam::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let lamp = s.add("Lamp", ObjectKind::Light { light: LightData { intensity, ..LightData::of(LightKind::Spot) } }, None).unwrap();
        s.edit(lamp).unwrap().transform = Transform { translation: Vec3::new(0.0, 2.0, 0.0), rotation: down, scale: Vec3::ONE };
        let cam = s.add("Camera", ObjectKind::Camera { camera: CameraData { focal_length: 400.0, ..CameraData::default() } }, None).unwrap();
        s.edit(cam).unwrap().transform = Transform { translation: Vec3::new(0.0, 5.0, 0.0), rotation: down, scale: Vec3::ONE };
        s.render.camera = Some(cam);
        s
    }

    fn centre_value(film: &craft_render_core::Film) -> f64 {
        let img = film.mean();
        let (w, h) = (film.width as usize, film.height as usize);
        let mut sum = 0.0f64;
        let mut n = 0;
        for y in h / 2 - 2..h / 2 + 2 {
            for x in w / 2 - 2..w / 2 + 2 {
                sum += f64::from(img[y * w + x][1]);
                n += 1;
            }
        }
        sum / f64::from(n)
    }

    /// One GPU device at a time in this test binary: several opened at once from one process have
    /// crashed inside a vendor driver on a busy machine (as in craft-render-gpu's own tests).
    static GPU_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn a_scene_lit_only_by_a_spot_light_matches_the_formula_on_cpu_and_gpu() {
        let mut eval = Evaluator::new();
        let options = ExportOptions { ground: false, aspect: 1.0, ..ExportOptions::default() };
        let mut render = |intensity: f32, gpu: bool| -> Option<f64> {
            let scene = spot_scene(intensity);
            let t = export_scene(&scene, &mut eval, &Camera::default(), &options).unwrap();
            // Only the spot lights the scene: no studio key, no sky.
            assert_eq!((t.lights.emissive.len(), t.lights.analytic.len()), (0, 1), "the scene's own light replaces the studio rig");
            assert!(matches!(t.lights.analytic[0], craft_render_core::Light::Spot { .. }));
            let settings = RenderSettings { width: 16, height: 16, ..RenderSettings::default() };
            if gpu {
                let _turn = GPU_TURN.lock().unwrap_or_else(|p| p.into_inner());
                let ctx = craft_render_gpu::GpuContext::new()?;
                let mut r = craft_render_gpu::GpuRenderer::new(&ctx, &t, settings).unwrap();
                r.render(&ctx, 32).unwrap();
                return Some(centre_value(&r.film(&ctx).unwrap()));
            }
            Some(centre_value(&render_image(Arc::new(t), settings, 64).unwrap()))
        };
        // Lambert: L = albedo/π · I·cosθ/d², straight below the lamp: 0.5/π · 100/2² = 3.98 (the
        // rough dielectric lobe adds a little).
        let want = 0.5 / std::f64::consts::PI * 100.0 / 4.0;
        let cpu = render(100.0, false).unwrap();
        assert!((cpu - want).abs() < 0.1 * want, "cpu {cpu} want {want}");
        let double = render(200.0, false).unwrap();
        assert!((double / cpu - 2.0).abs() < 0.05, "twice the intensity, twice the light: {double} vs {cpu}");
        match render(100.0, true) {
            Some(gpu) => assert!((gpu - cpu).abs() < 0.03 * cpu, "gpu {gpu} cpu {cpu}"),
            None => eprintln!("skipped the GPU half: no adapter on this machine; the CPU reference covers the formula"),
        }
    }

    #[test]
    fn every_light_kind_reaches_the_renderer_and_a_camera_object_sets_the_view() {
        use modelcraft_scene::Transform;
        use modelcraft_scene::stage::{CameraData, LightData, LightKind};
        let mut s = Scene::new();
        for kind in LightKind::ALL {
            let id = s.add(kind.title(), ObjectKind::Light { light: LightData::of(kind) }, None).unwrap();
            s.edit(id).unwrap().transform = Transform { translation: Vec3::new(1.0, 2.0, 3.0), ..Transform::default() };
        }
        let mut eval = Evaluator::new();
        let options = ExportOptions { ground: false, ..ExportOptions::default() };
        let t = export_scene(&s, &mut eval, &Camera::default(), &options).unwrap();
        use craft_render_core::Light as L;
        assert_eq!(t.lights.analytic.len(), 3, "point, spot and sun");
        assert!(matches!(t.lights.analytic[0], L::Point { position, .. } if (position - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-5));
        // A light faces -Z, so the sun's direction towards the light is +Z.
        assert!(t.lights.analytic.iter().any(|l| matches!(l, L::Sun { direction, .. } if (*direction - Vec3::Z).length() < 1e-5)));
        assert!(t.lights.analytic.iter().any(|l| matches!(l, L::Spot { direction, .. } if (*direction - Vec3::NEG_Z).length() < 1e-5)));
        assert_eq!(t.lights.emissive.len(), 2, "the area light is a glowing rectangle");
        assert_eq!(t.lights.environment, Vec3::ONE, "the dome is the sky");
        // Hidden lights add nothing.
        for id in s.lights() {
            s.edit(id).unwrap().visible = false;
        }
        let hidden = export_scene(&s, &mut eval, &Camera::default(), &options).unwrap();
        assert_eq!((hidden.lights.analytic.len(), hidden.lights.emissive.len()), (0, 2), "hidden: only the studio key remains");

        // The render camera sets eye, direction, lens and blur; the viewport camera is ignored.
        let cam = s
            .add("Cam", ObjectKind::Camera { camera: CameraData { focal_length: 100.0, f_stop: 2.0, focus_distance: 3.0, ..CameraData::default() } }, None)
            .unwrap();
        s.edit(cam).unwrap().transform = Transform { translation: Vec3::new(0.0, 1.0, 4.0), ..Transform::default() };
        let viewport = Camera::default();
        let plain = export_scene(&s, &mut eval, &viewport, &ExportOptions { ground: false, ..ExportOptions::default() }).unwrap();
        assert_eq!(plain.camera.eye, viewport.eye(), "no render camera chosen: the viewport's");
        s.render.camera = Some(cam);
        let t = export_scene(&s, &mut eval, &viewport, &ExportOptions { ground: false, aspect: 1.5, ..ExportOptions::default() }).unwrap();
        assert!((t.camera.eye - Vec3::new(0.0, 1.0, 4.0)).length() < 1e-5);
        assert!((t.camera.target - Vec3::new(0.0, 1.0, 1.0)).length() < 1e-5, "{:?}", t.camera.target);
        assert!((t.camera.fov_y - 2.0 * (0.18f32 / 1.5).atan()).abs() < 1e-5, "the 36 mm film is fitted across a 3:2 picture");
        assert!((t.camera.aperture_radius - 0.025).abs() < 1e-6 && t.camera.focus_distance == 3.0);
        // Switched off (a viewport render), the viewport camera is used again.
        let off = export_scene(&s, &mut eval, &viewport, &ExportOptions { ground: false, scene_camera: false, ..ExportOptions::default() }).unwrap();
        assert_eq!(off.camera.eye, viewport.eye());
    }

    #[test]
    fn a_dome_light_turns_with_its_object_and_lights_by_its_colour() {
        use modelcraft_scene::stage::{LightData, LightKind};
        let mut s = Scene::new();
        let dome =
            s.add("Sky", ObjectKind::Light { light: LightData { color: [0.0, 0.0, 1.0], intensity: 2.0, ..LightData::of(LightKind::Dome) } }, None).unwrap();
        let mut eval = Evaluator::new();
        let options = ExportOptions { ground: false, ..ExportOptions::default() };
        let t = export_scene(&s, &mut eval, &Camera::default(), &options).unwrap();
        assert_eq!(t.lights.environment, Vec3::new(0.0, 0.0, 2.0));
        // A picture that cannot be read falls back to the colour.
        if let ObjectKind::Light { light } = &mut s.edit(dome).unwrap().kind {
            light.image = Some("/nonexistent/sky.hdr".into());
        }
        let t = export_scene(&s, &mut eval, &Camera::default(), &options).unwrap();
        assert!(t.lights.env_map.is_none() && t.lights.environment == Vec3::new(0.0, 0.0, 2.0));
    }

    /// A glossy metal sphere whose roughness comes from a noise node of a material graph.
    fn graph_sphere(with_noise: bool) -> Scene {
        use modelcraft_matgraph::{Graph, NodeKind};
        let mut s = Scene::new();
        let id = s
            .add(
                "Ball",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 } }, steps: vec![] },
                None,
            )
            .unwrap();
        let mut graph = Graph::new();
        let surface = graph.add(NodeKind::Surface { base_color: [0.9, 0.7, 0.3], metallic: 1.0, roughness: 0.15, emission: [0.0; 3] }).unwrap();
        if with_noise {
            let noise = graph.add(NodeKind::Noise { scale: 6.0, detail: 3, roughness: 0.6, seed: 2 }).unwrap();
            graph.link(surface, 2, Some(noise)).unwrap();
        }
        s.edit(id).unwrap().material = Material { graph: Some(Arc::new(graph)), ..Material::default() };
        s
    }

    #[test]
    fn a_material_graph_with_noise_roughness_renders_the_same_on_cpu_and_gpu() {
        let mut eval = Evaluator::new();
        let options = ExportOptions { aspect: 1.0, ..ExportOptions::default() };
        let mut cam = Camera::default();
        cam.frame(Vec3::splat(-1.0), Vec3::splat(1.0));
        let settings = RenderSettings { width: 32, height: 32, ..RenderSettings::default() };
        let mut film = |with_noise: bool, gpu: bool| -> Option<craft_render_core::Film> {
            let t = export_scene(&graph_sphere(with_noise), &mut eval, &cam, &options).unwrap();
            assert_eq!(t.textures.len(), usize::from(with_noise), "the noise node becomes one map");
            if gpu {
                let _turn = GPU_TURN.lock().unwrap_or_else(|p| p.into_inner());
                let ctx = craft_render_gpu::GpuContext::new()?;
                let mut r = craft_render_gpu::GpuRenderer::new(&ctx, &t, settings).unwrap();
                r.render(&ctx, 32).unwrap();
                return Some(r.film(&ctx).unwrap());
            }
            Some(render_image(Arc::new(t), settings, 32).unwrap())
        };
        let flat = film(false, false).unwrap();
        let noisy = film(true, false).unwrap();
        let different = craft_render_core::relative_mse(&noisy.mean(), &flat.mean());
        assert!(different > 1e-4, "the noise changes the picture: {different}");
        match film(true, true) {
            Some(gpu) => {
                let err = craft_render_core::relative_mse(&gpu.mean(), &noisy.mean());
                assert!(err < 2e-3, "GPU and CPU agree on a graph-driven material: relative MSE {err}");
            }
            None => eprintln!("skipped the GPU half: no adapter on this machine"),
        }
    }

    #[test]
    fn a_compiled_graph_is_reused_until_the_graph_changes() {
        use modelcraft_matgraph::{Graph, NodeKind};
        let mut g = Graph::with_surface();
        let surface = g.surface().map(|n| n.id).unwrap();
        let noise = g.add(NodeKind::Noise { scale: 5.0, detail: 2, roughness: 0.5, seed: 31 }).unwrap();
        g.link(surface, 2, Some(noise)).unwrap();
        let first = compile_graph(&g, 32, false).unwrap();
        assert!(Arc::ptr_eq(&first, &compile_graph(&g, 32, false).unwrap()), "an unchanged graph is not evaluated again");
        g.set(noise, NodeKind::Noise { scale: 5.0, detail: 2, roughness: 0.5, seed: 32 }).unwrap();
        let second = compile_graph(&g, 32, false).unwrap();
        assert!(!Arc::ptr_eq(&first, &second) && first.roughness != second.roughness, "an edit compiles again");
        assert!(!Arc::ptr_eq(&second, &compile_graph(&g, 16, false).unwrap()), "another size is another entry");
    }

    #[test]
    fn a_broken_graph_renders_the_flat_material_and_says_so() {
        use modelcraft_matgraph::{Graph, NodeKind};
        let mut s = graph_sphere(false);
        let id = s.objects()[0].id;
        // A graph without a surface node cannot compile.
        let mut broken = Graph::new();
        broken.add(NodeKind::Value { value: 1.0 }).unwrap();
        s.edit(id).unwrap().material.graph = Some(Arc::new(broken));
        let mut eval = Evaluator::new();
        let t = export_scene(&s, &mut eval, &Camera::default(), &ExportOptions::default()).unwrap();
        assert_eq!(t.textures.len(), 0);
        assert!(t.triangle_count() > 0, "the object still renders");
    }
}
