# ModelCraft vs Cinema 4D, Maya, ZBrush, V-Ray and Arnold: parity map

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** major (full-product map) · **Target:** Maxon Cinema 4D 2026 (ease of use, procedural generators, motion graphics), Autodesk Maya 2026 (depth: graph, rigging, animation, dynamics, pipeline), Maxon ZBrush 2026 (sculpting and painting), Chaos V-Ray 7 and Autodesk Arnold 7 (production rendering)

This is the only file that names the reference products. Every row was written from their public
manuals, feature lists and release notes in our own words; no code, assets or file internals were
read (see [AGENTS.md](../AGENTS.md) §2). Labels inside ModelCraft are our own
([feature-names.md](feature-names.md)).

The program goal is **90 % parity, measured honestly**. Until this revision the map had 125
"first-year essentials" rows and scored 59 % while the full products were estimated at about 12 %.
That map could not measure a 90 % goal, so this revision lists the full products, area by area,
and the score below is counted from it.

## Headline

`cargo xtask parity` counts every table below and writes this block; `cargo xtask ci` validates the
rows strictly but only warns when the block is stale (feature PRs flip rows only; a maintainer
refreshes the block with `cargo xtask parity --write` in a separate PR). Numbers here and in [ROADMAP.md](../ROADMAP.md) are copied from its output,
never estimated by hand.

<!-- parity:start -->
| | Value | Kind |
|---|---|---|
| In-scope feature rows | 330 | counted |
| Done / exceeds / partial / missing | 78 / 7 / 48 / 197 | counted |
| Out of scope (not in any denominator) | 4 | counted |
| **Row parity**, (D + X + ½·P) / rows | **33.0 %** | counted |
| **Weighted parity**, area weights × size-weighted rows | **24.5 %** | counted |
| Size-weighted parity over all rows, no area weights | 24.0 % | counted |

| Area | Weight | Rows | D | X | P | M | Out | Row parity | Effort parity |
|---|---|---|---|---|---|---|---|---|---|
| Viewport, navigation and display | 6 (5 %) | 25 | 8 | 0 | 7 | 10 | 0 | 46 % | 39 % |
| Scene graph, object manager and attributes | 5 (4 %) | 17 | 5 | 0 | 1 | 11 | 0 | 32 % | 27 % |
| Dependency graph, history and node editing | 5 (4 %) | 14 | 7 | 1 | 0 | 6 | 0 | 57 % | 33 % |
| Primitives and generators | 4 (3 %) | 12 | 2 | 0 | 0 | 10 | 0 | 17 % | 8 % |
| Splines, curves and NURBS | 4 (3 %) | 9 | 0 | 0 | 0 | 9 | 0 | 0 % | 0 % |
| Polygon modeling | 9 (7 %) | 36 | 11 | 1 | 3 | 21 | 0 | 38 % | 30 % |
| Subdivision surfaces | 2 (2 %) | 6 | 3 | 0 | 0 | 3 | 0 | 50 % | 36 % |
| Deformers and fields | 4 (3 %) | 13 | 2 | 0 | 0 | 11 | 0 | 15 % | 8 % |
| Procedural cloning and effectors | 5 (4 %) | 9 | 0 | 0 | 0 | 9 | 0 | 0 % | 0 % |
| UVs | 4 (3 %) | 10 | 0 | 0 | 1 | 9 | 0 | 5 % | 2 % |
| Sculpting: brushes, strokes and masks | 7 (5 %) | 22 | 8 | 1 | 4 | 9 | 0 | 50 % | 36 % |
| Sculpting: resolution and topology | 6 (5 %) | 9 | 2 | 0 | 2 | 5 | 0 | 33 % | 35 % |
| Painting, texturing and baking | 5 (4 %) | 9 | 0 | 0 | 1 | 8 | 0 | 6 % | 2 % |
| Materials and shading | 5 (4 %) | 9 | 2 | 0 | 3 | 4 | 0 | 39 % | 30 % |
| Render engine: core | 8 (6 %) | 17 | 5 | 0 | 5 | 7 | 2 | 44 % | 53 % |
| Render engine: lights, camera effects and output | 6 (5 %) | 19 | 2 | 0 | 6 | 11 | 0 | 26 % | 20 % |
| Rigging | 6 (5 %) | 15 | 1 | 0 | 3 | 11 | 0 | 17 % | 11 % |
| Animation | 8 (6 %) | 17 | 3 | 0 | 6 | 8 | 0 | 35 % | 36 % |
| Simulation | 6 (5 %) | 10 | 0 | 0 | 0 | 10 | 0 | 0 % | 0 % |
| Hair, fur and grooming | 4 (3 %) | 6 | 0 | 0 | 0 | 6 | 0 | 0 % | 0 % |
| Volumes | 2 (2 %) | 2 | 0 | 0 | 0 | 2 | 0 | 0 % | 0 % |
| Scripting and automation | 5 (4 %) | 10 | 1 | 1 | 3 | 5 | 1 | 35 % | 21 % |
| Pipeline formats and referencing | 6 (5 %) | 15 | 5 | 1 | 0 | 9 | 1 | 40 % | 19 % |
| UI and workspaces | 4 (3 %) | 13 | 7 | 1 | 3 | 2 | 0 | 73 % | 62 % |
| Stability and performance | 3 (2 %) | 6 | 4 | 1 | 0 | 1 | 0 | 83 % | 75 % |
<!-- parity:end -->

Measured outside the map (see [performance.md](performance.md)): viewport 296 fps orbiting and
283 fps sculpting at 2.0 M triangles (RTX 4090, 1600×1000, no vsync); a sculpt dab on 1.0 M quads
takes 0.2–0.4 ms in the engine and 1.2 ms in the app.

## How to read the map

**Status** per row, checked against the code on `main` (2026-11-17, commit `86e8e51`):

| Code | Meaning | Credit |
|---|---|---|
| D | Done: runs in ModelCraft today and a test covers it | 1 |
| X | Exceeds the references in a way we mean to keep | 1 |
| P | Partial: works with the limit given in the notes | ½ |
| M | Missing | 0 |
| O | Out of scope, with the reason in the notes; left out of every denominator | — |

**Refs** says which reference has the feature in a comparable form: **C** Cinema 4D (with its
bundled renderer), **M** Maya (with its bundled renderer), **Z** ZBrush, **R** the production
renderers V-Ray and Arnold.

**Size** is the effort to reach parity on that row, in Opus 5.5 agent hours, rough: **S** ≈ up to 4 h
(weight 1), **M** ≈ 4–12 h (2), **L** ≈ 12–40 h (4), **XL** ≈ 40 h or more (8). The calibration: the
vertical slice (seven crates, app, CLI, docs) took about 6 hours; bevel-sized tools 2–4 hours.

