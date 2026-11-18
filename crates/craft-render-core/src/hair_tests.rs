//! Whole-renderer checks of hair: the furnace test, agreement of the three estimators, shadows,
//! determinism and limits.

use std::sync::Arc;

use glam::{Mat4, Vec3, Vec3A};

use crate::camera::Camera;
use crate::hair::HairParams;
use crate::integrator::{IntegratorSettings, Strategy};
use crate::light::Light;
use crate::material::Material;
use crate::render::{Film, RenderSettings, mean_rgb, render_image};
use crate::scene::{Scene, SceneBuilder, SceneError};
use crate::shapes::quad;

/// A unit sphere covered in strands, deterministic.
fn fur_points(strands: usize, length: f32, pps: usize) -> (Vec<Vec3>, Vec<f32>) {
    let mut pts = Vec::with_capacity(strands * pps);
    let h = |i: usize, k: u32| {
        ((i as u32).wrapping_mul(2_654_435_761).wrapping_add(k.wrapping_mul(0x9e37_79b9)).wrapping_mul(0x85eb_ca6b) >> 8) as f32 / 16_777_216.0
    };
    for i in 0..strands {
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / strands as f32;
        let phi = std::f32::consts::TAU * h(i, 1) * 977.0;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let dir = Vec3::new(r * phi.cos(), z, r * phi.sin());
        let curl = Vec3::new(h(i, 2) - 0.5, h(i, 3) - 0.5, h(i, 4) - 0.5) * 0.5;
        for k in 0..pps {
            let t = k as f32 / (pps - 1) as f32;
            pts.push(dir * (1.0 + length * t) + curl * (t * t * length));
        }
    }
    let radii = vec![0.012; pts.len()];
    (pts, radii)
}

fn fur_ball(sigma: f32, beta: f32) -> Result<SceneBuilder, SceneError> {
    let mut b = SceneBuilder::new();
    let (pts, radii) = fur_points(5000, 0.4, 5);
    b.add_curves(&pts, &radii, 5, HairParams { sigma_a: Vec3A::splat(sigma), eta: 1.55, beta_m: beta, beta_n: beta, alpha: 2.0f32.to_radians() })?;
    b.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 5.0), target: Vec3::ZERO, fov_y: 28f32.to_radians(), ..Camera::default() });
    Ok(b)
}

fn render(scene: Scene, w: u32, h: u32, spp: u32, integrator: IntegratorSettings) -> Film {
    render_image(Arc::new(scene), RenderSettings { width: w, height: h, integrator, seed: 11, ..RenderSettings::default() }, spp).unwrap()
}

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

fn deep() -> IntegratorSettings {
    IntegratorSettings { max_depth: 96, roulette_start: 24, ..IntegratorSettings::default() }
}

#[test]
fn white_fur_in_a_white_furnace_is_invisible() {
    // Fibres that absorb nothing scatter every bit of light that reaches them, so a ball of them in
    // uniform lighting is as bright as the lighting: the hair model conserves energy through all
    // the bounces among the fibres.
    for beta in [0.2f32, 0.5, 0.8] {
        let mut b = fur_ball(0.0, beta).unwrap();
        b.set_environment(Vec3::ONE);
        let f = render(b.build(), 40, 40, 256, deep());
        let img = f.mean();
        let m = mean_rgb(&img);
        assert!((m[0] - 1.0).abs() < 0.03, "beta {beta}: mean {m:?}");
        let c = centre_mean(&f, 8.0);
        assert!((c[0] - 1.0).abs() < 0.05, "beta {beta}: centre {c:?}");
        let brightest = img.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(brightest < 1.5, "beta {beta}: energy is not created ({brightest})");
    }
}

#[test]
fn absorbing_fur_is_darker_and_coloured_by_its_absorption() {
    let mut b = SceneBuilder::new();
    let (pts, radii) = fur_points(5000, 0.4, 5);
    // Red hair: absorbs green and blue much more than red.
    b.add_curves(&pts, &radii, 5, HairParams::from_color(Vec3A::new(0.6, 0.15, 0.08), 0.3)).unwrap();
    b.set_environment(Vec3::ONE);
    b.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 5.0), target: Vec3::ZERO, fov_y: 28f32.to_radians(), ..Camera::default() });
    let f = render(b.build(), 40, 40, 128, deep());
    let c = centre_mean(&f, 6.0);
    assert!(c[0] < 0.95 && c[0] > 0.1, "{c:?}");
    assert!(c[0] > c[1] * 1.3 && c[1] > c[2] * 0.9, "red hair is redder than it is green or blue: {c:?}");
}

