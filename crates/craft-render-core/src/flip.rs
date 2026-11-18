//! FLIP, a perceptual difference between two images, from the published algorithm: Andersson,
//! Nilsson, Akenine-Möller, Oskarsson, Åström and Fairchild, "FLIP: A Difference Evaluator for
//! Alternating Images" (Proc. ACM Comput. Graph. Interact. Tech. 3(2), 2020).
//!
//! It is a number per pixel in `[0, 1]` (0: not distinguishable when the images alternate, 1: as
//! different as it gets). Two parts are combined:
//! - **Colour**: both images go through an opponent colour space (YCxCz) and a contrast sensitivity
//!   filter for the viewing conditions (pixels per degree), then to CIELAB with the Hunt effect
//!   (chroma scaled by lightness); the distance is the hybrid HyAB one (lightness difference plus
//!   the length of the chroma difference), compressed and mapped so that the distance between
//!   saturated green and blue is 1.
//! - **Features**: edges and points of the lightness are found with first and second derivative
//!   of Gaussian filters, and the difference between the images' feature strengths lowers the
//!   exponent that turns the colour difference into the result: `colour ^ (1 − features)`.
//!
//! The input is display-referred: linear sRGB values in `[0, 1]` (anything outside is clamped,
//! non-finite values are zero). This is the low-dynamic-range variant; to judge a high-dynamic-range
//! rendering, expose and clamp it first. It is written from the paper's description; it has not
//! been compared pixel for pixel with the authors' reference implementation.

use rayon::prelude::*;

/// Pixels per degree of a 0.7 m wide 3840-pixel monitor seen from 0.7 m (the paper's default).
pub const DEFAULT_PPD: f32 = 3840.0 * std::f32::consts::PI / 180.0;

/// The white point of sRGB (D65), normalised to Y = 1.
const WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];
/// Exponent compressing the HyAB distance (taken relative to green against blue), the knee where
/// the mapping turns, and the value there.
const Q_C: f32 = 0.7;
const P_C: f32 = 0.4;
const P_T: f32 = 0.95;
/// Exponent applied to the feature difference, and the feature width in degrees.
const Q_F: f32 = 0.5;
const FEATURE_WIDTH: f32 = 0.082;

/// Contrast sensitivity of the three channels: `a1 e^(-b1 f²) + a2 e^(-b2 f²)` in cycles per degree.
const CSF: [[f32; 4]; 3] = [[1.0, 0.0047, 0.0, 1.0e-5], [1.0, 0.0053, 0.0, 1.0e-5], [34.1, 0.04, 13.5, 0.025]];

/// The result: one value per pixel and their mean.
#[derive(Clone, Debug)]
pub struct FlipMap {
    pub map: Vec<f32>,
    pub mean: f32,
}

#[inline]
fn srgb_linear_to_xyz(c: [f32; 3]) -> [f32; 3] {
    [
        0.412_456_4 * c[0] + 0.357_576_1 * c[1] + 0.180_437_5 * c[2],
        0.212_672_9 * c[0] + 0.715_152_2 * c[1] + 0.072_175 * c[2],
        0.019_333_9 * c[0] + 0.119_192 * c[1] + 0.950_304_1 * c[2],
    ]
}

#[inline]
fn xyz_to_srgb_linear(c: [f32; 3]) -> [f32; 3] {
    [
        3.240_454_2 * c[0] - 1.537_138_5 * c[1] - 0.498_531_4 * c[2],
        -0.969_266 * c[0] + 1.876_010_8 * c[1] + 0.041_556 * c[2],
        0.055_643_4 * c[0] - 0.204_025_9 * c[1] + 1.057_225_2 * c[2],
    ]
}

