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

## Live detail and even remesh (`modelcraft-cli bench`, last lines)

Measured 2026-11-17 on the same machine while other builds were running, engine only (no GPU
upload). A draw stroke of 200 dabs with live detail on:

| Mesh | Triangles before → after | Dab mean / p95 | Stroke end (rebuild the picking tree) | Undo |
|---|---|---|---|---|
| 49 k triangles, radius 0.2, edge 0.01 | 49,152 → 149,606 | 5.8 ms / 7.3 ms | 86 ms | 36 ms |
| 1 M triangles, radius 0.1, edge 0.004 | 995,328 → 1,156,008 | 6.5 ms / 7.0 ms | 0.76 s | 0.82 s |

A dab with live detail costs about twenty times a plain dab: each of up to four split rounds
scans every triangle under the brush. In the app each split also re-uploads the whole vertex and
index buffer; that is not in these numbers and not yet measured.

Even remesh (closed quad spheres, unit radius):

| Input quads | Cell | Output quads | Time | Volume change |
|---|---|---|---|---|
| 24,576 | 0.02 | 47,022 | 0.20 s | 0.045 % |
| 1,008,600 | 0.01 | 188,466 | 3.8 s | 0.011 % |
| 1,008,600 | 0.006 | 523,584 | 3.8 s | 0.004 % |

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
| Material showcase (glass, coat, fuzz, thin film, textures, bump), 960×540, 256 spp | 72,196 | 6.2 M paths/s (21.4 s) | not rendered: the GPU path declines layered materials and textures | — |

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

### Benchmark: error against samples and time (`modelcraft-cli render-bench`)

Protocol from [render-research.md](render-research.md) §8: six scenes built in code
(`craft-render-core::scenes`), 256×256, each against its own reference of 4096 samples made of two
independent halves (the last column is the reference's own error, a quarter of the squared
difference between the halves); five seeds per budget, mean relative MSE (ε = 0.01). Two of the
protocol's eight scenes are missing because the engine has no volumes, subsurface, hair or
instancing yet; FLIP is not implemented yet, so only relMSE is reported.

```
modelcraft-cli render-bench                 # CPU, all scenes, 1/4/16/64/256 spp, 5 seeds
modelcraft-cli render-bench --device gpu
```

**CPU, 24 threads.** The machine was shared with other builds during this run (load average
about 50), so the seconds are upper bounds; the errors per sample count do not depend on load.

| Scene (what it stresses) | Triangles | relMSE at 1 / 4 / 16 / 64 / 256 spp | Seconds at 256 spp | Reference's own relMSE |
|---|---|---|---|---|
| cornell (diffuse interreflection) | 36 | 0.220 / 0.054 / 0.0137 / 0.0035 / 0.0009 | 2.76 | 0.00005 |
| materials (small bright light on glossy metal: MIS stress) | 39,686 | 4.94 / 0.805 / 0.128 / 0.049 / 0.0121 | 3.39 | 0.0006 |
| interior (sun and sky through a small window) | 2,238 | 0.246 / 0.046 / 0.0185 / **0.262 / 0.148** | 5.50 | 0.0017 |
| night-street (10,000 emissive triangles, 500 point and spot lights) | 10,602 | 6.79 / 1.01 / 0.262 / 0.072 / 0.0198 | 2.43 | 0.0012 |
| studio (dome panorama, three area lights, coated and metal objects) | 36,104 | 0.326 / 0.081 / 0.0204 / 0.0052 / 0.0013 | 2.82 | 0.00008 |
| showcase (glass, coat, fuzz, thin film, textures) | 72,196 | 2.45 / 0.692 / 0.166 / 0.049 / 0.0135 | 3.46 | 0.0010 |

Read honestly:
- Error falls as 1/N on cornell and studio (a factor of 4 per 4× samples): the estimator is
  healthy there. Seconds to relMSE 0.01: 0.25 s (cornell) and 0.64 s (studio), interpolated.
- **interior gets worse at 64 and 256 samples** (the spread at 256 spp is 0.005–0.62 across
  seeds): a few paths that reach the sun through the window by way of the metal ball or the coat
  carry enormous energy, and one such sample in an image dominates its relMSE. This is the case
  path guiding, a radiance cache or at least path regularisation exists for; none is implemented.
  `clamp_indirect` hides it at the cost of energy and is off by default.
- materials, night-street and showcase converge at 1/N from 16 samples on but start from large
  errors (speckle from the small bright light on glossy metal, and caustic paths through glass).

**GPU, RTX 4090 (wgpu, Vulkan).** Three of the six scenes; the kernel declines panorama lighting
and textures, and those scenes are reported as not rendered rather than run on the CPU.

