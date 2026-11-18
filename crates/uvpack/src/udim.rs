//! UDIM tiles: a layout that spans several unit squares ("tiles") laid out side by side.
//!
//! A tile is numbered `1001 + u + 10 v` by its column `u` (0 to 9, rightwards) and its row `v`
//! (upwards, as the numbers are used by texture painting tools). ModelCraft stores UVs with v
//! growing downwards (see the crate docs), so tile `(u, v)` covers `u..u+1` across and `−v..1−v`
//! down: row 0 is the usual unit square, row 1 lies above it, at negative stored v. Exporting v
//! flipped (`1 − v`, as OBJ and FBX store it) therefore gives the tile numbers painters expect.
//!
//! Packing into tiles deals the islands out, largest first, to the tile with the least area so
//! far; packs each tile with the MaxRects packer of [`crate::pack`]; and then shrinks every
//! tile's layout to the smallest scale any tile needed, so the texel density is the same in all
//! tiles. With a target texel density, the number of tiles is the smallest that holds the islands
//! at that density.

use glam::{Mat2, Vec2};
use modelcraft_mesh::PolyMesh;

use crate::pack::{Density, PackIsland, PackOptions, Placement, Rotation, pack};
use crate::{MAX_FACES, PackError, uv_islands};

/// Most tiles a UDIM layout has: ten columns by ten rows.
pub const MAX_TILES: u32 = 100;
/// Tiles per row: the tile number counts 1001 + column + 10 row.
pub const TILE_COLUMNS: u32 = 10;

/// The tile `(column, row)` a UV point lies in (row counted upwards; see the module docs).
pub fn tile_of(uv: Vec2) -> (i32, i32) {
    ((uv.x.floor()) as i32, (1.0 - uv.y).floor() as i32)
}

/// The UDIM number of tile `(column, row)`, `None` outside 1001 to 1100.
pub fn udim_number(column: i32, row: i32) -> Option<u32> {
    if (0..TILE_COLUMNS as i32).contains(&column) && (0..(MAX_TILES / TILE_COLUMNS) as i32).contains(&row) {
        Some(1001 + column as u32 + TILE_COLUMNS * row as u32)
    } else {
        None
    }
}

/// The tile `(column, row)` of a UDIM number.
pub fn tile_of_number(number: u32) -> Option<(i32, i32)> {
    let k = number.checked_sub(1001)?;
    (k < MAX_TILES).then_some(((k % TILE_COLUMNS) as i32, (k / TILE_COLUMNS) as i32))
}

/// The UDIM numbers of the tiles a mesh's faces lie in (by the centre of each face's UVs), in
/// increasing order. Faces outside tiles 1001 to 1100 are not counted.
pub fn udim_tiles(mesh: &PolyMesh) -> Vec<u32> {
    if !mesh.has_uvs() {
        return Vec::new();
    }
    let mut seen = std::collections::BTreeSet::new();
    for f in 0..mesh.face_count() as u32 {
        let (mut sum, mut n) = (Vec2::ZERO, 0.0f32);
        for h in mesh.face_half_edges(f) {
            sum += mesh.uv(h);
            n += 1.0;
        }
        if n > 0.0 {
            let (c, r) = tile_of(sum / n);
            if let Some(k) = udim_number(c, r) {
                seen.insert(k);
            }
        }
    }
    seen.into_iter().collect()
}

