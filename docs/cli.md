# `modelcraft-cli` and the command reference

```
modelcraft-cli commands                       # list engine commands
modelcraft-cli new scene.modelcraft
modelcraft-cli exec scene.modelcraft '{"cmd":"add_primitive","kind":"cube"}'
modelcraft-cli run scene.modelcraft script.jsonl -o out.modelcraft
modelcraft-cli info scene.modelcraft          # object tree with counts
modelcraft-cli validate model.glb             # evaluate and check every mesh
modelcraft-cli export scene.modelcraft out.glb
modelcraft-cli convert in.obj out.glb
modelcraft-cli recover scene.modelcraft       # replay the crash journal and save
modelcraft-cli control [scene.modelcraft]     # JSON-lines channel on stdin/stdout
modelcraft-cli bake scene.modelcraft Cube out.mcpc --start 0 --end 48 --step 1 --world
                                              # an object's vertices per frame (point cache)
modelcraft-cli cache-info out.mcpc            # a point cache's header and bounds
modelcraft-cli bench-skin --verts 100000 --joints 60 --frames 120
                                              # bind, pose and skin a procedural tube
modelcraft-cli bench --polys 1000000          # sculpt and GPU timings
modelcraft-cli render scene.modelcraft -o out.png --samples 128   # path-traced image
```

## Commands

Every command is JSON with a `cmd` tag; the same values drive the app, the journal and the tests.
Object ids are numbers (`info` prints them).

| `cmd` | Fields |
|---|---|
| `add_primitive` | `kind` (cube, plane, sphere, round_cube, cylinder, cone, torus), `name?`, `parent?` |
| `add_group` | `name?` |
| `add_smooth_cage` | `input?`, `levels` (default 2) |
| `add_deformer` | `kind` (bend, twist, taper, ripple, round), `target` |
| `delete` | `ids` |
| `duplicate`, `collapse`, `begin_sculpt` | `id` |
| `rename` | `id`, `name` |
| `reparent` | `id`, `parent` (null for top level) |
| `set_transform` | `id`, `transform` {translation, rotation [x,y,z,w], scale} |
| `set_visible` | `id`, `visible` |
| `set_material` | `id`, `material` {base_color, metallic, roughness, linked?, emission?, emission_strength?, coat?, coat_roughness?, sheen?, transmission?, ior?} |
| `set_primitive` | `id`, `primitive` {kind, …parameters} |
| `set_cage_levels` | `id`, `levels` |
| `set_deformer` | `id`, `deformer` {kind, …} |
| `add_step` | `id`, `op`: {op: extrude, faces, distance} · {op: inset, faces, amount} · {op: bevel, edges, width} · {op: loop_cut, edge, cuts} · {op: merge, verts} · {op: weld, distance} · {op: delete_faces/delete_edges/delete_vertices, …} · {op: move_vertices, verts, translate, rotate, scale} · {op: flip, faces} · {op: triangulate} · {op: subdivide, levels} · {op: set_patch, faces, patch} · {op: bridge, a, b} · {op: fill_hole, edge} · {op: mirror, axis (0 X, 1 Y, 2 Z), weld} · {op: even_remesh, cell} (closed meshes; even quads of about `cell` size) |
| `set_step` | `id`, `index`, `op` |
| `enable_step` | `id`, `index`, `enabled` |
| `remove_step` | `id`, `index` |
| `import` | `path` (.glb, .gltf, .obj, .stl, .ply, .fbx, .3mf), `up?` (auto, y, z), `scale?` (metres per file unit) |
| `end_sculpt` | — |
| `sculpt_stroke` | `id`, `stroke` {brush {kind, radius, strength, falloff, stamp, invert, symmetry {x,y,z}, hardness? (0..1), stroke? ({mode: spaced, dots, spray {scatter}, trail {leash}, drag}), spacing? (brush radii), live_detail? (edge length: split the triangles under the brush down to it)}, dabs [{center, view_dir, pressure, grab_delta, scale?}]}. Kinds: draw, clay, smooth, inflate, grab, pinch, flatten, crease, mask, pull, plateau, trim, fill, polish, buildup, surface_grab, gouge, slide, bulge. Stamps: {kind: none, noise {scale}, rings {count}, square, image {alpha: dots, scales, cracks, weave, pores, star, stripes, hex; turn}} |
| `sculpt_mask` | `id`, `op`: clear, fill, invert, {patch: {triangle}} |
| `sculpt_detail` | `id`, `op`: up, down (one detail level; the sculpted detail is kept as a displacement per level) |
| `sculpt_shape` | `id`, `op`: {add: {name?}} · {remove: {index}} · {merge: {index}} · {strength: {index, strength −1..1}} · {select: {index or null}} · {rename: {index, name}} |
| `undo`, `redo` | — |

### Animation