| Scene | relMSE at 1 / 4 / 16 / 64 / 256 spp | Seconds at 256 spp |
|---|---|---|
| cornell | 0.220 / 0.054 / 0.0137 / 0.0035 / 0.0009 | 0.050 |
| materials | 6.07 / 1.22 / 0.221 / 0.069 / 0.0178 | 0.110 |
| night-street | 12.6 / 3.41 / 0.848 / 0.236 / 0.0656 | 0.043 |

On cornell the GPU reaches the same error per sample as the CPU (it runs the same samples) in
1/55 of the time. On night-street it needs about 3.3× the samples of the CPU for the same error,
because the GPU chooses lights by power while the CPU uses the light tree; it is still far ahead
in time. Porting the tree to the kernel is the next step for that scene.

### Sample order, feature images and environment sampling (measured by their tests)

| What | Measurement |
|---|---|
| z-curve pixel order against independent pixels, Cornell box, 1 spp, six seeds | error after averaging 4×4 pixel blocks at least 1.5× lower (the test's bound), raw error unchanged: the noise moves to high frequencies |
| MIS-compensated panorama table against the plain one, sun patch in a bright sky, 16 spp | MSE at least 1.3× lower, same mean within 1 % |
| per-pixel variance image against the actual error of a 16-spp Cornell box | predicts the mean squared error within the test's bounds (actual / predicted below 4) |

### Light sampling (variance at equal samples)

Measured by the tests that assert them (`cargo test -p craft-render-core`), 48×48 and 16×16
images, error against a 4096-sample reference:

| Scene | Estimator | Result |
|---|---|---|
| 400 small lamps over a floor (800 emissive triangles), direct light, 16 spp | light tree against choosing by power | MSE 2.8× lower with the tree with z-curve pixel order (0.0252 against 0.0696), 4.9× with independent pixels (0.0166 against 0.0821) |
| A panorama whose light is one patch of 0.5 % of the image, 64 spp | sampling the image against sampling the surface | variance more than 20× lower (the test's bound; both converge to the patch's closed form within 3 %) |

The tree has no orientation cones and does not split, so lights that face away still take
samples; the references' many-light samplers do both.

**What these numbers do not say.** They are not a comparison with V-Ray or Arnold: neither can be
run here, and no published figure of theirs uses these scenes. The CPU path is a scalar binary BVH
without SIMD packets or wide nodes, so it is certainly slower per ray than their tuned kernels; the
GPU path traverses the BVH in a compute shader rather than through hardware ray queries. A fair
comparison needs the same scene, hardware and noise level in both renderers; until someone runs
that, the honest statement is the absolute rates above.

## Skinned character (engine, `modelcraft-cli bench-skin --verts 100000 --joints 60 --frames 120`)

A procedurally generated stand-in for a character: a tube of 100,000 vertices (100 sides ×
1,000 rings) with a chain of 60 joints up its middle, bound with heat weights (4 influences per
point), every joint keyed on two rotation channels (180 curves) so that every point moves on
every frame. Each frame goes to the frame (curves → joint transforms) and evaluates the mesh
through the dependency graph (the primitive and history are cached; only the skin runs). Two runs,
measured while three other builds shared the machine; single-threaded.

| | Linear blend | Dual quaternion |
|---|---|---|
| Pose: curves → 60 joints | 0.10 ms | 0.11 ms |
| Evaluate the skinned mesh, mean | 6.1–6.6 ms | 6.4–6.5 ms |
| Evaluate, p95 | 9.5 ms | 7.6–8.2 ms |
| **Evaluated frames per second** | **150–162** | **151–155** |
| Skinning kernel alone, 100,000 points | 1.0 ms | 3.3 ms |

Binding (automatic heat weights: 60 conjugate-gradient solves over the surface): 10.6–11.4 s, once.
The gap between the kernel and the evaluation (≈ 5 ms) is copying the mesh and rebuilding its
positions for the new pose. These are engine numbers; playback in the app adds a full vertex
upload and normals for the changed mesh each frame, which has not been measured yet.

## How the numbers stay low

- Sculpt queries and picking use a triangle BVH whose boxes are padded by how far vertices have
  moved since the last refit, so a stroke never rebuilds it; it is refitted once per stroke.
- Normals are recomputed only around moved vertices.
- The GPU vertex buffer is updated in 4096-vertex chunks marked dirty by the brush.
- History evaluation reuses every step output before the first changed step.

## Not yet

Everything above is single-threaded on the CPU. Multithreaded dabs and evaluation are in
[gaps.md](gaps.md). Session start and subdivision of million-quad meshes take about a second.
