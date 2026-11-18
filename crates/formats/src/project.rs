//! The `.modelcraft` project: a ZIP archive.
//!
//! | Entry | Contents |
//! |---|---|
//! | `mimetype` | `application/vnd.storytold.modelcraft+zip` (stored, first) |
//! | `project.json` | `{format, schema_version, generator, saved_at, scene}`; stored meshes and sculpt layers are left empty and listed in `blobs` |
//! | `blobs/<object>-source.bin` | a stored mesh ([`crate::blob`]) |
//! | `blobs/<object>-<step>.bin` | a sculpt layer |
//! | `blobs/<object>-<step>-topology.bin` | the triangles of a sculpt layer changed by live detail (a mesh blob) |
//! | `blobs/<object>-cache.mcpc` | the baked points of a simulation object (a `.mcpc` point cache, see [`crate::pointcache`]) |
//! | `history/base.modelcraft` | optional: the scene before the oldest kept undo step, itself a project (without history) |
//! | `history/commands.json` | optional: `{format, version, done, undone}`, the commands of the undo history ([`History`]) |
//!
//! The two `history/` entries come together or not at all; a reader that does not know them
//! still opens the scene. They let undo reach back across sessions.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use modelcraft_mesh::PolyMesh;
use modelcraft_scene::{ObjectId, ObjectKind, Scene, Source, StepOp};
use serde::{Deserialize, Serialize};

use crate::{FormatError, GENERATOR, Result, blob};

pub const MIME_TYPE: &str = "application/vnd.storytold.modelcraft+zip";
pub const FORMAT_ID: &str = "modelcraft.project";
/// 2: sculpt layers carry a detail level, finer-level displacements and shape layers (layer
/// blob version 2). Schema 1 files still open.
pub const SCHEMA_VERSION: u32 = 2;
pub const EXTENSION: &str = "modelcraft";

const MAX_ENTRIES: u64 = 200_000;
const MAX_TOTAL_BYTES: u64 = 8 << 30;
const MAX_JSON_BYTES: u64 = 512 << 20;
const MAX_BLOB_BYTES: u64 = 2 << 30;

#[derive(Serialize)]
struct EnvelopeOut<'a> {
    format: &'a str,
    schema_version: u32,
    generator: &'a str,
    saved_at: String,
    scene: &'a Scene,
    blobs: &'a BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct EnvelopeIn {
    format: String,
    schema_version: u32,
    scene: serde_json::Value,
    #[serde(default)]
    blobs: BTreeMap<String, String>,
}

fn source_key(id: ObjectId) -> String {
    format!("{}/source", id.0)
}
fn layer_key(id: ObjectId, step: u64) -> String {
    format!("{}/step/{step}", id.0)
}
fn topology_key(id: ObjectId, step: u64) -> String {
    format!("{}/step/{step}/topology", id.0)
}
fn cache_key(id: ObjectId) -> String {
    format!("{}/cache", id.0)
}

fn zip_err(e: craft_zip::Error) -> FormatError {
    FormatError::Malformed(e.to_string())
}

pub const HISTORY_FORMAT_ID: &str = "modelcraft.history";
pub const HISTORY_VERSION: u32 = 1;
const BASE_ENTRY: &str = "history/base.modelcraft";
const COMMANDS_ENTRY: &str = "history/commands.json";
/// Largest command list read back.
pub const MAX_HISTORY_JSON_BYTES: u64 = 1 << 30;
/// Most history entries read back.
pub const MAX_HISTORY_ENTRIES: usize = 1_000_000;

/// The undo history stored in a project: replaying `done` on `base` gives the saved scene;
/// `undone` are the steps redo would bring back, the next one first.
#[derive(Clone, Debug, PartialEq)]
pub struct History<T> {
    pub base: Scene,
    pub done: Vec<T>,
    pub undone: Vec<T>,
}