**Scores.** *Row parity* counts every in-scope row the same. *Effort parity* weights each row of an
area by its size, so a missing path tracer weighs eight times a missing camera bookmark. *Weighted
parity* averages the areas' effort parities with the area weights; an area's weight reflects how
much users of the references rely on it and how much of the products' depth sits in it. Weighted
parity is the number the 90 % goal is measured against; row parity is shown because it is the
simpler count.

**Out of scope.** A row is marked O only when matching it is impossible or meaningless for an open,
Rust-only, offline app: a vendor's cloud service, a vendor's proprietary native file format or
scripting dialect, or a closed SDK the house rules forbid. Everything a user would expect from a
full 3D suite stays in the denominator, however large.

## Readiness by audience

| Audience | Ready? | Why |
|---|---|---|
| Hobbyist blocking out a model for TextureCraft or ArtCraft | Preview | Primitives, history, smooth cage, glTF export work; no UV editing yet |
| Sculptor roughing forms on a dense mesh | Preview | Nine brushes, masks, symmetry, pressure, 1 M+ polys interactive; no detail levels, live detail or remeshing |
| Professional modeler | No | Missing knife, booleans, vertex snapping, UV tools, normals editing, splines |
| Motion designer | No | No cloners, effectors, fields or animation |
| Animator keying objects and parameters | Preview | Keys on any number, curves, timeline, auto-key, Key Sheet and Curve Editor; no layers, clips or motion paths |
| Rigging, character animation | No | Joints, automatic skin weights, both skinning methods, IK with FK blending, constraints and blend shapes work; no weight brush or control shapes yet |
| Simulation | No | Not started |
| Look development and final rendering | No | Viewport shading only; no offline renderer |

## Viewport, navigation and display

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Orbit, pan, zoom with navigation presets | C M Z | D | S | Three presets (Alt + buttons, middle button, right button); arrows and ± keys; wheel |
| Frame selection and frame all | C M Z | D | S | F |
| Perspective and orthographic views with presets | C M Z | D | S | Front, Right, Top; Alt+1..4 |
| Single and four-view layouts | C M | D | S | Space |
| Ground grid and axis triad | C M Z | D | S | |
| Shaded with wireframe on selection | C M Z | D | S | Auto-smoothed normals (40°) |
| Display modes: flat, smooth, wireframe, see-through, bounding box | C M Z | P | M | Shaded and wireframe only |
| Move, rotate, scale gizmos | C M Z | D | M | Axis and free handles; numeric alternative |
| Gizmo step snapping (grid, angle, scale) | C M | D | S | Shift+S; Ctrl inverts per drag |
| Vertex, edge, face and surface snapping | C M Z | M | M | |
| Gizmo orientation (world, local, normal, custom) and pivot editing | C M Z | M | M | World axes only |
| Isolate selection, hide and unhide | C M Z | P | S | Per-object visibility only |
| Camera bookmarks and saved views | C M | M | S | |
| Reference images and image planes | C M Z | M | M | |
| Viewport lighting: lights, image-based lighting, shadows | C M | P | L | Studio three-light rig with roughness; no image lighting, shadows or scene lights |
| Screen-space ambient occlusion, reflections, antialiasing options | C M | M | M | |
| Matcap library for sculpting | C M Z | P | S | One clay look |
| Textures shown in the viewport | C M Z | M | M | |
| Heads-up statistics (polygons, frame rate) | C M Z | P | S | Frame rate in the status bar; no polygon counts |
| Headless viewport renders for thumbnails and CI | M | P | S | GPU readback works in `modelcraft-cli bench` and tests; no render command |
| Large-scene drawing: culling, level of detail, proxies | C M | P | L | 2.0 M triangles at ~290 fps; no culling or proxies |
| Measure tool and scene units | C M | M | M | |
| Camera objects: create, look through, focal length, clipping | C M | M | M | One editor camera |
| Safe frames and resolution gate | C M | M | S | |
| Colour-managed viewport (view transform from a colour config) | C M R | M | M | |

## Scene graph, object manager and attributes

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Object tree with nesting | C M | D | S | Scene panel; tree items with level and selection in the accessibility tree |
| Rename, duplicate, delete, reparent | C M | D | S | Reparent through the row menu |
| Groups and nulls | C M | D | S | |
| Attributes panel for the active object | C M Z | D | M | Transform, shape, cage, deformer, material, history |
| Numeric entry for every transform | C M Z | D | S | Properties fields and Transform by Numbers (Ctrl+T) |
| Multi-object editing in the attributes panel | C M | D | M | Transform and material edits apply to every selected object as one undo step (`batch`); a move, turn or scale is applied as a change, so each keeps its placement |
| Drag-and-drop in the tree | C M | D | S | Drop a row on a row to put it under it, below the rows for the top level; Move Under in the row menu for the keyboard |
| Search and filter in the tree | C M | D | S | By name or type; matches keep their parents |
| Tags: per-object behaviours (display, render, protection, look-at) | C | M | M | |
| Display and render layers | C M | M | M | |
| Instances that share a mesh | C M | M | M | |
| Selection sets | C M | D | S | Saved in the scene, follow deletes, selected by name |
| Custom user attributes on objects | C M | M | M | |
| Scene overrides (takes, render setups) | C M | M | L | |
| Lock and template objects (visible, not selectable) | C M | D | S | Locked objects refuse edits in the engine; reference-only objects are drawn but never picked |
| Copy and paste attributes; attribute presets | C M | P | S | Copy and paste transform, material and visibility onto many objects; no presets |
| Object pivot separate from the origin | C M Z | M | S | |

## Dependency graph, history and node editing

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Parametric primitives, editable after creation | C M Z | D | S | Seven generators |
| Construction history: edit an early step, later steps re-run | M | D | M | Build History; the evaluator re-runs from the first changed step |
| Toggle and remove a history step | M | D | S | |
| Collapse history into a mesh | C M | D | S | |
| Cached evaluation per step | M | D | S | `history_is_cached_and_reevaluated_from_the_change` |
| Stale steps degrade gracefully | M | X | S | A step whose selection no longer fits is skipped and flagged, never fatal |
| Generators as objects (cage of a child) | C | D | M | Smooth cage |
| Deformers as children | C M | D | M | Bend, twist, taper, ripple, round |
| Node view of the dependency graph | M | M | L | |
| Connect any attribute to any other | M | M | M | |
| Utility nodes: math, conditions, remap, vector ops | C M | M | M | |
| Expressions and driven keys | C M | P | M | Formulas over the frame, time and any attribute of any object (a small safe language: arithmetic, comparisons, 24 functions; property-tested against hostile text) and driven keys through a curve, both ordered with the constraints; the driven curve has no editor yet, formulas give one number (no vectors or strings) |
| Procedural geometry node graph (scene nodes, graph-based effects) | C M | M | XL | |
| Parallel graph evaluation | C M | M | L | Single-threaded today |

