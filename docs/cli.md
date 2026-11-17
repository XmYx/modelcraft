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
| `set_material` | `id`, `material` {base_color, metallic, roughness, linked?} |
| `set_primitive` | `id`, `primitive` {kind, …parameters} |
| `set_cage_levels` | `id`, `levels` |
| `set_deformer` | `id`, `deformer` {kind, …} |
| `add_step` | `id`, `op`: {op: extrude, faces, distance} · {op: inset, faces, amount} · {op: bevel, edges, width} · {op: loop_cut, edge, cuts} · {op: merge, verts} · {op: weld, distance} · {op: delete_faces/delete_edges/delete_vertices, …} · {op: move_vertices, verts, translate, rotate, scale} · {op: flip, faces} · {op: triangulate} · {op: subdivide, levels} · {op: set_patch, faces, patch} · {op: bridge, a, b} · {op: fill_hole, edge} · {op: mirror, axis (0 X, 1 Y, 2 Z), weld} |
| `set_step` | `id`, `index`, `op` |
| `enable_step` | `id`, `index`, `enabled` |
| `remove_step` | `id`, `index` |
| `import` | `path` (.glb, .gltf, .obj, .stl, .ply) |
| `end_sculpt` | — |
| `sculpt_stroke` | `id`, `stroke` {brush {kind, radius, strength, falloff, stamp, invert, symmetry {x,y,z}}, dabs [{center, view_dir, pressure, grab_delta}]} |
| `sculpt_mask` | `id`, `op`: clear, fill, invert, {patch: {triangle}} |
| `sculpt_detail` | `id`, `op`: up, down (one detail level; the sculpted detail is kept as a displacement per level) |
| `sculpt_shape` | `id`, `op`: {add: {name?}} · {remove: {index}} · {merge: {index}} · {strength: {index, strength −1..1}} · {select: {index or null}} · {rename: {index, name}} |
| `undo`, `redo` | — |

## Rendering

`modelcraft-cli render <file|builtin:NAME> -o <out.png>` path-traces a project, any importable
model file or a built-in scene (`builtin:cornell`, `builtin:materials`, `builtin:furnace`) and
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
| `--seed N` | The same seed gives the same image |

The scene graph has no light objects yet, so a project is lit by a studio set-up (a soft sky and
one key light sized to the scene) on a matte ground plane.

## Control channel

`modelcraft-cli control` reads one command per line and answers one line each:
`{"ok":true,"result":{"created":[…],"warnings":[…]}}` or `{"ok":false,"error":"…"}`. Extra verbs:
`{"cmd":"info"}` (object list and undo labels), `{"cmd":"save","path":…}`, `{"cmd":"export","path":…}`.
It runs locally over stdio only, so it needs no authentication; a socket listener will use
`craft-control`'s token files, as TextureCraft and MaterialCraft do.

Example session:

```
$ printf '%s\n' '{"cmd":"add_primitive","kind":"cube"}' '{"cmd":"add_step","id":1,"op":{"op":"subdivide","levels":2}}' '{"cmd":"info"}' | modelcraft-cli control
```

## Interop with the suite

- **ArtCraft**: GLB, Y up, metres, the same convention as its three.js stage and image-to-mesh output.
- **TextureCraft**: export GLB with UVs (TEXCOORD_0) and stable material names; TextureCraft
  auto-unwraps meshes without UVs.
- **MaterialCraft**: link an `.mcmat` on an object's material (stored; rendering it is planned).
