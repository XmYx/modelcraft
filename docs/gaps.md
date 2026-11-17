# Where ModelCraft falls short of the references

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** major (ranked plan toward 90 % parity) · **Target:** see [target-app-parity.md](target-app-parity.md)

This is the work list toward the program goal of **90 % weighted parity** with Cinema 4D, Maya,
ZBrush, V-Ray and Arnold. Every item is a group of rows from the
[parity map](target-app-parity.md); when an item lands, flip its rows there, run
`cargo xtask parity --write`, and delete or shrink the item here (the progress log records what
landed).

## Where we stand

| | Value | Kind |
|---|---|---|
| Weighted parity today | 21.7 % | counted by `cargo xtask parity` |
| Row parity today | 30.5 % of 330 in-scope rows | counted |
| Hours to 90 % weighted parity, in the order below | ≈ 2,760 | estimated from row sizes |
| Hours to 100 % | ≈ 3,425 | estimated from row sizes |

**How the numbers are made.** *Hours* come from the row sizes in the map (S ≈ 3 h, M ≈ 8 h,
L ≈ 25 h, XL ≈ 60 h, Opus 5.5 agent hours, a partial row counts half). *Gain* is the rise in
weighted parity, in percentage points, if every row of the item becomes D: it is computed with the
same formula `cargo xtask parity` uses, not guessed. The rank orders items by gain per hour,
taking an item only once the items it needs are done; *Parity after* and *Hours so far* are
cumulative along that order.

**What it means.** 90 % is reached after 64 of the 70 items. The six that can stay undone at 90 %
are R7 advanced light transport, G4 the procedural geometry graph, R6 GPU path tracing, D6 smoke,
fire and liquids, A8 muscles and M11 NURBS (≈ 660 hours together). No subset of modeling and
sculpting gets near 90 %: rendering (with materials), animation, rigging and simulation (with hair
and volumes) hold 35 % of the area weight and 41 % of the remaining gap (35.5 of 85.8 points). The workstreams are independent enough to run in parallel; the hours are effort,
not calendar time.

## By workstream

Sections for each workstream follow the ranked plan, in this order: render engine, sculpting,
animation and rigging, modeling, UV and paint, scripting and node graph, simulation, pipeline
formats, UI/UX.