One command, `anim`, with the operation in `op`: `{"cmd":"anim","op":"set_key",…}`.
Key operations work in the active layer.
Time is in frames. A key is named by its object, attribute path and frame: `{id, path, frame}`.
Attribute paths: `translate.x|y|z`, `rotate.x|y|z` (XYZ Euler degrees), `scale.x|y|z`, `visible`,
`material.metallic`, `material.roughness`, `material.base_color.0`…`3`, `primitive.<parameter>`,
`step.<step id>.<parameter>`, `step.<step id>.enabled`, and for the other kinds the saved field
names (`levels`, `deformer.angle`, …). Whole-number attributes round; switches flip at 0.5.

| `op` | Fields |
|---|---|
| `set_key` | `id`, `path`, `frame?` (current frame), `value?` (current value) |
| `key_transform` | `ids`, `frame?`, `translate?`, `rotate?`, `scale?` (all true by default) |
| `remove_keys` | `keys` |
| `move_keys` | `keys`, `frames`, `value` |
| `scale_keys` | `keys`, `pivot` (frame), `time` (factor), `value_pivot`, `value` (factor) |
| `edit_key` | `id`, `path`, `frame`, then any of `value`, `interp` (stepped, linear, bezier), `tangent` (auto, flat, free, broken), `in_slope`, `out_slope` (value per frame), `in_weight`, `out_weight` (0..1 of the segment) |
| `set_key_shape` | `keys`, `interp?`, `tangent?` |
| `set_extrapolation` | `id`, `path`, `pre?`, `post?` (hold, linear, cycle, cycle_offset, ping_pong) |
| `remove_track` | `id`, `path?` (every curve of the object without it) |
| `bake_track` | `id`, `path`, `start?`, `end?` (the playback range), `step` (default 1) |
| `simplify_track` | `id`, `path`, `tolerance` |
| `euler_filter` | `ids`: rewrite keyed rotations as the equal angles nearest to the key before |
| `turntable` | `ids`, `axis` (x, y, z; default y), `turns` (default 1), `start?`, `end?`; replaces the rotation curves |
| `add_layer` | `name?`, `mode?` (add, override); the new layer becomes active |
| `remove_layer`, `set_active_layer` | `layer` (0 is the base layer) |
| `set_layer` | `layer`, `name?`, `weight?` (0..1), `mute?`, `mode?` |
| `flatten_layers` | `step` (default 1): bake every layer into the base curves |
| `set_time` | `frame` (poses the scene; not an undo step) |
| `set_time_range` | `start?`, `end?`, `fps?` |
| `set_auto_key` | `on` |

```
$ printf '%s\n' '{"cmd":"add_primitive","kind":"cube"}' \
    '{"cmd":"anim","op":"set_key","id":1,"path":"translate.x","frame":0,"value":0}' \
    '{"cmd":"anim","op":"set_key","id":1,"path":"translate.x","frame":24,"value":6}' \
    '{"cmd":"anim","op":"set_time","frame":12}' | modelcraft-cli control
```

### Rigging

One command, `rig`, with the operation in `op`: `{"cmd":"rig","op":"add_joint","parent":3}`.

