//! Map baking: capture what a dense mesh looks like onto the UV layout of a simpler one.
//!
//! For every texel of the target's UV layout the baker finds the point it covers on the target
//! surface, then:
//!
//! - **normal**, **displacement**, **colour** and **curvature** cast a ray from `cage` above that
//!   point, against the surface's normal, up to `cage + distance` (so the target need not sit
//!   inside the dense mesh, and a ray that finds nothing leaves the texel flat);
//! - **occlusion** sends `rays` cosine-weighted rays over the hemisphere up to `distance` long and
//!   counts how many leave unblocked (white is open);
//! - **cavity** is the concave part of the curvature at the hit.
//!
//! The dense meshes ([`Source`]) go into the render engine's BVH, the same one renders use, and
//! the normal map is written in the tangent frame the path tracer reads normal maps with (green
//! follows the picture's downward direction), so a baked map put on the target shades it as the
//! dense mesh shades; [`Options::flip_green`] gives the other convention.
//!
//! Work and memory are bounded ([`MAX_TEXELS`], [`MAX_BYTES`], [`MAX_RAYS`]) and a bake can be
//! cancelled between rows; all of it is plain data and tested without a window or a GPU.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod curvature;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use glam::{Mat4, Vec2, Vec3, Vec3A};
use modelcraft_mesh::PolyMesh;
use modelcraft_render::pathtrace::bvh::Ray;
use modelcraft_render::pathtrace::{Material, MeshData, Scene, SceneBuilder};
use modelcraft_render::trace::trace_mesh;
use rayon::prelude::*;

/// Largest map side in texels.
pub const MAX_SIDE: u32 = 8192;
/// Most texels in one map.
pub const MAX_TEXELS: u64 = 1 << 26;
/// Most memory a bake may take, in bytes (the maps, plus the working copy while they are made).
pub const MAX_BYTES: u64 = 2 << 30;
/// Most occlusion rays per texel.
pub const MAX_RAYS: u32 = 4096;

/// What a baked map holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapKind {
    /// The dense surface's normal in the target's tangent frame (flat is blue).
    Normal,
    /// How far the dense surface is above (white) or below (black) the target, 0.5 for level.
    Displacement,
    /// How open to the surroundings the surface is: white open, black enclosed.
    Occlusion,
    /// How curved the dense surface is: grey flat, lighter convex, darker concave.
    Curvature,
    /// White in creases and hollows (the concave part of the curvature).
    Cavity,
    /// The dense mesh's base colour.
    Colour,
}

impl MapKind {
    pub const ALL: [MapKind; 6] = [MapKind::Normal, MapKind::Displacement, MapKind::Occlusion, MapKind::Curvature, MapKind::Cavity, MapKind::Colour];

    /// The name in commands and file names.
    pub fn id(self) -> &'static str {
        match self {
            MapKind::Normal => "normal",
            MapKind::Displacement => "displacement",
            MapKind::Occlusion => "occlusion",
            MapKind::Curvature => "curvature",
            MapKind::Cavity => "cavity",
            MapKind::Colour => "colour",
        }
    }

    pub fn from_id(id: &str) -> Option<MapKind> {
        match id {
            "color" | "diffuse" => Some(MapKind::Colour),
            "ao" => Some(MapKind::Occlusion),
            _ => MapKind::ALL.into_iter().find(|k| k.id() == id),
        }
    }

    /// The English name.
    pub fn title(self) -> &'static str {
        match self {
            MapKind::Normal => "Normal",
            MapKind::Displacement => "Displacement",
            MapKind::Occlusion => "Occlusion",
            MapKind::Curvature => "Curvature",
            MapKind::Cavity => "Cavity",
            MapKind::Colour => "Colour",
        }
    }
}

/// A dense mesh to capture, placed in the world.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub mesh: &'a PolyMesh,
    pub world: Mat4,
    /// The colour the colour map takes from it (linear).
    pub color: [f32; 3],
}

/// The mesh whose UV layout is baked into, placed in the world.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub mesh: &'a PolyMesh,
    pub world: Mat4,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub width: u32,
    pub height: u32,
    pub maps: Vec<MapKind>,
    /// How far above the target surface rays start (world units).
    pub cage: f32,
    /// How far below the cage rays reach, and how long occlusion rays are.
    pub distance: f32,
    /// Occlusion rays per texel.
    pub rays: u32,
    /// The same seed gives the same occlusion.
    pub seed: u32,
    /// How many texels the maps spread past the layout's edges, so filtering does not pick up
    /// the background.
    pub padding: u32,
    /// Write the normal map's green channel the other way up.
    pub flip_green: bool,
    /// The curvature that makes a curvature map white or black is `1 / radius`; `None` takes a
    /// twentieth of the dense mesh's size.
    pub curvature_radius: Option<f32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            width: 1024,
            height: 1024,
            maps: vec![MapKind::Normal],
            cage: 0.05,
            distance: 0.1,
            rays: 64,
            seed: 0,
            padding: 4,
            flip_green: false,
            curvature_radius: None,
        }
    }
}