/// How many tiles to fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UdimTiles {
    /// Exactly this many (1 to [`MAX_TILES`]); the texel density is as high as they allow.
    Count(u32),
    /// As many as needed (at most `max_tiles`) for this texel density, in UV units per metre
    /// of surface (`√(UV area / surface area)`); the islands then have exactly that density.
    Density { texel_density: f32, max_tiles: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UdimOptions {
    /// Gap between islands in each tile, as a fraction of the tile (0 to 0.1).
    pub margin: f32,
    pub rotation: Rotation,
    /// Scale every island to the same texel density (needs surface areas), or keep their sizes.
    pub density: Density,
    pub tiles: UdimTiles,
}

impl Default for UdimOptions {
    fn default() -> Self {
        UdimOptions { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform, tiles: UdimTiles::Count(1) }
    }
}

/// The outcome of [`pack_udim`].
#[derive(Clone, Debug, PartialEq)]
pub struct UdimResult {
    /// One per island, in the order given: the map into the tile's place in the whole layout.
    pub placements: Vec<Placement>,
    /// The tile index (0 for 1001, 1 for 1002, …) of each island.
    pub tile: Vec<u32>,
    /// Tiles with at least one island, as UDIM numbers, in increasing order.
    pub tiles_used: Vec<u32>,
    /// Scale shared by all tiles, on top of the density scaling.
    pub scale: f32,
    /// Share of each used tile (in `tiles_used` order) the islands cover.
    pub utilisation: Vec<f32>,
}

fn scaled(island: &PackIsland, k: f32) -> PackIsland {
    PackIsland { triangles: island.triangles.iter().map(|t| t.map(|p| p * k)).collect(), area_3d: island.area_3d }
}

/// Pack `islands` into tiles. See the module docs.
pub fn pack_udim(islands: &[PackIsland], opts: &UdimOptions) -> Result<UdimResult, PackError> {
    if islands.is_empty() {
        return Ok(UdimResult { placements: Vec::new(), tile: Vec::new(), tiles_used: Vec::new(), scale: 1.0, utilisation: Vec::new() });
    }
    if islands.iter().any(|i| i.triangles.iter().flatten().any(|p| !p.is_finite())) {
        return Err(PackError::NonFinite);
    }
    // Density first, here, so that it is the same across tiles: `pack` would equalise only within
    // one tile. Each island is scaled about the origin to the density wanted.
    let areas: Vec<f32> = islands.iter().map(PackIsland::uv_area).collect();
    let target_of = |wanted: Option<f32>| -> Vec<f32> {
        let (a3, auv): (f32, f32) =
            islands.iter().zip(&areas).filter(|(i, a)| i.area_3d > 0.0 && **a > 0.0).fold((0.0, 0.0), |(s3, su), (i, a)| (s3 + i.area_3d, su + a));
        let target = wanted.unwrap_or(if a3 > 0.0 { (auv / a3).sqrt() } else { 1.0 });
        islands
            .iter()
            .zip(&areas)
            .map(|(i, a)| if opts.density == Density::Uniform && i.area_3d > 0.0 && *a > 0.0 && target > 0.0 { target * (i.area_3d / *a).sqrt() } else { 1.0 })
            .collect()
    };
    let (density_scale, fixed_density, max_tiles) = match opts.tiles {
        UdimTiles::Count(n) => (target_of(None), false, n.clamp(1, MAX_TILES)),
        UdimTiles::Density { texel_density, max_tiles } => {
            if !(texel_density.is_finite() && texel_density > 0.0) {
                return Err(PackError::NonFinite);
            }
            if opts.density != Density::Uniform {
                return Err(PackError::Invalid("a texel density needs every island to have its surface area (uniform density)".into()));
            }
            (target_of(Some(texel_density)), true, max_tiles.clamp(1, MAX_TILES))
        }
    };
    let normalised: Vec<PackIsland> = islands.iter().zip(&density_scale).map(|(i, k)| scaled(i, *k)).collect();
    let norm_areas: Vec<f32> = normalised.iter().map(PackIsland::uv_area).collect();
    let total: f32 = norm_areas.iter().sum();
    let pack_opts = PackOptions { margin: opts.margin, rotation: opts.rotation, density: Density::Keep };

    // Try `n` tiles: deal out the islands, pack each tile, and report the common scale reached.
    let attempt = |n: u32| -> Result<(Vec<u32>, Vec<Vec<usize>>, Vec<crate::pack::PackResult>, f32), PackError> {
        let mut order: Vec<usize> = (0..normalised.len()).collect();
        order.sort_by(|a, b| norm_areas[*b].total_cmp(&norm_areas[*a]).then(a.cmp(b)));
        let mut load = vec![0.0f32; n as usize];
        let mut members: Vec<Vec<usize>> = vec![Vec::new(); n as usize];
        let mut tile = vec![0u32; normalised.len()];
        for i in order {
            let k = (0..n as usize).min_by(|a, b| load[*a].total_cmp(&load[*b]).then(a.cmp(b))).unwrap_or(0);
            load[k] += norm_areas[i];
            members[k].push(i);
            tile[i] = k as u32;
        }
        let mut results = Vec::with_capacity(n as usize);
        let mut common = f32::MAX;
        for m in &members {
            let subset: Vec<PackIsland> = m.iter().filter_map(|i| normalised.get(*i).cloned()).collect();
            let r = pack(&subset, &pack_opts)?;
            if !m.is_empty() {
                common = common.min(r.scale);
            }
            results.push(r);
        }
        Ok((tile, members, results, common))
    };

    let (tile, members, results, common) = if fixed_density {
        // The fewest tiles that hold everything at the density asked for (scale at least 1).
        let first = (total.ceil() as u32).clamp(1, max_tiles);
        let mut chosen = None;
        for n in first..=max_tiles {
            let a = attempt(n)?;
            if a.3 >= 1.0 {
                chosen = Some(a);
                break;
            }
        }
        chosen.ok_or(PackError::NoRoom)?
    } else {
        attempt(max_tiles)?
    };
    // Every tile at the same scale: the common one, or exactly 1 for a fixed density.
    let want = if fixed_density { 1.0 } else { common };
    let n_tiles = members.len();
    let mut placements = vec![Placement { matrix: Mat2::IDENTITY, offset: Vec2::ZERO }; islands.len()];
    for (k, (m, r)) in members.iter().zip(&results).enumerate() {
        let shrink = if r.scale > 0.0 { want / r.scale } else { 1.0 };
        let (col, row) = ((k as u32 % TILE_COLUMNS) as f32, (k as u32 / TILE_COLUMNS) as f32);
        for (&i, p) in m.iter().zip(&r.placements) {
            // Placement of the normalised island, then the density scaling in front of it.
            let matrix = p.matrix * shrink * density_scale[i];
            let offset = p.offset * shrink + Vec2::new(col, -row);
            placements[i] = Placement { matrix, offset };
        }
    }
    let used: Vec<usize> = (0..n_tiles).filter(|k| !members[*k].is_empty()).collect();
    let tiles_used: Vec<u32> = used.iter().map(|k| 1001 + *k as u32 % TILE_COLUMNS + TILE_COLUMNS * (*k as u32 / TILE_COLUMNS)).collect();
    let utilisation = used
        .iter()
        .map(|k| members[*k].iter().map(|i| f64::from(areas[*i]) * f64::from(placements[*i].matrix.determinant().abs())).sum::<f64>() as f32)
        .collect();
    Ok(UdimResult { placements, tile, tiles_used, scale: want, utilisation })
}

/// Pack the islands of `mesh` into tiles. Returns one UV per corner and what the packer did.
pub fn pack_udim_mesh(mesh: &PolyMesh, opts: &UdimOptions) -> Result<(Vec<Vec2>, UdimResult), PackError> {
    if !mesh.has_uvs() {
        return Err(PackError::NoUvs);
    }
    if mesh.face_count() > MAX_FACES {
        return Err(PackError::TooLarge(mesh.face_count()));
    }
    // Each island is moved by its own map, so where it lay before (in any tile) does not matter.
    let groups = uv_islands(mesh);
    if groups.is_empty() {
        return Err(PackError::NoFaces);
    }
    let islands = crate::pack_islands(mesh, &groups);
    let result = pack_udim(&islands, opts)?;
    let mut uvs: Vec<Vec2> = mesh.corner_uvs().to_vec();
    for (group, place) in groups.iter().zip(&result.placements) {
        for &f in group {
            for h in mesh.face_half_edges(f) {
                if let Some(u) = uvs.get_mut(h as usize) {
                    *u = place.apply(mesh.uv(h));
                }
            }
        }
    }
    Ok((uvs, result))
}
