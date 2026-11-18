# The `.modelcraft` file

A ZIP archive (written by craft-zip, saved atomically by craft-atomic):

| Entry | Contents |
|---|---|
| `mimetype` | `application/vnd.storytold.modelcraft+zip`, stored, first |
| `project.json` | `{ "format": "modelcraft.project", "schema_version": 2, "generator", "saved_at" (RFC 3339, UTC), "scene", "blobs" }` |
| `blobs/<object>-source.bin` | a stored mesh (`MCMB` encoding) |
| `blobs/<object>-<step>.bin` | a sculpt layer (`MCSL` encoding) |
| `blobs/<object>-<step>-topology.bin` | the triangles of a sculpt layer changed by live detail (`MCMB` encoding) |
| `history/base.modelcraft` | optional: the scene before the oldest kept undo step, a whole project of its own (stored, never with its own history) |
| `history/commands.json` | optional: `{ "format": "modelcraft.history", "version": 1, "done": [<Command>…], "undone": [<Command>…] }` |

`scene` is the serde form of `modelcraft_scene::Scene`: `objects` (id, name, parent, transform
{translation, rotation quaternion xyzw, scale}, visible, material, kind, `locked` and `template`
when true), `next_id` and `sets` (selection sets `{name, ids}`, left out when there are none). Large arrays
(stored meshes, sculpt layers) are left empty in the JSON and listed in `blobs` as
`"<object>/source"` or `"<object>/step/<step id>"` → entry name.

Blob encodings (little-endian) are documented in `crates/formats/src/blob.rs`. Readers check every
count against the bytes present before allocating; a mesh blob is rebuilt through `PolyMesh::build`.

Schema 2 (this version) added detail levels and shape layers to sculpt layers: `level`,
`shapes` (name, strength) and `active_shape` in the JSON, and the finer-level displacements, the
shape offsets and the anchor in the layer blob (version 2). Schema 1 files still open.

A newer `schema_version` is refused with a clear message; `0` is invalid. Scenes are repaired on
load (unique ids, parents exist, no cycles, depth ≤ 64, finite transforms).

## Joints and skins

Two more object kinds: `{"type":"joint","radius":…}` (a transform drawn as a bone to each child
joint) and `{"type":"skin","skin":{joints, inverse_bind, weights, dual_quaternion?}}`, a child of
the mesh it binds. `joints` are object ids; `inverse_bind[i]` (16 numbers, column-major) takes a
point of the mesh at bind time into joint `i`'s space; `weights` is
`{influences, joints: [...], weights: [...]}` with `influences` slots per point. On load, a skin
whose arrays disagree is emptied, weights on missing joints are dropped and every point is
re-normalised. Weights are JSON for now, not a blob.

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

## Deformer stack

An object of `type` `point_deformer` under a mesh holds `deformer`, with `kind` one of
`blend_shapes` (`targets`: `[{name, weight, frames: [{at, indices, deltas}]}]`, the offsets of the
listed points when the weight is `at`), `lattice` (`divisions` [x, y, z], `linear?`, `points`: the
control points in the lattice's unit box, X fastest) and `cluster` (`points`, `weights`, `bind`:
the inverse of the object's matrix when it was made). Offsets and points are JSON for now. On
load, counts are capped (512 targets, 16 shapes each, 8 cells per axis), a lattice with the wrong
number of points is reset and non-finite numbers are replaced.

## Constraints

`scene.constraints` (left out when the scene has none) holds `list`: `{id, name, target, enabled?,
weight?, type, …}` with `type` one of `point`, `orient`, `scale`, `parent` (`sources`:
`[{object, weight?, offset?}]`), `aim` (`sources`, `aim`, `up`, `up_object?`, `up_vector`,
`offset`), `ik` (`goal`, `pole?`, `bones`, `solver`, `iterations`, `tolerance`), `driven` (`path`,
`driver`, `driver_path`, `curve`) and `expression` (`path`, `expr`); and `authored`: per driven
object id, the transform it has without its constraints (the objects themselves hold the solved
pose). On load, weights and solver settings are clamped, duplicate ids and constraints on missing
objects are dropped, and counts are capped (4096 constraints, 16 sources each). Builds from before
constraints ignore the block.

## Point cache (`.mcpc`)

`modelcraft-cli bake` writes the vertices of one object at a run of frames (little-endian):
`"MCPC"`, `u32` version 1, `u32` vertices, `u32` frames, `f64` fps, `f64` start frame, `f64` step,
then frames × vertices × `f32[3]`. Sample `i` is at frame `start + i·step`; vertex order is the
baked mesh's. Readers check the counts against the bytes present and refuse non-finite headers.

## Undo history

The two `history/` entries come together. `done` lists the commands of the undo history, oldest
first; `undone` lists the steps redo would bring back, the next one first. Opening the file replays
`done` and then `undone` on the base scene and undoes the `undone` part, which rebuilds the undo
stack of the last session. The result must equal the saved scene exactly; if it does not (an
imported file moved, a command fails, the entries are damaged or newer), the history is dropped,
the saved scene opens unchanged and the status bar says so. Readers that do not know these entries
ignore them.

A save stores at most 256 MiB of commands; older steps are left out, cutting only where the scene
before a step is known. **Edit ▸ Keep Undo History in Saved Files** turns the entries off (a
preference). Autosave snapshots carry the history too. The history stores commands, not the
current frame between them, so a history whose steps depended on scrubbing to other frames
(auto-key) may not replay to the saved scene; it is then dropped as above.

## Journal

Next to the project, `<file>.modelcraft.journal` holds one JSON object per line:
`{"seq": n, "at": "<RFC 3339>", "cmd": <Command>}`, flushed after every command and synced on idle.
A save truncates it. Opening a project with a non-empty journal offers to replay it; a torn last line
is ignored. Untitled scenes journal into `$XDG_STATE_HOME/modelcraft/recovery/` with a snapshot
every three minutes.