#[test]
fn the_three_estimators_agree_on_fur() {
    let mean = |strategy| {
        let mut b = fur_ball(0.5, 0.4).unwrap();
        b.set_environment(Vec3::new(1.0, 0.9, 0.7));
        let f = render(b.build(), 32, 32, 512, IntegratorSettings { strategy, ..deep() });
        centre_mean(&f, 8.0)[0]
    };
    let (mis, light, bsdf) = (mean(Strategy::Mis), mean(Strategy::LightOnly), mean(Strategy::BsdfOnly));
    assert!((mis - bsdf).abs() < 0.03 * mis.max(0.1), "MIS {mis} vs BSDF sampling {bsdf}");
    assert!((mis - light).abs() < 0.03 * mis.max(0.1), "MIS {mis} vs light sampling {light}");
}

#[test]
fn hair_casts_shadows_and_translucent_hair_lets_light_through() {
    // A single dense layer of horizontal fibres 0.5 above a floor lit from straight above; the camera
    // looks at the floor under the layer through the gap beneath it.
    let build = |sigma: Option<f32>| {
        let mut b = SceneBuilder::new();
        let floor = b.add_material(Material::diffuse(Vec3::splat(0.8))).unwrap();
        b.add_mesh(&quad(Vec3::new(-2.0, 0.0, -2.0), Vec3::Z * 4.0, Vec3::X * 4.0), Mat4::IDENTITY, floor).unwrap();
        if let Some(sigma) = sigma {
            let mut pts = Vec::new();
            // Covers the whole floor, so every point the camera sees is under it.
            for i in 0..200 {
                for j in 0..200 {
                    let (x, z) = (-2.0 + i as f32 * 0.02, -2.0 + j as f32 * 0.02);
                    pts.extend([Vec3::new(x, 0.5, z - 0.012), Vec3::new(x, 0.5, z + 0.012)]);
                }
            }
            let radii = vec![0.02; pts.len()];
            let params = HairParams { sigma_a: Vec3A::splat(sigma), eta: 1.55, beta_m: 0.3, beta_n: 0.3, alpha: 0.03 };
            b.add_curves(&pts, &radii, 2, params).unwrap();
        }
        b.add_light(Light::Directional { direction: Vec3::new(0.0, 1.0, 0.0), irradiance: Vec3::splat(3.0) }).unwrap();
        b.set_camera(Camera { eye: Vec3::new(0.0, 0.15, 4.0), target: Vec3::ZERO, fov_y: 6f32.to_radians(), ..Camera::default() });
        b.build()
    };
    let at_floor = |sigma| centre_mean(&render(build(sigma), 24, 24, 64, deep()), 3.0)[0];
    let lit = at_floor(None);
    let opaque = at_floor(Some(40.0));
    let glass = at_floor(Some(0.02));
    assert!(lit > 0.5, "the floor is lit: {lit}");
    assert!(opaque < lit * 0.1, "dark fibres in a dense layer shade the floor: {opaque} against {lit}");
    assert!(glass > opaque * 4.0 + 0.05, "pale fibres pass light through (the TT lobe): {glass} against {opaque}");
}

