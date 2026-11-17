//! Built-in scenes, made in code, for tests and benchmarks. None of them comes from another
//! renderer's sample files.

use glam::{Mat4, Vec3};

use crate::camera::Camera;
use crate::material::Material;
use crate::scene::{Scene, SceneBuilder, SceneError};
use crate::shapes::{cuboid, quad, uv_sphere};

/// The Cornell box layout (Goral et al. 1984): a room with a red and a green wall, a small
/// ceiling light and two blocks. Sizes and colours here are our own round numbers.
pub fn cornell_box() -> Result<Scene, SceneError> {
    let mut b = SceneBuilder::new();
    let white = b.add_material(Material::diffuse(Vec3::splat(0.73)))?;
    let red = b.add_material(Material::diffuse(Vec3::new(0.63, 0.065, 0.05)))?;
    let green = b.add_material(Material::diffuse(Vec3::new(0.14, 0.45, 0.09)))?;
    let light = b.add_material(Material::emitter(Vec3::new(17.0, 12.0, 4.0)))?;
    let id = Mat4::IDENTITY;
    let (x, y, z) = (Vec3::X * 2.0, Vec3::Y * 2.0, Vec3::Z * 2.0);
    let o = Vec3::new(-1.0, 0.0, -1.0);
    b.add_mesh(&quad(o, z, x), id, white)?; // floor
    b.add_mesh(&quad(o + y, x, z), id, white)?; // ceiling
    b.add_mesh(&quad(o, x, y), id, white)?; // back
    b.add_mesh(&quad(o, y, z), id, red)?; // left
    b.add_mesh(&quad(o + x, z, y), id, green)?; // right
    b.add_mesh(&quad(Vec3::new(-0.25, 1.998, -0.25), Vec3::X * 0.5, Vec3::Z * 0.5), id, light)?;
    let block = cuboid(Vec3::new(-0.3, 0.0, -0.3), Vec3::new(0.3, 1.0, 0.3));
    b.add_mesh(&block, Mat4::from_translation(Vec3::new(-0.35, 0.0, -0.3)) * Mat4::from_rotation_y(0.3) * Mat4::from_scale(Vec3::new(1.0, 1.2, 1.0)), white)?;
    b.add_mesh(&block, Mat4::from_translation(Vec3::new(0.35, 0.0, 0.3)) * Mat4::from_rotation_y(-0.3) * Mat4::from_scale(Vec3::new(1.0, 0.6, 1.0)), white)?;
    b.set_camera(Camera {
        eye: Vec3::new(0.0, 1.0, 4.6),
        target: Vec3::new(0.0, 1.0, 0.0),
        fov_y: 36f32.to_radians(),
        focus_distance: 4.6,
        ..Camera::default()
    });
    Ok(b.build())
}

/// A sphere of `material` inside uniform lighting of radiance 1: the furnace test. A surface that
/// reflects everything must look exactly as bright as its surroundings.
pub fn furnace(material: Material) -> Result<Scene, SceneError> {
    let mut b = SceneBuilder::new();
    let m = b.add_material(material)?;
    b.add_mesh(&uv_sphere(Vec3::ZERO, 1.0, 96, 48), Mat4::IDENTITY, m)?;
    b.set_environment(Vec3::ONE);
    b.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 4.0), target: Vec3::ZERO, fov_y: 30f32.to_radians(), ..Camera::default() });
    Ok(b.build())
}

/// A row of spheres from rough to mirror-like metal and matte to glossy plastic on a ground
/// plane, lit by a small bright light and a large dim one: a multiple-importance-sampling
/// stress test (after the scene idea in Veach and Guibas 1995).
pub fn material_row() -> Result<Scene, SceneError> {
    let mut b = SceneBuilder::new();
    let ground = b.add_material(Material { base_color: Vec3::splat(0.5), specular_roughness: 0.6, ..Material::default() })?;
    b.add_mesh(&quad(Vec3::new(-8.0, 0.0, -8.0), Vec3::Z * 16.0, Vec3::X * 16.0), Mat4::IDENTITY, ground)?;
    let sphere = uv_sphere(Vec3::ZERO, 0.5, 64, 32);
    for i in 0..5 {
        let t = i as f32 / 4.0;
        let metal = b.add_material(Material::metal(Vec3::new(0.95, 0.75, 0.4), 0.05 + 0.6 * t))?;
        let plastic =
            b.add_material(Material { base_color: Vec3::new(0.1 + 0.6 * t, 0.25, 0.7 - 0.5 * t), specular_roughness: 0.6 - 0.55 * t, ..Material::default() })?;
        let x = (i as f32 - 2.0) * 1.2;
        b.add_mesh(&sphere, Mat4::from_translation(Vec3::new(x, 0.5, -0.7)), metal)?;
        b.add_mesh(&sphere, Mat4::from_translation(Vec3::new(x, 0.5, 0.7)), plastic)?;
    }
    let small = b.add_material(Material::emitter(Vec3::splat(400.0)))?;
    let large = b.add_material(Material::emitter(Vec3::new(1.2, 1.5, 2.0)))?;
    b.add_mesh(&quad(Vec3::new(-0.1, 4.0, -0.1), Vec3::X * 0.2, Vec3::Z * 0.2), Mat4::IDENTITY, small)?;
    b.add_mesh(&quad(Vec3::new(-5.0, 2.0, -6.0), Vec3::X * 10.0, Vec3::Y * 4.0), Mat4::IDENTITY, large)?;
    b.set_environment(Vec3::splat(0.05));
    b.set_camera(Camera { eye: Vec3::new(0.0, 2.6, 6.5), target: Vec3::new(0.0, 0.4, 0.0), fov_y: 38f32.to_radians(), ..Camera::default() });
    Ok(b.build())
}