## Primitives and generators

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Cube, plane, sphere, cylinder, cone, torus | C M Z | D | S | With UVs and segments |
| Rounded cube / quad sphere (cube-mapped) | C M Z | D | S | The sculpting base |
| Capsule, disc, tube, pyramid, platonic solids, helix | C M Z | M | S | |
| Extruded 3D text | C M | M | M | |
| Terrain / landscape generator | C | M | M | |
| Extrude, lathe, loft and sweep generators from splines | C M | M | L | |
| Array and symmetry generators (live) | C M | M | M | Mirror exists only as a destructive step |
| Boolean generator (live, nested) | C M Z | M | L | |
| Live polygon reduction generator | C | M | M | |
| Volume mesher: combine shapes as distance fields, then mesh | C | M | L | |
| Metaballs | C M | M | M | |
| Wireframe-to-geometry generator (atoms and bonds) | C | M | S | |

## Splines, curves and NURBS

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Spline primitives: circle, rectangle, arc, star, helix, text outline | C M | M | M | |
| Pen tool: Bézier and B-spline drawing and point editing | C M | M | M | |
| Spline operations: chamfer, offset, outline, 2D booleans | C M | M | M | |
| Objects placed or bent along a spline | C M | M | M | |
| NURBS curves: degree, knots, weighted control points | M | M | L | |
| NURBS surfaces: loft, revolve, extrude, planar, birail | M | M | XL | |
| Trimmed NURBS and surface editing (fillet, stitch, rebuild) | M | M | XL | |
| Curves on surfaces and curve projection | M | M | L | |
| NURBS to polygons tessellation | M | M | M | |

## Polygon modeling

Weight: 9

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Vertex, edge, face modes; click and box select | C M | D | S | 1–4, Ctrl+A, Shift adds |
| Loop, ring, grow and shrink selection | C M | D | S | Alt+L / double-click, Alt+R, Ctrl+= / Ctrl+- |
| Soft selection (falloff around the selection) | C M Z | M | M | |
| Extrude faces (region) | C M Z | D | S | |
| Extrude edges and vertices; extrude along a curve | C M | M | M | |
| Inset faces | C M | D | S | Per face |
| Bevel edges with segments and profile | C M Z | P | M | One flat segment; skips boundary edges |
| Bevel / chamfer vertices | C M | M | S | |
| Loop cut (edge ring) | C M | D | M | N cuts; stops at non-quads, keeps neighbours conforming |
| Slide edges and vertices | C M | M | S | |
| Connect vertices / split polygons | C M | M | S | |
| Knife, slice and plane cut | C M | M | M | |
| Merge vertices, weld by distance | C M | D | S | |
| Collapse edges and faces to a point | C M | M | S | |
| Delete vertices, edges, faces | C M Z | D | S | |
| Move, rotate, scale components | C M Z | D | S | Gizmo or numbers, recorded as a history step |
| Triangulate and flip normals | C M | D | S | |
| Quadrangulate (triangles to quads) | C M | M | S | |
| Spin edges, poke faces | C M | M | S | |
| Bridge two faces, fill a hole | C M Z | D | M | Both as history steps |
| Bridge edge loops, grid fill | C M Z | M | M | |
| Separate, combine, detach and duplicate faces | C M Z | M | S | |
| Shell / solidify (give thickness) | C M Z | M | S | |
| Smooth and relax vertices | C M Z | M | S | Sculpt smooth brush only |
| Mirror modeling with a welded seam | C M Z | D | S | Mirror X/Y/Z step, keeps UVs |
| Live symmetric component editing | C M Z | M | M | |
| Mesh booleans | C M Z | M | L | |
| Manual retopology: draw quads snapped to a surface | C M Z | M | L | |
| Edge creases and hard edges | C M Z | M | M | Auto-smoothing angle only |
| Custom normals: harden, average, weighted by face area | C M | M | M | |
| Polygon groups (patches) and grouping tools | C M Z | D | S | Group by crease angle or by shell, new patch from selection, select patch; history steps (`crates/ops/src/patches.rs`) |
| Mesh cleanup: non-manifold, lamina faces, n-gon finder | C M | D | S | `PolyMesh::build` repairs non-manifold and lamina faces on every result; Mesh ▸ Patches and Cleanup finds n-gons, triangles, open borders, faces with no area and poles, and Clean Up Mesh welds and removes no-area faces |
| Transfer attributes between meshes (positions, UVs, normals) | M | M | M | |
| Align, distribute and snap objects together | C M | M | S | |
| Polygon pen: draw polygons point by point | C M | M | M | |
| Topology validated after every operation | | X | S | Every result rebuilds through `PolyMesh::build`; property-tested |

## Subdivision surfaces

Weight: 2

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Live subdivision generator on a cage | C M | D | M | Smooth cage, 0–6 levels |
| Subdivide as a destructive history step | C M Z | D | S | |
| Boundary rules | C M Z | D | S | Border kept as a curve |
| Creases and semi-sharp edges | C M Z | M | M | |
| Separate viewport and render levels | C M | M | S | |
| GPU subdivision | M | M | L | |

## Deformers and fields

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Bend, twist, taper | C M Z | D | S | In the deformer's own frame |
| Ripple / wave, spherify | C M | D | S | Ripple, Round |
| Lattice / free-form deformation | C M | P | M | A lattice of up to 8 cells per axis around a mesh, as one Bézier volume (the 1986 paper) or cell by cell, with a deformer stack order shared with clusters, blend shapes and skins (reorder by command); control points are moved by number and are keyable; the cage is drawn but its points are not picked in the viewport; clusters (weighted points following an object, with a soft radius) live in the same stack |
| Wrap, shrink-wrap and surface deformers | C M | M | M | |
| Noise and texture displacement deformer | C M | M | S | |
| Smooth, relax and delta-smoothing deformers | C M | M | M | |
| Spline-driven deformation (curve warp, spline wrap) | C M | M | M | |
| Wire, wrinkle and sculpt-object deformers | M | M | M | |
| Collision deformer | C | M | M | |
| Explode / shatter deformer | C | M | S | |
| Deformer weights painted per vertex | C M | M | M | |
| Falloff fields: linear, spherical, radial, box, noise, shader | C | M | L | |
| Field layers: combine, remap, delay, freeze | C | M | M | |

## Procedural cloning and effectors

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Cloner: linear, radial, grid, honeycomb, on-object modes | C M | P | L | Clones object: linear, radial, grid, scatter, surface and vertex layouts, seeded; no honeycomb or per-clone selections |
| Effectors: plain, random, shader, step, delay, time, target, formula | C M | M | L | |
| Fields drive effectors | C | M | M | |
| Clone selections and weight maps | C | M | M | |
| Instancing at scale (millions of points, render instances) | C M | P | M | Clones share one GPU mesh and one evaluated mesh; capped at 10,000 per generator, one draw call each |
| Voronoi fracture | C M | M | L | |
| Animated text per letter / word / line | C | M | M | |
| Trace clone paths into splines | C | M | S | |
| Dynamics on clones | C M | M | M | |

