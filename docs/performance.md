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

## Detail levels and shape layers (`modelcraft-cli bench --polys 1000000`, last two lines)

Measured 2026-11-17 while three other builds shared the machine. A 249,698-vertex quad sphere
with a sculpt layer, stepped up to 998,786 vertices (≈ 2 M triangles), sculpted, stepped down and
up again. Every time includes rebuilding the sculpt session (normals, adjacency, BVH: ≈ 0.8 s at
this size) and the undo snapshot.

| Step | Time |
|---|---|
| Level up, 250 k → 1 M vertices | 3.8 s |
| Level down, 1 M → 250 k (fit the coarse level, store the displacement) | 1.7 s |
| Level up again with the stored displacement | 4.0 s |
| Round-trip error (down, then up) on a unit sphere | 1.0 × 10⁻⁷ |
| Add a shape layer at 1 M vertices | 25 ms |
| Change a shape layer's strength at 1 M vertices | 28 ms |

Level changes build the level topology twice through the validated mesh builder; that, not the
linear refinement itself, is most of the time, and it is single-threaded.

**Input-to-photon estimate while sculpting 2 M triangles**: one dab (≈1.2 ms in the app, including
symmetry and normals) + chunk upload (≈0.5 ms) + one frame (≈3.5 ms uncapped) ≈ **5 ms** before
display latency.

## Render engine (path tracer)

Measured 2026-11-17 on the same machine, release build. `modelcraft-cli render` prints one JSON
line per render with these numbers; "samples" are complete camera paths (up to 12 bounces, each
with a shadow ray), so a rate is paths per second, not rays per second.

```
modelcraft-cli render builtin:cornell   -o c.png --width 1024 --height 1024 --samples 256 --device cpu|gpu
modelcraft-cli render builtin:materials -o m.png --width 1280 --height 720  --samples 256 --device cpu|gpu
modelcraft-cli render dense.modelcraft  -o d.png --width 1280 --height 720  --samples 64  --device cpu|gpu
```

`dense.modelcraft` is a quad sphere of 1,003,686 quads (`new`, `add_primitive round_cube`,
`set_primitive … "segments":409`) on the default ground plane under the studio light.

| Scene | Triangles | CPU, 24 threads | GPU, RTX 4090 (wgpu, Vulkan) | GPU / CPU |
|---|---|---|---|---|
| Cornell box, 1024², 256 spp | 36 | 7.5 M paths/s (35.6 s) | 326 M paths/s (0.82 s) | 43× |
| Material row, 1280×720, 256 spp | 39,686 | 10.3 M paths/s (22.8 s) | 561 M paths/s (0.42 s) | 54× |
| Dense sphere, 1280×720, 64 spp | 2,007,376 | 11.3 M paths/s (5.2 s) | 406 M paths/s (0.15 s) | 36× |

Scene preparation for the dense sphere (evaluate, triangulate with smoothed normals, build the
BVH): 2.2 s. The BVH alone, on a 2,000,000-triangle height field: built in 560 ms (2.1 M nodes,
depth 24), then 2.6 M closest-hit rays per second on one thread
(`cargo test --release -p craft-render-bvh -- --ignored --nocapture`). The GPU times exclude shader compilation and
upload (about 1.3 s for the dense scene).

Correctness is tested, not assumed (see the test names in
[target-app-parity.md](target-app-parity.md)): a white surface in a uniform environment renders
within 2 % of the environment (furnace test) for matte, rough-metal and layered dielectric
surfaces; a random-material property test bounds every albedo below 1.04 (Monte Carlo slack);
point, directional, uniform-sky and rectangular-area lighting match their closed forms within
1–2 %; GPU and CPU images of the same scene agree to a relative MSE below 0.002.

**What these numbers do not say.** They are not a comparison with V-Ray or Arnold: neither can be
run here, and no published figure of theirs uses these scenes. The CPU path is a scalar binary BVH
without SIMD packets or wide nodes, so it is certainly slower per ray than their tuned kernels; the
GPU path traverses the BVH in a compute shader rather than through hardware ray queries. A fair
comparison needs the same scene, hardware and noise level in both renderers; until someone runs
that, the honest statement is the absolute rates above.

## How the numbers stay low

- Sculpt queries and picking use a triangle BVH whose boxes are padded by how far vertices have
  moved since the last refit, so a stroke never rebuilds it; it is refitted once per stroke.
- Normals are recomputed only around moved vertices.
- The GPU vertex buffer is updated in 4096-vertex chunks marked dirty by the brush.
- History evaluation reuses every step output before the first changed step.

## Not yet

Everything above is single-threaded on the CPU. Multithreaded dabs and evaluation are in
[gaps.md](gaps.md). Session start and subdivision of million-quad meshes take about a second.