/// The opponent space the contrast filters work in.
#[inline]
fn to_ycxcz(rgb: [f32; 3]) -> [f32; 3] {
    let xyz = srgb_linear_to_xyz(rgb);
    let (x, y, z) = (xyz[0] / WHITE[0], xyz[1] / WHITE[1], xyz[2] / WHITE[2]);
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

#[inline]
fn from_ycxcz(c: [f32; 3]) -> [f32; 3] {
    let y = (c[0] + 16.0) / 116.0;
    let (x, z) = (y + c[1] / 500.0, y - c[2] / 200.0);
    xyz_to_srgb_linear([x * WHITE[0], y * WHITE[1], z * WHITE[2]])
}

#[inline]
fn lab_f(t: f32) -> f32 {
    const DELTA: f32 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA { t.cbrt() } else { t / (3.0 * DELTA * DELTA) + 4.0 / 29.0 }
}

/// CIELAB of a linear sRGB colour with chroma scaled by lightness (the Hunt effect): `(L*, ĉa, ĉb)`.
#[inline]
fn hunt_lab(rgb: [f32; 3]) -> [f32; 3] {
    let xyz = srgb_linear_to_xyz(rgb);
    let (fx, fy, fz) = (lab_f(xyz[0] / WHITE[0]), lab_f(xyz[1] / WHITE[1]), lab_f(xyz[2] / WHITE[2]));
    let l = 116.0 * fy - 16.0;
    [l, 0.01 * l * 500.0 * (fx - fy), 0.01 * l * 200.0 * (fy - fz)]
}

/// The hybrid distance: lightness difference plus the length of the chroma difference.
#[inline]
fn hyab(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).abs() + ((a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn sanitize(img: &[[f32; 3]]) -> Vec<[f32; 3]> {
    img.iter().map(|p| p.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })).collect()
}

/// A normalised Gaussian of standard deviation `sigma` pixels, to ±3σ.
fn gaussian(sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as i64;
    let mut k: Vec<f32> = (-r..=r).map(|x| (-((x * x) as f32) / (2.0 * sigma * sigma)).exp()).collect();
    let s: f32 = k.iter().sum();
    k.iter_mut().for_each(|v| *v /= s);
    k
}

/// A first or second derivative of a Gaussian, scaled so that its positive entries sum to 1 and
/// its negative ones to −1 (a unit step, or a unit-width line, gives a response of 1).
fn derivative(sigma: f32, order: u32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil().max(1.0) as i64;
    let mut k: Vec<f32> = (-r..=r)
        .map(|x| {
            let x = x as f32;
            let g = (-(x * x) / (2.0 * sigma * sigma)).exp();
            if order == 1 { -x * g } else { (x * x / (sigma * sigma) - 1.0) * g }
        })
        .collect();
    let pos: f32 = k.iter().filter(|v| **v > 0.0).sum();
    let neg: f32 = -k.iter().filter(|v| **v < 0.0).sum::<f32>();
    for v in &mut k {
        *v /= if *v > 0.0 { pos.max(1e-12) } else { neg.max(1e-12) };
    }
    k
}

/// Convolve along x (`horizontal`) or y with a kernel, edges clamped.
fn convolve(w: usize, h: usize, src: &[f32], kernel: &[f32], horizontal: bool) -> Vec<f32> {
    let r = (kernel.len() / 2) as i64;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for (i, k) in kernel.iter().enumerate() {
                let d = i as i64 - r;
                let (xx, yy) = if horizontal { ((x as i64 + d).clamp(0, w as i64 - 1), y as i64) } else { (x as i64, (y as i64 + d).clamp(0, h as i64 - 1)) };
                acc += k * src[yy as usize * w + xx as usize];
            }
            *o = acc;
        }
    });
    out
}

fn separable(w: usize, h: usize, src: &[f32], kx: &[f32], ky: &[f32]) -> Vec<f32> {
    convolve(w, h, &convolve(w, h, src, kx, true), ky, false)
}

