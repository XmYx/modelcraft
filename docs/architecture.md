# Architecture

ModelCraft is a Cargo workspace of UI-free core crates with the app on top.

| Layer | Crate | What it holds | Depends on |
|---|---|---|---|
| L0 | `modelcraft-anim` | Keyed curves (stepped, linear, Bézier with automatic clamped tangents and weighted handles), extrapolation, baking, the scene clock and playback | glam |
| L0 | `modelcraft-mesh` | `PolyMesh`: validated half-edge mesh, per-corner UVs, patches, polygon-list (de)serialisation | glam |
| L1 | `modelcraft-ops` | Parametric primitives, polygon edits (extrude, inset, bevel, loop cut, merge, weld, delete, move, flip, triangulate), smooth-cage subdivision, deformers | mesh |
| L1 | `modelcraft-uv` | Seams, islands, automatic cuts into discs, LSCM unwrap with pins (sparse Cholesky, nested dissection; conjugate gradients as fallback), island layout, distortion measures | mesh |
| L1 | `modelcraft-uvpack` | UV island packing (MaxRects with a work bound and a linear shelf fallback, margin, rotation, even texel density) and planar, box, cylindrical, spherical and camera projections | mesh |
| L1 | `modelcraft-hair` | Guide curves grown from a surface (density and length maps, seeded), render hairs interpolated from the guides (barycentric or nearest-k blend, clump, frizz, width taper), hard caps on counts and memory | mesh, rayon |
| L1 | `modelcraft-sculpt` | `SculptMesh` (flat arrays, CSR adjacency, BVH), brushes, falloff, stamps, symmetry, masks, dirty chunks, stroke undo and replay | mesh |
| L2 | `modelcraft-scene` | Objects, hierarchy, transforms, materials, history steps, the `Evaluator` (dependency graph with per-step caches), picking | mesh, ops |
| L3 | `modelcraft-formats` | `.modelcraft` project (craft-zip + blobs), crash journal, glTF/GLB/OBJ/STL/PLY through craft-mesh, UV layout pictures (SVG, PNG) | scene, craft-mesh, craft-zip, craft-atomic |
| L4 | `modelcraft-engine` | `Document`, `Command`, undo (memory-bounded), journaling, sculpt session, export | all above |
| L1 | `craft-render-bvh` | Binned-SAH BVH (parallel build), flat GPU-ready nodes, closest-hit and any-hit triangle queries | glam, rayon |
| L2 | `craft-render-core` | The path tracer's CPU reference: scene description, cameras, sampling, the surface model, lights, the integrator, the progressive film | craft-render-bvh |
| L3 | `craft-render-gpu` | The same path tracer as a wgpu compute pipeline; capabilities from the created device | craft-render-core, wgpu |
| L4 | `modelcraft-render` | Camera, CPU vertex data, wgpu pipelines (MSAA offscreen target, mesh/line/overlay passes, readback); `trace`: the scene graph exported to the path tracer | mesh, scene, craft-render-*, wgpu |
| L5 | `modelcraft-script` | Sandboxed Rhai engine over the command registry: scene and mesh API, undo groups, permissions and limits (see [scripting.md](scripting.md)) | engine, rhai |
| L5 | `modelcraft` (app) | egui UI: viewport, gizmos, panels, workspaces, palette, i18n, kittest tests | engine, render, egui/eframe |
| L5 | `modelcraft-cli` | Batch commands, scripts, JSON-lines control channel, benchmarks | engine, render, script |

`cargo xtask layers` fails if any L0–L4 crate depends on egui, eframe, winit or rfd.

## Data model

- **Mesh**: built only through `PolyMesh::build(positions, polygons)`, which repairs (drops degenerate
  faces, flips inconsistent ones, detaches faces on edges used three times) and then validates every
  half-edge invariant. Operations read adjacency, write a polygon list and rebuild: topology is
  valid after every operation by construction.
- **Scene**: a flat list of objects in tree order. A mesh object = source (primitive parameters or a
  stored mesh) + steps (`StepOp`). A smooth cage subdivides its first mesh child; deformer children
  bend their parent. Revisions come from one process-wide counter.
