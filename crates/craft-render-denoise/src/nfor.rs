//! Weighted first-order regression for final frames, after Bitterli et al., "Nonlinearly Weighted
//! First-order Regression for Denoising Monte Carlo Renderings" (EGSR 2016).
//!
//! The input is two independent half renderings of the same image (the renderer's even and odd
//! samples). The mean of the two is the image; their difference says how noisy each pixel is.
//! For each pixel the filter fits a plane through the pixel's neighbourhood: the colour of every
//! neighbour is modelled as a constant plus a linear function of the difference in normal, depth
//! and screen position, and the constant is the result. Each neighbour is weighted by how well its
//! 3×3 patch of colours matches the pixel's, with the distance corrected for the noise in both
//! patches (Rousselle et al. 2012), and by whether it lies on the same surface. The weights of one
//! half come from the other half, so the noise they follow is independent of the noise they
//! average; the two filtered halves are then averaged.
//!
//! This is the regression of that paper without its second filtering stage; surfaces are told
//! apart by a soft test on normals and depth instead of by the colour weights alone.

use rayon::prelude::*;

use crate::{DenoiseError, Frame, albedo_floor, clean, dot, unit};

/// Number of regressors besides the constant: the difference in normal (3), in relative depth
/// and in screen position (2).
const FEATURES: usize = 6;
const DIM: usize = FEATURES + 1;

/// Largest window radius.
pub const MAX_RADIUS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NforSettings {
    /// Radius of the window of neighbours, in pixels.
    pub radius: usize,
    /// Radius of the patches that are compared (1 is a 3×3 patch).
    pub patch: usize,
    /// The colour bandwidth: patches closer than this many standard deviations count fully.
    pub bandwidth: f32,
    /// Ridge on the slopes, relative to the total weight.
    pub ridge: f32,
    /// Exponent on the cosine of the angle between normals.
    pub normal_power: f32,
    /// Depth differences are measured against this fraction of the pixel's depth.
    pub depth_tolerance: f32,
    /// The smallest albedo the image is divided by.
    pub albedo_floor: f32,
}

impl Default for NforSettings {
    fn default() -> Self {
        NforSettings { radius: 8, patch: 1, bandwidth: 0.5, ridge: 1e-2, normal_power: 16.0, depth_tolerance: 0.25, albedo_floor: crate::DEFAULT_ALBEDO_FLOOR }
    }
}

/// The means of two independent renderings of the image.
#[derive(Clone, Copy, Debug)]
pub struct Halves<'a> {
    pub a: &'a [[f32; 3]],
    pub b: &'a [[f32; 3]],
}

/// The image with its noise removed. `frame` supplies the feature images; its `color` is not
/// read (the halves are the colour).
pub fn nfor(frame: &Frame<'_>, halves: &Halves<'_>, settings: &NforSettings) -> Result<Vec<[f32; 3]>, DenoiseError> {
    frame.check()?;
    let (w, h) = (frame.width, frame.height);
    let n = w * h;
    if halves.a.len() != n || halves.b.len() != n {
        return Err(DenoiseError::Mismatch("half"));
    }
    let s = NforSettings { radius: settings.radius.clamp(1, MAX_RADIUS), patch: settings.patch.min(4), ..*settings };
    let albedo: Vec<[f32; 3]> = frame.albedo.iter().map(|a| albedo_floor(*a, s.albedo_floor)).collect();
    let demod =
        |img: &[[f32; 3]]| -> Vec<[f32; 3]> { img.iter().zip(&albedo).map(|(c, a)| [clean(c[0]) / a[0], clean(c[1]) / a[1], clean(c[2]) / a[2]]).collect() };
    let (a, b) = (demod(halves.a), demod(halves.b));
    // The variance of one half's pixel: half the squared difference, averaged over 5×5.
    let raw: Vec<[f32; 3]> = a.iter().zip(&b).map(|(p, q)| [0, 1, 2].map(|k| 0.5 * (p[k] - q[k]) * (p[k] - q[k]))).collect();
    let variance = box_blur(w, h, &raw, 2);
    let geometry = Geometry { normal: frame.normal.iter().map(|v| unit(*v)).collect(), depth: frame.depth.iter().map(|d| clean(*d)).collect() };
    let out_a = regress(w, h, &a, &b, &variance, &geometry, &s);
    let out_b = regress(w, h, &b, &a, &variance, &geometry, &s);
    Ok(out_a
        .iter()
        .zip(&out_b)
        .zip(&albedo)
        .map(|((p, q), al)| [0.5 * (p[0] + q[0]) * al[0], 0.5 * (p[1] + q[1]) * al[1], 0.5 * (p[2] + q[2]) * al[2]])
        .collect())
}

struct Geometry {
    normal: Vec<[f32; 3]>,
    depth: Vec<f32>,
}