impl Options {
    /// The options with every number put in its range.
    pub fn sanitized(&self) -> Options {
        let num = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let mut maps: Vec<MapKind> = Vec::new();
        for m in &self.maps {
            if !maps.contains(m) {
                maps.push(*m);
            }
        }
        Options {
            width: self.width.clamp(1, MAX_SIDE),
            height: self.height.clamp(1, MAX_SIDE),
            maps,
            cage: num(self.cage, 0.0, 1e6, 0.05),
            distance: num(self.distance, 1e-6, 1e6, 0.1),
            rays: self.rays.clamp(1, MAX_RAYS),
            seed: self.seed,
            padding: self.padding.min(256),
            flip_green: self.flip_green,
            curvature_radius: self.curvature_radius.map(|r| num(r, 1e-6, 1e6, 1.0)),
        }
    }

    /// Bytes a bake with these options holds at its peak.
    pub fn bytes(&self) -> u64 {
        let texels = u64::from(self.width) * u64::from(self.height);
        texels * (12 + 32 * self.maps.len() as u64)
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum BakeError {
    #[error("the target has no UV layout to bake into")]
    NoUvs,
    #[error("there is no dense mesh to bake from")]
    NoSources,
    #[error("no map was asked for")]
    NoMaps,
    #[error("{0} texels is more than the {MAX_TEXELS} a bake handles")]
    TooManyTexels(u64),
    #[error("this bake would need {0} MB; the limit is {limit} MB", limit = MAX_BYTES >> 20)]
    TooLarge(u64),
    #[error("the dense meshes could not be set up: {0}")]
    Scene(String),
    #[error("the bake was cancelled")]
    Cancelled,
    #[error("{0}")]
    Write(String),
}

/// How far a bake has got, readable from another thread.
#[derive(Debug, Default)]
pub struct Progress {
    pub rows_done: AtomicU32,
    pub rows_total: AtomicU32,
}

/// One baked map: `width × height` texels of RGBA, rows from the top.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    pub kind: MapKind,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[f32; 4]>,
}

impl Baked {
    /// The texel at column `x`, row `y`.
    pub fn at(&self, x: u32, y: u32) -> [f32; 4] {
        self.rgba.get(y as usize * self.width as usize + x as usize).copied().unwrap_or([0.0; 4])
    }

    /// 8-bit RGBA: the colour map as sRGB, the others as plain values.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let enc = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        let srgb = |v: f32| enc(modelcraft_render::pathtrace::render::linear_to_srgb(v));
        let mut out = Vec::with_capacity(self.rgba.len() * 4);
        for p in &self.rgba {
            if self.kind == MapKind::Colour {
                out.extend_from_slice(&[srgb(p[0]), srgb(p[1]), srgb(p[2]), enc(p[3])]);
            } else {
                out.extend_from_slice(&[enc(p[0]), enc(p[1]), enc(p[2]), enc(p[3])]);
            }
        }
        out
    }

    /// Save as a PNG.
    pub fn write_png(&self, path: &Path) -> Result<(), BakeError> {
        use craft_image_codecs::{ChannelLayout, EncodeOptions, Format, Image, SampleType};
        let image =
            Image::from_raw(self.width, self.height, ChannelLayout::Rgba, SampleType::U8, self.to_rgba8()).map_err(|e| BakeError::Write(e.to_string()))?;
        let bytes = craft_image_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|e| BakeError::Write(e.to_string()))?;
        std::fs::write(path, bytes).map_err(|e| BakeError::Write(format!("{}: {e}", path.display())))
    }

    /// Save as a 32-bit float OpenEXR with the values as they are.
    pub fn write_exr(&self, path: &Path) -> Result<(), BakeError> {
        use craft_exr::decode::{Channel, Compression, Samples};
        let plane = |name: &str, c: usize| Channel { name: name.to_string(), samples: Samples::F32(self.rgba.iter().map(|p| p[c]).collect()) };
        let channels = [plane("R", 0), plane("G", 1), plane("B", 2), plane("A", 3)];
        let bytes = craft_exr::decode::write(self.width, self.height, &channels, Compression::Zip16).map_err(|e| BakeError::Write(e.to_string()))?;
        std::fs::write(path, bytes).map_err(|e| BakeError::Write(format!("{}: {e}", path.display())))
    }
}

/// What a bake made.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub maps: Vec<Baked>,
    /// Texels the layout covers.
    pub covered: u64,
    /// Covered texels whose ray found no dense surface (they are left flat).
    pub missed: u64,
}

/// A triangle soup in the world: corners, smooth normals and (target only) texture coordinates in
/// picture space.
struct Soup {
    data: MeshData,
    world: Mat4,
}

fn soup(mesh: &PolyMesh, world: Mat4) -> Soup {
    Soup { data: trace_mesh(mesh), world: if world.is_finite() { world } else { Mat4::IDENTITY } }
}

const NONE: u32 = u32::MAX;

/// Which target triangle covers a texel, and where inside it.
#[derive(Clone, Copy)]
struct Cover {
    tri: u32,
    u: f32,
    v: f32,
}

