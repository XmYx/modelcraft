# ModelCraft vs Cinema 4D, Maya and ZBrush: parity assessment

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** minor (polish-pass re-measure) · **Target:** Maxon Cinema 4D 2026 (ease of use), Autodesk Maya 2026 (depth and modularity), Maxon ZBrush 2026 (sculpting and painting)

This is the only file that names the reference products. Everything here was learned from their
public manuals, tutorials and observed behaviour; no code, assets or file internals were read
(see [AGENTS.md](../AGENTS.md) §2). Labels inside ModelCraft are our own
([feature-names.md](feature-names.md)).

## Headline

| | Value | Kind |
|---|---|---|
| Share of the 125 rows in this map (D and X = 1, P = ½) | **59 %** (was 55 % of 121 before the polish pass) | counted |
| Against the full products (thousands of features this map does not list: rigging, animation, dynamics, rendering, scripting depth) | **≈ 12 %** (was ≈ 10 %) | estimated |
| Viewport, 2.0 M triangles, RTX 4090, 1600×1000 window, no vsync | 296 fps orbiting, 283 fps while sculpting | measured |
| Sculpt dab, 1.0 M quads, radius ≈ 30 k vertices, X symmetry | 0.2–0.4 ms (CLI), 1.2 ms in the app | measured |
| Rows: done / partial / missing / exceeds | 58 / 17 / 43 / 7 | counted |

Status per row: **D** done (works, tested), **P** partial (works with a stated limit), **M** missing,
**X** exceeds the references in a way we mean to keep. A dot in a reference column means the
reference app has the feature in a comparable form.

## By area (counted from the tables below)

| Area | Rows | Done | Partial | Missing | Exceeds | Share of rows |
|---|---|---|---|---|---|---|
| Viewport and navigation | 13 | 7 | 5 | 1 | 0 | 73 % |
| Scene graph, object manager, attributes | 10 | 5 | 1 | 4 | 0 | 55 % |
| Dependency graph and history | 9 | 6 | 0 | 2 | 1 | 78 % |
| Primitives and generators | 7 | 2 | 0 | 5 | 0 | 29 % |
| Deformers | 5 | 2 | 0 | 3 | 0 | 40 % |
| Polygon modeling | 18 | 11 | 2 | 4 | 1 | 72 % |
| Subdivision surfaces | 6 | 3 | 0 | 3 | 0 | 50 % |
| UVs | 4 | 0 | 1 | 3 | 0 | 12 % |
| Sculpting | 14 | 5 | 3 | 5 | 1 | 54 % |
| Painting | 3 | 0 | 1 | 2 | 0 | 17 % |
| Materials | 4 | 1 | 1 | 2 | 0 | 38 % |
| Rigging, skinning, animation | 3 | 0 | 0 | 3 | 0 | 0 % |
| Rendering | 3 | 0 | 1 | 2 | 0 | 17 % |
| Import and export | 8 | 5 | 0 | 2 | 1 | 75 % |
| Scripting, CLI, agent control | 4 | 1 | 1 | 1 | 1 | 62 % |
| UI and workspaces | 9 | 7 | 1 | 0 | 1 | 94 % |
| Stability and performance | 5 | 3 | 0 | 1 | 1 | 80 % |

The map lists the essentials a modeler or sculptor reaches for first, so its share is far above
the share of the whole products. Read the share as "how much of the first-year essentials exist",
and the ≈ 10 % as the honest distance to the references.

## Readiness by audience

| Audience | Ready? | Why |
|---|---|---|
| Hobbyist blocking out a model for TextureCraft or ArtCraft | Preview | Primitives, history, smooth cage, glTF export work; no UV editing yet |
| Sculptor roughing forms on a dense mesh | Preview | Nine brushes, masks, symmetry, pressure, 1 M+ polys interactive; no multiresolution or live detail yet |
| Professional modeler | No | Missing knife, bridge, booleans, snapping, UV tools, normals editing |
| Rigging/animation | No | Not started |

## Target and how it was measured

