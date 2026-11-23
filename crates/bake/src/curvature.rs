//! Mean curvature of a triangle soup, per triangle corner.
//!
//! Corners at the same place are welded (positions rounded to a grid of a millionth of the soup's
//! size), each welded vertex gets the area-weighted mean of its triangles' normals, and its
//! curvature is the mean over its edges of `(nᵥ − nᵤ)·(pᵥ − pᵤ) / |pᵥ − pᵤ|²`: for a sphere of
//! radius r every term is 1/r, positive where the surface bulges out and negative where it dips in.

use std::collections::HashMap;

use glam::Vec3;

/// Curvature at every corner of every triangle (positions are world space), in the same order as
/// `triangles`, and the smooth normal at each corner.
pub fn corner_curvature(triangles: &[[Vec3; 3]]) -> Vec<[f32; 3]> {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for t in triangles {
        for p in t {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
    }
    let size = (hi - lo).max_element();
    if !size.is_finite() || size <= 0.0 {
        return vec![[0.0; 3]; triangles.len()];
    }
    let grid = size * 1e-6;
    let key = |p: Vec3| {
        let q = ((p - lo) / grid).round();
        [q.x as i64, q.y as i64, q.z as i64]
    };
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let mut points: Vec<Vec3> = Vec::new();
    let mut corner: Vec<[u32; 3]> = Vec::with_capacity(triangles.len());
    for t in triangles {
        let mut c = [0u32; 3];
        for (k, p) in t.iter().enumerate() {
            c[k] = *ids.entry(key(*p)).or_insert_with(|| {
                points.push(*p);
                (points.len() - 1) as u32
            });
        }
        corner.push(c);
    }
    // Area-weighted vertex normals.
    let mut normals = vec![Vec3::ZERO; points.len()];
    for (t, c) in triangles.iter().zip(&corner) {
        let n = (t[1] - t[0]).cross(t[2] - t[0]);
        for i in c {
            if let Some(slot) = normals.get_mut(*i as usize) {
                *slot += n;
            }
        }
    }
    for n in &mut normals {
        *n = n.try_normalize().unwrap_or(Vec3::ZERO);
    }
    // Each edge once per triangle side; a shared edge is counted from both sides, which weighs
    // interior edges alike.
    let mut sum = vec![0.0f32; points.len()];
    let mut count = vec![0u32; points.len()];
    for c in &corner {
        for k in 0..3 {
            let (a, b) = (c[k] as usize, c[(k + 1) % 3] as usize);
            if a == b {
                continue;
            }
            let (pa, pb) = (points[a], points[b]);
            let d = pa - pb;
            let len2 = d.length_squared();
            if len2 <= 1e-20 {
                continue;
            }
            let term = (normals[a] - normals[b]).dot(d) / len2;
            if term.is_finite() {
                for v in [a, b] {
                    sum[v] += term;
                    count[v] += 1;
                }
            }
        }
    }
    let curvature: Vec<f32> = sum.iter().zip(&count).map(|(s, n)| if *n > 0 { s / *n as f32 } else { 0.0 }).collect();
    corner.iter().map(|c| [curvature[c[0] as usize], curvature[c[1] as usize], curvature[c[2] as usize]]).collect()
}