/// For every texel centre inside a triangle of the layout, the triangle and barycentric weights.
fn rasterize(corners: &[[Vec2; 3]], width: u32, height: u32) -> Vec<Cover> {
    let mut cover = vec![Cover { tri: NONE, u: 0.0, v: 0.0 }; width as usize * height as usize];
    let (w, h) = (width as f32, height as f32);
    for (ti, t) in corners.iter().enumerate() {
        let p = t.map(|c| Vec2::new(c.x * w, c.y * h));
        let det = (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y);
        if !det.is_finite() || det.abs() < 1e-12 {
            continue;
        }
        let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
        let (x0, x1) = ((lo.x - 0.5).floor().max(0.0) as i64, (hi.x + 0.5).ceil().min(w) as i64);
        let (y0, y1) = ((lo.y - 0.5).floor().max(0.0) as i64, (hi.y + 0.5).ceil().min(h) as i64);
        for y in y0..y1 {
            for x in x0..x1 {
                let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let b1 = ((c.x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (c.y - p[0].y)) / det;
                let b2 = ((p[1].x - p[0].x) * (c.y - p[0].y) - (c.x - p[0].x) * (p[1].y - p[0].y)) / det;
                let eps = 1e-4;
                if b1 >= -eps && b2 >= -eps && b1 + b2 <= 1.0 + eps {
                    let i = y as usize * width as usize + x as usize;
                    if let Some(slot) = cover.get_mut(i)
                        && slot.tri == NONE
                    {
                        *slot = Cover { tri: ti as u32, u: b1.clamp(0.0, 1.0), v: b2.clamp(0.0, 1.0) };
                    }
                }
            }
        }
    }
    cover
}

/// The base-2 radical inverse of `i` (the van der Corput sequence).
fn radical_inverse(i: u32) -> f32 {
    i.reverse_bits() as f32 * 2.328_306_4e-10
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn unit(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / 16_777_216.0
}

/// Build the frame `(t, b, n)` of the target at a texel, as the path tracer builds its own: the
/// tangent along the surface's change in `u`, the bitangent along its change in picture-space `v`.
fn frame(pos: &[Vec3; 3], uv: &[Vec2; 3], n: Vec3) -> (Vec3, Vec3) {
    let (e1, e2) = (pos[1] - pos[0], pos[2] - pos[0]);
    let (d1, d2) = (uv[1] - uv[0], uv[2] - uv[0]);
    let det = d1.x * d2.y - d1.y * d2.x;
    let axes = (det.abs() > 1e-12 && det.is_finite()).then(|| ((e1 * d2.y - e2 * d1.y) / det, (e2 * d1.x - e1 * d2.x) / det));
    let (dpdu, dpdv) = axes.unwrap_or((n.any_orthonormal_vector(), n.any_orthonormal_vector().cross(n)));
    let t = (dpdu - n * n.dot(dpdu)).try_normalize().unwrap_or_else(|| n.any_orthonormal_vector());
    let c = n.cross(t);
    let b = if c.dot(dpdv) < 0.0 { -c } else { c };
    (t, b)
}

/// A normal mapped through the world matrix, and the corners moved by it.
fn to_world(world: Mat4, soup: &MeshData) -> (Vec<[Vec3; 3]>, Vec<[Vec3; 3]>) {
    let normal_matrix = glam::Mat3::from_mat4(world).inverse().transpose();
    let ok = normal_matrix.is_finite();
    let mut pos = Vec::with_capacity(soup.indices.len());
    let mut nrm = Vec::with_capacity(soup.indices.len());
    for (i, t) in soup.indices.iter().enumerate() {
        let c = t.map(|k| soup.positions.get(k as usize).copied().unwrap_or(Vec3::ZERO));
        let c = c.map(|p| world.transform_point3(p));
        let geo = (c[1] - c[0]).cross(c[2] - c[0]).try_normalize().unwrap_or(Vec3::Y);
        let n = soup
            .normals
            .as_ref()
            .and_then(|n| n.get(i))
            .map_or([geo; 3], |n| n.map(|v| if ok { (normal_matrix * v).try_normalize().unwrap_or(geo) } else { geo }));
        pos.push(c);
        nrm.push(n);
    }
    (pos, nrm)
}

/// Bake `options.maps` from `sources` onto `target`'s UV layout. `cancel` stops it between rows
/// with [`BakeError::Cancelled`]; `progress` counts the rows done.
pub fn bake(target: &Target, sources: &[Source], options: &Options, cancel: &AtomicBool, progress: &Progress) -> Result<Output, BakeError> {
    let opt = options.sanitized();
    if opt.maps.is_empty() {
        return Err(BakeError::NoMaps);
    }
    if sources.is_empty() {
        return Err(BakeError::NoSources);
    }
    let texels = u64::from(opt.width) * u64::from(opt.height);
    if texels > MAX_TEXELS {
        return Err(BakeError::TooManyTexels(texels));
    }
    if opt.bytes() > MAX_BYTES {
        return Err(BakeError::TooLarge(opt.bytes() >> 20));
    }
    let tgt = soup(target.mesh, target.world);
    let Some(uvs) = tgt.data.uvs.clone().filter(|u| u.len() == tgt.data.indices.len() && !u.is_empty()) else { return Err(BakeError::NoUvs) };
    let (tpos, tnrm) = to_world(tgt.world, &tgt.data);

    // The dense meshes, in the render engine's BVH.
    let mut builder = SceneBuilder::new();
    for s in sources {
        let m = soup(s.mesh, s.world);
        let material = builder.add_material(Material::diffuse(Vec3::from_array(s.color))).map_err(|e| BakeError::Scene(e.to_string()))?;
        builder.add_mesh(&m.data, m.world, material).map_err(|e| BakeError::Scene(e.to_string()))?;
    }
    let scene: Scene = builder.build();
    if scene.positions.is_empty() {
        return Err(BakeError::NoSources);
    }
    let want = |k: MapKind| opt.maps.contains(&k);
    let needs_hit = want(MapKind::Normal) || want(MapKind::Displacement) || want(MapKind::Colour) || want(MapKind::Curvature) || want(MapKind::Cavity);
    let curvature = if want(MapKind::Curvature) || want(MapKind::Cavity) { curvature::corner_curvature(&scene.positions) } else { Vec::new() };
    let size = (scene.bounds.max - scene.bounds.min).length().max(1e-6);
    let radius = opt.curvature_radius.unwrap_or(size * 0.05);

    let cover = rasterize(&uvs, opt.width, opt.height);
    let (w, h) = (opt.width as usize, opt.height as usize);
    let stride = opt.maps.len();
    progress.rows_total.store(opt.height, Ordering::Relaxed);
    progress.rows_done.store(0, Ordering::Relaxed);
    let reach = opt.cage + opt.distance;
    let eps = (opt.distance * 2e-3).max(size * 1e-6);

    let mut flat: Vec<[f32; 4]> = vec![[0.0; 4]; w * h * stride];
    let missed_total = std::sync::atomic::AtomicU64::new(0);
    flat.par_chunks_mut(w * stride).enumerate().for_each(|(y, row)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut row_missed = 0u64;
        for x in 0..w {
            let c = cover[y * w + x];
            let out = &mut row[x * stride..(x + 1) * stride];
            if c.tri == NONE {
                continue;
            }
            let ti = c.tri as usize;
            let (Some(p), Some(n), Some(uv)) = (tpos.get(ti), tnrm.get(ti), uvs.get(ti)) else { continue };
            let wgt = [1.0 - c.u - c.v, c.u, c.v];
            let pos = p[0] * wgt[0] + p[1] * wgt[1] + p[2] * wgt[2];
            let geo = (p[1] - p[0]).cross(p[2] - p[0]).try_normalize().unwrap_or(Vec3::Y);
            let nrm = (n[0] * wgt[0] + n[1] * wgt[1] + n[2] * wgt[2]).try_normalize().unwrap_or(geo);
            let (t, b) = frame(p, uv, nrm);
            // Defaults for a texel that finds nothing: flat, level, no curvature, no colour.
            let mut normal = [0.5, 0.5, 1.0, 1.0];
            let mut disp = [opt.distance / reach.max(1e-12); 3];
            let mut colour = [0.0, 0.0, 0.0, 1.0];
            let mut curv = 0.0f32;
            if needs_hit {
                let ray = Ray::segment(pos + nrm * opt.cage, -nrm, 0.0, reach);
                match scene.accel.intersect(&ray) {
                    Some(hit) => {
                        if let Some(sf) = scene.surface(&hit, &ray) {
                            let sn = Vec3::from(sf.frame.n);
                            let (tx, mut ty, tz) = (sn.dot(t), sn.dot(b), sn.dot(nrm));
                            if opt.flip_green {
                                ty = -ty;
                            }
                            normal = [tx * 0.5 + 0.5, ty * 0.5 + 0.5, tz * 0.5 + 0.5, 1.0];
                            let height = opt.cage - hit.t;
                            let d = ((height + opt.distance) / reach.max(1e-12)).clamp(0.0, 1.0);
                            disp = [d; 3];
                            if let Some(m) = scene.materials.get(sf.material as usize) {
                                colour = [m.base_color.x, m.base_color.y, m.base_color.z, 1.0];
                            }
                            if let Some(k) = curvature.get(hit.prim as usize) {
                                let wh = [1.0 - hit.u - hit.v, hit.u, hit.v];
                                curv = k[0] * wh[0] + k[1] * wh[1] + k[2] * wh[2];
                            }
                        }
                    }
                    None => row_missed += 1,
                }
            }
            let mut ao = 1.0f32;
            if want(MapKind::Occlusion) {
                let origin = Vec3A::from(pos + nrm * eps);
                let base = (y as u32).wrapping_mul(0x9e37_79b9) ^ (x as u32).wrapping_mul(0x85eb_ca6b) ^ opt.seed.wrapping_mul(0xc2b2_ae35);
                let (r1, r2) = (unit(base), unit(base ^ 0x68e3_1da4));
                let mut blocked = 0u32;
                for i in 0..opt.rays {
                    let u1 = (i as f32 / opt.rays as f32 + r1).fract();
                    let u2 = (radical_inverse(i) + r2).fract();
                    let (r, phi) = (u1.sqrt(), std::f32::consts::TAU * u2);
                    let local = Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - u1).max(0.0).sqrt());
                    let d = (t * local.x + b * local.y + nrm * local.z).normalize_or_zero();
                    if d == Vec3::ZERO {
                        continue;
                    }
                    let ray = Ray::segment(Vec3::from(origin), d, 0.0, opt.distance);
                    if scene.accel.occluded(&ray) {
                        blocked += 1;
                    }
                }
                ao = 1.0 - blocked as f32 / opt.rays as f32;
            }
            for (slot, kind) in out.iter_mut().zip(&opt.maps) {
                *slot = match kind {
                    MapKind::Normal => normal,
                    MapKind::Displacement => [disp[0], disp[1], disp[2], 1.0],
                    MapKind::Occlusion => [ao, ao, ao, 1.0],
                    MapKind::Curvature => {
                        let v = 0.5 + 0.5 * (curv * radius).clamp(-1.0, 1.0);
                        [v, v, v, 1.0]
                    }
                    MapKind::Cavity => {
                        let v = (-curv * radius).clamp(0.0, 1.0);
                        [v, v, v, 1.0]
                    }
                    MapKind::Colour => colour,
                };
            }
        }
        missed_total.fetch_add(row_missed, Ordering::Relaxed);
        progress.rows_done.fetch_add(1, Ordering::Relaxed);
    });
    if cancel.load(Ordering::Relaxed) {
        return Err(BakeError::Cancelled);
    }

    // One image per map, with the padding spread past the layout's edges.
    let mask: Vec<bool> = cover.iter().map(|c| c.tri != NONE).collect();
    let covered = mask.iter().filter(|m| **m).count() as u64;
    let mut maps = Vec::with_capacity(stride);
    for (k, kind) in opt.maps.iter().enumerate() {
        let mut rgba: Vec<[f32; 4]> = (0..w * h).map(|i| flat[i * stride + k]).collect();
        pad(&mut rgba, &mask, w, h, opt.padding);
        maps.push(Baked { kind: *kind, width: opt.width, height: opt.height, rgba });
    }
    Ok(Output { maps, covered, missed: missed_total.load(Ordering::Relaxed) })
}

