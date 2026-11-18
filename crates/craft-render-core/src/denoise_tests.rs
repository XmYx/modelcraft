//! Denoising a film ([`crate::denoise`]).

use std::sync::Arc;

use crate::{DenoiseMode, RenderSettings, relative_mse, render_image, scenes};

fn film(name: &str, size: u32, spp: u32, seed: u32) -> crate::Film {
    let scene = Arc::new(scenes::by_name(name).unwrap().unwrap());
    render_image(scene, RenderSettings { width: size, height: size, seed, ..RenderSettings::default() }, spp).unwrap()
}

#[test]
fn the_two_halves_are_the_even_and_odd_samples() {
    let f = film("cornell", 12, 8, 1);
    let (a, b) = f.halves().unwrap();
    let mean = f.mean();
    for i in 0..mean.len() {
        for k in 0..3 {
            // Equal-sized halves: the image is their average.
            assert!((0.5 * (a[i][k] + b[i][k]) - mean[i][k]).abs() < 1e-4 * mean[i][k].max(1.0));
        }
    }
    // The halves are different renderings of the same image: close, not equal.
    let e = relative_mse(&a, &b);
    assert!(a != b && e > 0.0 && e < 20.0, "relative MSE between the halves {e}");
    // With more samples the halves come together.
    let (a, b) = film("cornell", 12, 256, 1).halves().unwrap();
    assert!(relative_mse(&a, &b) < 0.05 * e);
    assert!(film("cornell", 4, 1, 1).halves().is_none());
    // An odd count gives the even samples one more than the odd ones; each half is still a mean.
    let g = film("cornell", 12, 5, 1);
    let (a, b) = g.halves().unwrap();
    let (ma, mb) = (crate::mean_rgb(&a), crate::mean_rgb(&b));
    assert!((ma[1] - mb[1]).abs() < 0.2 * ma[1]);
}

#[test]
fn both_filters_lower_the_error_of_a_noisy_render() {
    let reference = film("cornell", 48, 2048, 99).mean();
    let noisy = film("cornell", 48, 16, 5);
    let before = relative_mse(&noisy.mean(), &reference);
    let svgf = relative_mse(&noisy.denoise(DenoiseMode::Svgf).unwrap(), &reference);
    let nfor = relative_mse(&noisy.denoise(DenoiseMode::Nfor).unwrap(), &reference);
    eprintln!("cornell 16 spp: {before:.5}, svgf {svgf:.5}, nfor {nfor:.5}");
    assert!(svgf * 1.5 < before, "svgf {svgf} against {before}");
    assert!(nfor * 3.0 < before, "nfor {nfor} against {before}");
    assert_eq!(noisy.denoise(DenoiseMode::Off).unwrap(), noisy.mean());
    // One sample has no halves and no variance; the filters still return a picture.
    let one = film("cornell", 16, 1, 3);
    assert_eq!(one.denoise(DenoiseMode::Nfor).unwrap().len(), 256);
    assert_eq!(one.denoise(DenoiseMode::Svgf).unwrap().len(), 256);
}

#[test]
fn filtering_keeps_the_brightness_of_the_picture() {
    for name in ["cornell", "studio", "night-street"] {
        let noisy = film(name, 32, 16, 7);
        let mean = crate::mean_rgb(&noisy.mean());
        for mode in [DenoiseMode::Svgf, DenoiseMode::Nfor] {
            let out = crate::mean_rgb(&noisy.denoise(mode).unwrap());
            for k in 0..3 {
                assert!((out[k] - mean[k]).abs() < 0.08 * mean[k].max(1e-3), "{name} {mode:?}: {out:?} against {mean:?}");
            }
        }
    }
}

/// Eight frames of 2 samples each with the camera moving `step` per frame; returns the error of the
/// last frame raw, filtered on its own, and filtered with the history carried through the motion image.
fn moving_camera_errors(step: f32) -> (f64, f64, f64, f32) {
    use crate::filters::svgf::{Svgf, SvgfSettings};
    let base = scenes::cornell_box().unwrap();
    let cam = base.camera;
    let scene = Arc::new(base);
    let camera_at = |i: usize| crate::Camera {
        eye: cam.eye + glam::Vec3::new(step * i as f32, 0.0, 0.0),
        target: cam.target + glam::Vec3::new(step * i as f32, 0.0, 0.0),
        ..cam
    };
    let render = |i: usize, spp: u32, seed: u32| {
        let mut s = (*scene).clone();
        s.camera = camera_at(i);
        let settings = RenderSettings { width: 48, height: 48, seed, previous_camera: i.checked_sub(1).map(camera_at), ..RenderSettings::default() };
        render_image(Arc::new(s), settings, spp).unwrap()
    };
    let last = 7;
    let reference = render(last, 2048, 99).mean();
    let mut temporal = Svgf::new(SvgfSettings::default());
    let mut out = Vec::new();
    for i in 0..=last {
        let data = render(i, 2, 40 + i as u32).frame_data();
        out = temporal.filter(&data.frame(), (i > 0).then_some(data.motion.as_slice())).unwrap();
    }
    let final_frame = render(last, 2, 40 + last as u32);
    let alone = final_frame.denoise(DenoiseMode::Svgf).unwrap();
    (relative_mse(&final_frame.mean(), &reference), relative_mse(&alone, &reference), relative_mse(&out, &reference), temporal.history_length(24 * 48 + 24))
}

#[test]
fn svgf_accumulates_frames_through_the_motion_image() {
    // A still camera: every frame adds to the history and the error halves again.
    let (raw, alone, temporal, length) = moving_camera_errors(0.0);
    eprintln!("still camera, 2 spp: raw {raw:.4}, filtered alone {alone:.4}, with history {temporal:.4}");
    assert!(alone < raw && temporal * 1.5 < alone && length > 3.0, "{raw} {alone} {temporal} {length}");
    // A camera moving a few tenths of a pixel per frame: the history is found through the motion
    // vectors (its length grows) and the result is still far better than the raw frame, but
    // resampling the history blurs it, so it is not better than filtering the frame alone.
    let (raw, alone, temporal, length) = moving_camera_errors(0.03);
    eprintln!("moving camera, 2 spp: raw {raw:.4}, filtered alone {alone:.4}, with history {temporal:.4}");
    assert!(temporal < raw * 0.9 && length > 3.0, "{raw} {alone} {temporal} {length}");
}