## UVs

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| UVs on primitives, kept through edits | C M Z | P | S | Interpolated through ops; extrude walls get simple UVs |
| UV editor | C M | D | L | UV Editor window: select vertices, edges, faces and islands (click, box, keyboard), move, rotate, scale by dragging, keys or numbers, pin, weld, flip, undoable history steps |
| Seams and conformal unwrap | C M Z | D | L | `modelcraft-uv`: seams by edge, angle and path, automatic cuts into discs, conformal (LSCM) unwrap with pins, as history steps, the UV menu and the UV Editor; seams show red in the viewport |
| Angle-based unwrap (ABF) | C M Z | M | M | |
| Packing | C M | D | M | `modelcraft-uvpack`: MaxRects with margin, rotation and even texel density (a work bound and a shelf fallback above 500 islands); Pack UV Islands in the UV Editor and menu, history step, CLI |
| UDIM tiles | C M Z | M | M | |
| Projections: planar, box, cylindrical, spherical, camera | C M Z | P | M | Planar, box, cylindrical and spherical from the UV Editor and menu (selected faces in face mode); camera projection through the history step and CLI only |
| Relax, straighten, align and stitch tools | C M | P | M | Weld (stitch) the sides of seams, flip, pivot-based numeric align; no relax or straighten |
| Checker display, texel density, distortion heat map | C M Z | P | S | Checker background and angle, area and stretch heat maps in the UV Editor, with texel density and distortion numbers; no checker on the 3D viewport yet |
| Automatic UVs from polygon groups | Z | M | M | |
| Transfer UVs between meshes | M Z | M | S | |
| UV layout export as an image | C M Z | D | S | `modelcraft-formats`: SVG and PNG of a mesh's UV outlines from the UV Editor, the UV menu and `modelcraft-cli export-uv`. UVs round-trip exactly through GLB and OBJ (tested) |

## Sculpting: brushes, strokes and masks

Weight: 7

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Draw, clay, smooth, inflate, grab, pinch | C M Z | D | M | |
| Flatten, crease | C M Z | D | S | |
| Broader brush set: snake hook, layer, clay build-up, trim, scrape, fill, blob, nudge, rotate, elastic move | C M Z | P | L | Ten more, nineteen in all: Pull, Plateau, Trim, Fill, Polish, Buildup, Surface grab, Gouge, Slide, Bulge. No rotate, elastic move or scrape yet |
| Mask brush; clear, invert, fill; mask by patch | C M Z | D | S | |
| Masks by cavity, curvature, occlusion; blur, sharpen, grow mask | C Z | P | M | Mask hollows, mask ridges, blur, sharpen, grow, shrink; no occlusion mask |
| Lasso and rectangle masks; clip, trim and slice brushes | Z | M | M | |
| X, Y, Z symmetry | C M Z | D | S | |
| Radial and topological symmetry | C Z | M | M | |
| Pen pressure to strength and size | C M Z | P | S | Through egui touch force; not verified on every tablet driver |
| Falloff curves and procedural stamps | C M Z | D | S | Seven curves and a hardness control; noise, rings, square stamps |
| Image alphas and an alpha library | C M Z | P | M | Eight built-in pictures (drawn in code), with a turn angle; importing your own images is missing |
| Stroke types: lazy / stabilized, dots, spray, rectangle drag, curve strokes | C M Z | P | M | Spaced (with a spacing control), Dots, Spray, Trail (stabilized) and Drag out; no curve strokes |
| Brush presets and saved brushes | C M Z | D | S | Ten built-in presets; save and remove your own (kept in the preferences) |
| Polygon-group visibility: hide, show, isolate | Z | M | M | |
| Sculpt layers with blend strength | C Z | D | L | "Shape layers": named offsets with a strength of −1..1; record, merge, remove. No per-layer mask; stepping down a detail level needs them merged |
| Morph target and restore brush | C Z | M | M | |
| Gizmo deformers in sculpt mode (bend, twist, stretch with a handle) | Z | M | M | |
| Insert-mesh brushes and curve brushes | Z | M | L | |
| Live booleans between sculpted parts | Z | M | L | |
| 1 M+ polygons interactive | C M Z | D | M | 2.0 M triangles at ~280 fps while sculpting (measured) |
| 20 M+ polygons interactive | Z | M | XL | Not reached: measured at 12.6 M triangles in the engine only (3–5 ms per dab with ~10 k vertices under the brush); 20 M and the app window at that size are not measured, and a session takes 6–10 s to start there |
| Strokes are data: per-stroke undo, bit-exact replay after a crash | C M Z | X | S | Journaled |

## Sculpting: resolution and topology

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Multiresolution subdivision levels | C M Z | D | L | "Detail levels": step up and down, a displacement per level in a surface frame, detail follows coarse edits; the object shows the level being worked on |
| Reconstruct lower levels from a dense mesh | Z | M | M | |
| Dynamic topology | Z | P | L | "Live detail": strokes split the triangles under the brush down to a detail size, with exact undo and journal replay. Splits only (no collapsing or edge flips yet); base detail level only; each split re-uploads the whole mesh to the GPU |
| Voxel / even remesh | Z | D | L | "Even remesh": a history step that rebuilds a closed mesh as even quads; volume kept within 0.05 % (measured). Closed meshes only; loses UVs and patches |
| Automatic quad retopology with guides and density | C M Z | M | XL | |
| Decimation that keeps UVs and vertex colour | Z | M | L | |
| Project detail from one mesh to another | M Z | M | M | |
| Extract a shell from a mask | Z | M | S | |
| Close holes and make watertight | Z | P | S | Fill one hole at a time |

## Painting, texturing and baking

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Paint workspace | C M Z | P | S | Patch view and export only |
| Vertex colour painting | C M Z | M | M | |
| Texture and PBR channel painting | C M Z | M | L | Hand-off to TextureCraft (GLB export) instead |
| Projection painting from images and stencils | Z | M | M | |
| Paint materials per region (material capture painting) | Z | M | M | |
| Vertex colour to texture and back | Z | M | M | |
| Bake normal maps from high to low resolution | C M Z | M | L | |
| Bake displacement maps (scalar and vector) | M Z | M | M | |
| Bake occlusion, curvature and cavity maps | C M Z | M | M | |