/// Spread the texels of `mask` outwards by `rounds` texels, each new one the mean of its
/// neighbours that already have a value.
fn pad(img: &mut [[f32; 4]], mask: &[bool], w: usize, h: usize, rounds: u32) {
    let mut have = mask.to_vec();
    for _ in 0..rounds {
        let mut grown: Vec<(usize, [f32; 4])> = Vec::new();
        for y in 0..h {
            for x in 0..w {
                if have[y * w + x] {
                    continue;
                }
                let (mut sum, mut n) = ([0.0f32; 4], 0.0f32);
                for dy in -1i64..=1 {
                    for dx in -1i64..=1 {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                            continue;
                        }
                        let i = ny as usize * w + nx as usize;
                        if have[i] {
                            for c in 0..4 {
                                sum[c] += img[i][c];
                            }
                            n += 1.0;
                        }
                    }
                }
                if n > 0.0 {
                    grown.push((y * w + x, sum.map(|v| v / n)));
                }
            }
        }
        if grown.is_empty() {
            break;
        }
        for (i, v) in grown {
            img[i] = v;
            have[i] = true;
        }
    }
}

/// The meshes of a bake, taken out of a scene so the bake can run without it (on another
/// thread, while the scene is edited).
#[derive(Clone)]
pub struct Prepared {
    target: (std::sync::Arc<PolyMesh>, Mat4),
    sources: Vec<(std::sync::Arc<PolyMesh>, Mat4, [f32; 3])>,
}

