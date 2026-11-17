//! Whole-renderer checks against analytic answers.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use glam::{Mat4, Vec3};

use crate::camera::Camera;
use crate::integrator::{IntegratorSettings, Strategy};
use crate::material::Material;
use crate::render::{Film, RenderSettings, Renderer, mean_rgb, mse, render_image};
use crate::scene::{Light, Scene, SceneBuilder};
use crate::scenes;
use crate::shapes::quad;

fn render(scene: Scene, w: u32, h: u32, spp: u32, integrator: IntegratorSettings) -> Film {
    render_image(Arc::new(scene), RenderSettings { width: w, height: h, integrator, seed: 7 }, spp).unwrap()
}

/// Mean over the pixels whose centre lies within `radius` (in pixels) of the image centre.
fn centre_mean(f: &Film, radius: f32) -> [f64; 3] {
    let img = f.mean();
    let mut picked = Vec::new();
    for y in 0..f.height {
        for x in 0..f.width {
            let (dx, dy) = (x as f32 + 0.5 - f.width as f32 / 2.0, y as f32 + 0.5 - f.height as f32 / 2.0);
            if dx * dx + dy * dy < radius * radius {
                picked.push(img[(y * f.width + x) as usize]);
            }
        }
    }
    mean_rgb(&picked)
}

#[test]
fn white_furnace_is_invisible() {
    // A sphere that reflects everything, lit equally from everywhere, is as bright as the light.
    for material in [
        Material::diffuse(Vec3::ONE),
        Material::metal(Vec3::ONE, 0.2),
        Material::metal(Vec3::ONE, 0.8),
        Material { base_color: Vec3::ONE, specular_roughness: 0.5, ..Material::default() },
    ] {
        let label = format!("{material:?}");
        let f = render(scenes::furnace(material).unwrap(), 48, 48, 256, IntegratorSettings::default());
        let img = f.mean();
        let m = mean_rgb(&img);
        assert!((m[0] - 1.0).abs() < 0.02, "{label}: mean {m:?}");
        // No pixel is meaningfully brighter than its surroundings: energy is not created.
        let brightest = img.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(brightest < 1.25, "{label}: brightest {brightest}");
    }
}

#[test]
fn grey_furnace_reflects_its_albedo() {
    let f = render(scenes::furnace(Material::diffuse(Vec3::new(0.5, 0.25, 0.75))).unwrap(), 48, 48, 256, IntegratorSettings::default());
    let m = centre_mean(&f, 8.0);
    for (got, want) in m.iter().zip([0.5, 0.25, 0.75]) {
        assert!((got - want).abs() < 0.01, "{m:?}");
    }
}

fn lit_plane(light: impl FnOnce(&mut SceneBuilder)) -> Scene {
    let mut b = SceneBuilder::new();
    let grey = b.add_material(Material::diffuse(Vec3::splat(0.5))).unwrap();
    b.add_mesh(&quad(Vec3::new(-50.0, 0.0, -50.0), Vec3::Z * 100.0, Vec3::X * 100.0), Mat4::IDENTITY, grey).unwrap();
    light(&mut b);
    b.set_camera(Camera { eye: Vec3::new(0.0, 5.0, 0.0), target: Vec3::ZERO, up: Vec3::Z, fov_y: 2f32.to_radians(), ..Camera::default() });
    b.build()
}

