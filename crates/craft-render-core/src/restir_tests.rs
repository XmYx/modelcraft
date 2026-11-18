//! Tests of resampled direct lighting ([`crate::restir`]).

use std::sync::Arc;

use crate::integrator::IntegratorSettings;
use crate::restir::{MAX_PIXELS, RestirMode, RestirSettings};
use crate::{RenderSettings, Scene, mean_rgb, relative_mse, render_image, scenes};

fn settings(size: u32, seed: u32, mode: RestirMode, depth: u32) -> RenderSettings {
    RenderSettings {
        width: size,
        height: size,
        seed,
        integrator: IntegratorSettings { max_depth: depth, restir: RestirSettings { mode, ..RestirSettings::default() }, ..IntegratorSettings::default() },
        ..RenderSettings::default()
    }
}

fn mean_of(scene: &Arc<Scene>, size: u32, seed: u32, mode: RestirMode, depth: u32, spp: u32) -> Vec<[f32; 3]> {
    render_image(scene.clone(), settings(size, seed, mode, depth), spp).unwrap().mean()
}

#[test]
fn the_unbiased_mode_has_the_expected_image_of_plain_sampling() {
    // Every kind of light and surface the benchmark scenes hold: area lights and a tree of
    // hundreds of lamps, a sun and sky through a window, a panorama, coated and glass objects.
    for (name, depth, tolerance) in [("cornell", 3, 0.02), ("night-street", 2, 0.04), ("interior", 2, 0.05), ("studio", 2, 0.03), ("showcase", 2, 0.04)] {
        let scene = Arc::new(scenes::by_name(name).unwrap().unwrap());
        let plain = mean_rgb(&mean_of(&scene, 32, 11, RestirMode::Off, depth, 512));
        let resampled = mean_rgb(&mean_of(&scene, 32, 12, RestirMode::Unbiased, depth, 512));
        for k in 0..3 {
            assert!((plain[k] - resampled[k]).abs() <= tolerance * plain[k].max(1e-3), "{name}: plain {plain:?}, resampled {resampled:?}");
        }
    }
}

#[test]
fn resampling_lowers_the_error_of_a_many_light_scene_at_equal_samples() {
    let scene = Arc::new(scenes::night_street().unwrap());
    let reference = mean_of(&scene, 48, 99, RestirMode::Off, 1, 2048);
    // The error of a heavy-tailed estimator is ruled by its worst seeds, so take many of them.
    let error = |mode: RestirMode| {
        let mut e: Vec<f64> = (0..8)
            .map(|seed| {
                let mut s = settings(48, seed + 1, mode, 1);
                s.integrator.restir.candidates = 16;
                relative_mse(&render_image(scene.clone(), s, 16).unwrap().mean(), &reference)
            })
            .collect();
        e.sort_by(f64::total_cmp);
        (e.iter().sum::<f64>() / 8.0, e[4])
    };
    let (plain, unbiased, viewport) = (error(RestirMode::Off), error(RestirMode::Unbiased), error(RestirMode::Viewport));
    eprintln!("night street, direct light, 16 spp, (mean, median): plain {plain:?}, unbiased {unbiased:?}, viewport {viewport:?}");
    assert!(unbiased.0 * 1.5 < plain.0 && unbiased.1 * 1.5 < plain.1, "plain {plain:?}, unbiased {unbiased:?}");
    assert!(viewport.0 < plain.0, "plain {plain:?}, viewport {viewport:?}");
}

#[test]
fn rendering_with_resampling_is_deterministic_and_progressive() {
    let scene = Arc::new(scenes::night_street().unwrap());
    for mode in [RestirMode::Unbiased, RestirMode::Viewport] {
        let s = settings(20, 3, mode, 3);
        let whole = render_image(scene.clone(), s, 6).unwrap();
        let again = render_image(scene.clone(), s, 6).unwrap();
        assert_eq!(whole.sum, again.sum, "{mode:?} is deterministic");
        let mut r = crate::Renderer::new(scene.clone(), s).unwrap();
        let never = std::sync::atomic::AtomicBool::new(false);
        r.render(2, &never);
        r.render(4, &never);
        assert_eq!(r.film().samples, 6);
        // Equal up to the order in which floating-point sums were added.
        let err = relative_mse(&r.film().mean(), &whole.mean());
        assert!(err < 1e-8, "{mode:?}: two passes equal one, relative MSE {err}");
        // The feature images are filled as they are without resampling.
        assert!(whole.depth.iter().any(|d| *d > 0.0) && whole.albedo.iter().any(|a| a[0] > 0.0));
    }
}

#[test]
fn the_viewport_mode_stays_near_the_answer_and_cancels() {
    let scene = Arc::new(scenes::studio().unwrap());
    let plain = mean_rgb(&mean_of(&scene, 32, 1, RestirMode::Off, 2, 256));
    let viewport = mean_rgb(&mean_of(&scene, 32, 2, RestirMode::Viewport, 2, 256));
    for k in 0..3 {
        // Biased by design near edges, but not by much.
        assert!((plain[k] - viewport[k]).abs() < 0.08 * plain[k], "plain {plain:?}, viewport {viewport:?}");
    }
    let mut r = crate::Renderer::new(scene, settings(16, 1, RestirMode::Viewport, 2)).unwrap();
    assert!(!r.render(4, &std::sync::atomic::AtomicBool::new(true)));
    assert_eq!(r.film().samples, 0);
}

#[test]
fn it_is_not_used_where_it_cannot_apply() {
    let on = RestirSettings { mode: RestirMode::Unbiased, ..RestirSettings::default() };
    let plain = IntegratorSettings::default();
    assert!(on.active(&plain, 100) && !on.active(&plain, MAX_PIXELS + 1));
    assert!(!RestirSettings::default().active(&plain, 100));
    assert!(!on.active(&IntegratorSettings { strategy: crate::Strategy::BsdfOnly, ..plain }, 100));
    assert!(!on.active(&IntegratorSettings { max_depth: 0, ..plain }, 100));
    let wild = RestirSettings { candidates: 0, neighbors: 1000, radius: f32::NAN, history: -1.0, ..on }.clamped();
    assert_eq!((wild.candidates, wild.neighbors), (1, crate::restir::MAX_NEIGHBORS));
    assert!(wild.radius >= 1.0 && wild.history >= 1.0);
    // A mirror-flat first surface and an empty scene render without panicking.
    let empty = Arc::new(crate::SceneBuilder::new().build());
    let f = render_image(empty, settings(8, 1, RestirMode::Unbiased, 3), 2).unwrap();
    assert!(f.sum.iter().all(|p| *p == [0.0; 3]));
}