#[derive(Serialize)]
struct HistoryOut<'a, T> {
    format: &'a str,
    version: u32,
    done: &'a [T],
    undone: &'a [T],
}

#[derive(Deserialize)]
struct HistoryIn<T> {
    format: String,
    version: u32,
    done: Vec<T>,
    #[serde(default = "Vec::new")]
    undone: Vec<T>,
}

/// Encode a scene as `.modelcraft` bytes.
pub fn to_bytes(scene: &Scene) -> Result<Vec<u8>> {
    to_bytes_with_history::<()>(scene, None)
}

/// Encode a scene and, when given, its undo history.
pub fn to_bytes_with_history<T: Serialize>(scene: &Scene, history: Option<&History<T>>) -> Result<Vec<u8>> {
    let mut light = scene.clone();
    let mut blobs: BTreeMap<String, String> = BTreeMap::new();
    let mut data: Vec<(String, Vec<u8>)> = Vec::new();
    let ids: Vec<ObjectId> = light.objects().iter().map(|o| o.id).collect();
    for id in ids {
        let Ok(o) = light.edit(id) else { continue };
        if let ObjectKind::Sim { sim } = &o.kind
            && let Some(cache) = &sim.cache
        {
            let entry = format!("blobs/{}-cache.{}", id.0, crate::pointcache::EXTENSION);
            data.push((entry.clone(), crate::pointcache::PointCache::from_frame_cache(cache).to_bytes()?));
            blobs.insert(cache_key(id), entry);
        }
        if let ObjectKind::Mesh { source, steps } = &mut o.kind {
            if let Source::Mesh { mesh } = source {
                let entry = format!("blobs/{}-source.bin", id.0);
                data.push((entry.clone(), blob::write_mesh(mesh)));
                blobs.insert(source_key(id), entry);
                *mesh = Arc::new(PolyMesh::default());
            }
            for s in steps.iter_mut() {
                if let StepOp::Sculpt { layer } = &mut s.op {
                    let entry = format!("blobs/{}-{}.bin", id.0, s.id);
                    data.push((entry.clone(), blob::write_layer(layer)));
                    layer.positions = Arc::default();
                    layer.mask = Arc::default();
                    layer.finer.clear();
                    layer.anchor = Arc::default();
                    for sh in &mut layer.shapes {
                        sh.delta = Arc::default();
                    }
                    if let Some(t) = &mut layer.topology {
                        let entry = format!("blobs/{}-{}-topology.bin", id.0, s.id);
                        data.push((entry.clone(), blob::write_mesh(t)));
                        blobs.insert(topology_key(id, s.id), entry);
                        *t = Arc::default();
                    }
                    blobs.insert(layer_key(id, s.id), entry);
                }
            }
        }
    }
    let env = EnvelopeOut {
        format: FORMAT_ID,
        schema_version: SCHEMA_VERSION,
        generator: GENERATOR,
        saved_at: chrono::Utc::now().to_rfc3339(),
        scene: &light,
        blobs: &blobs,
    };
    let json = serde_json::to_vec_pretty(&env).map_err(|e| FormatError::Malformed(e.to_string()))?;
    let mut w = craft_zip::Writer::new();
    w.add("mimetype", MIME_TYPE.as_bytes(), craft_zip::Compression::Stored).map_err(zip_err)?;
    w.add("project.json", &json, craft_zip::Compression::Deflated).map_err(zip_err)?;
    for (name, bytes) in data {
        w.add(&name, &bytes, craft_zip::Compression::Deflated).map_err(zip_err)?;
    }
    if let Some(h) = history {
        // The base is a whole project of its own; its blobs are already compressed.
        let base = to_bytes(&h.base)?;
        let json = serde_json::to_vec(&HistoryOut { format: HISTORY_FORMAT_ID, version: HISTORY_VERSION, done: &h.done, undone: &h.undone })
            .map_err(|e| FormatError::Malformed(e.to_string()))?;
        w.add(BASE_ENTRY, &base, craft_zip::Compression::Stored).map_err(zip_err)?;
        w.add(COMMANDS_ENTRY, &json, craft_zip::Compression::Deflated).map_err(zip_err)?;
    }
    w.finish().map_err(zip_err)
}

