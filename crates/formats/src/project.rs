//! The `.modelcraft` project: a ZIP archive.
//!
//! | Entry | Contents |
//! |---|---|
//! | `mimetype` | `application/vnd.storytold.modelcraft+zip` (stored, first) |
//! | `project.json` | `{format, schema_version, generator, saved_at, scene}`; stored meshes and sculpt layers are left empty and listed in `blobs` |
//! | `blobs/<object>-source.bin` | a stored mesh ([`crate::blob`]) |
//! | `blobs/<object>-<step>.bin` | a sculpt layer |

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use modelcraft_mesh::PolyMesh;
use modelcraft_scene::{ObjectId, ObjectKind, Scene, Source, StepOp};
use serde::{Deserialize, Serialize};

use crate::{FormatError, GENERATOR, Result, blob};

pub const MIME_TYPE: &str = "application/vnd.storytold.modelcraft+zip";
pub const FORMAT_ID: &str = "modelcraft.project";
pub const SCHEMA_VERSION: u32 = 1;
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

fn zip_err(e: craft_zip::Error) -> FormatError {
    FormatError::Malformed(e.to_string())
}

/// Encode a scene as `.modelcraft` bytes.
pub fn to_bytes(scene: &Scene) -> Result<Vec<u8>> {
    let mut light = scene.clone();
    let mut blobs: BTreeMap<String, String> = BTreeMap::new();
    let mut data: Vec<(String, Vec<u8>)> = Vec::new();
    let ids: Vec<ObjectId> = light.objects().iter().map(|o| o.id).collect();
    for id in ids {
        let Ok(o) = light.edit(id) else { continue };
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
                    data.push((entry.clone(), blob::write_layer(&layer.positions, &layer.mask)));
                    blobs.insert(layer_key(id, s.id), entry);
                    layer.positions = Arc::new(Vec::new());
                    layer.mask = Arc::new(Vec::new());
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
    w.finish().map_err(zip_err)
}

/// Decode `.modelcraft` bytes (hostile input: bounded, validated, repaired).
pub fn from_bytes(bytes: &[u8]) -> Result<Scene> {
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
                let (p, m) = blob::read_layer(&b)?;
                layer.positions = Arc::new(p);
                layer.mask = Arc::new(m);
            }
        }
    }
    Ok(scene)
}

/// Save atomically: the old file stays whole until the new one is complete on disk.
pub fn save(scene: &Scene, path: &Path) -> Result<()> {
    let bytes = to_bytes(scene)?;
    craft_atomic::atomic_write(path, &bytes).map_err(|e| FormatError::io(path, e))
}

pub fn open(path: &Path) -> Result<Scene> {
    let meta = std::fs::metadata(path).map_err(|e| FormatError::io(path, e))?;
    if meta.len() > MAX_TOTAL_BYTES {
        return Err(FormatError::Malformed("file is too large".into()));
    }
    let bytes = std::fs::read(path).map_err(|e| FormatError::io(path, e))?;
    from_bytes(&bytes)
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
                        op: StepOp::Sculpt { layer: SculptLayer { positions: Arc::new(vec![Vec3::ONE; n]), mask: Arc::new(vec![0.25; n]) } },
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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn corrupted_projects_never_panic(flips in prop::collection::vec((any::<usize>(), any::<u8>()), 1..20)) {
            let mut b = to_bytes(&sample()).unwrap();
            let n = b.len();
            for (i, x) in flips { if let Some(v) = b.get_mut(i % n) { *v ^= x | 1; } }
            let _ = from_bytes(&b);
        }
    }
}