fn box_blur(w: usize, h: usize, src: &[[f32; 3]], r: i64) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (mut acc, mut cnt) = ([0.0f32; 3], 0.0f32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx >= 0 && yy >= 0 && xx < w as i64 && yy < h as i64 {
                        let p = src[yy as usize * w + xx as usize];
                        for k in 0..3 {
                            acc[k] += p[k];
                        }
                        cnt += 1.0;
                    }
                }
            }
            *o = [acc[0] / cnt, acc[1] / cnt, acc[2] / cnt];
        }
    });
    out
}

/// Filter `target` with weights computed from `guide`.
fn regress(w: usize, h: usize, target: &[[f32; 3]], guide: &[[f32; 3]], variance: &[[f32; 3]], g: &Geometry, s: &NforSettings) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; w * h];
    let r = s.radius as i64;
    let pr = s.patch as i64;
    let inv_r = 1.0 / s.radius as f32;
    let k2 = s.bandwidth.max(1e-3) * s.bandwidth.max(1e-3);
    let patch_n = ((2 * pr + 1) * (2 * pr + 1)) as f32 * 3.0;
    let clamp_px = |x: i64, y: i64| -> usize { (y.clamp(0, h as i64 - 1) as usize) * w + x.clamp(0, w as i64 - 1) as usize };
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let p = y * w + x;
            let (np, zp) = (g.normal[p], g.depth[p]);
            let sky_p = np == [0.0; 3];
            let mut m = [[0.0f64; DIM]; DIM];
            let mut rhs = [[0.0f64; DIM]; 3];
            for dy in -r..=r {
                let yy = y as i64 + dy;
                if yy < 0 || yy >= h as i64 {
                    continue;
                }
                for dx in -r..=r {
                    let xx = x as i64 + dx;
                    if xx < 0 || xx >= w as i64 {
                        continue;
                    }
                    let q = yy as usize * w + xx as usize;
                    let weight = if q == p {
                        1.0
                    } else {
                        let (nq, zq) = (g.normal[q], g.depth[q]);
                        let cos = if sky_p && nq == [0.0; 3] { 1.0 } else { dot(np, nq).max(0.0) };
                        let wn = cos.powf(s.normal_power);
                        let dz = (zq - zp) / (s.depth_tolerance * zp.max(zq) + 1e-3);
                        let wg = wn * (-dz * dz).exp();
                        if wg < 1e-3 {
                            continue;
                        }
                        // The noise-corrected distance of the two patches in the guide image.
                        let mut d2 = 0.0f32;
                        for oy in -pr..=pr {
                            for ox in -pr..=pr {
                                let (ip, iq) = (clamp_px(x as i64 + ox, y as i64 + oy), clamp_px(xx + ox, yy + oy));
                                for k in 0..3 {
                                    let (vp, vq) = (variance[ip][k], variance[iq][k]);
                                    let diff = guide[ip][k] - guide[iq][k];
                                    d2 += (diff * diff - (vp + vp.min(vq))) / (1e-4 + k2 * (vp + vq));
                                }
                            }
                        }
                        let wc = (-(d2 / patch_n).max(0.0)).exp();
                        wg * wc
                    };
                    if weight < 1e-4 {
                        continue;
                    }
                    let nq = g.normal[q];
                    let f: [f64; DIM] = [
                        1.0,
                        f64::from(nq[0] - np[0]),
                        f64::from(nq[1] - np[1]),
                        f64::from(nq[2] - np[2]),
                        f64::from((g.depth[q] - zp) / (zp.max(g.depth[q]) + 1e-3)),
                        f64::from(dx as f32 * inv_r),
                        f64::from(dy as f32 * inv_r),
                    ];
                    let wd = f64::from(weight);
                    let t = target[q];
                    for i in 0..DIM {
                        let wf = wd * f[i];
                        for j in i..DIM {
                            m[i][j] += wf * f[j];
                        }
                        for (c, rc) in rhs.iter_mut().enumerate() {
                            rc[i] += wf * f64::from(t[c]);
                        }
                    }
                }
            }
            let total = m[0][0];
            if total <= 1e-9 {
                *o = target[p];
                continue;
            }
            for i in 1..DIM {
                m[i][i] += f64::from(s.ridge) * total;
            }
            let fallback = [0, 1, 2].map(|c| (rhs[c][0] / total) as f32);
            *o = match solve(&m, &rhs) {
                Some(b0) => [b0[0] as f32, b0[1] as f32, b0[2] as f32],
                None => fallback,
            };
            if !o.iter().all(|v| v.is_finite()) {
                *o = fallback;
            }
        }
    });
    out
}

