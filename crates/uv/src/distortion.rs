//! Numerical distortion of a UV layout: how far each triangle's angles and area are from the
//! surface's.
//!
//! The map from a triangle in its own 3D frame to its UV triangle is linear, `J`. Its singular
//! values `σ1 ≥ σ2` say how much it stretches: `σ1 / σ2` is the *stretch* (1 for a conformal
//! map, which keeps angles), `σ1 σ2` is the area scale. The *area ratio* divides each triangle's
//! share of the total UV area by its share of the total surface area, so a layout that scales
//! every triangle alike has ratio 1 everywhere whatever its overall scale. See also Sander,
//! Snyder, Gortler and Hoppe, "Texture Mapping Progressive Meshes", SIGGRAPH 2001, for the use
//! of singular values as a distortion measure.

use glam::{DVec2, DVec3};
use modelcraft_mesh::PolyMesh;

use crate::charts::face_triangles;
use crate::lscm::local_frame;

/// The measures of one triangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleMetric {
    pub face: u32,
    /// Largest difference between a corner angle on the surface and in UV space, in degrees.
    pub angle_error_deg: f32,
    /// `σ1 / σ2`, at least 1; large when the triangle is squeezed to a sliver.
    pub stretch: f32,
    /// Share of UV area over share of surface area (1 is ideal).
    pub area_ratio: f32,
    /// The triangle is mirrored in UV space: it winds the other way than on the surface.
    pub flipped: bool,
}

/// Averages (weighted by surface area) and extremes over a mesh.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Distortion {
    pub triangles: usize,
    pub flipped: usize,
    pub mean_angle_error_deg: f32,
    pub max_angle_error_deg: f32,
    pub mean_stretch: f32,
    pub max_stretch: f32,
    /// Root mean square of `ln(area ratio)`: 0 for a layout that is the same scale everywhere.
    pub area_rms_log: f32,
    pub min_area_ratio: f32,
    pub max_area_ratio: f32,
}

fn angle_at(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let (u, v) = (b - a, c - a);
    u.cross(v).length().atan2(u.dot(v))
}

fn angle_at_2d(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    let (u, v) = (b - a, c - a);
    u.perp_dot(v).abs().atan2(u.dot(v))
}

/// Measure the UVs of `mesh` (UV space has v down, as glTF; see the crate docs). A mesh without
/// UVs measures as all zeros.
pub fn measure(mesh: &PolyMesh) -> (Distortion, Vec<TriangleMetric>) {
    if !mesh.has_uvs() {
        return (Distortion::default(), Vec::new());
    }
    struct Raw {
        face: u32,
        a3: f64,
        auv: f64,
        angle: f64,
        stretch: f64,
        flipped: bool,
    }
    let mut raw: Vec<Raw> = Vec::new();
    let mut tris = Vec::new();
    for f in 0..mesh.face_count() as u32 {
        tris.clear();
        face_triangles(mesh, f, &mut tris);
        for t in &tris {
            let p = t.map(|c| mesh.position(mesh.origin(c)).as_dvec3());
            // v points down, so mirror it to compare orientations with the surface.
            let uv = t.map(|c| DVec2::new(f64::from(mesh.uv(c).x), -f64::from(mesh.uv(c).y)));
            let Some((q, a3)) = local_frame(p) else { continue };
            let (d1, d2) = (uv[1] - uv[0], uv[2] - uv[0]);
            let det_q = q[1].x * q[2].y - q[2].x * q[1].y;
            // J = D Q⁻¹
            let (ja, jb) = ((d1.x * q[2].y - d2.x * q[1].y) / det_q, (-d1.x * q[2].x + d2.x * q[1].x) / det_q);
            let (jc, jd) = ((d1.y * q[2].y - d2.y * q[1].y) / det_q, (-d1.y * q[2].x + d2.y * q[1].x) / det_q);
            let sum = ((ja + jd).powi(2) + (jc - jb).powi(2)).sqrt();
            let diff = ((ja - jd).powi(2) + (jc + jb).powi(2)).sqrt();
            let (s1, s2) = (0.5 * (sum + diff), 0.5 * (sum - diff).abs());
            let det = ja * jd - jb * jc;
            let stretch = if s2 > 1e-12 * s1.max(1e-300) { (s1 / s2).min(1e6) } else { 1e6 };
            let mut angle = 0.0f64;
            for k in 0..3 {
                let (k1, k2) = ((k + 1) % 3, (k + 2) % 3);
                angle = angle.max((angle_at(p[k], p[k1], p[k2]) - angle_at_2d(uv[k], uv[k1], uv[k2])).abs());
            }
            raw.push(Raw { face: f, a3, auv: det * a3, angle: angle.to_degrees(), stretch, flipped: det < 0.0 });
        }
    }
    let total3: f64 = raw.iter().map(|r| r.a3).sum();
    let total_uv: f64 = raw.iter().map(|r| r.auv.abs()).sum();
    let mut d = Distortion { triangles: raw.len(), min_area_ratio: f32::MAX, ..Distortion::default() };
    let mut metrics = Vec::with_capacity(raw.len());
    let (mut w_angle, mut w_stretch, mut w_log) = (0.0, 0.0, 0.0);
    for r in &raw {
        let ratio = if total3 > 0.0 && total_uv > 0.0 { (r.auv.abs() / total_uv) / (r.a3 / total3) } else { 0.0 };
        let w = if total3 > 0.0 { r.a3 / total3 } else { 0.0 };
        w_angle += w * r.angle;
        w_stretch += w * r.stretch;
        let l = ratio.max(1e-9).ln();
        w_log += w * l * l;
        d.max_angle_error_deg = d.max_angle_error_deg.max(r.angle as f32);
        d.max_stretch = d.max_stretch.max(r.stretch as f32);
        d.min_area_ratio = d.min_area_ratio.min(ratio as f32);
        d.max_area_ratio = d.max_area_ratio.max(ratio as f32);
        d.flipped += usize::from(r.flipped);
        metrics.push(TriangleMetric { face: r.face, angle_error_deg: r.angle as f32, stretch: r.stretch as f32, area_ratio: ratio as f32, flipped: r.flipped });
    }
    if raw.is_empty() {
        d.min_area_ratio = 0.0;
    }
    d.mean_angle_error_deg = w_angle as f32;
    d.mean_stretch = w_stretch as f32;
    d.area_rms_log = w_log.sqrt() as f32;
    (d, metrics)
}

/// The metrics of each face, averaged over its triangles (for colouring faces in a heat map).
pub fn per_face(mesh: &PolyMesh, metrics: &[TriangleMetric]) -> Vec<TriangleMetric> {
    let mut out: Vec<(TriangleMetric, u32)> =
        (0..mesh.face_count() as u32).map(|face| (TriangleMetric { face, angle_error_deg: 0.0, stretch: 0.0, area_ratio: 0.0, flipped: false }, 0)).collect();
    for m in metrics {
        if let Some((o, n)) = out.get_mut(m.face as usize) {
            o.angle_error_deg += m.angle_error_deg;
            o.stretch += m.stretch;
            o.area_ratio += m.area_ratio;
            o.flipped |= m.flipped;
            *n += 1;
        }
    }
    out.into_iter()
        .map(|(mut m, n)| {
            let k = n.max(1) as f32;
            m.angle_error_deg /= k;
            m.stretch /= k;
            m.area_ratio /= k;
            m
        })
        .collect()
}
