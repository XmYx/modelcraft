# `modelcraft-cli` and the command reference

```
modelcraft-cli commands                       # list engine commands
modelcraft-cli new scene.modelcraft
modelcraft-cli exec scene.modelcraft '{"cmd":"add_primitive","kind":"cube"}'
modelcraft-cli run scene.modelcraft script.jsonl -o out.modelcraft
modelcraft-cli info scene.modelcraft          # object tree with counts
modelcraft-cli validate model.glb             # evaluate and check every mesh
modelcraft-cli export scene.modelcraft out.glb
modelcraft-cli export-uv scene.modelcraft layout.png --size 2048   # UV layout as SVG or PNG (--udim: one file per tile)
modelcraft-cli convert in.obj out.glb
modelcraft-cli recover scene.modelcraft       # replay the crash journal and save
modelcraft-cli control [scene.modelcraft]     # JSON-lines channel on stdin/stdout
modelcraft-cli bake scene.modelcraft Cube out.mcpc --start 0 --end 48 --step 1 --world
                                              # an object's vertices per frame (point cache)
modelcraft-cli cache-info out.mcpc            # a point cache's header and bounds
modelcraft-cli run-script scene.modelcraft tool.rhai -o out.modelcraft   # a script over a project

modelcraft-cli bench-skin --verts 100000 --joints 60 --frames 120
                                              # bind, pose and skin a procedural tube
modelcraft-cli bench --polys 1000000          # sculpt and GPU timings
modelcraft-cli bench --groups --polys 1000000 # patch, visibility, region, shell, trim and occlusion timings
modelcraft-cli render scene.modelcraft -o out.png --samples 128   # path-traced image
modelcraft-cli uv scene.modelcraft            # UV islands and distortion of every mesh object
modelcraft-cli uv-bench --segments 64         # unwrap a sphere: time and distortion
modelcraft-cli uv-bench --method both --cut meridian --relax 15   # LSCM against ABF, then relax
modelcraft-cli hair-bench --strands 1000000 [--gpu]   # grow guides, interpolate hairs, draw them: time, memory, frame times
modelcraft-cli uv-pack-bench --islands 200    # pack random islands: time and utilisation
```

## Commands

Every command is JSON with a `cmd` tag; the same values drive the app, the journal and the tests.
Object ids are numbers (`info` prints them).

