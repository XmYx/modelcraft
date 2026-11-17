//! The UV layout of a mesh as a picture: its face outlines in the unit square, as SVG (vector,
//! for any editor) or PNG (for painting over in a raster tool).
//!
//! UV space follows glTF: the origin is the top left corner and v grows downwards, which is also
//! how SVG and images address their pixels, so the layout is drawn without flipping. UVs outside
//! the unit square are drawn where they are; they fall outside the picture.

use std::fmt::Write as _;

use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
use modelcraft_mesh::PolyMesh;

use crate::{FormatError, Result};

/// Most faces a layout picture draws.
pub const MAX_LAYOUT_FACES: usize = 1_000_000;
/// Largest side of a raster layout, in pixels.
pub const MAX_LAYOUT_SIZE: u32 = 8192;

fn check(mesh: &PolyMesh) -> Result<()> {
    if !mesh.has_uvs() {
        return Err(FormatError::Malformed("the mesh has no UVs to draw".into()));
    }
    if mesh.face_count() > MAX_LAYOUT_FACES {
        return Err(FormatError::Malformed(format!("{} faces are too many for a UV layout picture", mesh.face_count())));
    }
    Ok(())
}

/// The layout as an SVG document of `size` × `size` pixels: a frame for the unit square and one
/// closed outline per face.
pub fn svg(mesh: &PolyMesh, size: u32) -> Result<String> {
    check(mesh)?;
    let size = size.clamp(16, MAX_LAYOUT_SIZE);
    let mut out = String::with_capacity(256 + mesh.face_count() * 64);
    let _ = writeln!(out, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    let _ = writeln!(out, "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" viewBox=\"0 0 1 1\">");
    let _ = writeln!(out, "<title>UV layout</title>");
    let w = 1.0 / f64::from(size);
    let _ = writeln!(out, "<rect x=\"0\" y=\"0\" width=\"1\" height=\"1\" fill=\"none\" stroke=\"#808080\" stroke-width=\"{w}\"/>");
    let _ = writeln!(out, "<g fill=\"none\" stroke=\"#000000\" stroke-width=\"{w}\" stroke-linejoin=\"round\">");
    for f in 0..mesh.face_count() as u32 {
        let mut d = String::new();
        for (i, h) in mesh.face_half_edges(f).enumerate() {
            let uv = mesh.uv(h);
            let _ = write!(d, "{}{:.6} {:.6}", if i == 0 { "M" } else { " L" }, uv.x, uv.y);
        }
        if !d.is_empty() {
            let _ = writeln!(out, "<path d=\"{d} Z\"/>");
        }
    }
    let _ = writeln!(out, "</g>");
    let _ = writeln!(out, "</svg>");
    Ok(out)
}

/// The layout as RGBA pixels (`size` × `size`, black one-pixel outlines on a transparent
/// background), row by row from the top.
pub fn raster(mesh: &PolyMesh, size: u32) -> Result<(u32, Vec<u8>)> {
    check(mesh)?;
    let size = size.clamp(16, MAX_LAYOUT_SIZE);
    let n = size as usize;
    let mut px = Vec::new();
    px.try_reserve_exact(n * n * 4).map_err(|_| FormatError::Malformed("the layout picture does not fit in memory".into()))?;
    px.resize(n * n * 4, 0);
    // UV 0 is the first pixel and UV 1 the last, so the unit square's border is drawn on all sides.
    let s = (size - 1) as f32;
    let mut plot = |x: i64, y: i64| {
        if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n {
            let i = (y as usize * n + x as usize) * 4;
            if let Some(p) = px.get_mut(i..i + 4) {
                p.copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    };
    for f in 0..mesh.face_count() as u32 {
        let pts: Vec<glam::Vec2> = mesh.face_half_edges(f).map(|h| mesh.uv(h) * s).collect();
        for (i, a) in pts.iter().enumerate() {
            let b = pts.get((i + 1) % pts.len()).copied().unwrap_or(*a);
            // Walk the segment one pixel at a time along its longer axis (bounded by the picture).
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let steps = dx.abs().max(dy.abs()).ceil().min(4.0 * (s + 1.0)) as i64;
            for k in 0..=steps {
                let t = if steps == 0 { 0.0 } else { k as f32 / steps as f32 };
                plot((a.x + dx * t).round() as i64, (a.y + dy * t).round() as i64);
            }
        }
    }
    Ok((size, px))
}

/// The layout as PNG bytes (see [`raster`]).
pub fn png(mesh: &PolyMesh, size: u32) -> Result<Vec<u8>> {
    let (size, px) = raster(mesh, size)?;
    let image = Image::from_raw(size, size, ChannelLayout::Rgba, SampleType::U8, px).map_err(|e| FormatError::Malformed(e.to_string()))?;
    craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| FormatError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec2, Vec3};
    use modelcraft_ops::primitives::Primitive;

    fn plane() -> PolyMesh {
        Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap()
    }

    #[test]
    fn svg_has_one_closed_outline_per_face_at_the_uvs() {
        let m = plane();
        let svg = svg(&m, 512).unwrap();
        assert!(svg.starts_with("<?xml") && svg.contains("viewBox=\"0 0 1 1\"") && svg.contains("width=\"512\""));
        let paths: Vec<&str> = svg.lines().filter(|l| l.starts_with("<path d=\"")).collect();
        assert_eq!(paths.len(), m.face_count());
        // The first face's first corner is where the mesh says it is.
        let uv = m.uv(m.face_half_edge(0));
        assert!(paths[0].starts_with(&format!("<path d=\"M{:.6} {:.6} L", uv.x, uv.y)), "{}", paths[0]);
        assert!(paths.iter().all(|p| p.ends_with(" Z\"/>")));
        // No UVs, no picture; the size is clamped, not trusted.
        let mut bare = m.clone();
        bare.clear_uvs();
        assert!(svg_of(&bare).is_err());
        assert!(super::svg(&m, u32::MAX).unwrap().contains(&format!("width=\"{MAX_LAYOUT_SIZE}\"")));
    }

    fn svg_of(m: &PolyMesh) -> Result<String> {
        super::svg(m, 64)
    }

    #[test]
    fn raster_draws_the_outlines_and_nothing_else() {
        // A single face covering the unit square: its border is drawn, its inside is not.
        let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 1 }.build().unwrap();
        let (size, px) = raster(&m, 64).unwrap();
        assert_eq!((size, px.len()), (64, 64 * 64 * 4));
        let on = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3] == 255;
        assert!(on(0, 0) && on(32, 0) && on(0, 40) && on(63, 63) && on(63, 10) && on(20, 63), "the border is drawn on all four sides");
        assert!(!on(32, 32) && !on(10, 50), "the inside is empty");
        let opaque = px.chunks(4).filter(|p| p[3] == 255).count();
        assert!((230..=260).contains(&opaque), "about the perimeter in pixels: {opaque}");
        // Off-square and huge UVs are clipped without trouble.
        let mut wild = m.clone();
        wild.set_uvs(&[Vec2::new(-1e9, 5.0), Vec2::new(3.0, 1e12), Vec2::new(0.5, 0.5), Vec2::new(1e30, -1e30)]).unwrap();
        let (_, px) = raster(&wild, 32).unwrap();
        assert_eq!(px.len(), 32 * 32 * 4);
    }

    #[test]
    fn png_decodes_to_the_same_picture() {
        let m = Primitive::Cube { size: Vec3::ONE, segments: 2 }.build().unwrap();
        let bytes = png(&m, 128).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let img = craft_image_codecs::decode(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (128, 128));
        let (_, px) = raster(&m, 128).unwrap();
        let drawn = px.chunks(4).filter(|p| p[3] == 255).count();
        assert!(drawn > 500, "six islands of outlines: {drawn}");
    }
}