/// Decode `.modelcraft` bytes (hostile input: bounded, validated, repaired).
pub fn from_bytes(bytes: &[u8]) -> Result<Scene> {
    Ok(from_bytes_with_history::<serde::de::IgnoredAny>(bytes, false)?.0)
}

/// Decode a project and, when `with_history`, its stored undo history. A damaged or newer
/// history is dropped (the scene still opens); the string says why.
pub fn from_bytes_with_history<T: for<'de> Deserialize<'de>>(bytes: &[u8], with_history: bool) -> Result<(Scene, HistoryRead<T>)> {
    let mut zip = craft_zip::Archive::open(bytes, craft_zip::Limits { max_entries: MAX_ENTRIES, max_total_bytes: MAX_TOTAL_BYTES }).map_err(zip_err)?;
    if zip.has_duplicate_names() {
        return Err(FormatError::Malformed("two archive entries share a name".into()));
    }
    let json = zip.read("project.json", MAX_JSON_BYTES).map_err(zip_err)?.ok_or_else(|| FormatError::Malformed("project.json is missing".into()))?;
    let env: EnvelopeIn = serde_json::from_slice(&json).map_err(|e| FormatError::Malformed(format!("project.json: {e}")))?;
    if env.format != FORMAT_ID {
        return Err(FormatError::Malformed(format!("format is {:?}, expected {FORMAT_ID:?}", env.format)));
    }
    if env.schema_version > SCHEMA_VERSION {
        return Err(FormatError::NewerSchema { found: env.schema_version, supported: SCHEMA_VERSION });
    }
    if env.schema_version == 0 {
        return Err(FormatError::Malformed("schema_version 0".into()));
    }
    let scene: Scene = serde_json::from_value(env.scene).map_err(|e| FormatError::Malformed(format!("scene: {e}")))?;
    let mut scene = scene.repaired().map_err(|e| FormatError::Malformed(e.to_string()))?;
    let ids: Vec<ObjectId> = scene.objects().iter().map(|o| o.id).collect();
    for id in ids {
        let Ok(o) = scene.edit(id) else { continue };
        if let ObjectKind::Sim { sim } = &mut o.kind
            && let Some(entry) = env.blobs.get(&cache_key(id))
        {
            let b = zip.read(entry, MAX_BLOB_BYTES).map_err(zip_err)?.ok_or_else(|| FormatError::Malformed(format!("{entry} is missing")))?;
            Arc::make_mut(sim).cache = Some(Arc::new(crate::pointcache::PointCache::from_bytes(&b)?.to_frame_cache()?));
            continue;
        }
        let ObjectKind::Mesh { source, steps } = &mut o.kind else { continue };
        if let Source::Mesh { mesh } = source
            && let Some(entry) = env.blobs.get(&source_key(id))
        {
            let b = zip.read(entry, MAX_BLOB_BYTES).map_err(zip_err)?.ok_or_else(|| FormatError::Malformed(format!("{entry} is missing")))?;
            *mesh = Arc::new(blob::read_mesh(&b)?);
        }
        for s in steps.iter_mut() {
            if let StepOp::Sculpt { layer } = &mut s.op
                && let Some(entry) = env.blobs.get(&layer_key(id, s.id))
            {
                let b = zip.read(entry, MAX_BLOB_BYTES).map_err(zip_err)?.ok_or_else(|| FormatError::Malformed(format!("{entry} is missing")))?;
                let bulk = blob::read_layer(&b)?;
                layer.positions = Arc::new(bulk.positions);
                layer.mask = Arc::new(bulk.mask);
                layer.finer = bulk.finer.into_iter().map(Arc::new).collect();
                for (sh, delta) in layer.shapes.iter_mut().zip(bulk.shapes) {
                    sh.delta = Arc::new(delta);
                }
                layer.anchor = Arc::new(bulk.anchor);
                if layer.topology.is_some()
                    && let Some(entry) = env.blobs.get(&topology_key(id, s.id))
                {
                    let b = zip.read(entry, MAX_BLOB_BYTES).map_err(zip_err)?.ok_or_else(|| FormatError::Malformed(format!("{entry} is missing")))?;
                    layer.topology = Some(Arc::new(blob::read_mesh(&b)?));
                }
            }
        }
    }
    let history = if with_history { read_history(&mut zip) } else { HistoryRead::None };
    Ok((scene, history))
}