| `cmd` | Fields |
|---|---|
| `add_primitive` | `kind` (cube, plane, sphere, round_cube, cylinder, cone, torus, disc, polygon, tube, capsule, pyramid, tetrahedron, octahedron, dodecahedron, icosahedron, landscape), `name?`, `parent?` |
| `add_group` | `name?` |
| `add_smooth_cage` | `input?`, `levels` (default 2) |
| `add_deformer` | `kind` (bend, twist, taper, ripple, round, shear, bulge, squash, smooth, delta_smooth, displace, shrink_wrap), `target` |
| `delete` | `ids` |
| `duplicate`, `collapse`, `begin_sculpt` | `id` |
| `rename` | `id`, `name` |
| `reparent` | `id`, `parent` (null for top level) |
| `set_transform` | `id`, `transform` {translation, rotation [x,y,z,w], scale} |
| `set_visible` | `id`, `visible` |
| `set_material` | `id`, `material` {base_color, metallic, roughness, linked?, emission?, emission_strength?, coat?, coat_roughness?, sheen?, transmission?, ior?} |
| `set_primitive` | `id`, `primitive` {kind, …parameters} |
| `set_cage_levels` | `id`, `levels` |
| `set_deformer` | `id`, `deformer` {kind, …}, `limits?` {mode: limited / unlimited / within_box, fit_to_parent, size}, `target?`, `clear_target?` |
| `add_step` | `id`, `op`: {op: extrude, faces, distance} · {op: inset, faces, amount} · {op: bevel, edges, width} · {op: loop_cut, edge, cuts} · {op: merge, verts} · {op: weld, distance} · {op: delete_faces/delete_edges/delete_vertices, …} · {op: move_vertices, verts, translate, rotate, scale} · {op: flip, faces} · {op: triangulate} · {op: subdivide, levels} · {op: set_patch, faces, patch} · {op: bridge, a, b} · {op: fill_hole, edge} · {op: mirror, axis (0 X, 1 Y, 2 Z), weld} · {op: even_remesh, cell} (closed meshes; even quads of about `cell` size) · {op: unwrap, seams [[v, v]…], angle?, pins [{corner, uv}]…, auto_cut, margin, method (lscm, abf), from_patches?} · {op: stitch_uvs, edges, align?, scale?} · {op: relax_uvs, corners, iterations, mode (rigid, conformal)} · {op: straighten_uvs, corners, strength} · {op: group_patches, grouping: {by: angle, degrees} or {by: shells}} · {op: clean_up, distance} · {op: pack_uvs, margin, rotation (none, quarter, fit), density (keep, uniform), tiles?, texel_density?} · {op: project_uvs, projection {kind: planar/box/cylindrical/spherical/camera, from/axis/view_proj}, fit (none, uniform, stretch), faces} · {op: trim_plane, point, normal, cap} (cuts away the side the normal points to; `cap` closes the cut) · {op: decimate, ratio} (keeps about `ratio` of the triangles, 0..1, with UVs and patches; the result is triangles) |
| `set_step` | `id`, `index`, `op` |
| `enable_step` | `id`, `index`, `enabled` |
| `remove_step` | `id`, `index` |
| `set_locked` | `ids`, `locked` (a locked object refuses edits) |
| `set_template` | `ids`, `template` (drawn, never picked) |
| `add_selection_set` | `name`, `ids` |
| `remove_selection_set` | `index` |
| `import` | `path` (.glb, .gltf, .obj, .stl, .ply, .fbx, .3mf, .usda), `up?` (auto, y, z), `scale?` (metres per file unit) |
| `end_sculpt` | — |
| `sculpt_stroke` | `id`, `stroke` {brush {kind, radius, strength, falloff, stamp, invert, symmetry {x,y,z}, hardness? (0..1), stroke? ({mode: spaced, dots, spray {scatter}, trail {leash}, drag}), spacing? (brush radii), live_detail? (edge length: split the triangles under the brush down to it)}, dabs [{center, view_dir, pressure, grab_delta, scale?}]}. Kinds: draw, clay, smooth, inflate, grab, pinch, flatten, crease, mask, pull, plateau, trim, fill, polish, buildup, surface_grab, gouge, slide, bulge, rotate, elastic_move, scrape. `symmetry` also takes radial (copies around an axis, 2 to 64) and radial_axis (0 X, 1 Y, 2 Z). Stamps: {kind: none, noise {scale}, rings {count}, square, image {alpha: dots, scales, cracks, weave, pores, star, stripes, hex; turn}} |
| `sculpt_mask` | `id`, `op`: clear, fill, invert, blur, sharpen, grow, shrink, hollows, ridges, occlusion, {patch: {triangle}} |
| `sculpt_groups` | `id`, `op`: {by_normal: {angle}} (degrees) · by_uv_island · by_part · from_mask. Makes patches on the mesh being sculpted |
| `sculpt_visibility` | `id`, `op`: show_all · invert · hide_masked · hide_unmasked · {hide_patch: {triangle}} · {show_patch: {triangle}} · {isolate: {triangle}}. Hidden faces stay in the mesh; picking and brushes skip them |
| `sculpt_region` | `id`, `region` {view_proj (16 numbers, column-major, object space to clip space), points [[x, y]…] (3 to 4096 corners, each −1..1, y up), front_only?, eye? (object space)}, `action`: mask, unmask, hide, isolate |
| `sculpt_clip` | `id`, `point`, `normal` (object space): what lies on the normal's side moves onto the plane, as far as the mask allows |
| `sculpt_extract` | `id`, `thickness`: a new object from the fully masked faces, as a closed shell |
| `sculpt_project` | `id`, `source` (another object), `reach?` (how far a vertex may move, in the sculpted object's units; a tenth of its size when left out): the mesh being sculpted moves onto the source's surface, as far as the mask allows |
| `sculpt_detail` | `id`, `op`: up, down (one detail level; the sculpted detail is kept as a displacement per level) |
| `sculpt_shape` | `id`, `op`: {add: {name?}} · {remove: {index}} · {merge: {index}} · {strength: {index, strength −1..1}} · {select: {index or null}} · {rename: {index, name}} |
| `add_mesh` | `positions` [[x,y,z]…], `faces` [[i,j,k…]…], `name?`, `parent?` (a mesh from raw geometry; faces that do not fit are dropped with a warning) |
| `set_positions` | `id`, `positions` [[x,y,z]…] (objects that store their own mesh; one per vertex) |
| `batch` | `commands` [{cmd…}…], `label?` (scene commands as one undo step, all or none) |
| `undo`, `redo` | — |

### Simulation

One command, `sim`, with the operation in `op`: `{"cmd":"sim","op":"attach_cache","mesh":3,"path":"fall.mcpc"}`.
A simulation object sits under the mesh it drives and plays the points of its cache for the scene's
current frame (linear between frames; the first or last frame holds outside the cache).
`attach_cache` reads a `.mcpc` file whose point count is the mesh's vertex count, so any tool that
writes the format (or `modelcraft-cli bake`) can drive a mesh. `set` changes `start_frame`,
`end_frame`, `fps`, `substeps` (1 to 64), `seed` and `max_cache_mib`; `clear_cache` throws a baked
cache away.

`add` gives a mesh a simulation (`rigid_active` falls and collides, `rigid_passive` is a collider
that follows its keyed animation); `set_kind` replaces its settings; `bake` simulates from the start
frame to the end frame and caches every frame, in one undo step. A rigid body's settings (`kind`
with `type: "rigid"`): `role`, `shape` (auto, box, sphere, hull, mesh; a mesh shape is for
colliders), `mass` (kg), `friction`, `bounciness`, `linear_damping`, `angular_damping`,
`velocity` and `spin` at the start, `can_sleep`, `gravity` and `ground` (a floor at that height),
and `joints`: `{kind: fixed|hinge|point, other: <mesh object id, or none for the world>, anchor:
[x,y,z] (world, at the start frame), axis, limit: [min, max] degrees}`. All rigid bodies of a
scene are one world: baking any of them bakes every one, with the timing, seed, gravity and ground
of the one you baked. The same scene and seed give the same cache bit for bit. A body's own
animation is ignored while it is simulated; a collider's keys move it and push the bodies on it.
Each cache is stored in its mesh's own space under the object's own transform, so the object does
not move: the mesh does.