#[test]
fn analytic_lights_match_their_formulas() {
    // Lambert: L = albedo/π · E.
    let albedo_over_pi = 0.5 / std::f64::consts::PI;
    // Point light of intensity 10 W/sr, 2 m above the plane: E = I/d².
    let f = render(lit_plane(|b| b.add_light(Light::Point { position: Vec3::new(0.0, 2.0, 0.0), intensity: Vec3::splat(10.0) }).unwrap()), 16, 16, 16, IntegratorSettings::default());
    let m = centre_mean(&f, 4.0);
    assert!((m[0] - albedo_over_pi * 10.0 / 4.0).abs() < 2e-3, "point {m:?}");
    // Sun at 60° from the normal with irradiance 3 on a facing surface: E = 3·cos 60°.
    let dir = Vec3::new(60f32.to_radians().sin(), 60f32.to_radians().cos(), 0.0);
    let f = render(lit_plane(|b| b.add_light(Light::Directional { direction: dir, irradiance: Vec3::splat(3.0) }).unwrap()), 16, 16, 16, IntegratorSettings::default());
    let m = centre_mean(&f, 4.0);
    assert!((m[0] - albedo_over_pi * 1.5).abs() < 2e-3, "sun {m:?}");
    // Uniform sky of radiance 2: E = π·L, so the plane shows albedo·L.
    let f = render(lit_plane(|b| b.set_environment(Vec3::splat(2.0))), 16, 16, 512, IntegratorSettings::default());
    let m = centre_mean(&f, 4.0);
    assert!((m[0] - 1.0).abs() < 0.01, "sky {m:?}");
}

#[test]
fn area_light_matches_the_form_factor() {
    // A 1×1 square light of radiance 5, 1 m above the point and parallel to the plane. The
    // irradiance below the centre of a rectangle has a closed form (Lambert 1760; see any
    // radiative-transfer text): E = 4·L·(a/√(d²+a²))·atan(b/√(d²+a²)) with a = b = 1/2, d = 1,
    // doubled for the two symmetric terms.
    let (a, b, d, l) = (0.5f64, 0.5f64, 1.0f64, 5.0f64);
    let term = |a: f64, b: f64| a / (d * d + a * a).sqrt() * (b / (d * d + a * a).sqrt()).atan();
    let irradiance = 2.0 * l * (term(a, b) + term(b, a));
    let want = 0.5 / std::f64::consts::PI * irradiance;
    for strategy in [Strategy::Mis, Strategy::LightOnly, Strategy::BsdfOnly] {
        // Seen from the side through a narrow lens, so the light does not hide the point below it.
        let mut sb = SceneBuilder::new();
        let grey = sb.add_material(Material::diffuse(Vec3::splat(0.5))).unwrap();
        sb.add_mesh(&quad(Vec3::new(-50.0, 0.0, -50.0), Vec3::Z * 100.0, Vec3::X * 100.0), Mat4::IDENTITY, grey).unwrap();
        let e = sb.add_material(Material::emitter(Vec3::splat(5.0))).unwrap();
        sb.add_mesh(&quad(Vec3::new(-0.5, 1.0, -0.5), Vec3::X, Vec3::Z), Mat4::IDENTITY, e).unwrap();
        sb.set_camera(Camera { eye: Vec3::new(6.0, 0.5, 0.0), target: Vec3::ZERO, fov_y: 0.3f32.to_radians(), ..Camera::default() });
        let f = render(sb.build(), 12, 12, 4096, IntegratorSettings { max_depth: 1, strategy, ..IntegratorSettings::default() });
        let m = centre_mean(&f, 3.0);
        assert!((m[0] - want).abs() < 0.02 * want, "{strategy:?}: {m:?} want {want}");
    }
}

#[test]
fn the_three_estimators_agree_on_the_cornell_box() {
    let scene = Arc::new(scenes::cornell_box().unwrap());
    let run = |strategy, spp| {
        let s = RenderSettings { width: 40, height: 40, integrator: IntegratorSettings { strategy, max_depth: 5, ..IntegratorSettings::default() }, seed: 3 };
        mean_rgb(&render_image(scene.clone(), s, spp).unwrap().mean())
    };
    let mis = run(Strategy::Mis, 256);
    let light = run(Strategy::LightOnly, 256);
    let bsdf = run(Strategy::BsdfOnly, 2048);
    for k in 0..3 {
        assert!((mis[k] - light[k]).abs() < 0.02 * mis[k], "{mis:?} {light:?}");
        assert!((mis[k] - bsdf[k]).abs() < 0.04 * mis[k], "{mis:?} {bsdf:?}");
    }
    // Colour bleeding: the left third is redder and the right third greener than the middle.
    let f = render_image(scene, RenderSettings { width: 60, height: 60, ..RenderSettings::default() }, 64).unwrap();
    let img = f.mean();
    let col = |x0: u32, x1: u32| mean_rgb(&(40..55).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| img[(y * 60 + x) as usize]).collect::<Vec<_>>());
    let (l, r) = (col(2, 10), col(50, 58));
    assert!(l[0] > l[1] * 1.2 && r[1] > r[0] * 1.2, "{l:?} {r:?}");
}