## Materials and shading

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Per-object PBR material (base colour, metallic, roughness) | C M Z | D | S | glTF metallic-roughness core, exported to GLB |
| Link a MaterialCraft `.mcmat` | | D | M | Stored and saved; the path tracer reads its base colour, roughness, metallic, normal, height and opacity maps (the viewport still shows the flat colour, counted under viewport textures) |
| Full layered surface model (OpenPBR-class: coat, sheen, transmission, thin film) | C M R | P | L | The engine evaluates fuzz, coat, anisotropic metal with edge tint, dielectric with thin film, rough diffuse, rough transmission with absorption, emission and opacity on the CPU and the GPU (the same model in both, held together by tests), energy-checked by a furnace test; Properties exposes coat, sheen, transmission, index of refraction and emission; no nested dielectrics or dispersion |
| Texture inputs: images, procedural noise, ramps, triplanar | C M R | P | L | Image maps for base colour, roughness, metalness, normal, height and opacity, bilinear, through a linked `.mcmat`; no mip filtering, procedural textures, ramps or triplanar, and no per-map controls in the app |
| Bump, normal and displacement inputs | C M R | P | M | Tangent-space normal maps and height bump in the path tracer (a test checks the lit result against the tilted normal); no displacement |
| Node material editor | C M R | M | XL | Planned with MaterialCraft |
| Per-face / per-patch material assignment | C M Z | M | M | |
| Material library and preset browser | C M R | M | M | |
| MaterialX import and export | M R | M | M | |

## Render engine: core

Weight: 8

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Real-time viewport PBR | C M Z | P | M | Rasterized studio lighting; no shadows or image lighting |
| Path tracing on the CPU | C M R | D | XL | `craft-render-core`: unidirectional, all cores, deterministic per seed; checked against analytic lights and a furnace test. Which surface, light and output features it carries is counted in their own rows |
| Path tracing on the GPU | C R | D | XL | `craft-render-gpu`: everything the CPU path renders, as one wgpu compute pipeline (Vulkan, Metal, DX12): the full layered surface model, textures, normal and bump maps, cut-outs, absorbing glass, every light type, panorama lighting with importance sampling, the light tree and the feature images; 30–50× the 24-thread CPU on an RTX 4090 ([performance.md](performance.md)); software BVH traversal, no hardware ray queries; a scene too large for the device's buffers falls back to the CPU |
| One scene, matching CPU and GPU results | R | D | L | Same samples, same estimator: tests hold the two images to a relative MSE under 0.002 on the Cornell box, the layered-material showcase, a panorama-lit studio, a many-light street, an interior and a scene of cut-outs and maps (measured: 0.0001 or less), and the feature images to matching means |
| Multiple importance sampling and next-event estimation | R | D | L | Power heuristic over light and BSDF sampling; a test checks that light-only, BSDF-only and combined estimators agree |
| Ray-tracing acceleration structure for the whole scene | R | D | L | `craft-render-bvh`: binned-SAH BVH built in parallel (2.0 M triangles in 0.56 s), one flat layout for CPU and GPU; no instancing or motion yet (their own rows) |
| Adaptive sampling with a noise threshold | R | M | M | |
| Progressive mode | R | D | M | Passes accumulate on one film; any number of passes equals one render of the same sample count |
| Bucket mode | R | M | M | |
| Interactive preview render with live edits | C M R | P | L | Render View (F9) restarts on scene, material and camera changes; not during a sculpt stroke, no region, no light or exposure controls |
| Denoiser | C R | M | L | |
| Path controls: bounces per type, clamping, sample limits | R | P | S | Total bounces, indirect clamp and sample count; not per lobe type |
| Instanced geometry at render time | C M R | M | L | |
| Render region | C M R | M | S | |
| Batch and command-line renders | C M Z R | P | M | `modelcraft-cli render` writes one PNG per call (CPU or GPU); no frame ranges or EXR yet |
| Distributed rendering over the network | C M R | M | L | |
| Render queue | C M | M | M | |
| Vendor cloud rendering services | C R | O | L | A paid hosted service, not a feature of the app; distributed rendering above covers the user need |
| Vendor-SDK denoisers and GPU back ends shipped as closed or C++ libraries | R | O | L | House rules: Rust only, allowlisted licences; the in-house denoiser row counts instead |

## Render engine: lights, camera effects and output

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Point, spot and directional lights | C M R | P | M | Point, spot (cone with soft edge), directional and sun-disk lights in the engine on CPU and GPU, tested against their formulas; no light objects in the scene graph yet, so the app cannot place them |
| Area lights: rectangle, disk, sphere, mesh | C M R | P | M | Any object with emission is a light of its own shape (plane, disk, sphere or any mesh), sampled through a light tree and checked against a rectangle's closed-form irradiance; no dedicated light objects with units, spread or camera visibility |
| Dome / image-based lighting with importance sampling | C M R | D | M | Radiance `.hdr` panoramas, sampled in proportion to brightness and solid angle with MIS (a test compares its variance with BSDF sampling), rotation; Render View (Panorama image) and `modelcraft-cli render --env`; on the CPU and the GPU; no EXR panoramas yet |
| Physical sun and sky | C M R | D | M | Daylight: single-scattering Rayleigh and Mie atmosphere baked per render plus a sun disk attenuated by the same air; sun height and direction in the Render View and `--sun`; no multiple scattering, ozone or night sky |
| Light linking (include / exclude) | C M R | M | M | |
| Light groups and per-light outputs | R | M | M | |
| Subsurface scattering (random walk) | C M R | M | L | |
| Hair and fibre shading | C M R | M | L | |
| Volumes: homogeneous, heterogeneous, sparse grids | C M R | M | L | |
| Render-time displacement with adaptive subdivision | C M R | M | L | |
| Motion blur: transform and deformation | C M R | M | L | |
| Depth of field and bokeh | C M R | P | M | Thin-lens camera in the engine (tested); no aperture control in the app or CLI yet |
| Physical camera: exposure, white balance, lens distortion | C M R | M | S | |
| Caustics and path guiding | R | M | L | |
| Render passes (AOVs) and ID mattes | C M R | P | M | First-hit albedo, world normal, depth, screen motion and per-pixel variance accumulate with the image on the CPU and the GPU path and are written into the `.exr`; no ID mattes, no per-light or per-lobe passes |
| OpenEXR output: multi-layer, half and float | C M R | P | M | `modelcraft-cli render -o file.exr`: one part of 32-bit float channels, linear radiance plus the feature layers by name, through craft-exr; no half floats, multi-part files or metadata, and the Render View still saves PNG |
| Colour management: colour config files and an ACES pipeline | C M R | M | M | |
| Toon and contour rendering | C M R | M | M | |
| Render statistics and time limits | R | P | S | Every CLI render prints its time, samples per second and triangle count as JSON, and `render-bench` reports error against time with a converged reference; no time limit or per-ray statistics |