/// Solve `M β = rhs` for the constant term of each right-hand side, `M` symmetric positive
/// definite with only its upper triangle filled (Cholesky); `None` if it is not.
fn solve(m: &[[f64; DIM]; DIM], rhs: &[[f64; DIM]; 3]) -> Option<[f64; 3]> {
    let mut l = [[0.0f64; DIM]; DIM];
    for i in 0..DIM {
        for j in 0..=i {
            let mut sum = m[j][i];
            for k in 0..j {
                sum -= l[i][k] * l[j][k];
            }
            if i == j {
                if sum <= 1e-12 {
                    return None;
                }
                l[i][i] = sum.sqrt();
            } else {
                l[i][j] = sum / l[j][j];
            }
        }
    }
    let mut out = [0.0f64; 3];
    for (c, r) in rhs.iter().enumerate() {
        let mut y = [0.0f64; DIM];
        for i in 0..DIM {
            let mut sum = r[i];
            for k in 0..i {
                sum -= l[i][k] * y[k];
            }
            y[i] = sum / l[i][i];
        }
        let mut beta = [0.0f64; DIM];
        for i in (0..DIM).rev() {
            let mut sum = y[i];
            for k in i + 1..DIM {
                sum -= l[k][i] * beta[k];
            }
            beta[i] = sum / l[i][i];
        }
        out[c] = beta[0];
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relative_mse;
    use crate::test_images::{Scene, walls};

    fn halves(scene: &Scene, sigma: f32) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 3]>) {
        let (a, _) = scene.noisy(sigma, 21);
        let (b, _) = scene.noisy(sigma, 22);
        let mean = a.iter().zip(&b).map(|(p, q)| [0, 1, 2].map(|k| 0.5 * (p[k] + q[k]))).collect();
        (a, b, mean)
    }

    #[test]
    fn regression_removes_most_of_the_noise_and_keeps_edges_and_texture() {
        let scene = walls(96, 64);
        let (a, b, mean) = halves(&scene, 0.6);
        let frame = scene.frame(&mean, None);
        let out = nfor(&frame, &Halves { a: &a, b: &b }, &NforSettings::default()).unwrap();
        let (before, after) = (relative_mse(&mean, &scene.clean), relative_mse(&out, &scene.clean));
        eprintln!("nfor: {before:.4} -> {after:.4}");
        assert!(after * 6.0 < before, "{before} -> {after}");
        // The corner between the walls and the checker's edges stay sharp.
        let row = 40 * 96;
        for x in [44usize, 46, 49, 51] {
            let (o, c) = (out[row + x], scene.clean[row + x]);
            assert!((o[0] - c[0]).abs() < 0.2 * c[0].max(0.2), "x {x}: {o:?} against {c:?}");
        }
    }

    #[test]
    fn it_does_not_change_a_clean_image_and_keeps_gradients() {
        let scene = walls(48, 32);
        let frame = scene.frame(&scene.clean, None);
        let out = nfor(&frame, &Halves { a: &scene.clean, b: &scene.clean }, &NforSettings::default()).unwrap();
        let err = relative_mse(&out, &scene.clean);
        assert!(err < 2e-4, "a clean image changed by {err}");
    }

    #[test]
    fn a_pure_gradient_is_reproduced_where_a_plain_average_would_blur_it() {
        // A ramp in screen position on one flat surface with noise: the regression's linear
        // term follows it; an average over the window would flatten it to the window's mean.
        let (w, h) = (64usize, 32usize);
        let clean: Vec<[f32; 3]> = (0..w * h).map(|i| [0.2 + 0.8 * (i % w) as f32 / w as f32; 3]).collect();
        let scene = Scene { w, h, clean: clean.clone(), albedo: vec![[1.0; 3]; w * h], normal: vec![[0.0, 0.0, 1.0]; w * h], depth: vec![4.0; w * h] };
        let (a, b, mean) = halves(&scene, 0.3);
        let out = nfor(&scene.frame(&mean, None), &Halves { a: &a, b: &b }, &NforSettings::default()).unwrap();
        let err = relative_mse(&out, &clean);
        assert!(err * 8.0 < relative_mse(&mean, &clean), "{err}");
        // Across the ramp the output keeps the slope (the left quarter is darker than the right).
        let at = |x: usize| out[16 * w + x][0];
        assert!(at(48) - at(12) > 0.35, "{} {}", at(12), at(48));
    }

    #[test]
    fn bad_input_is_an_error_and_odd_pixels_do_not_spread() {
        let scene = walls(16, 16);
        let (a, b, mean) = halves(&scene, 0.3);
        let frame = scene.frame(&mean, None);
        assert!(nfor(&frame, &Halves { a: &a[..10], b: &b }, &NforSettings::default()).is_err());
        let mut bad = a.clone();
        bad[40] = [f32::NAN, f32::INFINITY, -3.0];
        let out = nfor(&frame, &Halves { a: &bad, b: &b }, &NforSettings { radius: 100, patch: 9, ..NforSettings::default() }).unwrap();
        assert!(out.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }
}