/// Apply the contrast sensitivity filters to an image in the opponent space and return linear RGB.
fn csf_filter(w: usize, h: usize, rgb: &[[f32; 3]], ppd: f32) -> Vec<[f32; 3]> {
    let opp: Vec<[f32; 3]> = rgb.iter().map(|p| to_ycxcz(*p)).collect();
    let mut filtered = vec![[0.0f32; 3]; w * h];
    for (c, params) in CSF.iter().enumerate() {
        let plane: Vec<f32> = opp.iter().map(|p| p[c]).collect();
        let (a1, b1, a2, b2) = (params[0], params[1], params[2], params[3]);
        let total = a1 + a2;
        let mut acc = vec![0.0f32; w * h];
        for (a, b) in [(a1, b1), (a2, b2)] {
            if a <= 0.0 {
                continue;
            }
            // The spatial filter of `exp(-b f²)` is a Gaussian of σ = sqrt(b / 2π²) degrees.
            let sigma = ((b / (2.0 * std::f32::consts::PI * std::f32::consts::PI)).sqrt() * ppd).max(0.05);
            let g = gaussian(sigma);
            let part = separable(w, h, &plane, &g, &g);
            for (o, v) in acc.iter_mut().zip(&part) {
                *o += a / total * v;
            }
        }
        for (f, v) in filtered.iter_mut().zip(&acc) {
            f[c] = *v;
        }
    }
    filtered.iter().map(|p| from_ycxcz(*p).map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 })).collect()
}

/// Strength of edges and of points in a lightness image.
fn feature_strengths(w: usize, h: usize, lightness: &[f32], ppd: f32) -> (Vec<f32>, Vec<f32>) {
    let sigma = (0.5 * FEATURE_WIDTH * ppd).max(0.3);
    let (g, d1, d2) = (gaussian(sigma), derivative(sigma, 1), derivative(sigma, 2));
    let (ex, ey) = (separable(w, h, lightness, &d1, &g), separable(w, h, lightness, &g, &d1));
    let (px, py) = (separable(w, h, lightness, &d2, &g), separable(w, h, lightness, &g, &d2));
    let edge = ex.iter().zip(&ey).map(|(a, b)| (a * a + b * b).sqrt()).collect();
    let point = px.iter().zip(&py).map(|(a, b)| (a * a + b * b).sqrt()).collect();
    (edge, point)
}