#[test]
fn rendering_is_deterministic_progressive_and_cancellable() {
    let scene = Arc::new(scenes::cornell_box().unwrap());
    let settings = RenderSettings { width: 24, height: 24, ..RenderSettings::default() };
    let once = render_image(scene.clone(), settings, 8).unwrap();
    let mut r = Renderer::new(scene.clone(), settings).unwrap();
    let go = AtomicBool::new(false);
    assert!(r.render(3, &go) && r.render(5, &go));
    // 3 + 5 samples are the same samples as 8 at once, whatever the thread schedule.
    assert_eq!(r.film().samples, 8);
    assert!(mse(&r.film().mean(), &once.mean()) < 1e-10);
    // More samples means less error against a longer render.
    let reference = render_image(scene.clone(), RenderSettings { seed: 99, ..settings }, 1024).unwrap().mean();
    let (e8, e64) = (mse(&once.mean(), &reference), mse(&render_image(scene.clone(), settings, 64).unwrap().mean(), &reference));
    assert!(e64 < e8 * 0.4, "{e8} {e64}");
    // A cancelled pass leaves the film untouched.
    let before = r.film().clone();
    assert!(!r.render(4, &AtomicBool::new(true)));
    assert_eq!(*r.film(), before);
    assert_eq!(r.film().to_srgb8(0.0).len(), 24 * 24 * 4);
}

#[test]
fn hostile_scenes_render_without_panicking() {
    use crate::scene::SceneError;
    use crate::shapes::MeshData;
    // Empty scene, zero-area and non-finite triangles, absurd parameters.
    let mut b = SceneBuilder::new();
    let m = b.add_material(Material { base_color: Vec3::splat(f32::NAN), specular_roughness: -3.0, specular_ior: f32::INFINITY, emission_luminance: f32::NAN, ..Material::default() }).unwrap();
    let mesh = MeshData {
        positions: vec![Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::splat(f32::NAN), Vec3::X, Vec3::Y],
        indices: vec![[0, 1, 2], [3, 4, 5], [0, 4, 5]],
        normals: Some(vec![[Vec3::ZERO; 3], [Vec3::splat(f32::INFINITY); 3], [Vec3::ZERO; 3]]),
        uvs: None,
    };
    b.add_mesh(&mesh, Mat4::ZERO, m).unwrap();
    b.add_mesh(&mesh, Mat4::from_scale(Vec3::splat(f32::NAN)), 999).unwrap();
    assert_eq!(b.add_mesh(&MeshData { positions: vec![], indices: vec![[0, 1, 9]], ..MeshData::default() }, Mat4::IDENTITY, 0), Err(SceneError::BadIndex(0)));
    b.add_light(Light::Point { position: Vec3::splat(f32::NAN), intensity: Vec3::ONE }).unwrap();
    b.add_light(Light::Directional { direction: Vec3::ZERO, irradiance: Vec3::ONE }).unwrap();
    b.set_environment(Vec3::splat(f32::NAN));
    b.set_camera(Camera { eye: Vec3::new(0.2, 0.2, 3.0), ..Camera::default() });
    let scene = b.build();
    assert!(scene.lights.analytic.is_empty());
    let f = render(scene, 16, 16, 4, IntegratorSettings { max_depth: u32::MAX, ..IntegratorSettings::default() });
    assert!(f.sum.iter().all(|p| p.iter().all(|c| c.is_finite())));
    let empty = render(SceneBuilder::new().build(), 8, 8, 2, IntegratorSettings::default());
    assert!(empty.sum.iter().all(|p| *p == [0.0; 3]));
    assert!(matches!(Film::new(0, 4), Err(SceneError::BadImageSize(0, 4))));
    assert!(Film::new(1 << 20, 4).is_err());
}