#[test]
fn a_light_behind_fibres_shines_through_them() {
    // Translucent hair: with the light behind a thin, pale fibre layer the layer glows (TT lobe).
    let mut b = SceneBuilder::new();
    let mut pts = Vec::new();
    for i in 0..80 {
        let x = -0.8 + i as f32 * 0.02;
        pts.extend([Vec3::new(x, -0.6, 0.0), Vec3::new(x, 0.6, 0.0)]);
    }
    b.add_curves(&pts, &vec![0.012; pts.len()], 2, HairParams { sigma_a: Vec3A::splat(0.05), eta: 1.55, beta_m: 0.3, beta_n: 0.3, alpha: 0.03 }).unwrap();
    b.set_environment(Vec3::ZERO);
    b.add_light(Light::Directional { direction: Vec3::new(0.0, 0.0, -1.0), irradiance: Vec3::splat(5.0) }).unwrap();
    b.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 4.0), target: Vec3::ZERO, fov_y: 20f32.to_radians(), ..Camera::default() });
    let f = render(b.build(), 32, 32, 64, deep());
    let c = centre_mean(&f, 6.0)[0];
    assert!(c > 0.02, "light travels through the fibres toward the viewer: {c}");
    // Lit from the camera's side the same fibres look different from lit from behind.
    let mut b2 = SceneBuilder::new();
    b2.add_curves(&pts, &vec![0.012; pts.len()], 2, HairParams { sigma_a: Vec3A::splat(0.05), eta: 1.55, beta_m: 0.3, beta_n: 0.3, alpha: 0.03 }).unwrap();
    b2.add_light(Light::Directional { direction: Vec3::new(0.0, 0.0, 1.0), irradiance: Vec3::splat(5.0) }).unwrap();
    b2.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 4.0), target: Vec3::ZERO, fov_y: 20f32.to_radians(), ..Camera::default() });
    let g = render(b2.build(), 32, 32, 64, deep());
    let d = centre_mean(&g, 6.0)[0];
    assert!(d > 0.0 && (d - c).abs() > 0.0, "front and back lighting differ ({d} and {c})");
}

#[test]
fn hair_renders_the_same_every_time_and_stays_finite() {
    let make = || {
        let mut b = fur_ball(0.3, 0.5).unwrap();
        b.set_environment(Vec3::ONE);
        b.build()
    };
    let (a, b) = (render(make(), 24, 24, 16, deep()), render(make(), 24, 24, 16, deep()));
    assert_eq!(a.sum, b.sum, "the same seed gives the same image");
    assert!(a.sum.iter().all(|p| p.iter().all(|v| v.is_finite())));
}

#[test]
fn curve_sets_have_limits_and_hostile_curves_are_ignored() {
    let mut b = SceneBuilder::new();
    for _ in 0..crate::scene::MAX_CURVE_SETS {
        b.add_curves(&[Vec3::ZERO, Vec3::X], &[0.1, 0.1], 2, HairParams::default()).unwrap();
    }
    assert_eq!(b.add_curves(&[Vec3::ZERO, Vec3::X], &[0.1, 0.1], 2, HairParams::default()), Err(SceneError::TooManyCurves));
    let mut b = SceneBuilder::new();
    b.add_curves(
        &[Vec3::NAN, Vec3::X, Vec3::ZERO],
        &[0.1, -1.0, 0.1],
        3,
        HairParams { sigma_a: Vec3A::NAN, eta: f32::NAN, beta_m: f32::NAN, beta_n: 7.0, alpha: f32::NAN },
    )
    .unwrap();
    b.add_curves(&[Vec3::ZERO; 3], &[0.1; 2], 3, HairParams::default()).unwrap();
    b.add_curves(&[], &[], 0, HairParams::default()).unwrap();
    b.set_environment(Vec3::ONE);
    let f = render(b.build(), 8, 8, 4, IntegratorSettings::default());
    assert!(f.sum.iter().all(|p| p.iter().all(|v| v.is_finite())));
}

#[test]
fn guiding_does_not_make_hair_disappear() {
    // The guided estimator cannot trace fibres, so a scene with hair renders with the plain one.
    let mut b = SceneBuilder::new();
    let (pts, radii) = fur_points(5000, 0.4, 5);
    b.add_curves(&pts, &radii, 5, HairParams::from_color(Vec3A::splat(0.1), 0.3)).unwrap();
    b.set_environment(Vec3::ONE);
    b.set_camera(Camera { eye: Vec3::new(0.0, 0.0, 5.0), target: Vec3::ZERO, fov_y: 28f32.to_radians(), ..Camera::default() });
    let scene = b.build();
    let plain = centre_mean(&render(scene.clone(), 32, 32, 32, deep()), 6.0)[0];
    let guided = centre_mean(&render(scene, 32, 32, 32, IntegratorSettings { guiding: true, adaptive_splitting: true, ..deep() }), 6.0)[0];
    assert!(plain < 0.6 && guided < 0.6, "dark fur is visible with and without guiding: {plain} {guided}");
    assert!((plain - guided).abs() < 0.08, "{plain} vs {guided}");
}