| `op` | Fields |
|---|---|
| `add_joint` | `parent?`, `position?` [x, y, z] in world space (one unit up the parent's Y without it), `name?` |
| `orient_joints` | `ids`, `aim` (x, y, z; default y), `up` (default z), `children?`: aim each joint at its first child joint, children stay in place |
| `mirror_joints` | `id`, `axis` (default x), `find?`, `replace?` (for example `_L` → `_R`) |
| `bind_skin` | `mesh`, `joints`, `method` (heat, distance; default heat), `influences` (1–8, default 4), `dual_quaternion?` |
| `unbind_skin` | `mesh` |
| `set_skin` | `mesh`, `dual_quaternion` |
| `edit_weights` | `mesh`, `weights`: `normalize` · `prune` + `threshold` · `limit` + `max` · `mirror` + `axis`, `find`, `replace`, `tolerance` · `set` + `points`, `joint`, `weight` · `paint` + `joint`, `center` [x, y, z] in the mesh's space, `radius`, `strength` (−1..1) |

A skinned mesh exports and bakes (`modelcraft-cli bake`) in its current pose.

## Rendering

`modelcraft-cli render <file|builtin:NAME> -o <out.png>` path-traces a project, any importable
model file or a built-in scene (`builtin:cornell`, `builtin:materials`, `builtin:interior`, `builtin:night-street`, `builtin:studio`, `builtin:showcase`, `builtin:furnace`) and
prints one JSON line: device, size, samples, triangles, scene and render seconds, and million
samples per second.

| Option | Meaning |
|---|---|
| `--width N --height N` | Image size, default 960×540 |
| `--samples N` | Samples per pixel, default 64 |
| `--depth N` | Bounces, default 12 |
| `--device auto\|cpu\|gpu` | `auto` uses a hardware GPU when there is one and falls back to the CPU; `gpu` fails if it cannot |
| `--yaw DEG --pitch DEG --fov DEG --ortho` | The camera, which always frames the whole scene |
| `--exposure EV` | Brightness in stops |
| `--sky R,G,B`, `--no-ground` | A uniform sky instead of the studio light; no ground plane |
| `--sun ELEVATION[,AZIMUTH]` | Daylight: a physical sky with the sun at these angles in degrees |
| `--env FILE.hdr`, `--env-rotation DEG` | Light the scene with a Radiance HDR panorama, optionally turned |
| `--seed N` | The same seed gives the same image |

The scene graph has no light objects yet, so a project is lit by a studio set-up (a soft sky and
one key light sized to the scene) on a matte ground plane.

An output ending in `.exr` holds linear radiance (`R`, `G`, `B`) and the feature images a denoiser
or compositor reads: `albedo.R/G/B`, `N.X/Y/Z` (world-space normal), `Z` (distance from the
camera), `motion.X/Y` and `variance`, all 32-bit float. The feature images come from the CPU path.

`modelcraft-cli render-bench [--scenes a,b] [--size N] [--spp 1,4,16] [--seeds N] [--reference N]
[--device cpu|gpu] [--order z|independent]` measures error against samples and time on the
benchmark scenes, each against its own converged reference, and prints one JSON line per scene and
a table ([performance.md](performance.md) explains the protocol).

## Control channel

The same requests reach a document four ways (`crates/engine/src/control.rs`):

| Method | Params | Result |
|---|---|---|
| `engine.execute` | a command (`{"cmd": …}`), or `{"command": {…}}` | `{created, warnings}` |
| `engine.commands` | — | `[{name, help}]` |
| `document.info` | — | `{path, dirty, revision, objects: [{id, name, type, depth, parent, visible, faces, steps}], undo, redo}` |
| `document.revision` | — | `{document_id, revision}` |
| `document.save` | `{path?}` | `{path}` |
| `document.export` | `{path}` (.glb, .obj, .stl) | `{path}` |

Every reply is `{"ok":true,"result":…}` or `{"ok":false,"error":"…"}`.

- **stdin/stdout**: `modelcraft-cli control [project]` reads one line each: an engine command,
  `{"method": …, "params": …}`, or the short verbs `{"cmd":"info"}`, `{"cmd":"save","path":…}`,
  `{"cmd":"export","path":…}`. Local, so no token.
- **Loopback socket in the window**: `modelcraft --control` (or `--control-port N`), or
  **Edit ▸ Allow Agents to Control ModelCraft** (a preference, off by default). The channel is
  craft-control's: loopback only, a fresh 32-byte token per launch written to
  `<data folder>/modelcraft/control/<port>.token` (readable only by you; `MODELCRAFT_CONTROL_DIR`
  moves the folder, `MODELCRAFT_CONTROL_TOKEN` sets the token), the port in `current.port`, default
  port 28730 then the next 15. The first request of a connection carries `"token"`; requests are
  `{"id", "method", "params"}` lines. The window answers between frames, never during a drag or
  stroke; the status bar says when an agent changed the scene.
- **Loopback socket without a window**: `modelcraft-cli serve [project] [--port N]`, the same
  channel on a document of its own.
- **One request from a shell**: `modelcraft-cli call <method> ['<json params>'] [--port N]
  [--token T]` finds the port and token the way any craft-control client does.
- **MCP**: `modelcraft-cli mcp [project]` is an MCP server on stdio (craft-mcp-server) with the
  tools `info`, `commands`, `execute`, `save` and `export` on its own document;
  `modelcraft-cli mcp --app [--port N]` sends every tool call to the running window instead, so an
  agent and a person work on the same scene. Register it with an MCP client as the command
  `modelcraft-cli mcp --app`.

Example sessions:

```
$ printf '%s\n' '{"cmd":"add_primitive","kind":"cube"}' '{"cmd":"add_step","id":1,"op":{"op":"subdivide","levels":2}}' '{"cmd":"info"}' | modelcraft-cli control
$ modelcraft --control &
$ modelcraft-cli call engine.execute '{"cmd":"add_primitive","kind":"torus"}'
$ modelcraft-cli call document.info
```

## Interop with the suite

- **ArtCraft**: GLB, Y up, metres, the same convention as its three.js stage and image-to-mesh output.
- **TextureCraft**: export GLB with UVs (TEXCOORD_0) and stable material names; TextureCraft
  auto-unwraps meshes without UVs.
- **MaterialCraft**: link an `.mcmat` on an object's material; the path tracer renders its base colour, roughness, metallic, normal, height and opacity maps.
