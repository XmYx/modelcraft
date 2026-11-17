# The `.modelcraft` file

A ZIP archive (written by craft-zip, saved atomically by craft-atomic):

| Entry | Contents |
|---|---|
| `mimetype` | `application/vnd.storytold.modelcraft+zip`, stored, first |
| `project.json` | `{ "format": "modelcraft.project", "schema_version": 2, "generator", "saved_at" (RFC 3339, UTC), "scene", "blobs" }` |
| `blobs/<object>-source.bin` | a stored mesh (`MCMB` encoding) |
| `blobs/<object>-<step>.bin` | a sculpt layer (`MCSL` encoding) |
| `blobs/<object>-<step>-topology.bin` | the triangles of a sculpt layer changed by live detail (`MCMB` encoding) |

`scene` is the serde form of `modelcraft_scene::Scene`: `objects` (id, name, parent, transform
{translation, rotation quaternion xyzw, scale}, visible, material, kind) and `next_id`. Large arrays
(stored meshes, sculpt layers) are left empty in the JSON and listed in `blobs` as
`"<object>/source"` or `"<object>/step/<step id>"` → entry name.

Blob encodings (little-endian) are documented in `crates/formats/src/blob.rs`. Readers check every
count against the bytes present before allocating; a mesh blob is rebuilt through `PolyMesh::build`.

Schema 2 (this version) added detail levels and shape layers to sculpt layers: `level`,
`shapes` (name, strength) and `active_shape` in the JSON, and the finer-level displacements, the
shape offsets and the anchor in the layer blob (version 2). Schema 1 files still open.

A newer `schema_version` is refused with a clear message; `0` is invalid. Scenes are repaired on
load (unique ids, parents exist, no cycles, depth ≤ 64, finite transforms).

## Animation

`scene.anim` (left out when the scene has none) holds `fps`, `start`, `end` (the playback range in
frames), `frame` (the current frame), `auto_key` and `tracks`: `{target, path, curve}` with
`curve.keys` = `[{t, v, interp?, tangent?, in_slope?, out_slope?, in_weight?, out_weight?}]`
sorted by `t` (frames), plus `pre` / `post` extrapolation. A track's `layer` (left out for the
base, 0) names an entry of `layers`: `{id, name, mode (add | override), weight, mute}`, bottom
first; `active_layer` is the layer keys go to. Defaults are left out. The attribute
`path` grammar is in [cli.md](cli.md#animation). On load, non-finite keys are dropped, keys are
sorted and de-duplicated, counts are capped (2²⁰ keys per curve, 200 000 tracks) and tracks on
missing objects are removed. Builds from before animation ignore the block.

## Point cache (`.mcpc`)

`modelcraft-cli bake` writes the vertices of one object at a run of frames (little-endian):
`"MCPC"`, `u32` version 1, `u32` vertices, `u32` frames, `f64` fps, `f64` start frame, `f64` step,
then frames × vertices × `f32[3]`. Sample `i` is at frame `start + i·step`; vertex order is the
baked mesh's. Readers check the counts against the bytes present and refuse non-finite headers.

## Journal

Next to the project, `<file>.modelcraft.journal` holds one JSON object per line:
`{"seq": n, "at": "<RFC 3339>", "cmd": <Command>}`, flushed after every command and synced on idle.
A save truncates it. Opening a project with a non-empty journal offers to replay it; a torn last line
is ignored. Untitled scenes journal into `$XDG_STATE_HOME/modelcraft/recovery/` with a snapshot
every three minutes.
