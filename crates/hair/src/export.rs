//! Curves as a glTF 2.0 binary (`.glb`).
//!
//! glTF has no curve primitive, so every segment of every strand is a line (primitive mode 1,
//! `LINES`) in one mesh: a `POSITION` accessor (with the required bounds), a `_WIDTH` vertex
//! attribute (an application-specific attribute, as the specification allows with a leading
//! underscore) holding the strand's width at each point, and an index buffer of segment pairs.
//! The mesh carries `extras.modelcraft` with the strand and point counts so a reader can rebuild
//! the strands without guessing: strand `i` is points `i·pointsPerStrand ..`.
//!
//! The container is the one of the specification: a 12-byte header, a JSON chunk padded with
//! spaces and a binary chunk padded with zeros, both to four bytes.

use glam::{Mat4, Vec3};
use serde_json::json;

use crate::HairError;

/// Most points one export holds (a GLB's length field is 32 bits).
pub const MAX_EXPORT_POINTS: usize = 40_000_000;

/// What to write.
#[derive(Clone, Copy, Debug)]
pub struct CurveExport<'a> {
    /// `points_per_strand` points per strand, back to back.
    pub points: &'a [Vec3],
    pub points_per_strand: usize,
    /// The width at each point (one per point), if the curves have one.
    pub widths: Option<&'a [f32]>,
}

fn pad4(v: &mut Vec<u8>, with: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(with);
    }
}

/// The curves of `export` as a `.glb`, placed by `transform` (identity for the emitter's own space).
pub fn to_glb(export: &CurveExport, name: &str, transform: Mat4) -> Result<Vec<u8>, HairError> {
    let pps = export.points_per_strand;
    if pps < 2 {
        return Err(HairError::OutOfRange { what: "points per strand", min: 2.0, max: 256.0, found: pps as f64 });
    }
    let n = export.points.len();
    if n == 0 || !n.is_multiple_of(pps) {
        return Err(HairError::LengthMismatch { what: "curve points", expected: n.div_ceil(pps).max(1) * pps, found: n });
    }
    if n > MAX_EXPORT_POINTS {
        return Err(HairError::TooMany { what: "points", requested: n, max: MAX_EXPORT_POINTS });
    }
    if let Some(w) = export.widths
        && w.len() != n
    {
        return Err(HairError::LengthMismatch { what: "widths", expected: n, found: w.len() });
    }
    if export.points.iter().any(|p| !p.is_finite()) {
        return Err(HairError::NotFinite("curve point"));
    }
    if !transform.is_finite() {
        return Err(HairError::NotFinite("transform"));
    }
    let strands = n / pps;
    let segments = strands * (pps - 1);

    // The binary chunk: positions, widths, indices.
    let mut bin: Vec<u8> = Vec::with_capacity(n * 12 + n * 4 + segments * 8);
    let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for p in export.points {
        let w = transform.transform_point3(*p);
        lo = lo.min(w);
        hi = hi.max(w);
        for c in w.to_array() {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let positions_len = bin.len();
    if let Some(widths) = export.widths {
        for w in widths {
            bin.extend_from_slice(&w.max(0.0).to_le_bytes());
        }
    }
    let widths_len = bin.len() - positions_len;
    for s in 0..strands {
        for j in 0..pps - 1 {
            let a = (s * pps + j) as u32;
            bin.extend_from_slice(&a.to_le_bytes());
            bin.extend_from_slice(&(a + 1).to_le_bytes());
        }
    }
    let indices_len = bin.len() - positions_len - widths_len;

    let mut views = vec![json!({"buffer": 0, "byteOffset": 0, "byteLength": positions_len, "target": 34962})];
    let mut accessors = vec![json!({"bufferView": 0, "componentType": 5126, "count": n, "type": "VEC3", "min": lo.to_array(), "max": hi.to_array()})];
    let mut attributes = serde_json::Map::new();
    attributes.insert("POSITION".into(), json!(0));
    if widths_len > 0 {
        views.push(json!({"buffer": 0, "byteOffset": positions_len, "byteLength": widths_len, "target": 34962}));
        accessors.push(json!({"bufferView": 1, "componentType": 5126, "count": n, "type": "SCALAR"}));
        attributes.insert("_WIDTH".into(), json!(1));
    }
    let index_view = views.len();
    views.push(json!({"buffer": 0, "byteOffset": positions_len + widths_len, "byteLength": indices_len, "target": 34963}));
    let index_accessor = accessors.len();
    accessors.push(json!({"bufferView": index_view, "componentType": 5125, "count": segments * 2, "type": "SCALAR"}));

    let doc = json!({
        "asset": {"version": "2.0", "generator": "ModelCraft hair export"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": name, "mesh": 0}],
        "meshes": [{
            "name": name,
            "primitives": [{"attributes": attributes, "indices": index_accessor, "mode": 1}],
            "extras": {"modelcraft": {"kind": "hair", "strands": strands, "pointsPerStrand": pps}}
        }],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut json_bytes = serde_json::to_vec(&doc).map_err(|e| HairError::Simulation(e.to_string()))?;
    pad4(&mut json_bytes, b' ');
    pad4(&mut bin, 0);
    // The buffer's declared length is the padded one.
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    if total > u32::MAX as usize {
        return Err(HairError::TooMany { what: "bytes", requested: total, max: u32::MAX as usize });
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    Ok(out)
}
