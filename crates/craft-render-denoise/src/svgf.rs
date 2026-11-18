//! The spatiotemporal variance-guided filter (Schied et al., HPG 2017) for the interactive view.
//!
//! One call per frame. The frame is divided by its albedo; the result of the previous call is
//! carried over through the motion vectors (a pixel's history is rejected where the surface it saw
//! has a different depth or normal) and blended in with a weight that shrinks as the history
//! lengthens; the variance of the blend is tracked; then an edge-avoiding à-trous wavelet filter of
//! several widening passes smooths the result, its luminance edge-stopping function scaled by that
//! variance (so a noisy pixel borrows from far away and a clean one is left alone). The first
//! pass's result is the history for the next frame. A frame without a variance image, or with
//! too few samples to have one, gets a variance estimate from its own neighbourhood.

use rayon::prelude::*;

use crate::{DenoiseError, Frame, albedo_floor, clean, dot, luminance, unit};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SvgfSettings {
    /// À-trous passes (the widths are 1, 2, 4, 8, …).
    pub iterations: u32,
    /// How strongly depth differences stop the filter (scaled by the depth gradient).
    pub sigma_depth: f32,
    /// Exponent on the cosine of the angle between normals.
    pub sigma_normal: f32,
    /// How strongly luminance differences stop the filter, in standard deviations.
    pub sigma_luminance: f32,
    /// Least weight of the current frame in the blend with the history.
    pub alpha: f32,
    /// Longest history, in frames.
    pub max_history: f32,
    /// The frame is not divided by the albedo where its luminance is below this (0 divides
    /// everywhere, above 1 nowhere).
    pub albedo_floor: f32,
    /// A history sample is valid when its depth differs by less than this fraction.
    pub reuse_depth: f32,
    /// …and the cosine of the angle between the normals is at least this.
    pub reuse_normal: f32,
}

impl Default for SvgfSettings {
    fn default() -> Self {
        SvgfSettings {
            iterations: 5,
            sigma_depth: 1.0,
            sigma_normal: 128.0,
            sigma_luminance: 4.0,
            alpha: 0.2,
            max_history: 32.0,
            albedo_floor: crate::DEFAULT_ALBEDO_FLOOR,
            reuse_depth: 0.1,
            reuse_normal: 0.9,
        }
    }
}

/// What carries over from frame to frame.
#[derive(Clone, Debug)]
struct History {
    width: usize,
    height: usize,
    /// Demodulated colour after the first filter pass.
    color: Vec<[f32; 3]>,
    /// Its variance (of the luminance).
    variance: Vec<f32>,
    /// Frames each pixel has been accumulated over.
    length: Vec<f32>,
    depth: Vec<f32>,
    normal: Vec<[f32; 3]>,
}

pub struct Svgf {
    pub settings: SvgfSettings,
    history: Option<History>,
}

impl Svgf {
    pub fn new(settings: SvgfSettings) -> Svgf {
        Svgf { settings, history: None }
    }

    /// Forget the history (a cut, or a scene edit).
    pub fn reset(&mut self) {
        self.history = None;
    }

    /// Frames the history of pixel `i` spans, or zero.
    pub fn history_length(&self, i: usize) -> f32 {
        self.history.as_ref().and_then(|h| h.length.get(i)).copied().unwrap_or(0.0)
    }