## Rigging

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Joints: create, mirror, orient | C M | P | M | Joints are objects in the tree (keyable, parented), drawn as bones; add (chains by one key), aim at the child with children kept in place, mirror across a plane with renaming; no click-to-place tool, no separate orientation channel, joints are not picked in the viewport |
| Skinning with linear blend and weight painting | C M | P | L | A skin under a mesh binds it to joints and evaluates through the graph (only the skin re-runs when a joint moves); weights are painted by a command dab with falloff, not yet by a viewport brush; weights are saved as JSON |
| Dual-quaternion skinning | C M | D | S | Per skin switch (keyable); a half twist keeps the radius where linear blending collapses (tests) |
| Weight tools: normalize, mirror, copy, prune, automatic binding | C M | P | M | Automatic binding by heat equilibrium over the surface (from the 2007 paper, without its visibility test) or by distance; normalize, prune, limit, mirror with joint renaming, set by number; no copying between meshes |
| IK solvers: two-bone, multi-chain, spline IK | C M | P | L | Closed-form two-bone with a pole, FABRIK and CCD for longer chains (a pole turns the solved chain), solved in dependency order after the goal's own constraints; bone lengths kept (property tests); no spline IK, no joint limits, no stretch |
| FK / IK switching and blending | C M | D | M | An IK constraint's weight blends from the joints' own rotations to the solved ones and is keyable (`constraint.<id>.weight`); Match FK to IK and Match IK to FK switch without a jump (tests) |
| Constraints: parent, point, orient, aim, scale, pole vector | C M | D | M | Position, rotation, scale, as-child and look-at constraints with several weighted sources, kept offsets and a keyable weight; the pole is part of the IK constraint; evaluated in dependency order with cycles reported and skipped (tests); no per-axis skipping |
| Blend shapes with in-betweens | C M Z | D | M | A blend-shape deformer under a mesh holds named targets made from another mesh with the same points (sparse offsets), each with in-between shapes at chosen weights and a keyable weight; runs first in the deformer stack, before skins (tests); targets are not edited in place and there are no per-target weight maps |
| Corrective shapes driven by pose | C M | M | M | |
| Control shapes and character sets | C M | M | M | |
| Rig templates: biped, quadruped | C M | M | L | |
| Retargeting between skeletons | C M | M | L | |
| Pose library | C M | M | S | |
| Facial rigging tools | C M | M | L | |
| Muscle and skin-sliding systems | C M | M | XL | |

## Animation

Weight: 8

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Keyframe any attribute; auto key | C M | D | M | Every number an object stores has a path (transform, visibility, material, primitive, history-step and deformer parameters); stepped, linear and Bézier keys; auto-key keys changed transform channels and already-animated attributes in the same undo step |
| Timeline: scrub, play, range | C M | D | M | Scene frame rate and range, whole-frame playback (once, loop, back and forth), next/previous key; the ruler is a keyboard slider |
| Graph editor: curves, tangents, Bézier handles | C M | P | L | Curve Editor: automatic clamped, flat, free and split tangents, weighted handles, five extrapolation modes, numeric fields for every drag; no box selection, no manual value zoom or normalised view, no copy/paste |
| Dope sheet | C M | P | M | Key Sheet: select, move, scale and delete keys by pointer, keyboard or number; no box selection, copy/paste or summary rows |
| Non-linear clips: blend, loop, offset | C M | M | L | |
| Animation layers | C M | P | M | Adding and replacing layers with weight and mute; keys go to the active layer and keying a pose solves for the layer's value; flatten to the base; no solo or lock, weights are not keyable, layers cannot be reordered |
| Motion paths and ghosting | C M | P | M | Paths of selected moving objects in the viewport with a tick per frame and a mark per key, following animated parents; no ghosts, paths are not editable |
| Viewport preview capture (playblast) | C M | M | S | |
| Bake constraints and simulations to keys | C M | P | S | Bake Constraints to Keys samples the solved transforms over a range into curves (angles kept continuous) and switches the constraints off, in one undo step; there are no simulations to bake |
| Curve cleanup: Euler filter, simplify | C M | D | S | Smooth Rotation Keys removes whole-turn and flip jumps (property test: every rotation kept); simplify within a tolerance; bake to linear keys |
| Follow a spline | C M | M | S | |
| Audio in the timeline | C M | M | S | |
| Motion-capture import (BVH) and mapping | C M | M | M | |
| Cached playback | M | M | M | |
| Camera sequencer / shot list | C M | M | M | |
| Point cache of deformed meshes | C M | P | M | `modelcraft-cli bake` writes an object's vertices per frame to a documented `.mcpc` file (local or world space); `sim attach_cache` plays such a file on a mesh through the timeline (see Simulation); no Alembic |
| Turntable and simple timeline for sculpts | Z | P | S | Turntable command (one linear turn that repeats) and the timeline; no movie recording |

## Simulation

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Rigid bodies with collisions and constraints | C M | M | L | |
| Soft bodies | C M | M | L | |
| Cloth | C M | M | L | |
| Particles: emitters, birth and death, per-particle attributes | C M | M | L | |
| Forces: gravity, wind, turbulence, vortex, attract | C M | M | M | |
| Smoke and fire | C M | M | XL | |
| Liquids | C M | M | XL | |
| Simulation cache and playback | C M | P | M | The framework is in: a fixed-step solver loop with substeps from a seed (repeatable bit for bit), a cache bounded in frames, points and bytes that bakes per frame, cancels between frames, saves in the project as a `.mcpc` blob and plays back by scene frame (linear between frames); a `.mcpc` file from any tool drives a mesh through `attach_cache`. No solver yet and no panel: nothing in the app bakes one |
| Ropes and hair dynamics | C M | M | L | |
| Cloth-like sculpt brushes | Z | M | M | |

## Hair, fur and grooming

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Guide curves on a surface; comb, cut, length brushes | C M Z | M | L | `modelcraft-hair` grows seeded guides from a surface with density and length maps; no brushes or UI yet |
| Render hair interpolated from guides: density, clumping, frizz | C M | M | L | `modelcraft-hair` blends up to millions of strands from guides (barycentric or nearest-k), with clump, frizz, width taper and hard memory caps; `modelcraft-cli hair-bench` measures it; not in the app yet |
| Sculpted fibre meshes | Z | M | M | |
| Hair to cards / geometry | C M | M | M | |
| Import and export of curve grooms | C M | M | S | |
| Instanced fur and feathers | C M | M | M | |

## Volumes

Weight: 2

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Read and write sparse volume grids (OpenVDB) | C M R | M | L | |
| Volume modeling: mesh to volume, volume booleans, smoothing, back to mesh | C | M | L | |

## Scripting and automation