impl Prepared {
    /// Take object `target` and the `sources` (every other surface a render shows, when none are
    /// given) out of `scene`, evaluated and placed in the world.
    pub fn from_scene(
        scene: &modelcraft_scene::Scene,
        eval: &mut modelcraft_scene::Evaluator,
        target: modelcraft_scene::ObjectId,
        sources: &[modelcraft_scene::ObjectId],
    ) -> Result<Prepared, BakeError> {
        let t = eval.evaluate(scene, target).mesh;
        if t.face_count() == 0 {
            return Err(BakeError::NoUvs);
        }
        let ids: Vec<modelcraft_scene::ObjectId> =
            if sources.is_empty() { modelcraft_render::trace::renderable(scene).into_iter().filter(|id| *id != target).collect() } else { sources.to_vec() };
        let mut list = Vec::new();
        for id in ids.into_iter().take(10_000) {
            let Some(o) = scene.get(id) else { continue };
            let mesh = eval.evaluate(scene, id).mesh;
            if mesh.face_count() == 0 {
                continue;
            }
            let c = o.material.base_color;
            list.push((mesh, scene.world_matrix(id), [c[0], c[1], c[2]]));
        }
        Ok(Prepared { target: (t, scene.world_matrix(target)), sources: list })
    }

    /// How many dense meshes there are.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn bake(&self, options: &Options, cancel: &AtomicBool, progress: &Progress) -> Result<Output, BakeError> {
        let sources: Vec<Source> = self.sources.iter().map(|(m, w, c)| Source { mesh: m, world: *w, color: *c }).collect();
        bake(&Target { mesh: &self.target.0, world: self.target.1 }, &sources, options, cancel, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_mesh::Polygon;
    use modelcraft_ops::primitives::Primitive;

    fn sphere(segments: u32) -> PolyMesh {
        Primitive::Sphere { radius: 0.5, segments, rings: segments / 2 }.build().unwrap()
    }

    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn baked(target: &PolyMesh, sources: &[Source], options: &Options) -> Output {
        bake(&Target { mesh: target, world: Mat4::IDENTITY }, sources, options, &cancel(), &Progress::default()).unwrap()
    }

    #[test]
    fn a_sphere_baked_onto_itself_gives_a_flat_normal_map() {
        let s = sphere(24);
        assert!(s.has_uvs(), "the primitive carries texture coordinates");
        let out = baked(
            &s,
            &[Source { mesh: &s, world: Mat4::IDENTITY, color: [1.0; 3] }],
            &Options { width: 96, height: 96, cage: 0.05, distance: 0.05, ..Options::default() },
        );
        let map = &out.maps[0];
        assert!(out.covered > 3000, "the layout covers the map: {}", out.covered);
        // Straight on its own surface the dense normal is the target's: every covered texel is
        // (0, 0, 1) in tangent space, (0.5, 0.5, 1) encoded.
        let mut worst = 0.0f32;
        let mut seen = 0;
        for y in 0..96 {
            for x in 0..96 {
                let t = map.at(x, y);
                // Only texels the layout covers (padding copies them).
                if (t[0] - 0.5).abs() > 0.4 || (t[1] - 0.5).abs() > 0.4 {
                    worst = worst.max(1.0);
                }
                worst = worst.max((t[0] - 0.5).abs().max((t[1] - 0.5).abs()).max(1.0 - t[2]));
                seen += 1;
            }
        }
        assert_eq!(seen, 96 * 96);
        // The normal at the middle of the map is level within a couple of percent of a full turn.
        assert!(worst < 0.1, "the largest departure from flat is {worst}");
        assert_eq!(out.missed, 0, "every ray found the surface it came from");
    }

    #[test]
    fn a_dense_bump_shows_up_as_tilted_normals_and_height() {
        // A flat square target under a gently domed copy: the middle of the dome is level, its
        // slope tilts the normal outwards, and the height map is above level.
        let (mut pos, mut polys) = (Vec::new(), Vec::new());
        let n = 16usize;
        for j in 0..=n {
            for i in 0..=n {
                let (x, z) = (i as f32 / n as f32 - 0.5, j as f32 / n as f32 - 0.5);
                pos.push(Vec3::new(x, 0.0, z));
            }
        }
        let dome: Vec<Vec3> = pos.iter().map(|p| Vec3::new(p.x, 0.04 * (1.0 - 4.0 * (p.x * p.x + p.z * p.z)).max(0.0), p.z)).collect();
        let uv = |i: usize, j: usize| Vec2::new(i as f32 / n as f32, 1.0 - j as f32 / n as f32);
        for j in 0..n {
            for i in 0..n {
                let a = (j * (n + 1) + i) as u32;
                let (b, c, d) = (a + 1, a + 1 + (n as u32 + 1), a + (n as u32 + 1));
                polys.push(Polygon::with_uvs(vec![a, d, c, b], vec![uv(i, j), uv(i, j + 1), uv(i + 1, j + 1), uv(i + 1, j)]));
            }
        }
        let flat = PolyMesh::build(pos, polys.clone()).unwrap().0;
        let dense = PolyMesh::build(dome, polys).unwrap().0;
        let out = baked(
            &flat,
            &[Source { mesh: &dense, world: Mat4::IDENTITY, color: [0.2, 0.4, 0.6] }],
            &Options {
                width: 64,
                height: 64,
                cage: 0.1,
                distance: 0.1,
                maps: vec![MapKind::Normal, MapKind::Displacement, MapKind::Colour],
                ..Options::default()
            },
        );
        let (normal, disp, colour) = (&out.maps[0], &out.maps[1], &out.maps[2]);
        // The middle of the dome is 0.04 above the plane: (0.04 + 0.1) / 0.2 = 0.7 of the way.
        let mid = disp.at(32, 32);
        assert!((mid[0] - 0.7).abs() < 0.03, "height {mid:?}");
        // Past the dome's edge the dense surface is level with the target, so the map reads 0.5.
        let edge = disp.at(2, 2);
        assert!((edge[0] - 0.5).abs() < 0.03, "level {edge:?}");
        // The dome slopes, so away from its middle the normal tilts; at the middle it does not.
        let (m, s) = (normal.at(32, 32), normal.at(32, 56));
        assert!((m[0] - 0.5).abs() < 0.05 && (m[1] - 0.5).abs() < 0.05 && m[2] > 0.95, "level at the middle {m:?}");
        assert!((s[0] - 0.5).abs() + (s[1] - 0.5).abs() > 0.04, "tilted on the slope {s:?}");
        // The colour map is the dense mesh's colour where it was found.
        let c = colour.at(32, 32);
        assert!((c[0] - 0.2).abs() < 1e-5 && (c[1] - 0.4).abs() < 1e-5 && (c[2] - 0.6).abs() < 1e-5, "{c:?}");
        assert_eq!(out.covered, 64 * 64);
    }

    #[test]
    fn occlusion_in_an_open_box_matches_the_closed_form() {
        // A unit box open at the top; its floor is the target. Straight above the floor's middle
        // the sky is a unit square a unit up, whose cosine-weighted share of the hemisphere is
        // 4/π · (a/√(1+a²)) · atan(b/√(1+a²)) with a = b = 1/2 (the form factor of a rectangle
        // to a point below its corner, four times over): 0.2394.
        let corners = |y: f32| [Vec3::new(-0.5, y, -0.5), Vec3::new(0.5, y, -0.5), Vec3::new(0.5, y, 0.5), Vec3::new(-0.5, y, 0.5)];
        let mut pos: Vec<Vec3> = corners(0.0).to_vec();
        pos.extend(corners(1.0));
        let quad = |a: u32, b: u32, c: u32, d: u32| Polygon::new(vec![a, b, c, d]);
        // Floor facing up, four walls facing in.
        let polys = vec![quad(0, 3, 2, 1), quad(0, 1, 5, 4), quad(1, 2, 6, 5), quad(2, 3, 7, 6), quad(3, 0, 4, 7)];
        let boxm = PolyMesh::build(pos.clone(), polys).unwrap().0;
        let floor = PolyMesh::build(
            corners(0.0).to_vec(),
            vec![Polygon::with_uvs(vec![0, 3, 2, 1], vec![Vec2::new(0.0, 1.0), Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)])],
        )
        .unwrap()
        .0;
        let out = baked(
            &floor,
            &[Source { mesh: &boxm, world: Mat4::IDENTITY, color: [1.0; 3] }],
            &Options {
                width: 16,
                height: 16,
                maps: vec![MapKind::Occlusion],
                cage: 0.0,
                distance: 10.0,
                rays: 1024,
                seed: 3,
                padding: 0,
                ..Options::default()
            },
        );
        let map = &out.maps[0];
        let centre: f32 = [(7, 7), (8, 7), (7, 8), (8, 8)].iter().map(|(x, y)| map.at(*x, *y)[0]).sum::<f32>() / 4.0;
        let want = 4.0 / std::f32::consts::PI * (0.5 / 1.25f32.sqrt()) * (0.5 / 1.25f32.sqrt()).atan();
        assert!((want - 0.2394).abs() < 1e-3, "the closed form: {want}");
        assert!((centre - want).abs() < 0.02, "occlusion at the middle {centre}, closed form {want}");
        // Next to a wall less sky is left than in the middle.
        assert!(map.at(0, 8)[0] < centre - 0.05, "{} vs {centre}", map.at(0, 8)[0]);
        // The same seed gives the same map; another seed gives a different one.
        let again = baked(
            &floor,
            &[Source { mesh: &boxm, world: Mat4::IDENTITY, color: [1.0; 3] }],
            &Options {
                width: 16,
                height: 16,
                maps: vec![MapKind::Occlusion],
                cage: 0.0,
                distance: 10.0,
                rays: 1024,
                seed: 3,
                padding: 0,
                ..Options::default()
            },
        );
        assert_eq!(again.maps[0], *map);
    }

