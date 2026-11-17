# ModelCraft roadmap

> **Last reviewed:** 2026-11-17 · **Last updated:** 2026-11-17 · **Change:** minor (polish pass)

## Current focus

The polish pass is in (driven by [user-research.md](user-research.md)): data-safety prompts, recent
files, incremental saves, rebindable shortcuts, an undo history panel, loop/ring selection, bridge,
fill, mirror and snapping. Before that, the vertical slice: half-edge mesh, parametric primitives, history steps, smooth cage,
deformers, nine sculpt brushes on 1 M+ polygons, journaled saves, glTF/OBJ/STL, a keyboard- and
screen-reader-ready UI. Next is making it useful for a full day of work: UVs, snapping, the missing
polygon tools and sculpt detail.

## Milestones

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0 Vertical slice** (done) | Everything in [target-app-parity.md](target-app-parity.md) marked D | `cargo xtask ci` green, screenshots, measured performance |
| **M0.5 Polish pass** (done) | Save prompt, Open Recent, Save Incremental, rebindable shortcuts, Undo History panel, loop/ring/grow selection, bridge, fill, mirror, gizmo snapping | [user-research.md](user-research.md) items covered; `cargo xtask ci` green |
| **M1 Modeling essentials** | Retopology tools, knife, edge-loop bridge, grid fill, bevel segments + creases, vertex/edge/surface snapping, units and measure, reference images, booleans, tree drag-and-drop, multi-object Properties | Model a stylised character body and prop set from primitives without leaving the app |
| **M2 UVs and hand-off** | Seams, unwrap, pack (shared with TextureCraft through craft-mesh-ops), live link to TextureCraft and MaterialCraft over craft-control | Model → unwrap → paint in TextureCraft → see it in ModelCraft |
| **M3 Sculpt detail (beta)** | Detail levels (multiresolution), live detail (dynamic topology), even remesh, image stamps, layer blending, threaded dabs | Sculpt a 10 M-polygon bust at interactive rates |
| **M4 Paint** | Vertex colour and PBR texture painting on the GPU, sharing TextureCraft's paint core | Paint albedo/roughness/normal on a sculpt |
| **M5 Rig and animate** | Joints, skinning, weights painting, keyframes, glTF skins/animations out | Pose and animate a character for ArtCraft |
| **M6 Render** | Path-traced preview through a shared craft-raster3d (RFC-0032) | Turntable renders from the CLI |

## Where we intend to exceed the references

- One app for parametric modeling, history and sculpting (a sculpt layer is a history step).
- Crash-proof: atomic saves and a journal that replays every command bit-exactly.
- Agent and CLI control of every action.
- GPU-first with a CPU fallback and full headless test coverage.
- Accessibility in a 3D tool: keyboard paths and screen-reader nodes everywhere.

## Shared work with the suite

- Adopt `craft-gpu` when RFC-0022's crates land; offer the sculpt renderer and the camera to
  `craft-raster3d` (RFC-0032) as its second consumer.
- Adopt `craft-mesh-ops` (craft-libs #1233) for tangents, UV unwrap and welding; offer our half-edge
  mesh, subdivision and the validated polygon operations to it.
