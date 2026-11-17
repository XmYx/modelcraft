# Performance

Measured 2026-11-17 on one machine: Intel Core i9-12900K (24 threads), 62 GiB RAM, NVIDIA
GeForce RTX 4090, Linux (Wayland session; X11 through XWayland for uncapped frame rates), release
build (`lto = "thin"`, `codegen-units = 1`). Numbers are wall-clock; reproduce with the commands shown.

## Viewport (in the app)

`modelcraft --bench-viewport 600` builds a 1,003,686-quad (2,007,372-triangle, 1,003,688-vertex)
quad sphere, enters sculpting, orbits the camera for 300 frames, then orbits while applying one
clay dab per frame with X symmetry (each dab touching ~30 k vertices) for 300 frames. Window
1600×1000 logical at scale 1.2, 4× MSAA, vsync off. It prints JSON.

| Session | Orbit frame mean / p95 | Sculpt frame mean / p95 | Notes |
|---|---|---|---|
| X11 (`env -u WAYLAND_DISPLAY`) | **3.38 ms (296 fps)** / 5.38 ms | **3.54 ms (283 fps)** / 5.56 ms | uncapped |
| Wayland | 16.9 ms (59 fps) / 18.1 ms | 17.4 ms (57.5 fps) / 21.0 ms | the compositor holds presentation to the 60 Hz display |

In the same run: dab 1.21 ms mean (1.84 ms p95), dirty-chunk upload 0.45 ms mean (0.61 ms p95).

## Sculpt latency (engine, `modelcraft-cli bench --polys 1000000 --dabs 300`)

Mesh: 1,003,686 quads → 2,007,372 triangles. Generating it: 1.1 s; starting a sculpt session
(normals, adjacency, BVH): 0.77 s. Brush radius 0.08 on a unit sphere (~30 k vertices per dab),
X symmetry on (two passes per dab).

| Brush | Dab mean | Dab p95 | Upload of dirty chunks | 1920×1080 draw + GPU wait |
|---|---|---|---|---|
| Draw | 0.38 ms | 0.43 ms | 0.15 ms | 0.41 ms |
| Clay | 0.31 ms | 0.36 ms | 0.14 ms | 0.39 ms |
| Smooth | 0.27 ms | 0.31 ms | 0.14 ms | 0.44 ms |
| Inflate | 0.27 ms | 0.30 ms | 0.14 ms | 0.39 ms |
| Grab | 0.20 ms | 0.30 ms | 0.23 ms | 0.42 ms |
| Pinch | 0.27 ms | 0.31 ms | 0.15 ms | 0.43 ms |
| Mask | 0.11 ms | 0.18 ms | 0.15 ms | 0.38 ms |

End of stroke: BVH refit 9.3 ms. A 250 k-quad primitive plus one subdivision step (≈1 M quads)
evaluates through the history in 1.1 s.

**Input-to-photon estimate while sculpting 2 M triangles**: one dab (≈1.2 ms in the app, including
symmetry and normals) + chunk upload (≈0.5 ms) + one frame (≈3.5 ms uncapped) ≈ **5 ms** before
display latency.

## How the numbers stay low

- Sculpt queries and picking use a triangle BVH whose boxes are padded by how far vertices have
  moved since the last refit, so a stroke never rebuilds it; it is refitted once per stroke.
- Normals are recomputed only around moved vertices.
- The GPU vertex buffer is updated in 4096-vertex chunks marked dirty by the brush.
- History evaluation reuses every step output before the first changed step.

## Not yet

Everything above is single-threaded on the CPU. Multithreaded dabs and evaluation are in
[gaps.md](gaps.md). Session start and subdivision of million-quad meshes take about a second.