Rows come from the three products' public feature lists and manuals, grouped into the areas
below. A row is **D** only when the feature runs in ModelCraft today and a test covers it (the test
names are in [What works today](#what-works-today-evidence)). Percentages count D as 1 and P as ½
over all rows of an area; the headline weights each area by its row count. Hours are Opus 5.5
agent hours, rough.

## By feature area

### Viewport and navigation

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Orbit / pan / zoom | • | • | • | D | Three presets: Alt + buttons, middle button, right button; keyboard arrows/±; wheel |
| Frame selection | • | • | • | D | F |
| Perspective + orthographic views | • | • | • | D | Front/Right/Top presets, Alt+1..4 |
| Single / four-view layout | • | • | | D | Space |
| Ground grid, axis triad | • | • | • | D | |
| Shaded + wireframe-on-selection display | • | • | • | D | Auto-smoothed normals (40°) |
| Gizmos: move / rotate / scale | • | • | • | D | Axis and free handles; numeric alternative |
| Snapping (grid, vertex, edge) | • | • | | P | Gizmo snaps to grid steps, 15° and 0.1 scale (Shift+S, Ctrl inverts per drag); no vertex/edge/surface snapping |
| Isolate / hide selection | • | • | • | P | Per-object visibility only |
| Camera bookmarks, saved views | • | • | | M | |
| Viewport PBR (lights, IBL, shadows) | • | • | | P | Studio three-light rig, roughness; no IBL/shadows |
| Matcap / sculpt shading | | | • | P | One clay look; no material library |
| Headless viewport renders (thumbnails, CI) | | • | | P | GPU readback works (`modelcraft-cli bench`, tests); no CLI render command yet |

### Scene graph, object manager, attributes

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Object tree with nesting | • | • | | D | Scene panel; tree items with level and selection in the accessibility tree |
| Rename, duplicate, delete, reparent | • | • | | D | Reparent through the row menu (no drag yet) |
| Groups / nulls | • | • | | D | |
| Attributes panel for the active object | • | • | • | D | Properties: transform, shape, cage, deformer, material, history |
| Multi-object selection | • | • | | P | Selection list; Properties edits the active object only |
| Drag-and-drop in the tree | • | • | | M | Menu alternative exists |
| Tags / per-object behaviours | • | | | M | |
| Layers / display layers | • | • | | M | |
| Instances | • | • | | M | |
| Numeric entry for every transform | • | • | • | D | Properties fields and Transform by Numbers (Ctrl+T) |

### Dependency graph and history

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Parametric primitives (editable after creation) | • | • | • | D | Seven generators |
| Construction history: edit an early step, later steps re-run | | • | | D | Build History; evaluator re-runs from the first changed step |
| Toggle / remove a history step | | • | | D | |
| Collapse history into a mesh | • | • | | D | |
| Stale steps degrade gracefully | | P | | X | A step whose selection no longer fits is skipped and flagged, never fatal |
| Node editor for the graph | | • | | M | |
| Expressions / driven keys | • | • | | M | |
| Generators as objects (cage of a child) | • | | | D | Smooth cage |
| Deformers as children | • | • | | D | Bend, twist, taper, ripple, round |

### Primitives and generators

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Cube, plane, sphere, cylinder, cone, torus | • | • | • | D | With UVs, segments |
| Quad sphere (cube-mapped) | • | • | • | D | The sculpting base |
| Capsule, disc, tube, pyramid, platonic solids | • | • | • | M | |
| Text, spline primitives | • | • | | M | |
| Extrude / lathe / loft / sweep generators | • | • | | M | |
| Array / cloner, symmetry generator | • | • | | M | |
| Boolean generator | • | • | • | M | |

### Deformers

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Bend, twist, taper | • | • | • | D | In the deformer's own frame |
| Ripple / wave, spherify | • | • | | D | Ripple, Round |
| Lattice / FFD | • | • | | M | |
| Wrap, shrink-wrap, surface | • | • | | M | |
| Falloff fields | • | • | | M | |

### Polygon modeling

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Vertex / edge / face modes, click and box select | • | • | | D | 1–4, Ctrl+A, Shift adds |
| Extrude faces (region) | • | • | • | D | |
| Inset faces | • | • | | D | Per face |
| Bevel / chamfer edges | • | • | • | P | One segment, flat; no profile or segments |
| Loop cut (edge ring) | • | • | | D | N cuts; stops at non-quads, keeps neighbours conforming |
| Merge vertices, weld by distance | • | • | | D | |
| Delete vertices / edges / faces | • | • | • | D | |
| Move / rotate / scale components | • | • | • | D | Gizmo or numbers, recorded as a history step |
| Triangulate, flip normals | • | • | | D | |
| Bridge, fill hole | • | • | • | D | Bridge two faces, fill one hole, both as history steps; no edge-loop bridge or grid fill |
| Knife / slice | • | • | | M | |
| Loop / ring selection, grow/shrink | • | • | | D | Alt+L / double-click, Alt+R, Ctrl+= / Ctrl+- |
| Booleans | • | • | • | M | |
| Retopology tools | • | • | • | M | |
| Edge creases / hard edges | • | • | • | M | Auto-smoothing angle only |
| Patches (face groups) | | | • | P | Stored per face, tinted, masked by patch; no grouping tools in the UI yet |
| Mirror modeling (symmetry with a welded seam) | • | • | • | D | Mirror X/Y/Z history step, keeps UVs |
| Topology validated after every operation | | | | X | Every result rebuilds through `PolyMesh::build`; property-tested |

### Subdivision surfaces

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Live subdivision generator on a cage | • | • | | D | Smooth cage, 0–6 levels |
| Subdivide as a destructive / history step | • | • | • | D | |
| Boundary rules | • | • | • | D | Border kept as a curve |
| Creases / semi-sharp edges | • | • | • | M | |
| Separate viewport / render levels | • | • | | M | |
| GPU subdivision | | • | | M | |

### UVs

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| UVs on primitives, kept through edits | • | • | • | P | Interpolated through ops; extrude walls get simple UVs |
| UV editor | • | • | | M | |
| Seams, unwrap (LSCM/ABF-like) | • | • | • | M | Wait for craft-mesh-ops `unwrap_uvs` (craft-libs #1233) |
| Packing, UDIM | • | • | | M | |

### Sculpting

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Draw, clay, smooth, inflate, grab, pinch | • | • | • | D | |
| Flatten, crease | • | • | • | D | |
| Mask brush, clear/invert/fill, mask by patch | • | • | • | D | |
| X/Y/Z symmetry | • | • | • | D | |
| Pen pressure → strength / size | • | • | • | P | Through egui touch force; not verified on every tablet driver |
| Falloff curves, stamps (brush patterns) | • | • | • | P | Five curves, three procedural stamps; no image stamps yet |
| 1 M+ polygons interactive | • | • | • | D | 2.0 M triangles at ~280 fps while sculpting (measured) |
| Multiresolution levels | • | • | • | M | Planned as "Detail levels" |
| Dynamic topology | | | • | M | Planned as "Live detail" |
| Voxel / even remesh | | | • | M | Planned as "Even remesh" |
| Auto retopology | | | • | M | |
| Layers | • | | • | P | One sculpt layer per history step; no blending of several |
| Polygon-group visibility, hide/show | | | • | M | |
| Stroke undo per stroke, replayable strokes | • | • | • | X | Strokes are data: journaled, replayed bit-exactly after a crash |

### Painting

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Vertex colour painting | • | • | • | M | |
| Texture / PBR channel painting | • | • | • | M | Hand-off to TextureCraft (GLB export) instead |
| Paint workspace | • | | • | P | Patch view and export only |

### Materials

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Per-object PBR material (base colour, metallic, roughness) | • | • | • | D | glTF metallic-roughness core, exported to GLB |
| Link a MaterialCraft `.mcmat` | | | | P | Stored and saved; not yet rendered |
| Node material editor | • | • | | M | Belongs to MaterialCraft |
| Textures in the viewport | • | • | • | M | |

### Rigging, skinning, animation

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Joints, skin weights | • | • | | M | |
| Keyframes, timeline, graph editor | • | • | | M | |
| Blend shapes | • | • | • | M | |

### Rendering

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Real-time viewport PBR | • | • | • | P | |
| Path-traced preview | • | • | | M | Later, ideally a shared craft-raster3d (RFC-0032) |
| Batch / CLI renders | • | • | • | M | |

### Import and export

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| glTF 2.0 / GLB import | • | • | • | D | Through craft-mesh; welded, parts become patches |
| GLB export | • | • | • | D | Hierarchy, TRS, materials, UVs, normals; triangulated (glTF has no quads) |
| OBJ import/export | • | • | • | D | Quads kept |
| STL export, STL/PLY import | • | • | • | D | |
| FBX | • | • | • | M | Proprietary; read-only support exists in craft-mesh, not wired |
| USD | • | • | | M | Later, with craft-usd |
| Native project, atomic saves | • | • | • | D | `.modelcraft` ZIP |
| Crash journal with exact replay | | | | X | Every command, sculpt strokes included |

### Scripting, CLI, agent control

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Scripting language | • | • | • | M | No embedded language |
| Command line (batch) | • | • | | D | `modelcraft-cli` run/exec/convert/export/info/validate/recover/bench |
| Machine control channel | | • | | P | JSON-lines on stdin/stdout; no socket, no MCP server yet |
| Every UI action is a serializable command | | P | | X | The same `Command`s drive UI, CLI, journal and tests |

### UI and workspaces

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Dense dark theme | • | • | • | D | Text ≥ 4.5:1 (tested) |
| Workspace layouts (model, sculpt, paint) | • | • | • | D | Ctrl+1..3 |
| Dockable panels | • | • | | P | Dock left/right, float, hide per workspace; no tabs or splits |
| Tool options bar | • | • | • | D | |
| Command palette, shortcut for every action | • | • | | D | Ctrl+K |
| Customisable shortcuts | • | • | • | D | Suite Keyboard Shortcuts dialog (craft-keymap): conflicts, saved sets, per-workspace contexts |
| Undo history panel (jump to any step) | | • | • | D | Undo History panel |
| Open Recent, incremental save | • | • | • | D | File ▸ Open Recent; Save Incremental (Ctrl+Alt+S) |
| Accessibility: keyboard paths, screen-reader tree | | | | X | Tree items, viewport object nodes, numeric gizmo path, kittest tests |

### Stability and performance

| Feature | C4D | Maya | ZB | ModelCraft | Notes |
|---|---|---|---|---|---|
| Autosave | • | • | • | D | Journal plus snapshots for untitled scenes |
| Prompt before discarding unsaved changes | • | • | • | D | New, Open, Open Recent and Quit ask Save / Don't Save / Cancel |
| Never-panic policy, hostile-input tests | | | | X | Lints deny panics; proptests for mesh ops and formats |
| Bounded undo memory | | | | D | 1 GiB budget, oldest dropped |
| Multithreaded evaluation | • | • | • | M | Single-threaded today |

## What works today (evidence)

- Mesh: `crates/mesh/src/tests.rs` (`arbitrary_soups_build_valid_meshes` proptest, repair, round trips).
- Ops: `crates/ops/src/edit.rs` tests (`bevel_all_cube_edges_stays_closed`, `loop_cut_around_a_cube`, `random_edit_sequences_stay_valid` proptest), `subdivide.rs`, `deform.rs`, `primitives.rs`.
- History: `crates/scene/src/tests.rs` (`history_is_cached_and_reevaluated_from_the_change`, `smooth_cage_and_deformers_follow_their_children`).
- Sculpt: `crates/sculpt/src/tests.rs` (every brush, mask protection, symmetry, deterministic replay, dirty chunks).
- Files: `crates/formats` (project round trip with blobs, corrupted-project proptest, GLB/OBJ/STL round trips, torn journal).
- Engine: `crates/engine/src/tests.rs` (`journal_recovers_unsaved_work_after_a_crash`, `undo_memory_is_bounded`).
- GPU: `crates/render/src/gpu.rs` `renders_a_quad_when_a_gpu_is_present` (renders and reads back).
- UI: `apps/modelcraft/src/ui_tests.rs`, eleven keyboard-driven kittest flows with the accessibility check on (save prompt, undo history, loop/grow selection, mirror, snapping, shortcuts dialog among them).
- Selection and new ops: `crates/ops/src/select.rs` (`loops_rings_and_borders`, `grow_and_shrink_faces`), `edit.rs` (`bridge_fill_and_mirror`, mirror/bridge/fill in the random-sequence proptest).
- Screenshots: [modeling with history](images/modelcraft-model.png), [sculpting 2 M triangles](images/modelcraft-sculpt.png).

## Where we mean to exceed the references

1. **One app for all three jobs**: parametric modeling with history, generators and deformers, and
   high-resolution sculpting share one scene; a sculpt layer is just another history step.
2. **Crash-proof by construction**: atomic saves plus a journal of every command (sculpt strokes
   included) that replays bit-exactly; no panics anywhere (lint-enforced).
3. **Agent and CLI control**: every action is a serializable command; the CLI, a JSON-lines channel
   and the tests drive the same engine.
4. **GPU-first with a CPU fallback**: chunked vertex uploads keep sculpting interactive at millions of
   polygons; machines without a GPU still get a wireframe view and full test coverage.
5. **Accessible 3D**: keyboard paths and screen-reader nodes for a domain where the references offer few.

## Calibration: how the hours were derived

The vertical slice (seven crates, app, CLI, docs) took about 6 agent hours. Remaining work in
[gaps.md](gaps.md) is estimated by comparison with it: a modeling tool of bevel's size ≈ 2–4 h, a
subsystem like multiresolution or UV unwrapping ≈ 15–30 h, rigging + animation ≈ 80 h. Full parity
with all three references is several thousand hours; the roadmap aims at the essentials first.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-17 | minor | Polish pass: snapping, loop/ring/grow, bridge, fill, mirror, rebindable shortcuts, undo history, recent files, incremental save, save prompt; 6 rows added |
| 2026-11-17 | major | First assessment with the vertical slice |