Weight: 5

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Embedded scripting language | C M Z | D | L | Rhai in `modelcraft-script`, sandboxed with limits and file access only by grant; runs in the app's Scripts window and from `modelcraft-cli run-script` ([scripting.md](scripting.md)) |
| Object-model API over every command | C M Z | D | M | `cmd::<name>` for every registry command, scene queries, dependency graph, history-node evaluation, mesh handles, undo groups |
| Command line: batch run, convert, export, validate | C M | D | S | `modelcraft-cli` run/exec/convert/export/info/validate/recover/bench |
| Headless scene API (batch interpreter) | C M | D | S | `modelcraft-cli run-script`: queries, control flow and commands over a project |
| Machine control channel and MCP server | M | D | M | stdio, a token-authenticated loopback socket in the window and in `modelcraft-cli serve` (craft-control), `modelcraft-cli call`, and an MCP server (`modelcraft-cli mcp`, `--app` drives the window) |
| Every UI action is a serializable command | M | X | S | The same `Command`s drive UI, CLI, journal and tests |
| Script editor with a console | C M | D | M | The Scripts window: highlighting, run script or selection, output with errors linked to their line, files on disk, startup script ([scripting.md](scripting.md)) |
| User tools, shelves and recorded macros | C M Z | P | M | The Scripts menu and the Quick Scripts strip run the scripts in a folder, and a script can register its own menu commands (`register_tool`); recorded macros are not built |
| Script editor with a console | C M | M | M | |
| User tools, shelves and recorded macros | C M Z | P | M | Any command can sit on the Quick Bar; no recorded macros or user-written tools yet |
| Event callbacks (scene changed, selection changed) | C M | D | M | `on_selection_changed`, `on_scene_changed`, `on_scene_loaded` and `on_frame_changed` in scripts; the app reports them once per frame, never during a drag or stroke ([scripting.md](scripting.md)) |
| Plugin SDK for native extensions | C M Z | M | L | |
| Running scripts written for the references' own APIs (MEL, ZScript, vendor Python modules) | C M Z | O | XL | Proprietary scripting dialects and object models; our own scripting rows count instead |

## Pipeline formats and referencing

Weight: 6

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| glTF 2.0 / GLB import | C M Z | D | S | Through craft-mesh; welded, parts become patches. Files with skins, morph targets or animations come in as joints, skins, blend shapes and curves (first clip) through `craft-gltf-anim` |
| GLB export | C M Z | D | S | Hierarchy, TRS, materials, UVs, normals; triangulated (glTF has no quads). A rigged or animated scene is written with its skins, morph targets and one clip baked from the solved scene; round-trip tested |
| OBJ import and export | C M Z | D | S | Quads kept |
| STL export, STL and PLY import | C M Z | D | S | |
| FBX import and export | C M Z | D | M | Import binary and ASCII 6.x/7.x through craft-mesh; export FBX 7.4 ASCII (meshes with quads, UVs, normals, material colour, hierarchy, TRS); round-trip tested |
| USD: layers, references, variants, import and export | C M | M | XL | Later, with craft-usd |
| Alembic caches: import and export | C M | M | L | |
| File references and proxies | C M | M | L | |
| Asset browser / content library | C M | M | M | |
| Live link with other apps (send and receive meshes) | C M Z | M | M | Planned with TextureCraft |
| EXR and HDR images as textures and lighting | C M R | M | M | |
| 3MF export for printing | C Z | M | S | |
| Up-axis and unit conversion on import and export | C M Z | D | S | FBX axes and unit read from the file; Y/Z up and metres/cm/mm/inches for any format (File ▸ Exchange Settings, `--up`, `--scale`) |
| Native project, atomic saves | C M Z | D | S | `.modelcraft` ZIP |
| Crash journal with exact replay | | X | S | Every command, sculpt strokes included |
| Reading and writing the references' native files (.c4d, .ma/.mb, .ztl/.zpr) | C M Z | O | XL | Undocumented proprietary formats; the open exchange formats above cover interchange |

## UI and workspaces

Weight: 4

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Dense dark theme, light theme from the system | C M Z | D | S | Text ≥ 4.5:1 (tested) |
| Workspace layouts (model, sculpt, paint) | C M Z | D | S | Ctrl+1..3 |
| Dockable panels | C M | P | M | Dock left/right, float, hide per workspace; no tabs, splits or saved layouts |
| Tool options bar | C M Z | D | S | |
| Command palette, shortcut for every action | C M | D | S | Ctrl+K |
| Customisable shortcuts | C M Z | D | S | Suite Keyboard Shortcuts dialog (craft-keymap): conflicts, saved sets, per-workspace contexts |
| Radial and marking menus | C M Z | D | M | Radial Menu (Shift+Q): eight context actions (add objects, component tools, brushes); numbers 1–8, Tab and Enter, or the pointer |
| User palettes and shelves | C M Z | D | M | Quick Bar: any command as a button, edited in Preferences or from its context menu, saved in the preferences |
| Undo history panel (jump to any step) | M Z | D | S | |
| Open Recent, incremental save | C M Z | D | S | |
| Preferences | C M Z | D | S | Preferences window (Ctrl+,): language, navigation, length units, display (grid, axis triad, wireframe, patches), snapping steps, brush size, Quick Bar, shortcuts |
| Translated interface | C M Z | P | M | Strings go through craft-i18n; no shipped translations |
| Accessibility: keyboard paths, screen-reader tree | | X | S | Tree items, viewport object nodes, numeric gizmo path, kittest tests |

## Stability and performance

Weight: 3

| Feature | Refs | Status | Size | Notes |
|---|---|---|---|---|
| Autosave | C M Z | D | S | Journal plus snapshots for untitled scenes |
| Prompt before discarding unsaved changes | C M Z | D | S | New, Open, Open Recent and Quit ask Save / Don't Save / Cancel |
| Never-panic policy, hostile-input tests | | X | S | Lints deny panics; proptests for mesh ops and formats |
| Bounded undo memory | | D | S | 1 GiB budget, oldest dropped |
| Undo history kept across sessions | M Z | D | M | Saves store a base scene and the history's commands; open replays and checks them (`undo_history_survives_save_and_open`) |
| Multithreaded dabs, evaluation and BVH build | C M Z | M | M | Single-threaded today |

## What works today (evidence)