/// What [`from_bytes_with_history`] found.
#[derive(Debug)]
pub enum HistoryRead<T> {
    /// The file stores no history.
    None,
    Found(History<T>),
    /// The file stores a history that could not be read; the reason.
    Dropped(String),
}

fn read_history<T: for<'de> Deserialize<'de>>(zip: &mut craft_zip::Archive<'_>) -> HistoryRead<T> {
    let base = match zip.read(BASE_ENTRY, MAX_BLOB_BYTES) {
        Ok(Some(b)) => b,
        Ok(None) => return HistoryRead::None,
        Err(e) => return HistoryRead::Dropped(e.to_string()),
    };
    let json = match zip.read(COMMANDS_ENTRY, MAX_HISTORY_JSON_BYTES) {
        Ok(Some(j)) => j,
        Ok(None) => return HistoryRead::Dropped(format!("{COMMANDS_ENTRY} is missing")),
        Err(e) => return HistoryRead::Dropped(e.to_string()),
    };
    // The base is decoded without its own history: no nesting.
    let base = match from_bytes_with_history::<serde::de::IgnoredAny>(&base, false) {
        Ok((s, _)) => s,
        Err(e) => return HistoryRead::Dropped(format!("history base: {e}")),
    };
    let h: HistoryIn<T> = match serde_json::from_slice(&json) {
        Ok(h) => h,
        Err(e) => return HistoryRead::Dropped(format!("{COMMANDS_ENTRY}: {e}")),
    };
    if h.format != HISTORY_FORMAT_ID {
        return HistoryRead::Dropped(format!("history format is {:?}", h.format));
    }
    if h.version > HISTORY_VERSION || h.version == 0 {
        return HistoryRead::Dropped(format!("history version {} is not supported", h.version));
    }
    if h.done.len().saturating_add(h.undone.len()) > MAX_HISTORY_ENTRIES {
        return HistoryRead::Dropped("the history is too long".into());
    }
    HistoryRead::Found(History { base, done: h.done, undone: h.undone })
}

/// Save atomically: the old file stays whole until the new one is complete on disk.
pub fn save(scene: &Scene, path: &Path) -> Result<()> {
    save_with_history::<()>(scene, None, path)
}

/// Save a scene with its undo history, atomically.
pub fn save_with_history<T: Serialize>(scene: &Scene, history: Option<&History<T>>, path: &Path) -> Result<()> {
    let bytes = to_bytes_with_history(scene, history)?;
    craft_atomic::atomic_write(path, &bytes).map_err(|e| FormatError::io(path, e))
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    let meta = std::fs::metadata(path).map_err(|e| FormatError::io(path, e))?;
    if meta.len() > MAX_TOTAL_BYTES {
        return Err(FormatError::Malformed("file is too large".into()));
    }
    std::fs::read(path).map_err(|e| FormatError::io(path, e))
}

pub fn open(path: &Path) -> Result<Scene> {
    from_bytes(&read_file(path)?)
}

