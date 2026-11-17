//! Binary encoding of meshes and sculpt layers inside a project (little-endian):
//!
//! ```text
//! mesh:  "MCMB" u32:version=1 u32:verts u32:faces u32:corners u32:has_uvs
//!        verts × f32[3]  faces × u32 (corner count)  corners × u32 (vertex)
//!        [corners × f32[2] if has_uvs]  faces × u32 (patch)
//! layer: "MCSL" u32:version=1 u32:count  count × f32[3]  u32:mask_count  mask_count × f32
//! ```
//!
//! Readers check every count against the bytes actually present before allocating.

use glam::{Vec2, Vec3};
use modelcraft_mesh::{PolyMesh, Polygon};

use crate::{FormatError, Result};

struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).ok_or_else(|| FormatError::Malformed("blob length overflow".into()))?;
        let s = self.b.get(self.at..end).ok_or_else(|| FormatError::Malformed("blob is truncated".into()))?;
        self.at = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }
    /// Check that `count` items of `size` bytes can still be there.
    fn fits(&self, count: u32, size: usize) -> Result<usize> {
        let n = count as usize;
        if n.saturating_mul(size) > self.left() {
            return Err(FormatError::Malformed("blob count exceeds its data".into()));
        }
        Ok(n)
    }
}

fn put(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn putf(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn write_mesh(m: &PolyMesh) -> Vec<u8> {
    let polys = m.to_polygons();
    let corners: usize = polys.iter().map(|p| p.verts.len()).sum();
    let mut out = Vec::with_capacity(24 + m.vertex_count() * 12 + corners * 12 + polys.len() * 8);
    out.extend_from_slice(b"MCMB");
    for v in [1, m.vertex_count() as u32, polys.len() as u32, corners as u32, u32::from(m.has_uvs())] {
        put(&mut out, v);
    }
    for p in m.positions() {
        for c in p.to_array() {
            putf(&mut out, c);
        }
    }
    for p in &polys {
        put(&mut out, p.verts.len() as u32);
    }
    for p in &polys {
        for v in &p.verts {
            put(&mut out, *v);
        }
    }
    if m.has_uvs() {
        for p in &polys {
            for uv in p.uvs.iter().flatten() {
                putf(&mut out, uv.x);
                putf(&mut out, uv.y);
            }
        }
    }
    for p in &polys {
        put(&mut out, p.patch);
    }
    out
}

pub fn read_mesh(b: &[u8]) -> Result<PolyMesh> {
    let mut r = Rd { b, at: 0 };
    if r.take(4)? != b"MCMB" {
        return Err(FormatError::Malformed("not a mesh blob".into()));
    }
    if r.u32()? != 1 {
        return Err(FormatError::Unsupported("mesh blob version".into()));
    }
    let (nv, nf, nc, has_uv) = (r.u32()?, r.u32()?, r.u32()?, r.u32()? != 0);
    let nv = r.fits(nv, 12)?;
    let mut pos = Vec::with_capacity(nv);
    for _ in 0..nv {
        pos.push(Vec3::new(r.f32()?, r.f32()?, r.f32()?));
    }
    let nf = r.fits(nf, 4)?;
    let mut sizes = Vec::with_capacity(nf);
    for _ in 0..nf {
        sizes.push(r.u32()? as usize);
    }
    let nc = r.fits(nc, 4)?;
    if sizes.iter().try_fold(0usize, |a, s| a.checked_add(*s)) != Some(nc) {
        return Err(FormatError::Malformed("face sizes do not add up to the corners".into()));
    }
    let mut polys = Vec::with_capacity(nf);
    for s in &sizes {
        let mut verts = Vec::with_capacity(*s);
        for _ in 0..*s {
            verts.push(r.u32()?);
        }
        polys.push(Polygon::new(verts));
    }
    if has_uv {
        r.fits(nc as u32, 8)?;
        for p in &mut polys {
            let mut uvs = Vec::with_capacity(p.verts.len());
            for _ in 0..p.verts.len() {
                uvs.push(Vec2::new(r.f32()?, r.f32()?));
            }
            p.uvs = Some(uvs);
        }
    }
    for p in &mut polys {
        p.patch = r.u32()?;
    }
    Ok(PolyMesh::build(pos, polys).map_err(|e| FormatError::Malformed(e.to_string()))?.0)
}

pub fn write_layer(positions: &[Vec3], mask: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + positions.len() * 12 + mask.len() * 4);
    out.extend_from_slice(b"MCSL");
    put(&mut out, 1);
    put(&mut out, positions.len() as u32);
    for p in positions {
        for c in p.to_array() {
            putf(&mut out, c);
        }
    }
    put(&mut out, mask.len() as u32);
    for m in mask {
        putf(&mut out, *m);
    }
    out
}

pub fn read_layer(b: &[u8]) -> Result<(Vec<Vec3>, Vec<f32>)> {
    let mut r = Rd { b, at: 0 };
    if r.take(4)? != b"MCSL" || r.u32()? != 1 {
        return Err(FormatError::Malformed("not a sculpt layer blob".into()));
    }
    let n = r.u32()?;
    let n = r.fits(n, 12)?;
    let mut pos = Vec::with_capacity(n);
    for _ in 0..n {
        let p = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        pos.push(if p.is_finite() { p } else { Vec3::ZERO });
    }
    let k = r.u32()?;
    let k = r.fits(k, 4)?;
    let mut mask = Vec::with_capacity(k);
    for _ in 0..k {
        let m = r.f32()?;
        mask.push(if m.is_finite() { m.clamp(0.0, 1.0) } else { 0.0 });
    }
    Ok((pos, mask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_ops::primitives::Primitive;
    use proptest::prelude::*;

    #[test]
    fn mesh_and_layer_round_trip() {
        let m = Primitive::Torus { radius: 1.0, tube: 0.3, segments: 12, sides: 8 }.build().unwrap();
        assert_eq!(read_mesh(&write_mesh(&m)).unwrap(), m);
        let p = vec![Vec3::X, Vec3::Y];
        assert_eq!(read_layer(&write_layer(&p, &[0.5])).unwrap(), (p, vec![0.5]));
    }

    proptest! {
        #[test]
        fn garbage_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..400), cut in 0usize..400) {
            let _ = read_mesh(&bytes);
            let _ = read_layer(&bytes);
            let good = write_mesh(&Primitive::Cube { size: Vec3::ONE, segments: 2 }.build().unwrap());
            let _ = read_mesh(good.get(..cut.min(good.len())).unwrap_or_default());
            let mut flipped = good.clone();
            for (i, b) in bytes.iter().enumerate() {
                if let Some(x) = flipped.get_mut((i * 7) % good.len()) { *x ^= *b; }
            }
            let _ = read_mesh(&flipped);
        }
    }
}