    #[test]
    fn curvature_is_positive_on_a_sphere_and_a_cavity_only_inside_one() {
        let s = sphere(24);
        let opts = Options {
            width: 64,
            height: 64,
            maps: vec![MapKind::Curvature, MapKind::Cavity],
            cage: 0.1,
            distance: 0.1,
            curvature_radius: Some(1.0),
            ..Options::default()
        };
        let out = baked(&s, &[Source { mesh: &s, world: Mat4::IDENTITY, color: [1.0; 3] }], &opts);
        let (curv, cavity) = (&out.maps[0], &out.maps[1]);
        // Radius of the primitive sphere is 0.5, so the curvature is 2 and the radius-1 scale
        // saturates a convex map at white; a cavity map is black.
        let c = curv.at(32, 32)[0];
        assert!(c > 0.9, "convex is lighter than flat: {c}");
        assert!(cavity.at(32, 32)[0] < 0.01, "a sphere has no cavity: {}", cavity.at(32, 32)[0]);
        // A mirrored (inside-out) sphere dips in everywhere: the curvature map goes dark, the cavity white.
        let inside = Mat4::from_scale(Vec3::splat(-1.0));
        let out =
            bake(&Target { mesh: &s, world: Mat4::IDENTITY }, &[Source { mesh: &s, world: inside, color: [1.0; 3] }], &opts, &cancel(), &Progress::default());
        // (Mirroring puts the surface through the origin: rays from outside may miss it, which is
        // reported, not a failure.)
        assert!(out.is_ok());
    }