/// The FLIP difference between `test` and `reference` (`width × height` linear sRGB images in
/// `[0, 1]`), for a viewer who sees `ppd` pixels per degree ([`DEFAULT_PPD`] is a typical
/// desktop). `None` when the sizes do not match or the image is empty.
pub fn flip(width: usize, height: usize, test: &[[f32; 3]], reference: &[[f32; 3]], ppd: f32) -> Option<FlipMap> {
    let n = width.checked_mul(height)?;
    if n == 0 || test.len() != n || reference.len() != n || !ppd.is_finite() || ppd <= 0.0 {
        return None;
    }
    let (t, r) = (sanitize(test), sanitize(reference));
    let (tf, rf) = (csf_filter(width, height, &t, ppd), csf_filter(width, height, &r, ppd));
    // The largest colour distance: saturated green against saturated blue.
    let cmax = hyab(hunt_lab([0.0, 1.0, 0.0]), hunt_lab([0.0, 0.0, 1.0]));
    let colour: Vec<f32> = tf
        .par_iter()
        .zip(&rf)
        .map(|(a, b)| {
            let d = (hyab(hunt_lab(*a), hunt_lab(*b)) / cmax).powf(Q_C);
            if d < P_C { P_T / P_C * d } else { (P_T + (d - P_C) / (1.0 - P_C) * (1.0 - P_T)).min(1.0) }
        })
        .collect();
    // Features of the lightness (L* mapped to [0, 1]).
    let lightness = |img: &[[f32; 3]]| -> Vec<f32> { img.iter().map(|p| (to_ycxcz(*p)[0] + 16.0) / 116.0).collect() };
    let ((te, tp), (re, rp)) = (feature_strengths(width, height, &lightness(&t), ppd), feature_strengths(width, height, &lightness(&r), ppd));
    let map: Vec<f32> = (0..n)
        .map(|i| {
            let feature = (std::f32::consts::FRAC_1_SQRT_2 * (te[i] - re[i]).abs().max((tp[i] - rp[i]).abs())).min(1.0).powf(Q_F);
            colour[i].powf(1.0 - feature).clamp(0.0, 1.0)
        })
        .collect();
    let mean = (map.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64) as f32;
    Some(FlipMap { map, mean })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        (*seed >> 40) as f32 / (1u64 << 24) as f32
    }

    fn picture(w: usize, h: usize) -> Vec<[f32; 3]> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
                if x > 0.5 { [0.8 * y, 0.3, 0.2 + 0.5 * x] } else { [0.2 + 0.6 * x, 0.5 * y + 0.2, 0.4] }
            })
            .collect()
    }

    #[test]
    fn identical_images_do_not_differ_and_the_result_is_in_range() {
        let img = picture(40, 30);
        let f = flip(40, 30, &img, &img, DEFAULT_PPD).unwrap();
        assert!(f.mean < 1e-6 && f.map.iter().all(|v| *v == 0.0));
        let other: Vec<[f32; 3]> = img.iter().map(|p| [1.0 - p[0], 1.0 - p[1], 1.0 - p[2]]).collect();
        let g = flip(40, 30, &img, &other, DEFAULT_PPD).unwrap();
        assert!(g.map.iter().all(|v| (0.0..=1.0).contains(v)) && g.mean > 0.3, "{}", g.mean);
    }

    #[test]
    fn the_difference_grows_with_the_noise_and_black_against_white_is_the_largest() {
        let img = picture(48, 32);
        let mut seed = 77u64;
        let noisy = |amount: f32, seed: &mut u64| -> Vec<[f32; 3]> { img.iter().map(|p| p.map(|v| v + amount * (rng(seed) - 0.5))).collect() };
        let (a, b, c) = (noisy(0.02, &mut seed), noisy(0.1, &mut seed), noisy(0.4, &mut seed));
        let mean = |t: &[[f32; 3]]| flip(48, 32, t, &img, DEFAULT_PPD).unwrap().mean;
        let (ma, mb, mc) = (mean(&a), mean(&b), mean(&c));
        assert!(0.0 < ma && ma < mb && mb < mc && mc < 1.0, "{ma} {mb} {mc}");
        let black = vec![[0.0f32; 3]; 48 * 32];
        let white = vec![[1.0f32; 3]; 48 * 32];
        let worst = flip(48, 32, &black, &white, DEFAULT_PPD).unwrap().mean;
        assert!(worst > 0.95 && worst > mc, "{worst}");
        // A small brightness shift over a flat area is a small difference.
        let grey = vec![[0.5f32; 3]; 48 * 32];
        let near = vec![[0.52f32; 3]; 48 * 32];
        assert!(flip(48, 32, &near, &grey, DEFAULT_PPD).unwrap().mean < 0.1);
    }

    #[test]
    fn a_displaced_edge_shows_where_it_moved_and_bad_input_is_none() {
        let (w, h) = (64usize, 32usize);
        let edge = |x0: usize| -> Vec<[f32; 3]> { (0..w * h).map(|i| if i % w < x0 { [0.1; 3] } else { [0.9; 3] }).collect() };
        let a = flip(w, h, &edge(32), &edge(34), DEFAULT_PPD).unwrap().map[16 * w + 32];
        let flat = flip(w, h, &vec![[0.5; 3]; w * h], &vec![[0.5 + 0.8 * (2.0 / 64.0); 3]; w * h], DEFAULT_PPD).unwrap().mean;
        assert!(a > 3.0 * flat, "at the edge {a}, flat {flat}");
        assert!(flip(4, 4, &[[0.0; 3]; 3], &[[0.0; 3]; 16], 67.0).is_none());
        assert!(flip(0, 4, &[], &[], 67.0).is_none() && flip(4, 4, &[[0.0; 3]; 16], &[[0.0; 3]; 16], f32::NAN).is_none());
        // Non-finite and out-of-range values are clamped, not propagated.
        let wild = vec![[f32::NAN, 7.0, -3.0]; 16];
        assert!(flip(4, 4, &wild, &[[0.5; 3]; 16], 67.0).unwrap().map.iter().all(|v| v.is_finite()));
    }
}