| Workstream | Rows | Hours | Gain (points) | Gain per 100 h |
|---|---|---|---|---|
| [Modeling](#modeling) | 65 | 731 | 18.2 | 2.5 |
| [Render engine](#render-engine) | 48 | 759 | 15.0 | 2.0 |
| [Animation and rigging](#animation-and-rigging) | 32 | 387 | 10.9 | 2.8 |
| [Simulation](#simulation) | 19 | 404 | 9.6 | 2.4 |
| [Sculpting](#sculpting) | 25 | 400.5 | 9.3 | 2.3 |
| [UV and paint](#uv-and-paint) | 19 | 197 | 6.8 | 3.5 |
| [UI/UX](#uiux) | 31 | 181 | 6.5 | 3.6 |
| [Scripting and node graph](#scripting-and-node-graph) | 14 | 217.5 | 5.6 | 2.6 |
| [Pipeline formats](#pipeline-formats) | 9 | 148 | 3.8 | 2.6 |

## Top 10 by gain per hour

| Rank | Item | Workstream | Hours | Gain (points) | Gain per 100 h |
|---|---|---|---|---|---|
| 1 | ~~**X7** Undo history kept across sessions~~ (done) | UI/UX | 8 | 0.58 | 7.3 |
| 2 | **A1** Keyframes and timeline: key any attribute, auto key, timeline, dope sheet, playblast, follow spline, audio, turntable | Animation and rigging | 36 | 1.94 | 5.4 |
| 3 | **U3** Vertex colour painting and conversion to textures | UV and paint | 17.5 | 0.83 | 4.7 |
| 4 | **X3** Docking with tabs and splits, radial menus, shelves, full preferences, shipped translations | UI/UX | 25.5 | 1.19 | 4.6 |
| 5 | **G2** Control channel over a socket and an MCP server | Scripting and node graph | 4 | 0.18 | 4.6 |
| 6 | **M14** Patch grouping tools and mesh cleanup in the UI | Modeling | 3 | 0.13 | 4.3 |
| 7 | **A6** Layers and clips: non-linear clips, animation layers, motion paths, cached playback, camera sequencer, point cache | Animation and rigging | 65 | 2.71 | 4.2 |
| 8 | **X4** Object manager depth: multi-edit, drag-and-drop, search, tags, layers, instances, sets, user attributes, lock, copy attributes, pivots | UI/UX | 54 | 2.24 | 4.1 |
| 9 | **F1** FBX import and export, up-axis and unit conversion | Pipeline formats | 11 | 0.44 | 4.0 |
| 10 | **G1** Embedded scripting: language, object-model API, headless API, editor, callbacks, user tools | Scripting and node graph | 54.5 | 2.12 | 3.9 |

These are cheap because they finish areas that are already partly built (undo, docking, the object
manager, scripting over the existing command surface) or open an empty, heavily weighted area at
its cheapest entry point (keyframes and timeline). They are not the most important features:
the render core (R1), detail levels (S1), live detail (S2) and UV unwrapping (U1) matter more to
users and sit lower only because they are expensive.

## Ranked plan

| # | Item | Workstream | Rows | Hours | Gain (points) | Gain per 100 h | Needs | Parity after | Hours so far |
|---|---|---|---|---|---|---|---|---|---|
| 1 | ~~**X7** Undo history kept across sessions~~ (done) | UI/UX | 1 | 8 | 0.58 | 7.3 | — | 14.8 % | 8 |
| 2 | **A1** Keyframes and timeline: key any attribute, auto key, timeline, dope sheet, playblast, follow spline, audio, turntable | Animation and rigging | 7 | 36 | 1.94 | 5.4 | — | 16.8 % | 44 |
| 3 | **U3** Vertex colour painting and conversion to textures | UV and paint | 3 | 17.5 | 0.83 | 4.7 | — | 17.6 % | 61.5 |
| 4 | **X3** Docking with tabs and splits, radial menus, shelves, full preferences, shipped translations | UI/UX | 5 | 25.5 | 1.19 | 4.6 | — | 18.8 % | 87 |
| 5 | **G2** Control channel over a socket and an MCP server | Scripting and node graph | 1 | 4 | 0.18 | 4.6 | — | 19.0 % | 91 |
| 6 | **M14** Patch grouping tools and mesh cleanup in the UI | Modeling | 2 | 3 | 0.13 | 4.3 | — | 19.1 % | 94 |
| 7 | **A6** Layers and clips: non-linear clips, animation layers, motion paths, cached playback, camera sequencer, point cache | Animation and rigging | 6 | 65 | 2.71 | 4.2 | A1 | 21.8 % | 159 |
| 8 | **X4** Object manager depth: multi-edit, drag-and-drop, search, tags, layers, instances, sets, user attributes, lock, copy attributes, pivots | UI/UX | 11 | 54 | 2.24 | 4.1 | — | 24.0 % | 213 |
| 9 | **F1** FBX import and export, up-axis and unit conversion | Pipeline formats | 2 | 11 | 0.44 | 4.0 | — | 24.5 % | 224 |
| 10 | **G1** Embedded scripting: language, object-model API, headless API, editor, callbacks, user tools | Scripting and node graph | 6 | 54.5 | 2.12 | 3.9 | — | 26.6 % | 278.5 |
| 11 | **F5** Live link with suite apps, EXR/HDR images, 3MF | Pipeline formats | 3 | 19 | 0.73 | 3.8 | — | 27.3 % | 297.5 |
| 12 | **M1** Modeling essentials II: knife, multi-segment bevel, vertex bevel, slide, connect, collapse, quadrangulate, spin/poke, separate/combine, shell, relax, loop bridge, grid fill, edge extrude | Modeling | 13 | 55 | 2.07 | 3.8 | — | 29.4 % | 352.5 |
| 13 | **A2** Graph editor with curve cleanup and baking | Animation and rigging | 3 | 31 | 1.16 | 3.8 | A1 | 30.6 % | 383.5 |
| 14 | **M4** Creases, hard edges, custom normals, render subdivision levels | Modeling | 4 | 27 | 0.94 | 3.5 | — | 31.5 % | 410.5 |
| 15 | **D4** Hair and grooming: guides and brushes, interpolated hair, fibre meshes, cards, groom exchange, fur, hair dynamics | Simulation | 7 | 102 | 3.54 | 3.5 | A1 | 35.0 % | 512.5 |
| 16 | **M2** Soft selection, live symmetric editing, align and distribute | Modeling | 3 | 19 | 0.65 | 3.4 | — | 35.7 % | 531.5 |
| 17 | **M8** Deformer set: lattice, wrap, noise, delta smooth, spline, wire, collision, explode, painted weights | Modeling | 9 | 62 | 2.07 | 3.3 | — | 37.8 % | 593.5 |
| 18 | **X2** Viewport aids: reference images, bookmarks, display modes, isolate, matcaps, stats, headless render command, safe frames, camera objects | UI/UX | 9 | 32 | 1.02 | 3.2 | — | 38.8 % | 625.5 |
| 19 | **D5** Sparse volumes and volume modeling | Simulation | 2 | 50 | 1.55 | 3.1 | — | 40.3 % | 675.5 |
| 20 | **S4** Masks and visibility: cavity/curvature masks, lasso and clip tools, hide by group, shell extract, watertight | Sculpting | 5 | 28.5 | 0.87 | 3.1 | — | 41.2 % | 704 |
| 21 | **G5** Plugin SDK | Scripting and node graph | 1 | 25 | 0.74 | 3.0 | — | 41.9 % | 729 |
| 22 | **R4** Materials: OpenPBR-class layered surface, texture inputs, bump/normal/displacement, per-patch assignment, rendered .mcmat links, MaterialX, library | Render engine | 7 | 86 | 2.44 | 2.8 | — | 44.4 % | 815 |
| 23 | **X1** Vertex/edge/surface snapping, gizmo orientations and pivots, measure tool and units | UI/UX | 3 | 24 | 0.68 | 2.8 | — | 45.1 % | 839 |
| 24 | **S1** Detail levels: multiresolution with reconstruction of lower levels | Sculpting | 2 | 33 | 0.93 | 2.8 | — | 46.0 % | 872 |
| 25 | **S7** Decimation and detail projection | Sculpting | 2 | 33 | 0.93 | 2.8 | — | 46.9 % | 905 |
| 26 | **U1** UV editor, seams, unwrap, packing, checker and distortion display | UV and paint | 5 | 62.5 | 1.70 | 2.7 | — | 48.6 % | 967.5 |
| 27 | **U2** UV tools: UDIMs, projections, relax/straighten/stitch, UVs from groups, UV transfer | UV and paint | 5 | 35 | 1.33 | 3.8 | U1 | 49.9 % | 1002.5 |
| 28 | **U5** Baking: normal, displacement, occlusion, curvature, cavity | UV and paint | 3 | 41 | 1.48 | 3.6 | U1 | 51.4 % | 1043.5 |
| 29 | **U4** Texture and PBR painting, projection painting, material painting | UV and paint | 3 | 41 | 1.48 | 3.6 | U1 | 52.9 % | 1084.5 |
| 30 | **M7** More primitives and generators: capsule/platonics, terrain, array/symmetry, live reduction, metaballs, wireframe, volume mesher | Modeling | 7 | 63 | 1.67 | 2.7 | — | 54.6 % | 1147.5 |
| 31 | **A5** Blend shapes and pose-driven correctives | Animation and rigging | 2 | 16 | 0.42 | 2.6 | — | 55.0 % | 1163.5 |
| 32 | **F4** File references, proxies and an asset browser | Pipeline formats | 2 | 33 | 0.87 | 2.6 | — | 55.9 % | 1196.5 |
| 33 | **M5** Manual retopology, polygon pen, attribute transfer | Modeling | 3 | 41 | 1.03 | 2.5 | — | 56.9 % | 1237.5 |
| 34 | **R9** Viewport realism: image lighting and shadows, screen-space effects, textures, colour-managed view | Render engine | 5 | 40.5 | 1.01 | 2.5 | — | 57.9 % | 1278 |
| 35 | **D1** Rigid bodies, forces, simulation cache, dynamics on clones | Simulation | 4 | 49 | 1.22 | 2.5 | A1 | 59.1 % | 1327 |
| 36 | **M12** Voronoi fracture | Modeling | 1 | 25 | 0.67 | 2.7 | D1 | 59.8 % | 1352 |
| 37 | **S2** Live detail and even remesh | Sculpting | 2 | 50 | 1.24 | 2.5 | — | 61.0 % | 1402 |
| 38 | **G3** Node view of the graph, attribute connections, utility nodes, expressions and driven keys | Scripting and node graph | 4 | 49 | 1.17 | 2.4 | — | 62.2 % | 1451 |
| 39 | **X5** Scene overrides (takes) | UI/UX | 1 | 25 | 0.60 | 2.4 | — | 62.8 % | 1476 |
| 40 | **M9** Falloff fields and field layers | Modeling | 2 | 33 | 0.78 | 2.3 | — | 63.6 % | 1509 |
| 41 | **M10** Cloners and effectors: cloner modes, effectors, fields, selections, instancing, per-letter text, tracer | Modeling | 7 | 85 | 2.86 | 3.4 | M9 | 66.5 % | 1594 |
| 42 | **F2** Alembic caches | Pipeline formats | 1 | 25 | 0.58 | 2.3 | A1 | 67.0 % | 1619 |
| 43 | **S3** Brush depth: ten more brushes, stroke types, presets, image alphas, radial symmetry, verified pressure | Sculpting | 6 | 49.5 | 1.12 | 2.3 | — | 68.2 % | 1668.5 |
| 44 | **M13** GPU subdivision | Modeling | 1 | 25 | 0.56 | 2.3 | — | 68.7 % | 1693.5 |
| 45 | **S5** Sculpt layers with blending, morph target, sculpt gizmo deformers | Sculpting | 3 | 28.5 | 0.64 | 2.2 | S1 | 69.4 % | 1722 |
| 46 | **M6** Splines and spline generators: primitives, pen, spline ops, along-spline, extrude/lathe/loft/sweep, 3D text | Modeling | 6 | 65 | 1.45 | 2.2 | — | 70.8 % | 1787 |
| 47 | **A3** Joints and skinning with weight painting and weight tools | Animation and rigging | 4 | 44 | 0.95 | 2.2 | A1 | 71.8 % | 1831 |
| 48 | **A4** IK solvers, FK/IK switching, constraints, control shapes | Animation and rigging | 4 | 49 | 1.06 | 2.2 | A3 | 72.8 % | 1880 |
| 49 | **S9** Scale: 20 M+ polygons and multithreaded dabs, evaluation and BVH | Sculpting | 2 | 68 | 1.43 | 2.1 | — | 74.2 % | 1948 |
| 50 | **S6** Automatic quad retopology | Sculpting | 1 | 60 | 1.24 | 2.1 | — | 75.5 % | 2008 |
| 51 | **A7** Characters: rig templates, retargeting, motion capture, pose library, facial tools | Animation and rigging | 5 | 86 | 1.76 | 2.0 | A3, A4 | 77.2 % | 2094 |
| 52 | **M3** Booleans: mesh booleans as history steps and a live boolean generator | Modeling | 2 | 50 | 0.99 | 2.0 | — | 78.2 % | 2144 |
| 53 | **F3** USD layers, references and variants | Pipeline formats | 1 | 60 | 1.16 | 1.9 | — | 79.4 % | 2204 |
| 54 | **R10** Node material editor (with MaterialCraft) | Render engine | 1 | 60 | 1.15 | 1.9 | R4 | 80.5 % | 2264 |
| 55 | **D2** Cloth, soft bodies, cloth sculpt brushes | Simulation | 3 | 58 | 1.11 | 1.9 | D1 | 81.7 % | 2322 |
| 56 | **G6** Parallel graph evaluation | Scripting and node graph | 1 | 25 | 0.47 | 1.9 | — | 82.1 % | 2347 |
| 57 | **X6** Large-scene drawing: culling, level of detail, proxies | UI/UX | 1 | 12.5 | 0.23 | 1.8 | — | 82.4 % | 2359.5 |
| 58 | **R1** CPU path tracer: whole-scene BVH, multiple importance sampling, progressive mode, path controls, render region, CLI renders | Render engine | 7 | 119.5 | 2.14 | 1.8 | — | 84.5 % | 2479 |
| 59 | **R3** Camera and output: depth of field, physical camera, render passes, multi-layer EXR, colour management, adaptive sampling, bucket mode, statistics | Render engine | 8 | 54 | 1.40 | 2.6 | R1 | 85.9 % | 2533 |
| 60 | **R2** Lights: point, spot, directional, area, dome with importance sampling, sun and sky, light linking, light groups | Render engine | 6 | 48 | 1.16 | 2.4 | R1 | 87.1 % | 2581 |
| 61 | **R8** Render at scale: render-time instancing, distributed rendering, render queue | Render engine | 3 | 58 | 1.07 | 1.8 | R1 | 88.1 % | 2639 |
| 62 | **D3** Particles | Simulation | 1 | 25 | 0.44 | 1.8 | D1 | 88.6 % | 2664 |
| 63 | **R5** Interactive preview render and denoiser | Render engine | 2 | 50 | 0.86 | 1.7 | R1 | 89.4 % | 2714 |
| 64 | **S8** Insert-mesh and curve brushes, live booleans | Sculpting | 2 | 50 | 0.85 | 1.7 | — | 90.3 % | 2764 |
| 65 | **R7** Advanced light transport: subsurface, hair shading, volumes, render-time displacement, motion blur, caustics, toon lines | Render engine | 7 | 158 | 2.52 | 1.6 | R1, R2 | 92.8 % | 2922 |
| 66 | **G4** Procedural geometry node graph | Scripting and node graph | 1 | 60 | 0.94 | 1.6 | G3 | 93.7 % | 2982 |
| 67 | **R6** GPU path tracing with results matching the CPU path | Render engine | 2 | 85 | 1.28 | 1.5 | R1 | 95.0 % | 3067 |
| 68 | **D6** Smoke, fire and liquids | Simulation | 2 | 120 | 1.77 | 1.5 | D3, D5 | 96.8 % | 3187 |
| 69 | **A8** Muscle and skin-sliding systems | Animation and rigging | 1 | 60 | 0.85 | 1.4 | A3 | 97.6 % | 3247 |
| 70 | **M11** NURBS curves and surfaces with trimming and tessellation | Modeling | 5 | 178 | 2.37 | 1.3 | — | 100.0 % | 3425 |

## Render engine

The biggest single gap: two whole areas (weights 8 and 6) and the materials area score almost nothing. **R1** is the keystone: lights, camera effects, outputs, preview render, GPU rendering and scale all need it, so its real value is the chain R1 → R2 → R3 → R5 (≈ 5.6 points for 271 hours) rather than its own 2.1 points. Build the core as a UI-free crate that could move to craft-libs (a shared craft-raster3d is proposed in RFC-0032), with a CPU reference path that CI can test without a GPU; GPU tests skip cleanly when no adapter exists. Materials (**R4**) follow OpenPBR and MaterialX as open specifications and should render the MaterialCraft `.mcmat` links the scene already stores. Vendor cloud rendering and closed denoiser SDKs are out of scope (see the map).

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **R1** CPU path tracer: whole-scene BVH, multiple importance sampling, progressive mode, path controls, render region, CLI renders | 7 | 119.5 | 2.14 | 58 | — |
| **R2** Lights: point, spot, directional, area, dome with importance sampling, sun and sky, light linking, light groups | 6 | 48 | 1.16 | 60 | R1 |
| **R3** Camera and output: depth of field, physical camera, render passes, multi-layer EXR, colour management, adaptive sampling, bucket mode, statistics | 8 | 54 | 1.40 | 59 | R1 |
| **R4** Materials: OpenPBR-class layered surface, texture inputs, bump/normal/displacement, per-patch assignment, rendered .mcmat links, MaterialX, library | 7 | 86 | 2.44 | 22 | — |
| **R5** Interactive preview render and denoiser | 2 | 50 | 0.86 | 63 | R1 |
| **R6** GPU path tracing with results matching the CPU path | 2 | 85 | 1.28 | 67 | R1 |
| **R7** Advanced light transport: subsurface, hair shading, volumes, render-time displacement, motion blur, caustics, toon lines | 7 | 158 | 2.52 | 65 | R1, R2 |
| **R8** Render at scale: render-time instancing, distributed rendering, render queue | 3 | 58 | 1.07 | 61 | R1 |
| **R9** Viewport realism: image lighting and shadows, screen-space effects, textures, colour-managed view | 5 | 40.5 | 1.01 | 34 | — |
| **R10** Node material editor (with MaterialCraft) | 1 | 60 | 1.15 | 54 | R4 |

## Sculpting

Brush depth (**S3**) is mostly in: ten more brushes, hardness, five stroke modes, eight built-in stamp pictures and presets. Left of it: rotate, elastic-move and scrape brushes, curve strokes, importing your own stamp images, radial symmetry and the pen-pressure check. Live detail and even remesh (**S2**) are in, live detail partly: it only splits (collapsing short edges and flipping edges are missing, so a region never gets coarser and slivers appear at the rim of a refined area), it works on the base detail level without shape layers, and every split re-uploads the whole mesh to the GPU. Even remesh needs a closed mesh and drops UVs and patches. Detail levels (the first row of **S1**) and shape layers (the first row of **S5**) are in; what is left of them: rebuilding lower levels from a dense mesh that has none, showing the top level while working on a lower one, a mask per shape layer, and stepping down while shape layers exist. **S1** detail levels and **S2** live detail with even remesh are the two features sculptors ask for most after retopology ([user-research.md](user-research.md)); both need crash-safe undo that keeps the stroke journal exact. Pen pressure arrives through egui touch force and is not verified on every Wayland, X11 and Windows tablet stack (the winit fork may need pen events); **S3** includes that check. Interactive counts above 2 M triangles have not been measured; **S9** starts by measuring.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **S1** Detail levels: multiresolution with reconstruction of lower levels | 2 | 33 | 0.93 | 24 | — |
| **S2** Live detail and even remesh | 2 | 50 | 1.24 | 37 | — |
| **S3** Brush depth: ten more brushes, stroke types, presets, image alphas, radial symmetry, verified pressure | 6 | 49.5 | 1.12 | 43 | — |
| **S4** Masks and visibility: cavity/curvature masks, lasso and clip tools, hide by group, shell extract, watertight | 5 | 28.5 | 0.87 | 20 | — |
| **S5** Sculpt layers with blending, morph target, sculpt gizmo deformers | 3 | 28.5 | 0.64 | 45 | S1 |
| **S6** Automatic quad retopology | 1 | 60 | 1.24 | 50 | — |
| **S7** Decimation and detail projection | 2 | 33 | 0.93 | 25 | — |
| **S8** Insert-mesh and curve brushes, live booleans | 2 | 50 | 0.85 | 64 | — |
| **S9** Scale: 20 M+ polygons and multithreaded dabs, evaluation and BVH | 2 | 68 | 1.43 | 49 | — |

## Animation and rigging

**A1** has its core: keys on any numeric attribute, curves, the timeline, auto-key, the Key Sheet
and the Curve Editor (see the limits below). Still open in A1: preview capture, following a spline,
audio and the sculpt turntable. Everything else is not started. **A1** came first because rigging, simulation, Alembic and point caches all need a time model; design keys as data on attributes so the journal, CLI and undo cover them like every other command. **A3** skinning and **A4** constraints are the gate for character work; the muscle systems (**A8**) sit last.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **A1** Keyframes and timeline: key any attribute, auto key, timeline, dope sheet, playblast, follow spline, audio, turntable | 7 | 36 | 1.94 | 2 | — |
| **A2** Graph editor with curve cleanup and baking | 3 | 31 | 1.16 | 13 | A1 |
| **A3** Joints and skinning with weight painting and weight tools | 4 | 44 | 0.95 | 47 | A1 |
| **A4** IK solvers, FK/IK switching, constraints, control shapes | 4 | 49 | 1.06 | 48 | A3 |
| **A5** Blend shapes and pose-driven correctives | 2 | 16 | 0.42 | 31 | — |
| **A6** Layers and clips: non-linear clips, animation layers, motion paths, cached playback, camera sequencer, point cache | 6 | 65 | 2.71 | 7 | A1 |
| **A7** Characters: rig templates, retargeting, motion capture, pose library, facial tools | 5 | 86 | 1.76 | 51 | A3, A4 |
| **A8** Muscle and skin-sliding systems | 1 | 60 | 0.85 | 69 | A3 |

Limits of what exists:
- Rotation is keyed as XYZ Euler angles (keying one angle keys all three); there are no other
  rotation orders and no quaternion interpolation.
- Rigging: joints are added by command and moved with the gizmo or by number (no click-to-place
  tool, no viewport picking of joints); a joint's rest orientation lives in its rotation, not in a
  separate channel. Skin weights are painted by a command dab, not by a viewport brush; heat
  weights skip the paper's visibility test, so two limbs that touch can share weights; weights
  cannot be copied between meshes and are saved as JSON. Evaluation is single-threaded and
  `Scene::get` is a linear search, which shows with thousands of objects. glTF export writes
  the posed mesh, not the skin or the animation.
- Animation layers have weight, mute and two modes; there is no solo or lock, weights cannot
  be keyed and layers cannot be reordered. The Key Sheet and Curve Editor show the active layer only.
- Motion paths are drawn, not edited, and there are no ghosts. Point caches are written
  (`modelcraft-cli bake`) but cannot be played back on a mesh yet.
- No box selection or copy/paste of keys.
- The Curve Editor fits its value axis to the shown curves; hide curves to see small ones next to
  large ones.
- Playback shows whole frames at the scene rate on the UI thread.
- Duplicating an object does not copy its curves.
- The app's shortcut handler gives every key to a focused widget, so with keyboard focus on a
  button or a tree row single-key shortcuts do not run; the Animation panel handles Undo, Redo and
  Delete itself while its ruler or a key has focus.

## Modeling

The largest workstream by points because it spans six areas. **M1** closes the everyday tool gaps a professional modeler hits first (knife, multi-segment bevel, slide, connect). Booleans (**M3**) must stay valid as history steps when their inputs change. Splines (**M6**) unlock the extrude/lathe/loft/sweep generators and the along-spline tools; NURBS (**M11**) is Maya-specific depth and ranks last on gain per hour.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **M1** Modeling essentials II: knife, multi-segment bevel, vertex bevel, slide, connect, collapse, quadrangulate, spin/poke, separate/combine, shell, relax, loop bridge, grid fill, edge extrude | 13 | 55 | 2.07 | 12 | — |
| **M2** Soft selection, live symmetric editing, align and distribute | 3 | 19 | 0.65 | 16 | — |
| **M3** Booleans: mesh booleans as history steps and a live boolean generator | 2 | 50 | 0.99 | 52 | — |
| **M4** Creases, hard edges, custom normals, render subdivision levels | 4 | 27 | 0.94 | 14 | — |
| **M5** Manual retopology, polygon pen, attribute transfer | 3 | 41 | 1.03 | 33 | — |
| **M6** Splines and spline generators: primitives, pen, spline ops, along-spline, extrude/lathe/loft/sweep, 3D text | 6 | 65 | 1.45 | 46 | — |
| **M7** More primitives and generators: capsule/platonics, terrain, array/symmetry, live reduction, metaballs, wireframe, volume mesher | 7 | 63 | 1.67 | 30 | — |
| **M8** Deformer set: lattice, wrap, noise, delta smooth, spline, wire, collision, explode, painted weights | 9 | 62 | 2.07 | 17 | — |
| **M9** Falloff fields and field layers | 2 | 33 | 0.78 | 40 | — |
| **M10** Cloners and effectors: cloner modes, effectors, fields, selections, instancing, per-letter text, tracer | 7 | 85 | 2.86 | 41 | M9 |
| **M11** NURBS curves and surfaces with trimming and tessellation | 5 | 178 | 2.37 | 70 | — |
| **M12** Voronoi fracture | 1 | 25 | 0.67 | 36 | D1 |
| **M13** GPU subdivision | 1 | 25 | 0.56 | 44 | — |
| **M14** Patch grouping tools and mesh cleanup in the UI | 2 | 3 | 0.13 | 6 | — |

## UV and paint

**U1** waits on craft-mesh-ops `unwrap_uvs` (craft-libs #1233) for the unwrap itself; the editor, seams and packing can start before it lands. TextureCraft auto-unwraps meshes without UVs today, and the Paint workspace only shows patches and hands off through GLB, so **U3** vertex colour painting is the cheapest real painting step.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **U1** UV editor, seams, unwrap, packing, checker and distortion display | 5 | 62.5 | 1.70 | 26 | — |
| **U2** UV tools: UDIMs, projections, relax/straighten/stitch, UVs from groups, UV transfer | 5 | 35 | 1.33 | 27 | U1 |
| **U3** Vertex colour painting and conversion to textures | 3 | 17.5 | 0.83 | 3 | — |
| **U4** Texture and PBR painting, projection painting, material painting | 3 | 41 | 1.48 | 29 | U1 |
| **U5** Baking: normal, displacement, occlusion, curvature, cavity | 3 | 41 | 1.48 | 28 | U1 |

## Scripting and node graph

Every action is already a serializable command, so **G1** binds a language to an existing surface rather than building one; the language must be Rust-native and on the licence allowlist. G1's language (Rhai, see [scripting.md](scripting.md)), object-model API and headless API are in; the script editor, event callbacks and user tools remain. **G2** wires craft-control (token auth) and craft-mcp-server, which exist in craft-libs. The node view (**G3**) shows the dependency graph the evaluator already has.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **G1** Embedded scripting: language, object-model API, headless API, editor, callbacks, user tools | 6 | 54.5 | 2.12 | 10 | — |
| **G2** Control channel over a socket and an MCP server | 1 | 4 | 0.18 | 5 | — |
| **G3** Node view of the graph, attribute connections, utility nodes, expressions and driven keys | 4 | 49 | 1.17 | 38 | — |
| **G4** Procedural geometry node graph | 1 | 60 | 0.94 | 66 | G3 |
| **G5** Plugin SDK | 1 | 25 | 0.74 | 21 | — |
| **G6** Parallel graph evaluation | 1 | 25 | 0.47 | 56 | — |

## Simulation

Not started. **D1** rigid bodies needs the time model from A1. Hair (**D4**) ranks high because grooming is a whole area with no rows done; its render half (hair shading) is in R7. Smoke, fire and liquids (**D6**) are the most expensive items in the plan and are among the few that can stay undone at 90 %.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **D1** Rigid bodies, forces, simulation cache, dynamics on clones | 4 | 49 | 1.22 | 35 | A1 |
| **D2** Cloth, soft bodies, cloth sculpt brushes | 3 | 58 | 1.11 | 55 | D1 |
| **D3** Particles | 1 | 25 | 0.44 | 62 | D1 |
| **D4** Hair and grooming: guides and brushes, interpolated hair, fibre meshes, cards, groom exchange, fur, hair dynamics | 7 | 102 | 3.54 | 15 | A1 |
| **D5** Sparse volumes and volume modeling | 2 | 50 | 1.55 | 19 | — |
| **D6** Smoke, fire and liquids | 2 | 120 | 1.77 | 68 | D3, D5 |

## Pipeline formats

craft-mesh already reads FBX, so **F1** is mostly wiring plus a writer; FBX is proprietary, and the suite prefers glTF, so the writer must follow public descriptions only. USD (**F3**) waits for a scene-level reader in craft-libs (craft-usd has wire primitives only). glTF stores triangles only, so our GLB round trip loses quads; quad hints in `extras` wait for craft-mesh to keep `extras`. craft-mesh writes GLB only; a `.gltf` JSON writer belongs there.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **F1** FBX import and export, up-axis and unit conversion | 2 | 11 | 0.44 | 9 | — |
| **F2** Alembic caches | 1 | 25 | 0.58 | 42 | A1 |
| **F3** USD layers, references and variants | 1 | 60 | 1.16 | 53 | — |
| **F4** File references, proxies and an asset browser | 2 | 33 | 0.87 | 32 | — |
| **F5** Live link with suite apps, EXR/HDR images, 3MF | 3 | 19 | 0.73 | 11 | — |

## UI/UX

Undo history now survives save and open (X7, done). **X3** builds on craft-ui `docking` for tabs and splits. **X4** is the object-manager depth Cinema 4D users expect; the tree has no drag-and-drop reparenting today (the row menu does it), and Properties edits only the active object.

| Item | Rows | Hours | Gain (points) | Rank | Needs |
|---|---|---|---|---|---|
| **X1** Vertex/edge/surface snapping, gizmo orientations and pivots, measure tool and units | 3 | 24 | 0.68 | 23 | — |
| **X2** Viewport aids: reference images, bookmarks, display modes, isolate, matcaps, stats, headless render command, safe frames, camera objects | 9 | 32 | 1.02 | 18 | — |
| **X3** Docking with tabs and splits, radial menus, shelves, full preferences, shipped translations | 5 | 25.5 | 1.19 | 4 | — |
| **X4** Object manager depth: multi-edit, drag-and-drop, search, tags, layers, instances, sets, user attributes, lock, copy attributes, pivots | 11 | 54 | 2.24 | 8 | — |
| **X5** Scene overrides (takes) | 1 | 25 | 0.60 | 39 | — |
| **X6** Large-scene drawing: culling, level of detail, proxies | 1 | 12.5 | 0.23 | 57 | — |

## Known defects outside the map

These do not move parity but are real.

### Stability
- **View changes are not undoable.** Camera moves never enter the edit history (good: they do not
  flood it) but cannot be undone either. Add a separate view history (previous / next view).
- **Undo budget is estimated, not measured.** Scene-change entries count a flat ~300 bytes per
  object because meshes are shared; a scene with thousands of imported meshes changed one by one
  may hold more than the 1 GiB budget suggests. Measure shared allocations instead.
- **Recovery replays `Import` by path.** If the file moved, that command fails during recovery and
  later steps on the imported object fail with it. Store imported meshes in the journal (as a
  blob next to it) instead.

### Viewport
- Vertex points are drawn by the UI painter and capped at 100 k.

### Performance
- A 1 M-quad smooth cage takes about a second to evaluate; sculpt session start on 1 M quads takes
  ~0.8 s (single-threaded; see S9 and G6).

### Release
- **macOS DMG background.** `packaging/macos/dmg/` has the generator but no ModelCraft art yet:
  draw `background.svg` in the suite style, then render `background.tiff` and
  `dmg-layout.DS_Store` with `generate.py` (needs resvg, Pillow, ds_store and mac_alias). The DMG
  opens with Finder's default window meanwhile.
- **Nix flake.** The suite's `nix.yml` workflow is left out until a flake (`flake.nix`,
  `flake.lock`, `nix/package.nix`) is added the way a sibling that has one does.
- **Windows executable icon.** The installer takes the shortcut icon from the exe, but
  `apps/modelcraft/build.rs` does not embed `assets/app-icon/modelcraft.ico` yet (winresource).
- The icon (a contributor draft) is rendered to every size; the Linux recipe (nfpm, desktop file,
  metainfo, MIME) follows BoardCraft. macOS, Windows and Flatpak recipes and the launcher catalog
  entry need the maintainers' signing setup and assets.