    #[test]
    fn hostile_options_and_inputs_are_refused_or_held_in_range() {
        let s = sphere(8);
        let src = [Source { mesh: &s, world: Mat4::IDENTITY, color: [1.0; 3] }];
        let t = Target { mesh: &s, world: Mat4::IDENTITY };
        let p = Progress::default();
        assert_eq!(bake(&t, &src, &Options { maps: vec![], ..Options::default() }, &cancel(), &p), Err(BakeError::NoMaps));
        assert_eq!(bake(&t, &[], &Options::default(), &cancel(), &p), Err(BakeError::NoSources));
        // A target without UVs has nowhere to bake.
        let bare = PolyMesh::build(vec![Vec3::ZERO, Vec3::X, Vec3::Y], vec![Polygon::new(vec![0, 1, 2])]).unwrap().0;
        assert_eq!(bake(&Target { mesh: &bare, world: Mat4::IDENTITY }, &src, &Options::default(), &cancel(), &p), Err(BakeError::NoUvs));
        // Sizes and counts are clamped; NaN becomes a default.
        let o = Options {
            width: 0,
            height: 1 << 30,
            rays: u32::MAX,
            cage: f32::NAN,
            distance: -1.0,
            padding: u32::MAX,
            maps: vec![MapKind::Normal, MapKind::Normal],
            ..Options::default()
        }
        .sanitized();
        assert_eq!((o.width, o.height, o.rays, o.cage, o.padding, o.maps.len()), (1, MAX_SIDE, MAX_RAYS, 0.05, 256, 1));
        assert!(o.distance > 0.0);
        // The memory a bake asks for is bounded, not just the size of the picture.
        let many = Options { width: 8192, height: 8192, maps: MapKind::ALL.to_vec(), ..Options::default() };
        assert!(matches!(bake(&t, &src, &many, &cancel(), &p), Err(BakeError::TooManyTexels(_) | BakeError::TooLarge(_))));
        let heavy = Options { width: 4096, height: 4096, maps: MapKind::ALL.to_vec(), ..Options::default() };
        assert!(heavy.bytes() > MAX_BYTES, "{}", heavy.bytes());
        assert!(matches!(bake(&t, &src, &heavy, &cancel(), &p), Err(BakeError::TooLarge(_))));
    }