- Mesh: `crates/mesh/src/tests.rs` (`arbitrary_soups_build_valid_meshes` proptest, repair, round trips).
- Ops: `crates/ops/src/edit.rs` tests (`bevel_all_cube_edges_stays_closed`, `loop_cut_around_a_cube`, `random_edit_sequences_stay_valid` proptest), `subdivide.rs`, `deform.rs`, `primitives.rs`.
- History: `crates/scene/src/tests.rs` (`history_is_cached_and_reevaluated_from_the_change`, `smooth_cage_and_deformers_follow_their_children`).
- Sculpt: `crates/sculpt/src/tests.rs` (every brush, mask protection, symmetry, deterministic replay, dirty chunks).
- Files: `crates/formats` (project round trip with blobs, corrupted-project proptest, GLB/OBJ/STL round trips, torn journal).
- Engine: `crates/engine/src/tests.rs` (`journal_recovers_unsaved_work_after_a_crash`, `undo_memory_is_bounded`).
- GPU: `crates/render/src/gpu.rs` `renders_a_quad_when_a_gpu_is_present` (renders and reads back).
- Render engine: `crates/craft-render-bvh` (`matches_brute_force` proptest), `crates/craft-render-core/src/tests.rs` (`white_furnace_is_invisible`, `analytic_lights_match_their_formulas`, `area_light_matches_the_form_factor`, `the_three_estimators_agree_on_the_cornell_box`, `rendering_is_deterministic_progressive_and_cancellable`), `bsdf.rs` (`never_creates_energy` proptest), `crates/craft-render-gpu` (`gpu_matches_the_cpu_reference_on_the_cornell_box`, `gpu_matches_the_cpu_on_layered_materials_and_textures`, `gpu_matches_the_cpu_under_a_panorama`, `gpu_matches_the_cpu_with_the_light_tree_and_by_power`, `gpu_matches_the_cpu_on_cutouts_and_normal_maps`, `gpu_fills_the_feature_images_like_the_cpu`), `crates/render/src/trace.rs` (`material_layers_uvs_and_linked_maps_reach_the_render`), `apps/modelcraft-cli/src/render_cmd.rs`, `apps/modelcraft/src/material_ui.rs`, the output tests `feature_images_describe_the_first_surface`, `z_curve_pixel_order_moves_error_to_high_frequencies`, `mis_compensation_lowers_the_noise_of_image_lighting`, `apps/modelcraft-cli/src/bench_cmd.rs`, the light tests `spot_sun_and_image_lighting_match_their_formulas`, `the_light_tree_is_unbiased_and_beats_power_sampling_with_many_lights`, `sky.rs` (`the_sky_is_blue_at_noon_and_red_at_sunset`), the material tests `layers_conserve_energy_and_glass_passes_light`, `fresnel_and_thin_film_follow_optics`, `glass_is_invisible_in_a_furnace_and_absorbs_by_distance`, `cutouts_textures_and_normal_maps_change_the_picture`, `apps/modelcraft/src/render_view.rs` (two keyboard-driven kittest flows).
- UI: `apps/modelcraft/src/ui_tests.rs`, keyboard-driven kittest flows with the accessibility check on (save prompt, undo history, loop/grow selection, mirror, snapping, shortcuts dialog among them).
- Selection and mesh tools: `crates/ops/src/select.rs` (`loops_rings_and_borders`, `grow_and_shrink_faces`), `edit.rs` (`bridge_fill_and_mirror`, mirror/bridge/fill in the random-sequence proptest).
- Animation: `crates/anim/src/curve.rs` (`passes_through_every_key`, `automatic_tangents_never_overshoot`, `edits_keep_the_invariants` proptests), `crates/scene/src/anim.rs` (`every_number_of_an_object_is_an_attribute`, `frames_pose_the_scene_and_only_changed_objects_are_re_evaluated`, `hostile_animation_is_repaired`), `crates/engine/src/anim_tests.rs` (`auto_key_turns_edits_into_keys`, `animation_survives_save_and_crash_recovery`), `apps/modelcraft/src/anim_ui_tests.rs` (keying, scrubbing, Key Sheet and Curve Editor by keyboard and number).
- Simulation framework: `crates/sim/src/tests.rs` (`the_same_seed_gives_the_same_cache_and_another_seed_does_not`, `memory_and_step_counts_are_bounded`, `cancelling_at_any_frame_reports_how_far_it_got` and `random_runs_stay_within_budget_and_sample_without_panicking` proptests), `crates/scene/src/sim.rs` (`the_timeline_plays_the_cache_and_only_then`), `crates/engine/src/sim_tests.rs` (`a_baked_point_cache_plays_on_the_timeline_and_survives_undo_and_save`), `crates/formats/src/project.rs` (`a_simulation_cache_is_saved_as_a_point_cache_blob_and_comes_back`).
- Constraints and IK: `crates/anim/src/ik.rs` (`two_bone_keeps_lengths_and_reaches_when_it_can`, `chains_stay_finite_and_keep_their_bones` proptests), `crates/anim/src/expr.rs` (`any_text_parses_or_fails_cleanly` proptest), `crates/scene/src/constraint_tests.rs` (`constraints_run_in_dependency_order_whatever_their_list_order`, `cycles_are_reported_and_skipped_and_the_rest_still_runs`, `any_constraints_solve_finitely_and_repeatably` proptest), `crates/engine/src/constraint_tests.rs`, `apps/modelcraft/src/constraint_ui_tests.rs`.
- Deformer stack: `crates/anim/src/deform.rs` (`ffd_is_a_convex_combination`, `frame_shares_are_continuous_and_exact_at_the_frames` proptests), `crates/engine/src/deformer_tests.rs` (`blend_shapes_with_in_betweens_and_keyed_weights`, `lattices_clusters_and_the_order_of_the_stack`, `shapes_run_before_the_skin_and_everything_survives_a_save`), `apps/modelcraft/src/deform_ui_tests.rs`.
- Rigged glTF: `crates/craft-gltf-anim/src/tests.rs` (`a_rigged_animated_document_round_trips_exactly`, `hostile_files_are_refused_or_trimmed_never_trusted`, `corrupted_files_never_panic` proptest), `crates/engine/src/gltf_tests.rs` (`skins_shapes_and_clips_round_trip_through_glb`, `constraints_are_baked_into_the_clip_and_limits_are_said`).
- Rigging: `crates/anim/src/skin.rs` (`dual_quaternions_keep_the_radius_where_linear_blending_collapses`, `heat_weights_follow_the_surface_and_blend_at_the_joint`, `skinning_is_finite_and_bounded_by_the_joint_motions` proptest), `crates/anim/src/skeleton.rs`, `crates/scene/src/rig.rs` (`posing_a_joint_deforms_the_skinned_mesh_and_only_then`, `hostile_rigs_are_repaired`), `crates/engine/src/rig_tests.rs`, `apps/modelcraft/src/rig_ui_tests.rs`.
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

## What 90 % means in hours

Summing the sizes of the rows that are not done gives about 3,425 agent hours to 100 % and about
2,760 hours to 90 % weighted parity along the ranked plan in [gaps.md](gaps.md). Reaching 90 % weighted parity needs most rows of every heavy area, including
an offline renderer, rigging, animation and simulation; no subset of modeling and sculpting alone
can get there.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-11-18 | minor | Deformation: blend shapes with in-betweens done; lattice partial |
| 2026-11-18 | minor | IK and constraints: constraints and FK/IK blending done; IK solvers, baking constraints, expressions and driven keys partial |
| 2026-11-17 | minor | Rigging: dual-quaternion skinning done; joints, linear-blend skinning and weight tools partial |
| 2026-11-17 | minor | Animation tools: rotation cleanup done; animation layers, motion paths, point cache and turntable partial |
| 2026-11-17 | minor | Animation core: keys on any attribute, timeline, auto-key done; Curve Editor, Key Sheet and curve cleanup partial |
| 2026-11-17 | major | Full-product map: 25 areas with weights, per-row sizes, out-of-scope rows; scored by `cargo xtask parity` |
| 2026-11-17 | minor | Polish pass: snapping, loop/ring/grow, bridge, fill, mirror, rebindable shortcuts, undo history, recent files, incremental save, save prompt; 6 rows added |
| 2026-11-17 | major | First assessment with the vertical slice |