    /// Filter one frame. `motion` is the screen-space motion in pixels from the previous frame
    /// to this one (what a renderer's motion image holds: this position minus the previous);
    /// without it nothing moves.
    pub fn filter(&mut self, frame: &Frame<'_>, motion: Option<&[[f32; 2]]>) -> Result<Vec<[f32; 3]>, DenoiseError> {
        frame.check()?;
        let (w, h) = (frame.width, frame.height);
        let n = w * h;
        if motion.is_some_and(|m| m.len() != n) {
            return Err(DenoiseError::Mismatch("motion"));
        }
        let s = self.settings;
        let albedo: Vec<[f32; 3]> = frame.albedo.iter().map(|a| albedo_floor(*a, s.albedo_floor)).collect();
        let normal: Vec<[f32; 3]> = frame.normal.iter().map(|v| unit(*v)).collect();
        let depth: Vec<f32> = frame.depth.iter().map(|d| clean(*d)).collect();
        let illum: Vec<[f32; 3]> = frame.color.iter().zip(&albedo).map(|(c, a)| [clean(c[0]) / a[0], clean(c[1]) / a[1], clean(c[2]) / a[2]]).collect();
        let has_variance = frame.variance.is_some_and(|v| v.iter().any(|x| *x > 0.0));
        let lum: Vec<f32> = illum.iter().map(|c| luminance(*c)).collect();
        let var_in: Vec<f32> = if has_variance {
            frame.variance.unwrap_or(&[]).iter().zip(&albedo).map(|(v, a)| clean(*v) / luminance(*a).powi(2).max(1e-6)).collect()
        } else {
            spatial_variance(w, h, &lum, &depth, &normal)
        };

        // Temporal accumulation.
        let mut color = illum.clone();
        let mut variance = var_in.clone();
        let mut length = vec![1.0f32; n];
        if let Some(hist) = self.history.as_ref().filter(|p| p.width == w && p.height == h) {
            let rows: Vec<(Vec<[f32; 3]>, Vec<f32>, Vec<f32>)> = (0..h)
                .into_par_iter()
                .map(|y| {
                    let mut rc = Vec::with_capacity(w);
                    let mut rv = Vec::with_capacity(w);
                    let mut rl = Vec::with_capacity(w);
                    for x in 0..w {
                        let i = y * w + x;
                        let m = motion.and_then(|m| m.get(i)).copied().unwrap_or([0.0; 2]);
                        let (px, py) = (x as f32 + 0.5 - clean_signed(m[0]), y as f32 + 0.5 - clean_signed(m[1]));
                        let (c, v, l) = match reproject(hist, px, py, depth[i], normal[i], &s) {
                            Some((hc, hv, hl)) => {
                                let alpha = (1.0 / (hl + 1.0)).max(s.alpha);
                                let c = [0, 1, 2].map(|k| hc[k] + (illum[i][k] - hc[k]) * alpha);
                                let v = (1.0 - alpha) * (1.0 - alpha) * hv + alpha * alpha * var_in[i];
                                (c, v, (hl + 1.0).min(s.max_history))
                            }
                            None => (illum[i], var_in[i], 1.0),
                        };
                        rc.push(c);
                        rv.push(v);
                        rl.push(l);
                    }
                    (rc, rv, rl)
                })
                .collect();
            for (y, (rc, rv, rl)) in rows.into_iter().enumerate() {
                color[y * w..(y + 1) * w].copy_from_slice(&rc);
                variance[y * w..(y + 1) * w].copy_from_slice(&rv);
                length[y * w..(y + 1) * w].copy_from_slice(&rl);
            }
        }

        // À-trous passes.
        let grad = depth_gradient(w, h, &depth);
        let (mut cur_c, mut cur_v) = (color, variance);
        let mut saved: Option<(Vec<[f32; 3]>, Vec<f32>)> = None;
        for pass in 0..s.iterations.clamp(1, 8) {
            let (next_c, next_v) = atrous(w, h, &cur_c, &cur_v, &depth, &normal, &grad, 1usize << pass, &s);
            cur_c = next_c;
            cur_v = next_v;
            if pass == 0 {
                saved = Some((cur_c.clone(), cur_v.clone()));
            }
        }
        if let Some((hc, hv)) = saved {
            self.history = Some(History { width: w, height: h, color: hc, variance: hv, length, depth, normal });
        }
        Ok(cur_c.iter().zip(&albedo).map(|(c, a)| [c[0] * a[0], c[1] * a[1], c[2] * a[2]]).collect())
    }
}

#[inline]
fn clean_signed(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1.0e5, 1.0e5) } else { 0.0 }
}

/// The history at the (continuous) pixel position `(px, py)`: colour, variance and length,
/// bilinearly blended over the taps that saw the same surface; `None` when none did.
fn reproject(hist: &History, px: f32, py: f32, depth: f32, normal: [f32; 3], s: &SvgfSettings) -> Option<([f32; 3], f32, f32)> {
    let (fx, fy) = (px - 0.5, py - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (mut sum_w, mut c, mut v, mut l) = (0.0f32, [0.0f32; 3], 0.0f32, 0.0f32);
    for (dy, wy) in [(0i64, 1.0 - ty), (1, ty)] {
        for (dx, wx) in [(0i64, 1.0 - tx), (1, tx)] {
            let (x, y) = (x0 as i64 + dx, y0 as i64 + dy);
            if x < 0 || y < 0 || x >= hist.width as i64 || y >= hist.height as i64 {
                continue;
            }
            let j = y as usize * hist.width + x as usize;
            let (hd, hn) = (hist.depth.get(j).copied().unwrap_or(0.0), hist.normal.get(j).copied().unwrap_or([0.0; 3]));
            let same_depth = (hd - depth).abs() <= s.reuse_depth * hd.max(depth).max(1e-3);
            let same_normal = if depth == 0.0 && hd == 0.0 { true } else { dot(hn, normal) >= s.reuse_normal };
            if !(same_depth && same_normal) {
                continue;
            }
            let w = wx * wy;
            if w <= 0.0 {
                continue;
            }
            sum_w += w;
            let hc = hist.color.get(j).copied().unwrap_or([0.0; 3]);
            for k in 0..3 {
                c[k] += hc[k] * w;
            }
            v += hist.variance.get(j).copied().unwrap_or(0.0) * w;
            l += hist.length.get(j).copied().unwrap_or(0.0) * w;
        }
    }
    (sum_w > 0.01).then(|| ([c[0] / sum_w, c[1] / sum_w, c[2] / sum_w], v / sum_w, l / sum_w))
}

/// The variance of luminance in a 7×7 window of similar surfaces: the estimate for a frame that
/// has none of its own.
fn spatial_variance(w: usize, h: usize, lum: &[f32], depth: &[f32], normal: &[[f32; 3]]) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (mut sw, mut m1, mut m2) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -3i64..=3 {
                for dx in -3i64..=3 {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx < 0 || yy < 0 || xx >= w as i64 || yy >= h as i64 {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    let similar = (depth[j] - depth[i]).abs() <= 0.1 * depth[i].max(depth[j]).max(1e-3) && dot(normal[i], normal[j]) >= 0.8;
                    if similar {
                        sw += 1.0;
                        m1 += lum[j];
                        m2 += lum[j] * lum[j];
                    }
                }
            }
            if sw > 1.0 {
                let mean = m1 / sw;
                *o = ((m2 / sw - mean * mean) * sw / (sw - 1.0)).max(0.0);
            }
        }
    });
    out
}