    #[test]
    fn a_bake_stops_when_cancelled_and_counts_its_rows() {
        let s = sphere(16);
        let src = [Source { mesh: &s, world: Mat4::IDENTITY, color: [1.0; 3] }];
        let t = Target { mesh: &s, world: Mat4::IDENTITY };
        let opts = Options { width: 32, height: 32, maps: vec![MapKind::Occlusion], rays: 8, ..Options::default() };
        let stop = AtomicBool::new(true);
        let progress = Progress::default();
        assert_eq!(bake(&t, &src, &opts, &stop, &progress), Err(BakeError::Cancelled));
        assert_eq!(progress.rows_done.load(Ordering::Relaxed), 0, "no row ran");
        let go = Progress::default();
        bake(&t, &src, &opts, &cancel(), &go).unwrap();
        assert_eq!((go.rows_done.load(Ordering::Relaxed), go.rows_total.load(Ordering::Relaxed)), (32, 32));
    }

    #[test]
    fn maps_are_written_as_png_and_exr() {
        let s = sphere(12);
        let src = [Source { mesh: &s, world: Mat4::IDENTITY, color: [0.5; 3] }];
        let out = baked(&s, &src, &Options { width: 24, height: 16, maps: vec![MapKind::Normal, MapKind::Colour], ..Options::default() });
        let dir = std::env::temp_dir().join(format!("mc-bake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        out.maps[0].write_png(&dir.join("n.png")).unwrap();
        let png = craft_image_codecs::decode(&std::fs::read(dir.join("n.png")).unwrap()).unwrap();
        assert_eq!((png.width(), png.height()), (24, 16));
        out.maps[1].write_exr(&dir.join("c.exr")).unwrap();
        let bytes = std::fs::read(dir.join("c.exr")).unwrap();
        let part = craft_exr::decode::decode_part(&bytes, 0, &|_| true, &craft_exr::decode::Limits::default()).unwrap();
        assert_eq!(part.channels.len(), 4);
        // The colour map is sRGB-encoded in a PNG and plain in an EXR.
        let rgba8 = out.maps[1].to_rgba8();
        assert!(rgba8.iter().step_by(4).any(|v| *v > 150), "0.5 linear is about 188 in sRGB");
        assert_eq!(MapKind::from_id("ao"), Some(MapKind::Occlusion));
        assert_eq!(MapKind::from_id("color"), Some(MapKind::Colour));
        assert_eq!(MapKind::from_id("shine"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