`cloth` (`add` with `kind: "cloth"`) simulates the mesh as a cloth with extended position-based
dynamics. Its settings (`kind` with `type: "cloth"`): `mass` (kg), `stretch`, `shear`, `bend`
(0 to 1; 1 is inextensible for stretch), `damping`, `thickness` (the radius of a vertex in
collisions, metres), `friction`, `iterations` per substep, `gravity`, `wind` (`direction`,
`strength` in metres per second, `turbulence` 0 to 1, `scale` and `speed` of the gusts, seeded by
the simulation's `seed`), `collide` (with other visible meshes, or only the ids in `collide_with`),
`self_collision`, and the pins: `pinned` (vertex indices; `sim pin_vertices` adds or releases
some), `pin_mask` (also hold the vertices whose sculpt mask is above one half) and `pin_follow`
(a mesh or group id: the pins move as that object does). Colliders are read from the scene at every
frame, so moving objects, deformed meshes and baked rigid bodies all work; a cloth does not push
its colliders back. A cloth is baked on its own (`bake` on it), and a cloth baked earlier is a
collider for the next. Edits to the colliders do not mark a cloth's cache stale.

`particles` (`add` with `kind: "particles"`, under the emitter object) is a particle system shown as
instances. Its settings (`kind` with `type: "particles"`): `emitter` (`point` at the object's
origin, `surface` of the parent mesh, `volume` inside it), `rate` per second and `burst` at the start
frame, `lifetime` and `speed` with their `_variation`, `direction` (`normal`, `fixed` with
`fixed_direction`, `random`) and a `spread` cone in degrees, `inherit` (the emitter's own speed),
`gravity`, `drag` (against the air, or the `wind` with its `turbulence`), a `noise_*` acceleration
field, `attractors` (`attract` or `vortex`, `position`, `strength`, `radius`, `axis`), `collide`
with `bounciness` and `friction` (against every other visible mesh, or `collide_with`),
`capacity` (the slots of the pool: the most alive at once), and the drawn `size` with
`size_variation`. A pool slot is one point of the cache and a dead slot is stored as NaN. The cache is
in world space, so moving the system's own object does not move the particles. The system shows its
pool as instances of the first mesh inside it, or of a small sphere; the system object's own
material colours them. The viewport and the path-traced render draw the instances; up to 10,000
are drawn at once.

`modelcraft-cli commands` lists the operations as `sim op=…`, and `modelcraft-cli sim-bench rigid`,
`sim-bench cloth` and `sim-bench particles` measure a bake.

### Splines

One command, `spline`, with the operation in `op`: `{"cmd":"spline","op":"add","kind":"circle"}`.
A spline is an object that has no surface: it is drawn as a curve, and the generators below take
splines as children. `add` makes one: `kind` is `circle`, `rectangle`, `star`, `n_side`, `arc` or
`helix` (a primitive whose numbers `set_primitive` changes: `{"kind":"star","points":5,"inner":0.45,"outer":1,"plane":"xy"}`;
`plane` is `xy`, `yz` or `zx`, the axis the shape faces), or `empty` (a curve with no points, to
draw into). `set_tessellation` sets how many points the curve gets between its knots: `{"mode":"fixed","steps":8}`,
`{"mode":"uniform","length":0.1}` or `{"mode":"adaptive","tolerance":0.01,"angle_deg":12}`.
`make_editable` turns a primitive into points and handles; `set_curve` replaces the curve
(`spline.segments[]`, each with `kind` linear, bezier or b_spline, `closed` and `knots[]` of
`{pos, tan_in, tan_out, mode}`; handles are offsets from `pos`, `mode` is free, aligned, mirrored
or auto).

`edit` changes the points with one `edit` object (an undo step each; a primitive becomes editable
first): `{"cmd":"spline","op":"edit","id":3,"edit":{"edit":"append","segment":0,"knot":{"pos":[1,0,0]}}}`.
The edits are `append` (a point at the end of a segment; `segment` equal to the number of segments
starts a new one; `kind?`), `move_knots` (`moves [{at:{segment,knot}, pos}]`), `translate`
(`knots`, `by`), `set_handle` (`at`, `side` in or out, `offset`), `set_mode`, `set_kind`,
`set_closed`, `insert` (`segment`, `u`: one unit per piece of curve; the shape is kept), `remove`
(`knots`; a segment left empty goes), `add_segment` and `remove_segment`. A failed edit changes
nothing. A spline may have 1,024 segments and 100,000 points, and a curve is cut into at most
1,000,000 points when it is tessellated; numbers that are not finite are refused.

Corner and shape operations are edits too: `reverse` (`segment`) runs a curve the other way (a
closed curve keeps its first point first); `chamfer` (`knots`, `distance`) and `fillet` (`knots`,
`radius`) cut the corners at those points with a straight edge or a tangent arc, limited to half
of the shorter edge (only corners between straight edges are cut; an error says why when none
could be); `to_polygon` (`tess`) turns every curve into a polyline of the points the tessellation
gives; `set_closed` with `closed: true` joins a last point that lies on the first. `to_mesh`
(`id`) makes a mesh object with a flat face that fills the closed curves of a spline (a closed
curve inside another is a hole; open curves are left out).

`add_generator` makes a generator (`kind`: `extrude`, `revolve`, `loft` or `sweep`) that shows a
surface built from the splines inside it; `inputs` lists spline objects to move inside it (with
none, it starts with example splines). **Extrude** and **revolve** use every spline inside (a
closed spline inside another is a hole); **loft** blends through the splines in order (all closed
or all open); **sweep** moves the first spline (the profile: its origin rides the path) along the
second (the path) without twisting it. `set_generator` changes the options, which are different for
each kind: extrude `{distance, axis (auto, x, y, z), steps, cap_start, cap_end}`, revolve
`{angle (degrees, 360 is a full turn), steps, axis, caps}`, loft `{steps (between curves), smooth,
caps, points (0: the most any curve has)}`, sweep `{end_scale, twist, roll (degrees), caps}`.
The surface follows the splines: moving a point of a child re-evaluates the generator, and a
generator can sit inside an array, a symmetry or a smooth cage. Faces point outwards however the
splines were drawn, with UVs, and closed curves get caps (holes and concave outlines included).
A request that would build more than 2,000,000 faces, or a cap of more than 30,000 points, is an
error before anything is built. The exported mesh of a scene holds the generator's surface.

`modelcraft-cli commands` lists the operations as `spline op=…`.

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
| `set_layer` | `layer`, `name?`, `weight?` (0..1), `mute?`, `mode?`, `solo?` (only solo layers play over the base), `lock?` (its keys cannot be changed) |
| `key_layer_weight` | `layer`, `frame?`, `value?` (its weight there without it), `clear?` (remove its weight keys): the layer's weight over time |
| `move_layer` | `layer`, `delta` (1 up, −1 down) |
| `paste_keys` | `keys`: `[{id, path, key: {t, v, interp?, tangent?, …}}]`, `offset?` (frames), `to?` (another object; attributes it lacks are skipped) |
| `set_attr` | `id`, `path`, `value`: set any numeric attribute without keying it |
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

## Scripts

```
modelcraft-cli run-script <project|new> <script.rhai> [-o <out>] [options]
modelcraft-cli run-script <project|new> -e '<script text>' [options]
modelcraft-cli batch-script <script.rhai> <file>... --out-dir DIR [--format modelcraft|glb|obj|stl] [options]
```

`run-script` runs a [Rhai script](scripting.md) over a project (or an empty scene) and saves like
`run` does. `batch-script` runs one script over many files (projects or any importable model),
saves each result in `--out-dir` (named after the input; a second input with the same file name gets `_2`, `_3`, ... instead of overwriting the first), prints one line per file and exits with status 1 if any file
failed (`--stop-on-error` stops at the first). Scripts have no file access unless it is granted:
they may read the script's folder and the project's folder, and write nowhere (`batch-script`
lets them write in `--out-dir`).

| Option | Meaning |
|---|---|
| `--arg NAME=VALUE` | An argument the script reads with `arg("NAME", default)` or `args()` (repeatable; values read as true/false, whole numbers, numbers or text) |
| `--allow-read DIR`, `--allow-write DIR` | More folders the script may read or write (repeatable) |
| `--library DIR` | The folder `import "lib/name" as m;` reads [libraries](scripting.md#libraries) from (default: the script's own folder; none for `-e`) |
| `--time-limit SECS`, `--max-ops N` | The limits of one run (defaults 10 s, 10,000,000 operations) |
| `--json` | `run-script` prints one JSON object on stdout: `ok`, `value`, `output` (kind, text, line), `commands`, `operations`, `seconds`, `error` (file, line, column, message) |
| `--no-save` | Run and report, write nothing |

Errors print as `file:line:column: message` and give a non-zero exit status. Commands the script
ran before an error stay applied in memory but nothing is saved.

### Hair

One command, `hair`, with the operation in `op`: `{"cmd":"hair","op":"add","mesh":3}`. Hair is an
object under the mesh it grows from. Every operation is one undo step; strokes, numeric brush
applications and map edits give the same result from the window, a script or the journal.

| `op` | Fields |
|---|---|
| `add` | `mesh`, `grow?` (`amount` {`Count`: n} or {`PerUnitArea`: d}, `length`, `length_jitter`, `points_per_strand`, `seed`, `lean`, `lean_toward`, `bend`), `interp?` (`amount`, `blend` (`Barycentric` or {`Nearest`: {`k`}}), `points_per_strand?`, `length_jitter`, `clump`, `frizz`, `width`), `name?`; without them: 1,000 guides a tenth of the mesh's diagonal long and 20,000 hairs |
| `remove` | `id` |
| `regrow` | `id`, `grow?`: grow the guides again, following the density map; grooming is lost |
| `set_render` | `id`, `interp` |
| `stroke` | `id`, `stroke`: `brush` {`kind` (comb, cut, length, puff, smooth, clump, frizz), `radius`, `strength`, `falloff`, `invert`, `cut_length`, `frequency`, `seed`}, `symmetry` {`x`, `y`, `z`}, `dabs` [{`center`, `normal`, `delta`, `pressure`}] in the emitter's space |
| `apply` | `id`, `brush`, `symmetry?`, `center?` [x, y, z], `direction?` [x, y, z]: one dab, over the whole groom when `center` is left out; `direction` is the comb movement |
| `map` | `id`, `map` (mask, density), `edit`: `fill` + `value` · `invert` · `clear` · `paint` + `center`, `radius`, `value`, `strength`, `falloff` |
| `set_dynamics` | `id`, `dynamics?` (`gravity`, `wind` in world units per second squared, `damping`, `stiffness`, `iterations`, `collide`, `clearance`), `timing?` (`start_frame`, `end_frame`, `fps`, `substeps`), `seed?`; throws a bake away |
| `bake` | `id`: simulate the guides over the frames (Verlet, length constraints, emitter collision) and cache them; the timeline, the viewport, renders and exports play the cache |
| `clear_bake` | `id` |

`modelcraft-cli render` and the Render View include hair: the object's base colour is the hair colour
and its roughness the fibre roughness (CPU path tracer only; the GPU one declines hair).

A bake is derived data: it is not saved in the project (the settings and seed are, so baking again gives
the same bytes) and it is dropped when the guides or the settings change. Hair exports from the command
line: `modelcraft-cli hair-export scene.modelcraft Hair out.glb [--strands N] [--guides] [--emitter]` writes
glTF lines (one `LINES` primitive; the point width as the `_WIDTH` vertex attribute; `extras.modelcraft` holds
the strand and point counts) and `hair-export … out.mcpc` the guides as a point cache (every baked frame, or
one frame without a bake), in world space unless `--emitter` is given.

The mask (1 where brushes act fully, 0 where they never touch a guide) and the density (where
`regrow` puts guides) are values on the emitter's vertices. Limits (checked before allocating):
200,000 guides, 5,000,000 hairs, 64 points per strand and 40 million points per operation, 50,000
dabs per stroke.

```
modelcraft-cli macro-script <commands.jsonl|project.journal> [--name NAME] [-o <out.rhai>]
```

`macro-script` writes a [recorded macro](scripting.md#recorded-macros) script from one JSON command
per line (as `run` reads them) or from a crash journal, printing it without `-o`. It replays the
commands on an empty scene to learn which objects each one creates, so the script names them
instead of relying on their numbers, and the script repeats the commands as one undo step.

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

| `add_constraint` | `kind` (point, orient, scale, parent, aim), `target`, `sources`, `weights?` (one per source), `maintain_offset` (default true), `aim` (default y), `up` (default z), `up_object?`, `name?` |
| `add_ik` | `end` (the chain's end joint), `goal?` (an object; a group is created at the end joint without it), `pole?`, `bones` (default 2), `solver` (auto, two_bone, fabrik, ccd), `name?` |
| `set_driven_key` | `driver`, `driver_path`, `target`, `path`, `driver_value?`, `value?` (the attributes' current values without them): one key of the curve that maps the driver's value to the driven attribute |
| `set_expression` | `target`, `path`, `expr` (empty removes it) |
| `set_constraint` | `id`, `weight?` (0–1; the IK/FK blend of an IK constraint), `enabled?`, `name?`, `source_weights?`, `solver?`, `iterations?`, `pole?` (an object id, 0 for none) |
| `remove_constraint` | `id` |
| `match_ik` | `id`, `to`: `fk` (the joints keep the solved pose, blend 0) or `ik` (the goal and pole move onto the chain, blend 1) |
| `bake_constraints` | `objects`, `start?`, `end?` (the playback range without them), `step` (default 1), `keep?`: key the solved transforms, then switch the constraints off |

| `add_shape` | `mesh`, `from` (a mesh with the same points, in the shape wanted), `name?`; with `target` (a shape's index) and `at` (0–1) the shape becomes an in-between of that target |
| `set_shape` | `mesh`, `target`, `weight?`, `name?` |
| `remove_shape` | `mesh`, `target`, `frame?` (one in-between; the whole shape without it) |
| `add_lattice` | `mesh`, `divisions` [x, y, z] (1–8 cells, default 2), `linear?` (cell by cell instead of one Bézier volume) |
| `edit_lattice` | `id`, `points` (control point indices, X fastest, then Y, then Z), `translate?` or `to?` [x, y, z] in the lattice's space (the box is one unit wide), `reset?`, `linear?` |
| `add_cluster` | `mesh`, `points?`, `weights?`, `radius?` (every point within it of the centre, with a smooth falloff), `center?` [x, y, z] in the mesh's space, `name?` |
| `set_cluster_weights` | `id`, `points`, `weight` (0 takes them out) |
| `move_in_stack` | `id`, `delta` (negative: earlier) |

Blend shapes, lattices, clusters and skins are children of the mesh they deform and run in child
order after the parametric deformers: the deformer stack. A blend shape's weight is the attribute
`target.<index>.weight` of the blend-shape object and a lattice's control points are
`point.<index>.x|y|z` of the lattice, so `anim set_key` keys them. A cluster follows its own
transform.

Constraints run after every command and frame change, in dependency order: a constraint that reads
what another writes runs after it, and constraints that depend on themselves are skipped and named
in the command's `warnings`. A constraint's weight is the attribute `constraint.<id>.weight` of its
target, so `anim set_key` keys it (an IK/FK switch over time).

An expression is one formula giving one number: `+ - * / % ^`, comparisons, `&&`, `||`, `!`,
`cond ? a : b`, brackets, the constants `pi`, `tau`, `e`, and the functions `sin cos tan asin acos
atan atan2 sqrt abs floor ceil round min max clamp lerp pow exp ln sign step smoothstep deg rad`.
Names: `frame`, `time` (seconds), `fps`, `self.<path>` (an attribute of the target) and
`@<object id>.<path>`. For example `{"cmd":"rig","op":"set_expression","target":4,"path":"rotate.z","expr":"sin(time * 2) * 30 + @3.translate.x"}`.

A point cache (`modelcraft-cli bake`) holds a skinned mesh as it is posed frame by frame.
Exporting to `.glb` keeps the rig instead: when the scene has skins, blend shapes, curves or
constraints, the file holds the joints, each bound mesh in its rest shape with joints and
weights, blend shapes as morph targets and one animation clip sampled at every frame of the
playback range (curves, layers, constraints and IK all baked into it). What glTF cannot hold is
printed as warnings. Importing a `.glb` or `.gltf` with skins, morph targets or animations
builds joints, skins, blend shapes and curves from it, under one group named after the file.

### Organising objects

One command, `organize`, with the operation in `op`: `{"cmd":"organize","op":"set_tag","ids":[1],"tag":{"type":"protection","position":true,"rotation":false,"scale":false}}`.
`modelcraft-cli commands` lists the operations.

| `op` | Fields |
|---|---|
| `set_tag`, `remove_tag` | `ids`, `tag` (`display` + `mode`: auto, smooth, flat, wireframe, see_through, bounding_box; `render` + `visible`; `protection` + `position`, `rotation`, `scale`; `look_at` + `target`) or `kind` |
| `add_layer`, `edit_layer`, `remove_layer`, `assign_layer` | `name`, `ids`, `id`, `color`, `visible`, `render`, `locked`, `layer` |
| `set_user_attr`, `remove_user_attr` | `ids`, `name`, `value` (`{"type":"number","value":2.5}`; types number, switch, text, colour, vector) |
| `save_preset`, `apply_preset`, `remove_preset` | `from`, `name`, `parts`, `index`, `ids` |
| `set_pivot`, `pivot_to_center` | `ids`, `pivot` (object space), `keep_in_place` (default true) |
| `instance`, `make_unique` | `ids` |

A look-at tag is an aim constraint (+Z at the target) that the tag keeps in step. A protection tag
makes `set_transform` refuse a change of the channels it names. Hidden layers hide their objects
in the viewport, in renders and in exports; a layer with Render off shows them only in the
viewport; a locked layer refuses edits like a locked object. A number or switch user attribute is
the attribute `user.<name>` (a vector part is `user.<name>.x`), so `anim set_key` keys it.

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
| `--guiding`, `--splitting` | Learn where light comes from and send part of the paths that way; end or multiply paths by what they are expected to add to their pixel. Both render on the CPU (`auto` falls back to it), keep the expected image and help rooms lit indirectly from 64 samples up ([performance.md](performance.md)) |
| `--restir unbiased\|viewport` | Resample the direct light at the first surface (ReSTIR DI on the light tree), on the CPU and the GPU. `unbiased` keeps the expected image (for finals; about 3.5× lower error at equal samples on the many-light benchmark, at 2–3× the time per sample); `viewport` adds reuse between passes, which cleans the first few passes fastest but is up to a few percent darker and stops improving ([performance.md](performance.md)) |
| `--restir-candidates N`, `--restir-neighbors N` | Light samples per pixel and pass (default 8) and neighbouring pixels merged (default 4) |
| `--flip` (`render-bench`) | Also report the FLIP perceptual difference to the reference (low-dynamic-range form, exposure 0, 67 pixels per degree; not checked against the reference implementation) |
| `--denoise [svgf\|nfor]` | Filter the noise out: `nfor` (default; regression on the two half renderings, for finals) or `svgf` (the interactive filter). A `.exr` then holds the filtered image as R, G, B and the original as `noisy.R/G/B`. On `render-bench` it also reports the error after the filter and the filter's time ([performance.md](performance.md)) |
| `--hardware-rays` | Trace through the GPU's ray-query hardware. Needs a build with the cargo feature `hw-ray-query` (off by default, see AGENTS.md) and a device that has it; otherwise software tracing, with a note. Scenes with cut-outs use software too ([performance.md](performance.md)) |
| `--bvh binary\|wide` | Trace through the binary tree (default) or its compressed 8-wide form: the same images, faster on the CPU for large scenes, slower on the GPU ([performance.md](performance.md)) |
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
[--device cpu|gpu] [--order z|independent] [--guiding] [--splitting]` measures error against samples and time on the
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
| `script.run` | `{source, name?, args?}` | the script's report: `{ok, value, output, commands, operations, seconds, error, selection?}` ([scripting.md](scripting.md#over-the-control-channel)) |

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