/// Open a project with its stored undo history.
pub fn open_with_history<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<(Scene, HistoryRead<T>)> {
    from_bytes_with_history(&read_file(path)?, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use modelcraft_ops::primitives::Primitive;
    use modelcraft_scene::{SculptLayer, Step};
    use proptest::prelude::*;

    fn sample() -> Scene {
        let mut s = Scene::new();
        let g = s.add("Group", ObjectKind::Group, None).unwrap();
        let mesh = Arc::new(Primitive::Torus { radius: 1.0, tube: 0.3, segments: 8, sides: 6 }.build().unwrap());
        let n = mesh.vertex_count();
        let id = s
            .add(
                "Ring",
                ObjectKind::Mesh {
                    source: Source::Mesh { mesh },
                    steps: vec![Step {
                        id: 7,
                        enabled: true,
                        op: StepOp::Sculpt {
                            layer: SculptLayer {
                                positions: Arc::new(vec![Vec3::ONE; n]),
                                mask: Arc::new(vec![0.25; n]),
                                finer: vec![Arc::new(vec![Vec3::X; 3])],
                                shapes: vec![modelcraft_scene::ShapeLayer { name: "Brow".into(), strength: 0.5, delta: Arc::new(vec![Vec3::Y; n]) }],
                                active_shape: Some(0),
                                anchor: Arc::new(vec![Vec3::ONE; n]),
                                level: 0,
                                topology: Some(Arc::new(Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap())),
                                patches: Arc::new(vec![3, 3, 1, 1, 1, 2]),
                                hidden: Arc::new(vec![false, true, true, false, false, false]),
                            },
                        },
                    }],
                },
                Some(g),
            )
            .unwrap();
        s.edit(id).unwrap().transform.translation = Vec3::new(1.0, 2.0, 3.0);
        s.add("Box", ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::default_of("cube").unwrap() }, steps: vec![] }, None).unwrap();
        s
    }

    fn strip_revs(s: &Scene) -> String {
        serde_json::to_value(s).unwrap()["objects"].to_string()
    }

    #[test]
    fn a_simulation_cache_is_saved_as_a_point_cache_blob_and_comes_back() {
        use modelcraft_sim::{FrameCache, SimKind, SimObject};
        let mut s = sample();
        let mesh = s.objects().iter().find(|o| o.name == "Box").unwrap().id;
        let mut cache = FrameCache::new(30.0, 4.0, 1.0, 2);
        cache.push(&[Vec3::ZERO, Vec3::X], usize::MAX).unwrap();
        cache.push(&[Vec3::Y, modelcraft_sim::DEAD], usize::MAX).unwrap();
        let mut sim = SimObject::new(SimKind::Cache);
        sim.cache = Some(Arc::new(cache.clone()));
        let id = s.add("Cache", ObjectKind::Sim { sim: Arc::new(sim) }, Some(mesh)).unwrap();
        let bytes = to_bytes(&s).unwrap();
        let json = String::from_utf8_lossy(&bytes);
        assert!(json.contains(&format!("blobs/{}-cache.mcpc", id.0)), "stored as an .mcpc entry");
        let back = from_bytes(&bytes).unwrap();
        let ObjectKind::Sim { sim } = &back.get(id).unwrap().kind else { panic!("not a simulation") };
        let got = sim.cache.as_ref().unwrap();
        assert_eq!((got.fps, got.start, got.points(), got.frames()), (30.0, 4.0, 2, 2));
        assert_eq!(got.frame(0), cache.frame(0));
        assert!(!got.frame(1).unwrap()[1].is_finite(), "a dead point stays dead");
        // A simulation without a cache stores no blob.
        let mut empty = s.clone();
        if let ObjectKind::Sim { sim } = &mut empty.edit(id).unwrap().kind {
            Arc::make_mut(sim).cache = None;
        }
        assert!(!String::from_utf8_lossy(&to_bytes(&empty).unwrap()).contains("-cache.mcpc"));
    }

    #[test]
    fn project_round_trips_with_blobs() {
        let s = sample();
        let bytes = to_bytes(&s).unwrap();
        let back = from_bytes(&bytes).unwrap();
        assert_eq!(strip_revs(&back), strip_revs(&s));
    }

    #[test]
    fn atomic_save_and_open() {
        let dir = std::env::temp_dir().join(format!("mc-proj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.modelcraft");
        save(&sample(), &p).unwrap();
        save(&sample(), &p).unwrap();
        assert_eq!(open(&p).unwrap().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_other_and_newer_files() {
        let mut w = craft_zip::Writer::new();
        w.add("project.json", br#"{"format":"modelcraft.project","schema_version":99,"scene":{}}"#, craft_zip::Compression::Deflated).unwrap();
        assert!(matches!(from_bytes(&w.finish().unwrap()), Err(FormatError::NewerSchema { found: 99, .. })));
        assert!(from_bytes(b"").is_err());
        assert!(from_bytes(b"PK\x03\x04junk").is_err());
    }

    #[test]
    fn history_round_trips_and_damage_only_drops_it() {
        let h = History { base: Scene::new(), done: vec!["a".to_string(), "b".to_string()], undone: vec!["c".to_string()] };
        let bytes = to_bytes_with_history(&sample(), Some(&h)).unwrap();
        // Readers that ignore history still see the scene.
        assert_eq!(from_bytes(&bytes).unwrap().len(), 3);
        let (scene, got) = from_bytes_with_history::<String>(&bytes, true).unwrap();
        assert_eq!(scene.len(), 3);
        match got {
            HistoryRead::Found(g) => {
                assert_eq!(g.done, h.done);
                assert_eq!(g.undone, h.undone);
                assert!(g.base.is_empty());
            }
            other => panic!("{other:?}"),
        }
        // A file without history reports none.
        let plain = to_bytes(&sample()).unwrap();
        assert!(matches!(from_bytes_with_history::<String>(&plain, true).unwrap().1, HistoryRead::None));
        // Commands of another type are dropped, not fatal.
        assert!(matches!(from_bytes_with_history::<u32>(&bytes, true).unwrap().1, HistoryRead::Dropped(_)));
        // A newer history version is dropped too.
        let mut w = craft_zip::Writer::new();
        // An empty scene has no blobs, so these are all its entries.
        let empty = to_bytes_with_history(&Scene::new(), Some(&h)).unwrap();
        let mut a = craft_zip::Archive::open(&empty, craft_zip::Limits { max_entries: 100, max_total_bytes: 1 << 30 }).unwrap();
        for name in ["mimetype", "project.json", BASE_ENTRY] {
            let b = a.read(name, 1 << 30).unwrap().unwrap();
            w.add(name, &b, craft_zip::Compression::Stored).unwrap();
        }
        w.add(COMMANDS_ENTRY, br#"{"format":"modelcraft.history","version":9,"done":[]}"#, craft_zip::Compression::Stored).unwrap();
        let newer = w.finish().unwrap();
        assert!(matches!(from_bytes_with_history::<String>(&newer, true).unwrap().1, HistoryRead::Dropped(_)));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn corrupted_histories_never_panic(flips in prop::collection::vec((any::<usize>(), any::<u8>()), 1..20)) {
            let h = History { base: sample(), done: vec!["a".to_string()], undone: vec![] };
            let mut b = to_bytes_with_history(&sample(), Some(&h)).unwrap();
            let n = b.len();
            for (i, x) in flips { if let Some(v) = b.get_mut(i % n) { *v ^= x | 1; } }
            let _ = from_bytes_with_history::<String>(&b, true);
        }

        #[test]
        fn corrupted_projects_never_panic(flips in prop::collection::vec((any::<usize>(), any::<u8>()), 1..20)) {
            let mut b = to_bytes(&sample()).unwrap();
            let n = b.len();
            for (i, x) in flips { if let Some(v) = b.get_mut(i % n) { *v ^= x | 1; } }
            let _ = from_bytes(&b);
        }
    }
}