- **Evaluation**: `Evaluator::steps_output` caches the output of every step; when an object changes it
  compares the new step list with the cached one and re-runs only from the first difference. Final
  meshes (with deformers and cages) are cached by a combined revision of the subtree. A step whose
  selection no longer fits is skipped and reported (`StepError`).
- **Animation**: `Scene::anim` holds tracks (object + attribute path + curve), the time settings
  and the current frame. Every number an object stores is an attribute with a dotted path
  (`modelcraft_scene::attr`): the transform has fast paths, everything else is reached through the
  part's saved form, so a new parameter is keyable without animation code. The scene always holds
  the pose of the current frame: `Scene::set_frame` writes the curve values into the objects and
  bumps the revision only of objects whose values changed, so the evaluator re-runs exactly what
  the frame touched. Going to a frame is not an undo step and is journaled lazily (before the next
  command). Auto-key compares the scene before and after a command and keys what changed.
  Tracks live in layers: the base, then layers that add to or replace the value below by a weight
  (`Animation::value_at`); key commands work in the active layer.
- **Rigging**: a joint is an object (`ObjectKind::Joint`), so the tree, the gizmo, keys and layers
  work on it unchanged. A skin (`ObjectKind::Skin`) is a child of a mesh, like a deformer: it
  holds the joints, their inverse bind matrices and the per-point weights, and runs after the
  deformers. The evaluator's cache key for a mesh with a skin also hashes its joints' world
  matrices (`rig::pose_key`), so moving a joint re-runs the skin and nothing else. The maths
  (weights, linear blend, dual quaternions, heat weights, aiming and mirroring joints) is in
  `modelcraft-anim`, free of the scene.
- **Deformer stack**: blend shapes, lattices and clusters are one object kind
  (`ObjectKind::PointDeformer`, `modelcraft_scene::point_deform`), children of the mesh like a
  skin. `rig::apply_stack` runs them and the skins in child order on the mesh's points, after the
  parametric deformers; `Scene::move_in_stack` reorders them. Weights and control points are
  attributes (`target.<i>.weight`, `point.<i>.x`), so they are keyed like everything else. The
  maths (morph with in-betweens, free-form deformation, clusters) is in `modelcraft-anim::deform`.
- **Constraints**: `Scene::constraints` holds constraints, IK, driven keys and formulas
  (`modelcraft_scene::constraint`). Each names what it reads and writes (object and channel);
  `Scene::solve_constraints` sorts them topologically (a reader after its writers), skips and
  reports the ones in a cycle, and writes the results into the objects, after every command and
  frame change. The transform an object has without its constraints is kept beside it, and every
  solve starts from those, so weights and the IK/FK blend never accumulate and switching a
  constraint off restores the object. The solvers (two-bone, FABRIK, CCD) and the formula
  language are in `modelcraft-anim`.
- **Sculpt**: entering sculpting adds (or reuses) a `Sculpt` step holding positions and mask for every
  vertex of its input. The session works on a `SculptMesh` and syncs back into the step on save,
  export, workspace change or any scene command.
- **Commands**: `Command` (serde, `"cmd"` tag) is the only way to change a document. Each run records
  undo (scene snapshots sharing meshes through `Arc`, or sparse stroke deltas) and writes the journal.

## Rendering