/// Screen-space gradient of depth (central differences, one-sided at the borders).
fn depth_gradient(w: usize, h: usize, depth: &[f32]) -> Vec<[f32; 2]> {
    let at = |x: usize, y: usize| depth[y * w + x];
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let dzdx = at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y);
            let dzdy = at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1));
            [dzdx * 0.5, dzdy * 0.5]
        })
        .collect()
}

const KERNEL: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];

/// One à-trous pass of width `step`: returns the filtered colour and its variance.
#[allow(clippy::too_many_arguments)]
fn atrous(
    w: usize,
    h: usize,
    color: &[[f32; 3]],
    variance: &[f32],
    depth: &[f32],
    normal: &[[f32; 3]],
    grad: &[[f32; 2]],
    step: usize,
    s: &SvgfSettings,
) -> (Vec<[f32; 3]>, Vec<f32>) {
    // The variance of each pixel as the median over 3×3, so that one noisy neighbour (the edge of a
    // light, say) does not switch the luminance edge-stopping off for the pixels beside it.
    let smooth: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let mut v = [0.0f32; 9];
            let mut n = 0;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (xx, yy) = ((x as i64 + dx).clamp(0, w as i64 - 1), (y as i64 + dy).clamp(0, h as i64 - 1));
                    v[n] = variance[yy as usize * w + xx as usize];
                    n += 1;
                }
            }
            v.sort_by(f32::total_cmp);
            v[4]
        })
        .collect();
    let lum: Vec<f32> = color.iter().map(|c| luminance(*c)).collect();
    let mut out_c = vec![[0.0f32; 3]; w * h];
    let mut out_v = vec![0.0f32; w * h];
    out_c.par_chunks_mut(w).zip(out_v.par_chunks_mut(w)).enumerate().for_each(|(y, (rc, rv))| {
        for x in 0..w {
            let i = y * w + x;
            let sigma_l = s.sigma_luminance * smooth[i].max(0.0).sqrt() + 1e-4;
            let (mut cs, mut vs, mut ws) = ([0.0f32; 3], 0.0f32, 0.0f32);
            for (ky, hy) in KERNEL.iter().enumerate() {
                let oy = (ky as i64 - 2) * step as i64;
                let yy = y as i64 + oy;
                if yy < 0 || yy >= h as i64 {
                    continue;
                }
                for (kx, hx) in KERNEL.iter().enumerate() {
                    let ox = (kx as i64 - 2) * step as i64;
                    let xx = x as i64 + ox;
                    if xx < 0 || xx >= w as i64 {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    let weight = if j == i {
                        hx * hy
                    } else {
                        let dz = (depth[i] - depth[j]).abs();
                        let reach = s.sigma_depth * (grad[i][0] * ox as f32 + grad[i][1] * oy as f32).abs() + 1e-3;
                        let wz = (-dz / reach).exp();
                        let wn = dot(normal[i], normal[j]).max(0.0).powf(s.sigma_normal);
                        let wl = (-(lum[i] - lum[j]).abs() / sigma_l).exp();
                        hx * hy * wz * wn * wl
                    };
                    if weight > 0.0 {
                        ws += weight;
                        for k in 0..3 {
                            cs[k] += color[j][k] * weight;
                        }
                        vs += variance[j] * weight * weight;
                    }
                }
            }
            if ws > 1e-8 {
                rc[x] = [cs[0] / ws, cs[1] / ws, cs[2] / ws];
                rv[x] = vs / (ws * ws);
            } else {
                rc[x] = color[i];
                rv[x] = variance[i];
            }
        }
    });
    (out_c, out_v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relative_mse;
    use crate::test_images::walls;

    #[test]
    fn a_single_noisy_frame_gets_much_closer_to_the_truth_without_losing_its_edges() {
        let scene = walls(96, 64);
        let (noisy, var) = scene.noisy(0.5, 3);
        let mut f = Svgf::new(SvgfSettings::default());
        let out = f.filter(&scene.frame(&noisy, Some(&var)), None).unwrap();
        let (before, after) = (relative_mse(&noisy, &scene.clean), relative_mse(&out, &scene.clean));
        eprintln!("svgf single frame: {before:.4} -> {after:.4}");
        assert!(after * 4.0 < before, "{before} -> {after}");
        // The wall edge stays sharp: pixels either side of it keep their sides' brightness.
        let row = 40 * 96;
        let (l, r) = (out[row + 46], out[row + 49]);
        let (cl, cr) = (scene.clean[row + 46], scene.clean[row + 49]);
        assert!((l[0] - cl[0]).abs() < 0.15 * cl[0] && (r[0] - cr[0]).abs() < 0.15 * cr[0], "{l:?} {cl:?} {r:?} {cr:?}");
        // Without a variance image the filter estimates one from the frame and still helps.
        let mut g = Svgf::new(SvgfSettings::default());
        let blind = relative_mse(&g.filter(&scene.frame(&noisy, None), None).unwrap(), &scene.clean);
        assert!(blind < before, "{before} -> {blind}");
    }

    #[test]
    fn accumulating_frames_along_a_motion_lowers_the_error_further() {
        let scene = walls(96, 64);
        let mut f = Svgf::new(SvgfSettings::default());
        let mut errors = Vec::new();
        for frame in 0..6 {
            let (noisy, var) = scene.noisy(0.5, 10 + frame);
            let out = f.filter(&scene.frame(&noisy, Some(&var)), None).unwrap();
            errors.push(relative_mse(&out, &scene.clean));
        }
        eprintln!("svgf over frames: {errors:?}");
        assert!(errors[5] * 1.5 < errors[0], "{errors:?}");
        assert!(f.history_length(40 * 96 + 20) > 4.0);
        // A cut: after a reset the first frame is filtered on its own again.
        f.reset();
        assert_eq!(f.history_length(0), 0.0);
    }

    #[test]
    fn history_is_followed_through_motion_vectors_and_dropped_at_a_disocclusion() {
        let scene = walls(64, 48);
        let (noisy, var) = scene.noisy(0.3, 5);
        let mut f = Svgf::new(SvgfSettings::default());
        f.filter(&scene.frame(&noisy, Some(&var)), None).unwrap();
        // Everything moved 3 pixels right: the history of x comes from x − 3.
        let motion = vec![[3.0f32, 0.0]; 64 * 48];
        let (noisy2, var2) = scene.noisy(0.3, 6);
        f.filter(&scene.frame(&noisy2, Some(&var2)), Some(&motion)).unwrap();
        // Interior pixels were found in the history; those that came from beyond the left border were not.
        assert!(f.history_length(30 * 64 + 20) > 1.5 && f.history_length(30 * 64 + 1) < 1.5);
        // A different surface in the history (the depth changed) is not reused.
        let mut deeper = scene.depth.clone();
        deeper.iter_mut().for_each(|d| *d *= 2.0);
        let frame = Frame { depth: &deeper, ..scene.frame(&noisy2, Some(&var2)) };
        f.filter(&frame, None).unwrap();
        assert!(f.history_length(30 * 64 + 20) < 1.5);
        assert!(f.filter(&scene.frame(&noisy2, Some(&var2)), Some(&motion[..5])).is_err());
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let f = Frame { width: 0, height: 4, color: &[], albedo: &[], normal: &[], depth: &[], variance: None };
        assert!(Svgf::new(SvgfSettings::default()).filter(&f, None).is_err());
        let g = Frame { width: 2, height: 2, color: &[[0.0; 3]; 4], albedo: &[[0.0; 3]; 3], normal: &[[0.0; 3]; 4], depth: &[0.0; 4], variance: None };
        assert_eq!(g.check(), Err(DenoiseError::Mismatch("albedo")));
        // Non-finite pixels do not spread.
        let scene = walls(16, 16);
        let (mut noisy, var) = scene.noisy(0.2, 1);
        noisy[100] = [f32::NAN, f32::INFINITY, -1.0];
        let out = Svgf::new(SvgfSettings::default()).filter(&scene.frame(&noisy, Some(&var)), None).unwrap();
        assert!(out.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }
}
