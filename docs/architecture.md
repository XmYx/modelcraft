# Architecture

ModelCraft is a Cargo workspace of UI-free core crates with the app on top.

| Layer | Crate | What it holds | Depends on |
|---|---|---|---|
| L0 | `modelcraft-mesh` | `PolyMesh`: validated half-edge mesh, per-corner UVs, patches, polygon-list (de)serialisation | glam |
| L1 | `modelcraft-ops` | Parametric primitives, polygon edits (extrude, inset, bevel, loop cut, merge, weld, delete, move, flip, triangulate), smooth-cage subdivision, deformers | mesh |
| L1 | `modelcraft-sculpt` | `SculptMesh` (flat arrays, CSR adjacency, BVH), brushes, falloff, stamps, symmetry, masks, dirty chunks, stroke undo and replay | mesh |
| L2 | `modelcraft-scene` | Objects, hierarchy, transforms, materials, history steps, the `Evaluator` (dependency graph with per-step caches), picking | mesh, ops |
| L3 | `modelcraft-formats` | `.modelcraft` project (craft-zip + blobs), crash journal, glTF/GLB/OBJ/STL/PLY through craft-mesh | scene, craft-mesh, craft-zip, craft-atomic |
| L4 | `modelcraft-engine` | `Document`, `Command`, undo (memory-bounded), journaling, sculpt session, export | all above |
| L4 | `modelcraft-render` | Camera, CPU vertex data, wgpu pipelines (MSAA offscreen target, mesh/line/overlay passes, readback) | mesh, scene, wgpu |
| L5 | `modelcraft` (app) | egui UI: viewport, gizmos, panels, workspaces, palette, i18n, kittest tests | engine, render, egui/eframe |
| L5 | `modelcraft-cli` | Batch commands, JSON-lines control channel, benchmarks | engine, render |

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

## Shared libraries and gaps (house rule 16)

Used from craft-libs at `0649a07`: `craft-mesh` (readers/writers), `craft-zip`, `craft-atomic`,
`craft-image-codecs` (screenshots), `craft-i18n`, `craft-fonts-build`.

Gaps found, to raise in craft-libs rather than copy:
- `craft-mesh` ignores glTF `extras` and writes GLB only (no `.gltf` JSON); it cannot carry quads.
- No shared GPU crate on main yet (RFC-0022 / craft-libs #663): the viewport uses wgpu directly
  behind `modelcraft-render`, with a CPU fallback, ready to move to `craft-gpu`.
- No shared mesh-processing crate on main (craft-libs #1233 `craft-mesh-ops` is open): ModelCraft's
  half-edge mesh, subdivision and validated polygon operations are candidates to contribute there,
  next to its tangents, UV unwrap and BVH. Its `OrbitCamera` and ours should converge.
- RFC-0032 (`craft-pbr`, `craft-raster3d`): ModelCraft can be the second consumer for headless
  renders, thumbnails and the no-GPU fallback. Our material is the glTF metallic-roughness core it
  names.

Not used yet but planned: `craft-ui` docking, `craft-keymap`, `craft-settings`, `craft-control` and
`craft-mcp-server`.