The viewport renders into an offscreen 4× MSAA colour + depth target with wgpu 30 (the version
egui-wgpu uses in the suite's fork) and shows the resolved texture as an egui image; each of the four
views has its own target. Evaluated meshes are uploaded once and cached by allocation; the sculpt mesh
is uploaded once per session and then patched per 4096-vertex chunk. Without a wgpu device the
viewport paints a bounded wireframe with the egui painter (also used by the UI tests).

## Path tracing

The `craft-render-*` crates know nothing about ModelCraft: they take triangles, materials, lights
and a camera, and could move to craft-libs under RFC-0032 as they are. `craft-render-core` is the
reference; every algorithm in it names the paper or specification it comes from, and no other
renderer's code was read. `craft-render-gpu` runs the same sample sequence and estimator in one
WGSL compute kernel over the same BVH bytes, so the two converge to the same image (tested).
`modelcraft-render::trace` is the only ModelCraft-specific part: it evaluates the visible surfaces,
bakes world transforms, converts the material and adds lighting (the scene graph has no light
objects yet). The Render View (`apps/modelcraft/src/render_view.rs`) and `modelcraft-cli render`
are thin callers; the render itself runs on a worker thread.

The kernel asks for rays through two functions, `trace_closest` and `trace_any`; today both are
BVH traversal in the shader, which is the default and the reference. A hardware ray-query backend
is planned behind the same two functions: off by default behind a cargo feature (`hw-ray-query`)
and a runtime setting, its one `unsafe` call (wgpu's experimental-features constructor) confined
to a module of its own and named as an exception in AGENTS.md when it lands, with a test that
compares it against the software traversal and an automatic fall-back. It is not implemented yet.

## Shared libraries and gaps (house rule 16)

Used from craft-libs at `0649a07`: `craft-mesh` (readers/writers), `craft-zip`, `craft-atomic`,
`craft-image-codecs` (screenshots), `craft-i18n`, `craft-fonts-build`, `craft-keymap` (catalog,
dispatch, the shared Keyboard Shortcuts dialog), `craft-settings` (preferences: navigation, recent
files, snapping, shortcut overrides), and `craft-svg` in xtask (icons).

Crash recovery is ModelCraft's own command journal (`modelcraft-formats::journal` plus
`Document::recover`), because it replays engine commands, sculpt strokes included. BoardCraft has an
app-local snapshot autosave; the two are candidates for one shared recovery crate in craft-libs
(see the sibling apps' shared plan), not to be copied between apps.

Gaps found, to raise in craft-libs rather than copy:
- `craft-mesh` ignores glTF `extras` and writes GLB only (no `.gltf` JSON); it cannot carry quads.
- `craft-mesh` has no skins, morph targets or animations: its glTF reader drops them and its
  writer cannot emit them. Until it can, `crates/craft-gltf-anim` carries them: a plain document
  model (nodes, one triangle list per mesh with up to four joints per vertex and position morph
  targets, skins, clips with translation / rotation / scale / weights channels, PBR factors), a
  GLB writer and a GLB / `.gltf` reader with limits. It depends on serde_json and thiserror only
  (no app type, no maths crate: plain arrays in glTF's conventions), follows craft-mesh's error
  and `Limits` shape, and is meant to move to craft-libs, as a module of craft-mesh or beside
  it. `modelcraft-formats::gltf_rig` maps it to and from the scene. Not opened upstream yet.
- No shared GPU crate on main yet (RFC-0022 / craft-libs #663): the viewport uses wgpu directly
  behind `modelcraft-render`, with a CPU fallback, ready to move to `craft-gpu`. `craft-render-gpu`
  follows the RFC's rules too (capabilities from the created device, no adapter is a defined
  answer, its tests run on a real backend including a software one) and has one place,
  `GpuContext`, to swap for `craft-gpu`.
- No shared BVH on main (craft-libs #1233 `craft-mesh-ops` proposes one behind a feature):
  `craft-render-bvh` is written to be offered there; the sculpt BVH in `modelcraft-sculpt` stays
  separate because it refits under moving vertices.
- No shared mesh-processing crate on main (craft-libs #1233 `craft-mesh-ops` is open): ModelCraft's
  half-edge mesh, subdivision and validated polygon operations are candidates to contribute there,
  next to its tangents, UV unwrap and BVH. Its `OrbitCamera` and ours should converge.
- RFC-0032 (`craft-pbr`, `craft-raster3d`): ModelCraft can be the second consumer for headless
  renders, thumbnails and the no-GPU fallback. Our material is the glTF metallic-roughness core it
  names.

Checked for animation (house rule 16): `craft-timeline` is editorial interchange (EDL, FCPXML,
OTIO); it carries keyframes as data (`model::Keyframe`) but has no curve evaluation, tangent rules
or key editing. `craft-anicraft-format` is the file envelope only: AniCraft's document, keys
included, stays in the app, and there is no shared EffectCraft crate. So the curve kernel is
`modelcraft-anim`, which depends only on glam and serde and can move to craft-libs as the shared
one (API gap to raise there: curve evaluation and editing behind `craft-timeline`'s key model).

Not used yet but planned: `craft-ui` docking, `craft-control` and `craft-mcp-server`.
