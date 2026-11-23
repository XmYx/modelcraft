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

### The app window after the per-frame copy was removed (2026-11-18)

The Sculpt panel synced the sculpt session into the scene on every frame. During a stroke that
copied every vertex and made the Properties panel rebuild the whole object, which the table
above (measured before that panel existed) does not show. Measured with the machine's load
average at 23 to 31 on 24 threads (other builds), `modelcraft --bench-viewport N [--bench-polys P]`:

| Triangles | Session | Sculpt frame before | Sculpt frame after (mean / p95) | Orbit frame after | Dab | Upload |
|---|---|---|---|---|---|---|
| 2,007,372 | Wayland (60 Hz cap) | 64.7 ms (15.5 fps), p95 96.2 ms | 17.0 ms (59 fps) / 17.3 ms | 16.7 ms | 1.5 ms | 1.6 ms mean, 0.6 ms p50 |
| 2,007,372 | X11, uncapped | not measured | 5.9 ms (169 fps) / 8.8 ms | 5.0 ms (198 fps) | 1.9 ms | 0.8 ms |
| 12,582,912 | X11, uncapped | not measured | 16.8 ms (59.5 fps) / 23.8 ms | 6.2 ms (161 fps) | 11.2 ms mean, 16.2 ms p95 | 7.8 ms mean, 3.1 ms p50 |

The 12.6 M row is the first measurement of the app window at that size (`--bench-polys 6291456`,
the largest quad sphere the primitive makes): one clay dab per frame with X symmetry at a radius
of 12 % of the object, which touches far more vertices than the engine bench's radius. 20 M
triangles are still not measured: the primitive stops at 1024 segments.

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

## The ten newer brushes (same bench, same mesh and radius)

Measured 2026-11-17 while other builds shared the machine (the seven rows above re-measured in
the same run at 0.26–0.47 ms, so read these as upper bounds).

| Brush | Dab mean | Dab p95 |
|---|---|---|
| Pull | 0.58 ms | 0.78 ms |
| Plateau | 0.48 ms | 0.76 ms |
| Trim | 0.42 ms | 0.66 ms |
| Fill | 0.31 ms | 0.53 ms |
| Polish | 0.38 ms | 0.73 ms |
| Buildup | 0.44 ms | 0.66 ms |
| Gouge | 0.30 ms | 0.45 ms |
| Slide | 0.31 ms | 0.52 ms |
| Bulge | 0.34 ms | 0.56 ms |

Surface grab is not in the table: the bench moves the brush centre while a grab keeps what it
captured at the first dab, so its row there measures almost nothing.

## Rotate, Elastic move and Scrape (`modelcraft-cli bench --polys 1000000 --dabs 200`)

Measured 2026-11-18 on the same 2,007,372-triangle quad sphere, radius 0.08, X symmetry on, load
average about 20 on 24 threads. Draw on this run, for comparison: 0.36 ms mean, 0.62 ms p95.

| Brush | Dab mean | Dab p95 | Vertices a stroke touched | Notes |
|---|---|---|---|---|
| Rotate | 0.02 ms | 0.04 ms | 798 | Moves what the first dab caught |
| Elastic move | 2.9 ms | 3.7 ms | 99,926 | Reaches three brush radii, so nine times the surface of a plain dab |
| Scrape | 0.87 ms | 1.1 ms | 9,404 | |

Radial copies cost one more dab per copy: six copies of a Draw dab take six times a Draw dab.

## 12.6 M triangles (`modelcraft-cli bench --polys 10000000 --dabs 200`)

Measured 2026-11-17 with the machine's load average near 30 on 24 threads (other builds), so
these are upper bounds and vary by a factor of two between runs. The mesh is the largest quad
sphere the primitive makes: 6,291,456 quads, 12,582,912 triangles, 6,291,458 vertices. Same
brush radius as above (0.08 on a unit sphere), which is about 10 k vertices per dab here, X
symmetry on.

| What | Time |
|---|---|
| Generate the mesh | 12–17 s |
| Start a sculpt session (normals, adjacency, picking tree) | 6–10 s |
| Dab: Draw, Clay, Smooth, Inflate, Pinch, Flatten, Crease | 3.0–4.9 ms mean, 4.7–7.7 ms p95 |
| Dab: Grab, Mask | 1.7–2.0 ms mean |
| Dab: Plateau, Trim, Polish | 5.8–9.6 ms mean, 11–20 ms p95 |
| Upload of the dirty chunks (~150 k vertices) | 0.9–4.5 ms |
| 1920×1080 draw + GPU wait | 1.3–20 ms (the GPU was shared; 1.3–1.7 ms in the quiet rows) |
| End of a stroke (undo record, refit of what moved) | 12–46 ms |
| Refit of the whole picking tree, for comparison | 110 ms |

So a dab plus its upload stays under 10 ms for the common brushes at 12.6 M triangles in the
engine. The app window was not measured at this size, 20 M was not measured at all, and a session
takes several seconds to start: that is why the parity row for 20 M+ polygons stays Missing.

The end of a stroke now refits only the leaves of the picking tree that hold moved triangles
(and the boxes above them). At 2 M triangles a stroke end takes 2.3–7.3 ms where the full refit
alone took 9–10 ms.

**Threads, tried and not kept.** Working out a dab's weights and normals on a thread pool was
measured on this machine while it was busy: with 12 threads Draw went from 6.1 ms to 15.0 ms per
dab at 12.6 M triangles (p95 8.6 ms to 32 ms), and session start showed no gain outside the
noise. A dab touches a few thousand vertices, too little to pay for the hand-over when the cores
are contended. Dabs therefore stay on one thread until this is measured on an idle machine.

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

## Patches, visibility and regions (`modelcraft-cli bench --groups --polys 1000000`)

Measured 2026-11-18 on a quad sphere of 1,003,686 quads (2,007,372 triangles), single-threaded,
with the machine's load average at 51 on 24 threads (other builds), so these are upper bounds.

| What | Time |
|---|---|
| Patches by angle (30°) | 47 ms |
| Patches by UV island (6 islands) | 27 ms |
| Bring patches into the sculpt session | 2 ms |
| Faces inside a 64-corner lasso, front faces only (14,920 faces) | 59 ms |
| Hide them in the session | 2 ms |
| Mask the same lasso (15,197 vertices) | 33 ms |
| Shell from that mask (30,392 faces) | 72 ms |
| Clip to a plane (244,928 vertices moved) | 27 ms |
| Trim by a plane with a cap (652,430 faces left) | 2.5 s |
| Occlusion mask, 4 rays per vertex (4.0 M rays) | 3.8 s |

Trim rebuilds the whole mesh through the validating builder, which is where its time goes. The
occlusion mask casts about four million rays whatever the size (24 per vertex on small meshes,
never fewer than 4), so a 12.6 M-triangle mesh takes about six times as long. Undoing a change of
patches or visibility restores the scene and rebuilds the sculpt session (about 0.8 s here); doing
one does not rebuild it. In the app, a change of patches or visibility uploads the whole sculpt
mesh again (not measured separately; the same upload as starting a session).

## Decimate and Project Detail (`MODELCRAFT_BENCH_ONLY=decimate modelcraft-cli bench --groups --polys N`)

Measured 2026-11-18, single-threaded, load average about 30 on 24 threads (other builds). The
mesh is a quad sphere; the projection target is the same sphere and its source a copy with a
bump pattern, so both have the same count.

| Triangles | Decimate to 10 % | Largest gap between a face and the sphere after it | Project Detail (all vertices) | Worst distance left to the source |
|---|---|---|---|---|
| 202,800 | 2.8 s | 6.6e-4 of the radius | 0.67 s | 1.2e-7 |
| 2,007,372 | 50.7 s | 8.2e-5 of the radius | 28.1 s | 1.2e-7 |

Decimation spends its time re-checking each collapse (the link condition, UV carry-over, patch
lines, triangle turning) when its turn comes; nothing in it runs in parallel yet. Every vertex of
the result is a vertex of the input (within 1.2e-7 of the sphere here, the float error of the
input itself). Project Detail casts two rays per vertex and falls back to a closest-point search.

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
| Material showcase (glass, coat, fuzz, thin film, textures, bump), 960×540, 256 spp | 72,196 | 8.5 M paths/s (15.7 s; load average 22) | 299 M paths/s (0.44 s) | 35× |

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
1–2 %; GPU and CPU images of the same scene agree to a relative MSE below 0.002 (the tests'
bound; measured with the same 64 samples: showcase 0.00002, cut-outs and maps 0.0001, the Cornell
box, studio, night street and interior below 0.000001).

### Benchmark: error against samples and time (`modelcraft-cli render-bench`)

Protocol from [render-research.md](render-research.md) §8: six scenes built in code
(`craft-render-core::scenes`), 256×256, each against its own reference of 4096 samples made of two
independent halves (the last column is the reference's own error, a quarter of the squared
difference between the halves); five seeds per budget, mean relative MSE (ε = 0.01). Two of the
protocol's eight scenes are missing because the engine has no volumes, subsurface, hair or
instancing yet; relMSE is the main measure and FLIP (below) can be added with `--flip`.

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
| interior (sun and sky through a small window) | 2,238 | 0.246 / 0.046 / 0.0185 / **0.219 / 0.120** | 5.50 | 0.0014 |
| night-street (10,000 emissive triangles, 500 point and spot lights) | 10,602 | 6.79 / 1.01 / 0.262 / 0.072 / 0.0198 | 2.43 | 0.0012 |
| studio (dome panorama, three area lights, coated and metal objects) | 36,104 | 0.326 / 0.081 / 0.0204 / 0.0052 / 0.0013 | 2.82 | 0.00008 |
| showcase (glass, coat, fuzz, thin film, textures) | 72,196 | 2.45 / 0.692 / 0.166 / 0.049 / 0.0135 | 3.46 | 0.0010 |

Read honestly:
- Error falls as 1/N on cornell and studio (a factor of 4 per 4× samples): the estimator is
  healthy there. Seconds to relMSE 0.01: 0.25 s (cornell) and 0.64 s (studio), interpolated.
- **interior gets worse at 64 and 256 samples** (the spread at 256 spp is 0.005–0.48 across
  seeds): a few paths that reach the sun through the window by way of the metal ball or the coat
  carry enormous energy, and one such sample in an image dominates its relMSE. Learned sampling
  (below) is the answer to it. `clamp_indirect` hides it at the cost of energy and is off by
  default.
- materials, night-street and showcase converge at 1/N from 16 samples on but start from large
  errors (speckle from the small bright light on glossy metal, and caustic paths through glass).

**GPU, RTX 4090 (wgpu, Vulkan).** All six scenes: the kernel renders everything the CPU path does
(layered materials, textures, panorama lighting, the light tree). Load average 20 during the run;
the GPU itself was idle.

| Scene | relMSE at 1 / 4 / 16 / 64 / 256 spp | Seconds at 256 spp |
|---|---|---|
| cornell | 0.220 / 0.054 / 0.0137 / 0.0035 / 0.0009 | 0.053 |
| materials | 4.94 / 0.805 / 0.128 / 0.049 / 0.0121 | 0.123 |
| interior | 0.246 / 0.046 / 0.0185 / 0.219 / 0.120 | 0.115 |
| night-street | 6.79 / 1.01 / 0.262 / 0.072 / 0.0198 | 0.057 |
| studio | 0.326 / 0.081 / 0.0205 / 0.0052 / 0.0013 | 0.115 |
| showcase | 2.45 / 0.692 / 0.166 / 0.049 / 0.0135 | 0.168 |

Every error equals the CPU's to the digits shown, because the two devices run the same samples
through the same estimator; the GPU takes 1/20 to 1/50 of the time. Before this kernel the GPU
rendered three of these scenes and chose lights by power: night-street then stood at
12.6 / 3.41 / 0.848 / 0.236 / 0.0656 (3.3× the samples of the CPU for the same error) and
materials at 6.07 / 1.22 / 0.221 / 0.069 / 0.0178. The interior scene's rise at 64 and 256 samples
is the estimator's (see above), now on both devices. (The interior rows were measured again
after sun disks became lights that paths find; they were 0.262 / 0.148 at 64 / 256 spp before.)

### Indirect light and fireflies: learned sampling (`--guiding --splitting`)

`craft-render-guide` is a hash grid over the scene (three resolutions, 16,384 cells). Each cell
holds a radiance cache (mean and second moment of the light leaving it) and a 16×16 equal-area
image of where the light it reflects arrives from, both filled by the paths of earlier passes
(passes end at 1, 2, 4, 8, … samples). Two things read it:

- **guiding**: part of the continuing directions at diffuse and fuzz lobes is drawn from the
  cell's image instead of the BSDF, as one mixture density;
- **adaptive roulette and splitting**: a path is ended, kept or split into up to four by its
  expected contribution (throughput × the root second moment of the cell's cache) against the root
  second moment of its pixel's samples.

The grid takes about 50 MB whatever the scene (fixed-point sums in atomic integers, so recording
from many threads gives the same tables in any order).

Neither changes the expected image (a test holds a long render's mean to 2 % of the plain
estimator's, and the errors below fall to the reference's own error), and a render stays
deterministic per seed.

The **interior** scene (a room lit through a small window by sun and sky), CPU, 256×256, **eight**
seeds per budget, reference 4096 samples of the plain estimator (its own relMSE 0.0014). The load
average was 47–64 during these runs (other builds), so seconds are upper bounds and comparable
only roughly; errors do not depend on load. Mean relMSE, with the median and the worst seed, because
the mean of a few seeds is ruled by its worst one here:

| Estimator | 16 spp | 64 spp | 256 spp | 1024 spp | Seconds at 256 / 1024 spp |
|---|---|---|---|---|---|
| plain: mean | 0.0190 | 0.145 | 0.0856 | 0.0382 | 7.1 / 28.4 |
| plain: median, worst | 0.0116, 0.050 | 0.0052, 0.67 | 0.0293, 0.48 | 0.0082, 0.14 | |
| guiding: mean | 0.0705 | 0.0706 | 0.0710 | 0.0107 | 9.5 / 33.3 |
| guiding: median, worst | 0.0133, 0.36 | 0.0329, 0.18 | 0.0192, 0.45 | 0.0062, 0.038 | |
| adaptive splitting: mean | 0.0421 | 0.113 | 0.0307 | 0.0071 | 8.7 / 33.4 |
| adaptive splitting: median, worst | 0.0104, 0.15 | 0.0174, 0.74 | 0.0277, 0.073 | 0.0066, 0.013 | |
| **both: mean** | 0.0518 | 0.0384 | **0.0120** | **0.0043** | 14.7 / 44.2 |
| both: median, worst | 0.0114, 0.21 | 0.0070, 0.21 | 0.0104, 0.020 | 0.0039, 0.0072 | |

Read honestly:
- **Before**, the error did not fall with samples: 0.145 at 64 spp, 0.086 at 256, 0.038 at 1024,
  with seeds between 0.004 and 0.48 at 256 spp. **After** (both), it falls every step from 64 spp
  on, is 7× lower at 256 spp and 9× lower at 1024 spp, and the seeds agree (0.003–0.020 at 256 spp).
  Both at 256 spp (14.7 s) has a third of the error of the plain estimator at 1024 spp (28.4 s).
  Efficiency, 1 / (relMSE × seconds): 1.7 → 5.7 at 256 spp, 0.9 → 5.3 at 1024 spp.
- **Below about 64 samples learned sampling is worse** (0.052 against 0.019 at 16 spp): the first
  passes sample from distributions learned from very few paths. It is a final-frame technique,
  off by default.
- **Guiding alone** helps late (3.6× at 1024 spp) and is unreliable before; the two belong
  together, because the mixture makes paths that miss the learned directions heavier at every
  bounce, and splitting is what spreads such a path's weight. The share of guided samples is kept
  below one minus the surface's albedo so that this weight never exceeds 1. Measured on the way:
  without that bound guiding alone stood at 1.16 and 0.131 at 256 and 1024 spp (worst seed 8.0);
  with the bound but with the sun a point for paths, at 0.99 and 0.30; the figures above need
  both the bound and the sun's disk (next point but one).
- **What is left**: the remaining bright samples are the sun's reflection off the table's
  mirror-smooth coat onto the walls (a caustic through a solid angle of 7·10⁻⁵ sr). A cell's
  16×16 image has bins of 0.05 sr, so it can raise the chance of finding that direction by tens,
  not thousands. Finer directions where the light is (adaptive trees, or lobes fitted per cell)
  and a caustics method are the next steps.
- Sun disks are now lights with a size in the sky: a path that leaves the scene inside the disk
  finds it, weighted against aiming at it (a test: a sun in a mirror, MSE more than 10× lower
  than aiming alone at 16 spp). This is correct for mirrors seen directly, and guided paths, which
  learn to head for the window, need it. It did **not** change the plain estimator's numbers on
  the interior (0.172 / 0.096 / 0.037 at 64 / 256 / 1024 spp before it, the same within seed
  noise), because the rare event there is the wall's bounce towards the mirror, not the mirror's
  view of the sun.

The same pair on the test scene of `guiding_and_adaptive_splitting_keep_the_image_and_lower_its_noise`
(a closed room lit only by a lamp shining at the ceiling, 32×32, 256 spp, three seeds): relMSE
0.266 plain, 0.250 guiding, 0.192 adaptive splitting, 0.124 both; with two bounces only, where
finding the bright patch is all there is, 0.583 plain against 0.190 guided.

On the other five scenes the pair buys little and costs time, which is why it is off by default
(five seeds, relMSE at 64 / 256 spp with both on, against the plain table above; seconds at 256
spp under a load average of 38–53, against the plain run's under a load of about 50):

| Scene | Both on | Plain | Seconds at 256 spp, both / plain |
|---|---|---|---|
| cornell | 0.0035 / 0.0009 | 0.0035 / 0.0009 | 5.4 / 2.8 |
| materials | 0.0469 / 0.0093 | 0.049 / 0.0121 | 3.8 / 3.4 |
| night-street | 0.0768 / 0.0206 | 0.072 / 0.0198 | 3.0 / 2.4 |
| studio | 0.0050 / 0.0013 | 0.0052 / 0.0013 | 3.6 / 2.8 |
| showcase | 0.0328 / 0.0085 | 0.049 / 0.0135 | 5.5 / 3.5 |

`render-bench` now prints the median next to the mean and takes `--guiding` and `--splitting`;
its reference is always the plain estimator, so a technique that changed the expected image would
show as an error that stops falling.

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

### Many lights: resampled direct lighting (`--restir`)

`craft-render-core::restir` resamples the direct light at the first surface (Bitterli et al. 2020,
ReSTIR DI) with the light tree as the source of candidates; `craft-render-gpu` runs the same
four stages as compute kernels. `unbiased` is for final frames: `M` candidates per pixel and pass,
one kept, a shadow ray for the kept one, then one pass of merging with the reservoirs of up to four
neighbours, normalised exactly (a shadow ray from each merged pixel finds how many could have
produced the kept point). `viewport` adds temporal reuse (the previous pass's reservoir at the same
pixel, its count capped at 20 passes' worth) and normalises by the sum of counts, which needs no
extra rays but is biased and correlates consecutive passes. Indirect bounces still use the light
tree, so only the first bounce changes.

Tests: the unbiased mode's mean image equals the plain estimator's within 2–5 % on every benchmark
scene (measured on the benchmark scenes at 512 spp: 0.996–1.004 of the plain mean); a viewport render
stays within 8 % of it (measured 0.92–0.99; it is darker near shadow edges and in partly occluded
lighting); GPU and CPU images of the same samples agree to a relative MSE of 0.000001 on night-street
(both modes), studio and interior.

Night-street, 256×256, 5 seeds, reference 4096 spp. Mean relMSE at 1 / 4 / 16 / 64 / 256 spp.
`M` is candidates and `k` merged neighbours. (The M=4, k=0 row used a reference of 2048 spp, whose own
error is about twice as large.)

| Estimator | relMSE | Seconds at 64 spp (CPU) / 256 spp (GPU) |
|---|---|---|
| CPU, plain (light tree) | 6.79 / 1.009 / 0.262 / 0.072 / – | 0.58 |
| CPU, unbiased, M=8, k=4 | 1.026 / 0.276 / 0.080 / 0.0195 / – | 2.03 |
| CPU, unbiased, M=16, k=0 | 1.153 / 0.280 / 0.071 / 0.0186 / – | 3.26 |
| CPU, viewport, M=8, k=4 | 1.001 / 0.343 / 0.101 / 0.050 / – | 2.66 |
| GPU, plain | 6.79 / 1.009 / 0.262 / 0.072 / 0.0198 | 0.059 |
| GPU, unbiased, M=4, k=0 | – / – / – / 0.0308 / 0.0094 | 0.106 |
| GPU, unbiased, M=8, k=4 | 1.026 / 0.275 / 0.080 / 0.0195 / 0.0058 | 0.161 |
| GPU, unbiased, M=16, k=4 | 1.138 / 0.276 / 0.069 / 0.0189 / 0.0055 | 0.207 |
| GPU, viewport, M=8, k=4 | 1.001 / 0.343 / 0.101 / 0.051 / 0.0369 | 0.144 |

The CPU rows ran at a load average of 36–48 and the GPU rows at 27–35 (other builds; the GPU itself
was idle), so seconds are upper bounds; errors do not depend on load.

Read honestly:
- **At equal samples** the unbiased mode lowers the error 3.3–3.7× on both devices (0.0198 → 0.0058
  at 256 spp on the GPU), and the benefit stops at about that: the error left at 0.0055–0.006 comes
  from the bounces after the first, which this technique does not touch (more candidates or
  neighbours do not lower it).
- **At equal time it is roughly even, not a win, on this scene.** The extra work costs 2–3.5× the
  time of a plain sample (a candidate is a tree descent, a light point and a full BSDF
  evaluation), so efficiency 1 / (relMSE × seconds) is 1.0–1.25× the plain estimator's on the GPU
  (M=4: 1.16×) and 1.0–1.6× on the CPU (best at 1–4 spp). The research note's 2–10× target was
  not met: our baseline, the light tree, is far stronger than the power sampling of the paper, and
  most of what is left is indirect.
- **The viewport mode is not for converging.** It cleans a few passes fastest (0.101 against
  0.262 at 16 spp) and then stops improving (0.050 at 64 spp, 0.037 at 256 spp on the GPU) because
  consecutive passes share reservoirs; its mean image is 1–8 % darker than the plain estimator's
  (night-street 0.92–0.94, interior 0.94–0.96, studio and Cornell box 0.99, materials 0.96,
  showcase 0.94–0.99).
- Throughput on the GPU at 1024×1024 (64 spp, night-street): plain 316 M samples/s; unbiased
  M=8,k=4 120 M/s (2.6× the time), M=4 149 M/s. The plumbing alone (M=1, no neighbours) costs
  26 % more than a plain sample, each further candidate about 14 %, and four neighbours with their
  exact normalisation about 40–47 %.
- Where it should pay most is a scene whose cost is almost all first-bounce direct light from very
  many lights (a long depth-1 render: CPU, 48×48, 10 seeds, 16 spp, relMSE 0.31 plain against 0.12
  with M=16); it needs the following passes to be cheap for the whole picture to gain.

### Denoising (`--denoise svgf|nfor`)

`craft-render-denoise` filters a render with its own feature images (albedo, normal, depth, variance;
the film now also keeps the sum of its odd-numbered samples, so a second, independent half rendering
is available on the CPU and the GPU). Both filters divide the picture by its albedo first (not where
the albedo is nearly black, so emitters are left alone).

- **SVGF** (Schied et al. 2017) for the interactive view: reprojection of the previous frame's
  result through the motion image, variance tracking, five à-trous wavelet passes whose luminance
  edge-stopping follows the variance (the variance is the median over 3×3, so the edge of a light does
  not switch the stop off for its neighbours). It is the Render View's filter while passes arrive. A-SVGF's
  gradient-based adaptive history is not implemented.
- **NFOR-class regression** (Bitterli et al. 2016) for finals: first-order regression on normal, depth and
  screen position over a 17×17 window, weighted by the variance-aware distance of 3×3 colour patches and a
  soft same-surface test, each half rendering filtered with the weights of the other, the two averaged. It
  is the Render View's filter on the finished picture. Without the paper's second stage; fireflies are not
  treated separately.

256×256, five seeds, `modelcraft-cli render-bench --device gpu --denoise svgf|nfor`, reference 4096 spp
(mean relMSE, before; after). The CPU films are the same samples, so the errors are the same to the digits
shown (checked on cornell and studio); the filters run on the CPU in both cases (24 threads, load average 16 to 40
from other builds, so times are upper bounds).

| Scene | relMSE raw, 1 / 4 / 16 / 64 spp | SVGF after | NFOR after |
|---|---|---|---|
| cornell | 0.2203 / 0.0543 / 0.0137 / 0.0035 | 0.0703 / 0.012 / 0.0016 / 0.0005 | 0.0703 / 0.0367 / 0.0015 / 0.0003 |
| materials | 4.939 / 0.805 / 0.1275 / 0.0493 | 1.78 / 0.6877 / 0.0944 / 0.0406 | 1.78 / 0.0398 / 0.0157 / 0.0055 |
| interior | 0.2459 / 0.0464 / 0.0185 / 0.2193 | 0.0842 / 0.0445 / 0.0154 / 0.2182 | 0.0842 / 0.0016 / 0.0007 / 0.0018 |
| night-street | 6.793 / 1.009 / 0.2622 / 0.072 | 2.511 / 0.778 / 0.1955 / 0.0563 | 2.511 / 0.6951 / 0.0995 / 0.019 |
| studio | 0.3259 / 0.0806 / 0.0205 / 0.0052 | 0.1157 / 0.0313 / 0.0058 / 0.0015 | 0.1157 / 0.0138 / 0.0042 / 0.0013 |
| showcase | 2.452 / 0.6923 / 0.1657 / 0.0494 | 0.4998 / 0.5623 / 0.0559 / 0.0196 | 0.4998 / 0.0325 / 0.0123 / 0.0052 |

Filter time per 256×256 image: SVGF 31 to 53 ms, NFOR 210 to 700 ms (it needs two samples; with one, `nfor`
falls back to SVGF, which is why the 1 spp columns match).

Read honestly:
- NFOR lowers the error 2.6 to 125× from 4 spp up (cornell 1.5× at 4 spp, 9 to 14× from 16 spp; interior at
  64 spp, where a few bright paths ruled the raw error, 0.219 → 0.0018). SVGF lowers it 3 to 9× at 1 spp and
  1.0 to 3.5× from 4 spp up, least on materials, interior and night-street, whose noise is a few very bright samples
  that a variance-guided filter keeps.
- Both are biased, and measure against the unfiltered reference. The picture's mean brightness stays within 8 % (the test's bound) on three scenes at 16 spp; on the
  Cornell box the SVGF mean differed from the raw one by 0.1 %.
- Temporal reuse: a still camera, eight frames of 2 samples on the Cornell box, 0.060 filtered alone → 0.030 with the
  history. With the camera moving 0.4 pixel per frame the history is found through the motion vectors but resampling it
  blurs it: 0.052 alone against 0.078 with history (raw 0.102). Frame-to-frame reuse under motion therefore does not
  pay yet; a sharper resampling filter is the next step.
- No GPU filter yet: the filters run on the CPU, at 31 ms (SVGF) for 65 k pixels, which scales to about 0.5 s
  at 1080p on this machine; a compute-shader à-trous pass is the way to a viewport frame budget.

### Wide BVH and PLOC build (`--bvh wide`)

`craft-render-bvh::wide` collapses the binary tree into compressed 8-wide nodes (Ylitie et al. 2017: child
boxes as bytes relative to the node's, rounded outwards; 112 bytes a node, a ray tests eight boxes at once
and visits the hit children nearest first), on the CPU and in the GPU kernel. The collapse opens the child
with the largest surface area until a node has eight; the paper's dynamic-programming collapse is not
implemented. `craft-render-bvh::ploc` builds the binary tree bottom-up by parallel locally-ordered clustering
(Meister and Bittner 2018; radius 25, Morton order, small subtrees collapsed into leaves by the surface area
heuristic). H-PLOC (a one-kernel GPU build) is not implemented; PLOC here runs on the CPU through rayon.

Measured with `cargo test --release -p craft-render-bvh -- --ignored --nocapture bench_wide` (2,000,000-triangle
height field, 1,000,000 incoherent rays on one thread; two runs, load average 41 to 54) and
`cargo test --release -p craft-render-gpu -- --ignored --nocapture bench_wide` (the same field lit by sun and
sky, 960×540, 16 samples, RTX 4090):

| | Build | Nodes | Rays per second, one CPU thread | GPU, M paths/s |
|---|---|---|---|---|
| binned SAH, binary | 0.63 to 0.68 s | 2,138,233 | 0.46 to 0.52 M | 158 |
| binned SAH, 8-wide | same + 0.12 to 0.17 s collapse | 323,155 | 0.65 to 0.82 M (1.25 to 1.8×) | 132 (0.84×) |
| PLOC, binary | 1.02 to 1.04 s | 1,802,763 | 0.42 to 0.43 M | |
| PLOC, 8-wide | same + 0.12 s | | 0.64 to 0.66 M | |

Read honestly:
- **On the CPU the wide tree traces 1.25 to 1.8× faster** on incoherent rays over a large scene, with a seventh of
  the nodes (36 MB against 68 MB). The research note's 1.9 to 2.1× was measured on GPUs with a tuned traversal;
  ours is lane-parallel scalar Rust that the compiler vectorises.
- **On the GPU it is slower here (0.84×)**, so the binary tree stays the default. The compressed nodes halve the
  memory the kernel reads, but WGSL has no way to load and sort eight children without extra registers
  and local memory: the first version, which read the node a word at a time, ran at 0.66× and one that copied a
  node into a local array at 0.26×; reading seven four-word rows got to 0.84×, and not sorting the hit children
  (pushing them in slot order) at 0.19×. An octant-ordered traversal that avoids the sort (Ylitie's) is the next
  step.
- **Whole renders of the benchmark scenes (10 to 72 thousand triangles) do not change measurably** with the
  wide tree on the CPU (within the noise of a machine at load 45 to 56): their trees fit in cache and shading
  dominates. Images are identical to the binary tree's (tests: CPU wide against binary to 1e-9, GPU wide
  against binary and against the CPU to 1e-4, measured 0 to 1e-5).
- **PLOC is slower to build (1.5×) and gives a worse tree on the CPU**: on 20,000 random triangles its
  expected traversal cost is 15 % above the binned build's (79 against 69), on a regular grid 2.8× above
  (49 against 17: ordering by Morton code is poor there), and rays are 15 to 20 % slower through it. Its
  point is that every round is one parallel loop and so maps to a GPU build for trees rebuilt each frame;
  that kernel is not written.

### Hardware ray queries (`--hardware-rays`, feature `hw-ray-query`)

An optional backend for the GPU path (see the exception in [architecture.md](architecture.md)): the scene's triangles
become a bottom-level and a top-level acceleration structure through wgpu 30's experimental ray-query support, and
`trace_closest` / `trace_any` become `rayQuery` loops (`bvh_rayquery.wgsl`). Off by default at both levels (cargo
feature and runtime option); scenes with cut-outs or too large for the device, and devices without the feature,
fall back to the software traversal.

Same height field and settings as the wide-tree table (2,000,000 triangles, 960×540, 16 samples, RTX 4090 on
Vulkan, load average high from other builds), one run:

| Backend | Setup | M paths/s |
|---|---|---|
| software, binary BVH | 189 ms | 120 (158 in the earlier run) |
| hardware ray queries | 232 ms (building the structure) | 1066 |

About 7 to 9× the software path on this scene. Hardware and software agree on fixed rays: the first-hit depth of
every pixel differs by more than 0.2 % in none of 3,072 pixels on each of cornell, materials and night-street, and the
8-sample images differ by a relative MSE below 1e-7. Not measured: other vendors, Metal and DX12 (the feature
lists Vulkan), scenes near the device's limits, acceleration structure rebuilds for animation.

### FLIP (`render-bench --flip`)

`craft-render-core::flip` implements the FLIP difference of Andersson et al. 2020 (colour pipeline: opponent
space, contrast sensitivity filters at 67 pixels per degree, CIELAB with the Hunt effect, HyAB distance
remapped; feature pipeline: edges and points; combined as `colour ^ (1 − features)`), in the low-dynamic-range
form: the benchmark's linear images are clamped to [0, 1] at exposure 0 first. It was written from the paper's
description and has **not been compared with the authors' reference implementation**, so its numbers are
comparable with each other (same code, same images) but not with published FLIP figures. Tests: identical images
give 0, the value is in [0, 1] and grows with noise, black against white is the maximum, a shifted edge shows at
the edge, bad input is `None`.

Mean FLIP of the benchmark scenes, 256×256, GPU renders, five seeds, against the 4096-sample reference (the CPU's
are the same). Raw, then after the filters of the denoising section (FLIP before / after):

| Scene | spp | Raw | after regression (NFOR) | after SVGF |
|---|---|---|---|---|
| cornell | 4 / 16 / 64 | 0.085 / 0.052 / 0.032 | 0.050 / 0.033 / 0.024 | 0.063 / 0.038 / – |
| studio | 4 / 16 / 64 | 0.092 / 0.049 / 0.026 | 0.045 / 0.031 / 0.021 | 0.045 / 0.029 / – |
| night-street | 4 / 16 / 64 | 0.188 / 0.115 / 0.072 | 0.171 / 0.089 / 0.054 | 0.176 / 0.104 / – |
| showcase | 4 / 16 / 64 | 0.227 / 0.123 / 0.071 | 0.120 / 0.092 / 0.065 | 0.222 / 0.088 / – |

FLIP falls with samples like relMSE does, and it says the filters help less than relMSE suggests: the regression's
9 to 14× lower relMSE on cornell at 16 and 64 spp is 1.6× and 1.3× lower FLIP, because FLIP counts the lost fine
detail that relMSE hardly sees. The 1 spp SVGF figures (cornell 0.134 → 0.091, studio 0.167 → 0.062, night-street 0.312 → 0.216,
showcase 0.350 → 0.233) are in the same direction.

**What these numbers do not say.** They are not a comparison with V-Ray or Arnold: neither can be
run here, and no published figure of theirs uses these scenes. The CPU path is a scalar binary BVH
without SIMD packets or wide nodes, so it is certainly slower per ray than their tuned kernels; the
GPU path traverses the BVH in a compute shader rather than through hardware ray queries. A fair
comparison needs the same scene, hardware and noise level in both renderers; until someone runs
that, the honest statement is the absolute rates above.
## Undo history on open

A project whose history holds 502 steps (a sphere, a subdivision step and 500 transforms) is
2.8 KB on disk and opens with its history rebuilt and checked in **62 ms** in total, mesh
evaluation included (`modelcraft-cli info`, release build, Core i9-12900K). Reproduce: run the
502-line script with `modelcraft-cli run h.modelcraft h.jsonl`, then `time modelcraft-cli info
h.modelcraft`. Sculpt strokes replay at their dab cost (table above), so a history with many long
strokes on a dense mesh opens proportionally slower.

## Skinned character (engine, `modelcraft-cli bench-skin --verts 100000 --joints 60 --frames 120`)

A procedurally generated stand-in for a character: a tube of 100,000 vertices (100 sides ×
1,000 rings) with a chain of 60 joints up its middle, bound with heat weights (4 influences per
point), every joint keyed on two rotation channels (180 curves) so that every point moves on
every frame. Each frame goes to the frame (curves → joint transforms) and evaluates the mesh
through the dependency graph (the primitive and history are cached; only the skin runs). Two runs,
measured while three other builds shared the machine; single-threaded (as the code was then).

| | Linear blend | Dual quaternion |
|---|---|---|
| Pose: curves → 60 joints | 0.10 ms | 0.11 ms |
| Evaluate the skinned mesh, mean | 6.1–6.6 ms | 6.4–6.5 ms |
| Evaluate, p95 | 9.5 ms | 7.6–8.2 ms |
| **Evaluated frames per second** | **150–162** | **151–155** |
| Skinning kernel alone, 100,000 points | 1.0 ms | 3.3 ms |

Binding (automatic heat weights: 60 conjugate-gradient solves over the surface): 10.6–11.4 s, once.
The gap between the kernel and the evaluation (≈ 5 ms) is copying the mesh and rebuilding its
positions for the new pose. These are engine numbers from before skinning ran on several cores;
the next section has the app's playback and the kernels as they are now.

## Skinned character in the app (`modelcraft --bench-skin 300`, add `--bench-skin-dq`)

The same character as above, built by the same code (`rig::bench_character`: 100,000 vertices,
60 joints, heat weights, 4 influences), played in the app: each frame goes to the next frame
(curves → joints, constraints), evaluates the mesh through the dependency graph, and the viewport
turns the new mesh into triangles with smoothed normals, uploads it and draws it. RTX 4090,
1600×1000, no vsync, 24 hardware threads. **The machine was shared with other builds throughout
(load average 27 to 58 on 24 threads), so these numbers are noisy and pessimistic**; each line
says the load it was taken under.

| | Frame, mean (p95) | Frames per second | Evaluate | Upload (triangles, normals, buffers) | Load |
|---|---|---|---|---|---|
| Before: skinning and viewport data on one thread (`RAYON_NUM_THREADS=1`, same session as the next line) | 57.0 ms (95.5) | 17.6 | 5.8 ms | 38.3 ms | 27 |
| **Now**, linear blend | 36.0–39.2 ms (52–66) | **25.5–27.8** | 4.0–4.3 ms | 20.7–21.1 ms | 28 |
| **Now**, dual quaternion | 38.8 ms (55.0) | **25.8** | 4.5 ms | 21.0 ms | 28 |
| Skinning on all cores, viewport data still on one thread, linear blend | 65.6 ms (101) | 15.2 | 8.3 ms | 44.4 ms | 46–58 |
| The same, dual quaternion | 90.1 ms (152) | 11.1 | 12.9 ms | 64.7 ms | 46–58 |

Pose (curves → 60 joints, constraints) is 0.10–0.19 ms in every run.

Read honestly:
- Playback of this character is about 26 frames per second, just at its 24: there is no
  headroom, and under heavier load it falls to 15.
- **The upload is the frame**, not the skinning: more than half of every frame goes into
  rebuilding the mesh's drawing data (one vertex per face corner with a smoothed normal, 400,000
  of them, and the triangle list) and creating new GPU buffers, although only positions changed.
  Building that data in up to 8 runs of faces at once (`buffers::mesh_data`) took it from 38 to
  21 ms in the same session. The fix that remains is structural: keep the GPU mesh and its
  triangle list while a mesh only deforms, and write positions and normals into it.
- Evaluation (4 ms) is mostly copying the mesh for the new pose (≈ 15 MB of topology that did
  not change), not the skinning kernel.

**Skinning kernels alone** (`modelcraft-cli bench-skin`, 100,000 points, 4 influences, best of
five batches of ten):

| | One core | In 4 pieces (`RAYON_NUM_THREADS=4`) | In 8 pieces (`=8`) | Default (at most 8 pieces, 24 threads), three runs |
|---|---|---|---|---|
| Linear blend | 1.05–1.94 ms | 0.36 ms | 0.57 ms | 0.32, 2.31, 3.93 ms |
| Dual quaternion | 3.13–6.14 ms | 0.74 ms | 0.96 ms | 0.73, 5.60, 6.29 ms |

On idle cores the kernels are 3 to 4 times faster in pieces (0.3 ms and 0.7 ms). On a saturated
machine the same code can be *slower* than one core (the pieces wait for a thread): with one piece
per hardware thread it was 4.9–7.8 ms against 2.0 ms, which is why skinning is cut into at most
8 pieces. The results are bit-identical on one core and on many (tested).

**Engine evaluation** (`bench-skin`, pose + evaluate, no viewport), under load 45–50: linear blend
7.5–15.0 ms mean (65–131 evaluated frames per second), dual quaternion 8.2–11.0 ms (90–121);
before this change, under the same kind of load, 6.2–13.7 ms and 11.1–12.5 ms. The spread between
runs is larger than any difference: evaluation is dominated by the mesh copy, and nothing can be
claimed here beyond "not slower".

**Finding an object** (`Scene::get`) used to walk the object list; the scene now keeps an index
from id to position, kept in step by everything that adds, removes or reorders objects (a test
compares it with a walk of the list after each kind of change). On this 62-object scene it does
not show (pose stayed at 0.1 ms); it is there for scenes with thousands of objects, where every
world matrix walked the list once per ancestor. It was not timed on such a scene.

## UV unwrap (engine, `modelcraft-cli uv-bench --segments N`)

A welded UV sphere of N×N/2 quads with no seams: the automatic cut adds a path, then LSCM solves
the single island with a sparse Cholesky factorisation (nested-dissection order, two steps of
refinement). Distortion is measured on the result (`modelcraft_uv::measure`; area-weighted, angle
error in degrees, stretch is σ1/σ2 of the per-triangle map, 1 = conformal; area RMS is the RMS of
the log of each triangle's UV-to-surface area share, 0 = one scale everywhere). Same machine as
above, release build.

| Sphere | Vertices | Unwrap | Solve residual | Mean / max angle error | Mean / max stretch | Area RMS (log) | Flipped |
|---|---|---|---|---|---|---|---|
| 32×16 | 482 | 2 ms | 2e-14 | 0.54° / 1.41° | 1.013 / 1.025 | 0.486 | 0 |
| 64×32 | 1,986 | 7 ms | 7e-14 | 0.14° / 0.43° | 1.003 / 1.008 | 0.493 | 0 |
| 128×64 | 8,066 | 32 ms | 3e-13 | 0.035° / 0.11° | 1.001 / 1.002 | 0.496 | 0 |
| 256×128 | 32,514 | 196 ms | 1e-12 | 0.010° / 0.36° | 1.000 / 1.008 | 0.499 | 0 |

A conformal map keeps angles and cannot keep areas on a sphere: the area RMS near 0.5 is the price
of the cut. On developable shapes (plane, open tube, cube faces) the unwrap is exact to float
precision (angle error below 1e-4°, area RMS below 1e-6; see `crates/uv/src/tests.rs`).
Preconditioned conjugate gradients on the same system stalled (residual 1.7e-5 after 8,000
iterations at 64×32), which is why the solver is direct; conjugate gradients remain as the
fallback above 24 M factor entries.

### Angle-based flattening (ABF) against LSCM (`uv-bench --method both`)

`UnwrapMethod::Abf` solves for the angles of the flat triangles (Sheffer and de Sturler 2001) with
the reduced Newton system of ABF++ (Sheffer et al. 2005): per iteration one sparse Cholesky
factorisation of two unknowns per interior vertex, then a least-squares fit of triangles of those
angles (the LSCM system) for the positions. Convergence is a constraint residual below 1e-8; an island
that does not reach it is flattened by LSCM and `IslandReport::abf_fell_back` says so. Same machine
and release build as above (loaded). Angle error is in degrees against the surface; area RMS is the
RMS of the log area ratio, so 0 means one scale everywhere. All unwraps have 0 flipped triangles.

Sphere with one pole-to-pole seam (`--shape sphere --cut meridian`): one interior everywhere else,
the hard case for an angle-preserving map.

| Sphere | Solver | Time | Newton iterations | Mean / max angle error | Max stretch | Area RMS (log) |
|---|---|---|---|---|---|---|
| 64×32 (1,986 vertices) | LSCM | 11 ms | | 2.18° / 9.78° | 1.489 | 0.98 |
| | ABF | 43 ms | 3 | 2.55° / 6.76° | 1.208 | 1.35 |
| 256×128 (32,514 vertices) | LSCM | 661 ms | | 0.55° / 5.04° | 1.217 | 1.01 |
| | ABF | 3,938 ms | 3 | 0.67° / 2.08° | 1.051 | 1.45 |

ABF has the smaller worst-case angle and stretch error and the larger mean angle error and area
distortion: it minimises the relative squared deviation of angles (and spreads the curvature of
the surface evenly), where LSCM minimises the conformal energy. It is 4 to 6 times slower here
(three factorisations and a reconstruction, against one solve).

The same shapes with the automatic cut (`--cut auto`, which gives a closed shape a branching cut;
most vertices are then on the boundary, where angles are free):

| Shape (segments 64) | Solver | Time | Mean / max angle error | Area RMS (log) |
|---|---|---|---|---|
| Sphere | LSCM | 10 ms | 0.14° / 0.43° | 0.49 |
| | ABF | 15 ms | 0.11° / 0.19° | 0.008 |
| Round cube | LSCM | 5 ms | 0.34° / 1.92° | 2.07 |
| | ABF | 9 ms | 0.20° / 0.97° | 0.044 |
| Torus | LSCM | 11 ms | 0.23° / 0.78° | 4.84 |
| | ABF | 12 ms | 0.18° / 0.69° | 0.024 |

Developable shapes (a tube cut along a line, a cone cut along a generatrix) are already feasible
angle sets: ABF takes 0 Newton iterations and both solvers are exact (angle error and area RMS
below 1e-4). The tests also check that ABF's energy never exceeds LSCM's on the sphere, round cube
and torus (`abf_is_closer_to_the_surface_angles_than_lscm_on_curved_islands`).

### Relax (`uv-bench --relax N`)

`relax_corners` runs N local/global iterations (Liu et al. 2008) on the whole layout: the global
system (cotangent Laplacian, two unknowns per vertex) is factored once and solved N times. After
a LSCM unwrap of a sphere with one seam (`--shape sphere --cut meridian`), 15 iterations, same
machine and release build as above:

| Sphere | Mode | Time | Fit energy before to after |
|---|---|---|---|
| 64×32 (1,986 vertices) | keep lengths | 46 ms | 0.223 to 0.101 |
| | keep angles | 29 ms | 2.1e-3 to 1.9e-3 |
| 256×128 (32,514 vertices) | keep lengths | 815 ms | 0.230 to 0.098 |
| | keep angles | 784 ms | 1.3e-4 to 1.3e-4 |

Keeping lengths spreads the area out (on the 32×16 sphere in the tests the area RMS falls from
0.92 to 0.36 and the largest area ratio from 5.6 to 3.1) and gives up angles (mean angle error
6.0° to 15.1°). Keeping angles has little left to improve on a layout that is already conformal.
The energy is the fit energy of that mode, and a relaxation that would raise it is discarded.
On a 8×8 grid whose interior vertices were knocked about by up to 0.015 of its width, 60 iterations
bring the maximum angle error from 17.3° to 4e-5° (`relax_restores_a_jittered_grid_to_an_isometry`).

### UV transfer (`uv-transfer-bench --segments N`)

An unwrapped sphere of N×N/2 quads gives its UVs to a sphere 1.5 times finer (so every target
corner lies on or just off the source surface), by closest point on the triangles of the source
(uniform grid, expanding shells). Same machine and release build as above.

| Source / target faces | Target corners | Per island | Plain | Mean distance to the source |
|---|---|---|---|---|
| 512 / 1,152 | 4,512 | 10 ms | 6 ms | 4.4e-3 |
| 8,192 / 18,432 | 73,344 | 171 ms | 159 ms | 2.7e-4 |
| 14,450 / 32,385 | 129,030 | 539 ms | 487 ms | 1.7e-4 |
| 31,250 / 70,125 | 279,750 | 1,188 ms | 937 ms | 7.6e-5 |

About 4 microseconds a corner, growing slowly with size. Per island costs one more lookup per face
(the anchor). The distance is the gap between the two tessellations of the sphere, not an error of
the search: a mesh gets its own UVs back to 1e-4 (`a_mesh_gets_its_own_uvs_back`).

### UDIM packing (`uv-pack-bench --islands N --tiles T`)

The same random rectangles as above, dealt to T tiles largest first, packed per tile with MaxRects
and brought to the common smallest scale, so the texel density is the same in every tile. Same
machine and release build.

| Islands | Tiles | Rotation | Time | Common scale (one tile: 200 islands 0.154) | Utilisation per tile |
|---|---|---|---|---|---|
| 200 | 1 | fit | 63 ms | 0.154 | 84.8 % |
| 200 | 4 | none / fit | 22 / 16 ms | 0.308 / 0.313 | 84.9 % / 87.5 % |
| 1,000 | 4 | none / fit | 417 / 457 ms | 0.138 / 0.139 | 81.9 % / 83.5 % |
| 1,000 | 16 | none / fit | 82 / 90 ms | 0.278 / 0.282 | 83.3 % / 86.2 % |

Four tiles double the linear scale of one, as they should (the areas are four times larger), and
dealing by area keeps the tiles within a fraction of a percent of each other. Packing many small
sets is faster than one large one because MaxRects work grows faster than linearly with islands.

## UV packing (engine, `modelcraft-cli uv-pack-bench --islands N`)

Random rectangular islands (fixed seed, sides 0.05 to 1.55, even texel density), packed into the
unit square; utilisation is the share of the square the islands cover. Same machine as above
(a loaded one: other builds were running), release build. The margin is 0.004 up to 1,000 islands
and 0.0005 above (it is per island on every side, so very many islands need a thinner one to fit
at all). Up to 500 islands MaxRects packs (four heuristics up to 350, one above); beyond that, or
when MaxRects has examined 300 million free rectangles, a linear next-fit shelf packer does. `work`
counts free rectangles examined and is identical on every run, so the bound is tested without a
clock (`packing_work_is_bounded_at_the_island_cap`).

| Islands | Packer | Time (rotation none / fit) | Utilisation (none / fit) | Work |
|---|---|---|---|---|
| 50 | MaxRects | 3 / 4 ms | 87.4 / 88.1 % | 1.2 M |
| 200 | MaxRects | 68 / 55 ms | 84.0 / 84.8 % | 43 to 50 M |
| 350 | MaxRects | 327 / 352 ms | 80.9 / 82.0 % | 190 to 200 M |
| 500 | MaxRects, one heuristic | 234 / 178 ms | 78.6 / 79.2 % | 220 to 250 M |
| 1,000 | shelves | 0 / 1 ms | 69.8 / 70.6 % | 21 k |
| 20,000 | shelves | 12 / 10 ms | 84.7 / 84.6 % | 0.42 M |
| 100,000 (the cap) | shelves | 55 / 63 ms | 70.3 / 70.3 % | 2.1 M |

Rectangles gain little from rotation by definition; the rotation modes matter for slanted islands
(a diagonal strip packs 1.8 times as densely once straightened, `fit_rotation_straightens_a_diagonal_strip`).
Before the work bound, 2,000 islands took 20 s with MaxRects alone (reported in review).
## Hair (`modelcraft-cli hair-bench`)

Guides grown on a 128×64 UV sphere (16,128 triangles, area 12.56), then render hairs blended from
them by the nearest four guides (`knn`) or the three-guide barycentric rule (`bary`). All 24 cores,
release build, `modelcraft-cli hair-bench --strands N [--blend bary --clump A --frizz A --points P] [--gpu]`.
The machine is shared with other builds, so each table says how loaded it was (`uptime` load
average on 24 cores) and the numbers move with it.

### Making the hairs

| Strands × points | Guides | Blend, extras | Interpolation | Strands per second | Memory held | Load average |
|---|---|---|---|---|---|---|
| 100,000 × 8 | 5,000 | knn | 102 ms | 0.98 M | 12 MB | 58 |
| 1,000,000 × 8 | 5,000 | knn | 467 ms | 2.14 M | 120 MB | 58 |
| 1,000,000 × 8 | 5,000 | bary, clump 0.5, frizz 0.1 | 653 ms | 1.53 M | 120 MB | 58 |
| 1,000,000 × 16 | 5,000 | knn | 614 ms | 1.63 M | 216 MB | 58 |
| 100,000 × 8 | 5,000 | knn | 12 ms | 8.07 M | 12 MB | 8 |
| 1,000,000 × 8 | 5,000 | knn | 96 ms | 10.40 M | 120 MB | 8 |

Growing 5,000 guides takes 2–30 ms. Memory is 12 bytes per point plus 24 bytes per strand (root and
two widths); `modelcraft_hair::estimate_bytes` matches the measured size exactly. Hard caps (checked
before allocating): 200,000 guides, 5,000,000 strands, 64 points per strand and 40 million points
per operation by default; no caller can raise them past 1,000,000 guides, 20,000,000 strands, 256
points per strand and 200 million points.

## Simulation

Measured 2026-11-18, release build, one thread (every solver here runs single-threaded and
deterministically), on the same 24-thread machine while other workstreams built on it (load average
about 40), so treat the times as pessimistic. Reproduce with `modelcraft-cli sim-bench <kind> …`.

**Rigid bodies** (`sim-bench rigid`, Rapier 0.36 with `enhanced-determinism`): N unit boxes
spread over a grid at several heights drop onto a ground plane, 120 frames at 24 fps, 4 substeps per
frame (480 solver steps), box colliders as convex hulls, 8 points cached per body.

| Bodies | Solve, 120 frames | Per frame | Per step | Asleep at the end | Cache |
|---|---|---|---|---|---|
| 100 | 19 ms | 0.16 ms | 0.040 ms | 100 | 1.1 MiB |
| 1,000 | 198 ms | 1.65 ms | 0.41 ms | 1,000 | 11 MiB |
| 4,000 (the most one world holds) | 2.45 s | 20.4 ms | 5.1 ms | 3,888 | 44 MiB |

The time per body grows from 1.6 µs per frame at 1,000 bodies to 5 µs at 4,000: the bodies of the
4,000 case actually collide with each other (a denser pile), which the 1,000 case mostly does not.

**Cloth** (`sim-bench cloth`, our own XPBD solver, one thread): a flat square sheet two metres
across over a UV sphere of 2,304 triangles, 60 frames at 24 fps, the load average about 18 on the
24 threads. Constraints are stretch on every edge, shear on the quad diagonals and a dihedral bend
per interior edge (about six per vertex). "Stretch" is the relative change of an edge from its
rest length at the last frame: mean, 99th percentile and largest.

| Vertices | Setup | Substeps × iterations | Extras | Solve, 60 frames | Per frame | Per substep | Stretch mean / p99 / max |
|---|---|---|---|---|---|---|---|
| 1,089 | 0.4 ms | 8 × 2 | sphere | 1.10 s | 18.3 ms | 2.3 ms | 0.014 / 0.064 / 0.10 |
| 10,201 | 5 ms | 8 × 2 | none (free fall) | 1.25 s | **20.8 ms** | 2.6 ms | 0 |
| 10,201 | 4 ms | 8 × 2 | two corners pinned, hanging | 1.63 s | 27.1 ms | 3.4 ms | 0.052 / 0.37 / 6.2 |
| 10,201 | 6 ms | 8 × 2 | sphere | 4.20 s | **70.0 ms** | 8.7 ms | 0.073 / 0.44 / 0.94 |
| 10,201 | 6 ms | 8 × 2 | sphere and self collision | 6.87 s | 114.5 ms | 14.3 ms | 0.071 / 0.41 / 2.9 |
| 10,201 | 4 ms | 16 × 4 | sphere | 11.49 s | 191.4 ms | 12.0 ms | 0.013 / 0.068 / 0.31 |
| 40,401 | 16 ms | 8 × 2 | sphere | 8.62 s | 143.7 ms | 18.0 ms | 0.35 / 2.5 / 5.0 |

A 10,000-vertex cloth costs about 21 ms a frame to simulate free, and 70 ms draped over a sphere
at the default 8 substeps and 2 iterations; it does not run in real time, which is why simulations
are baked. About 29 ns per constraint projection. The stretch columns are the honest limit of the
method: with 16 iterations per frame, information crosses at most 16 edges, so a 100 × 100 sheet
stretches where it is pulled (a quarter of its edges by more than 7 % over the sphere); 64
iterations per frame bring the worst edge to 31 % and the 99th percentile to 6.8 %. A hanging
sheet pinned only at two corners stretches most of all. The 40,401-vertex row shows the same
effect worse; raising iterations or substeps is the lever, at proportional cost. Self collision adds
about 5.6 ms per substep at 10,000 vertices. Collision cost is per vertex near a collider: the
box test culls the rest.

**Particles** (`sim-bench particles`, our own solver, one thread): a surface emitter of 2,048
triangles fills a pool of N slots at N per second over the run, 120 frames at 24 fps and 2 substeps
(60 frames for the last row); particles live 2× the run, so none die. Per frame includes the births,
the forces and the write of the whole pool into the cache (12 bytes a slot). Collisions are a floor
and a sphere of 2,304 triangles, swept; "forces" is turbulent wind, noise acceleration and an
attractor with drag. The load average was about 25.

| Particles (slots) | Extras | Solve | Per frame | Per substep | Cache |
|---|---|---|---|---|---|
| 10,000 | none | 24 ms | 0.20 ms | 0.10 ms | 14 MiB |
| 100,000 | none | 198 ms | **1.65 ms** | 0.83 ms | 137 MiB |
| 100,000 | collisions | 3.24 s | **27.0 ms** | 13.5 ms | 137 MiB |
| 100,000 | collisions and forces | 8.97 s | 74.7 ms | 37.4 ms | 137 MiB |
| 500,000 | collisions (60 frames) | 7.25 s | 120.9 ms | 60.4 ms | 343 MiB |

A 100,000-particle system runs at about 600 frames a second without collisions and 37 with them, so
it plays back from a cache, not live; the cache is the cost (137 MiB for 120 frames), which is why
the byte budget exists. Collisions cost 135 ns a particle a substep; the noise field (three value
noises per particle) is most of the "forces" row. Display is capped at 10,000 instances at a time
(the clone limit), whatever the pool holds.

### Drawing them (`--gpu`)

The viewport pass (`modelcraft_render::hair_gpu`): positions in one vertex buffer, one index buffer of
line strips with primitive restart, one draw call. Strands are in random order, so a display
percentage draws a prefix of the index buffer: changing it costs nothing and re-uploads nothing.
`hair-bench --gpu` draws the sphere and the hair at 1920×1080 with 4× MSAA, orbiting the camera, 60
frames per row after 10 warm-up frames; a frame is render, submit and wait for the GPU (frames per
second is 1000 / mean). Adapter: NVIDIA GeForce RTX 4090, load average 8. Lines are one pixel wide
and alpha blended; there is no strand shading yet, so these numbers say how fast the lines go down,
not what a shaded render costs.

| Strands in the groom | Display | Strands drawn | Mean frame | Frames per second | 95th percentile | GPU memory |
|---|---|---|---|---|---|---|
| 100,000 | 100 % | 100,000 | 0.31 ms | 3,217 | 0.38 ms | 13 MB |
| 100,000 | 50 % | 50,000 | 0.20 ms | 4,892 | 0.21 ms | |
| 100,000 | 10 % | 10,001 | 0.10 ms | 9,715 | 0.11 ms | |
| 100,000 | 1 % | 1,000 | 0.08 ms | 12,390 | 0.13 ms | |
| 1,000,000 | 100 % | 1,000,000 | 1.22 ms | 821 | 1.25 ms | 132 MB |
| 1,000,000 | 50 % | 500,000 | 0.76 ms | 1,309 | 0.81 ms | |
| 1,000,000 | 10 % | 100,001 | 0.29 ms | 3,443 | 0.30 ms | |
| 1,000,000 | 1 % | 10,000 | 0.10 ms | 9,686 | 0.11 ms | |

Uploading 1,000,000 strands takes 55 ms. In the app the default display is capped at 100,000
strands while editing ("Limit strands while editing", on by default) and the strands of a hair under
the brush are replaced by its guides until the stroke ends. The app's own frame time was not
measured (the window needs a display); the figures above are the hair pass alone, and the egui
overhead is the same with or without hair. Without a GPU adapter the viewport shows guides only.

### Rendering them (`modelcraft-cli render`, CPU path tracer)

Hair is traced as round-cone segments (`craft-render-bvh::curves`) in a BVH of their own, scattered
by the fibre model of `craft-render-core::hair` (Chiang et al. 2016 lobes: R, TT, TRT and a residual
tail). A sphere with 3,000 guides grown with a bend and clumped, 10 points per strand, the strands
cut off by the default render settings: 640×480, 16 samples per pixel, 12 bounces, studio light and
ground, `--device cpu` (the GPU path tracer declines hair and the CPU one is used), all 24 cores,
load average about 25. "Seconds" are `scene_seconds` (building the curve BVH) and `render_seconds`
from the command's report.

| Strands | Segments | Scene build | Render | Million samples per second |
|---|---|---|---|---|
| none (the bare sphere) | 0 | 0.003 s | 0.52 s | 9.55 |
| 10,000 | 90,000 | 0.09 s | 1.75 s | 2.81 |
| 150,000 | 1,350,000 | 0.38 s | 4.00 s | 1.23 |
| 1,000,000 | 9,000,000 | 3.35 s | 7.85 s | 0.63 |

The same 150,000-strand picture at 64 samples per pixel takes 16.5 s. Hair costs about 8 to 15 times
the bare sphere here, because most camera paths cross many fibres and every fibre hit spends a shadow
ray and a bounce. A render of hair is capped at 1,000,000 strands per hair object (a prefix of the
groom, an unbiased thinner copy).

### Baking and exporting (`hair-bench --bake`, `hair-export`)

Dynamics (`modelcraft_hair::StrandSolver` run by `modelcraft_sim::bake`): Verlet with gravity, damping, a
weak shape spring, 8 length-constraint passes per step and collision with the sphere (a grid of its
16,128 triangles), 4 substeps per frame, all 24 cores, strands independent of each other. Load average
about 22.

| Guides × points | Frames | Bake | Per frame | Cache |
|---|---|---|---|---|
| 5,000 × 8 | 60 | 431 ms | 7.2 ms | 28.8 MB |
| 50,000 × 8 | 24 | 1,104 ms | 46 ms | 115.2 MB |

A bake runs on the window's thread, so the app waits for it; scrubbing afterwards plays the cache (and a
baked hair's strands are made again for each frame shown). Exports: 10,000 strands of 10 points as glTF
lines is 2.3 MB (100,000 points, 90,000 segments); 3,000 guides of 10 points as one frame of point cache
is 0.36 MB.

## Script memory (`cargo test -p modelcraft-script --test memory -- --nocapture`)

How far the process's peak resident size (`VmHWM`) grows while a hostile script runs under the
default limits (10 M operations, 10 s, 256 KiB of strings and 20,000 array and 2,000 map entries per
container, 4 M geometry items). Linux, test profile, release-like `opt-level` of this workspace, one
run on a shared 24-core machine; the process starts at its own baseline and the peak only rises, so
a script that needs less than an earlier one in the list shows +0. There is no allocator cap
(`docs/scripting.md`, "Memory"), so these are measurements, guarded by a 512 MiB budget per script.

| Script | Peak growth | Time until stopped |
|---|---|---|
| map keys with integer values, in a loop | +63 MiB | 1.1 s |
| map keys with an array of 8 each | +140 MiB | 1.2 s |
| map keys with a 1 KiB string each | +0 MiB (stopped by the string limit) | 1.0 s |
| array `push` of an array in a loop | +0 MiB (stopped by the array limit) | 0.02 s |
| string doubling | +0 MiB | 0.00 s |
| nested containers, 1000 arrays of 1000 | +0 MiB | 0.01 s |
| mesh vertices in a loop | +0 MiB (stopped by the geometry budget) | 0.29 s |
| all seven together | +203 MiB | |

## Mesh booleans (`cargo test -p modelcraft-ops boolean_timings -- --ignored --nocapture`)

Union of two UV spheres of the same size, one moved by (0.7, 0.2, 0.1) and tilted, run on a shared
machine under load (single thread, optimized test profile, 2026-11-23). The work follows the part
of the surfaces that touches, not the whole of them: the grid only pairs triangles whose boxes
overlap, so the number of pairs tested grows with the length of the intersection curve.

| Triangles in each | Time | Crossing points | Triangle pairs tested | Faces in the result |
|---|---|---|---|---|
| 3,968 | 16 ms | 352 | 2,778 | 6,460 |
| 16,128 | 53 ms | 732 | 5,745 | 24,954 |
| 65,024 | 195 ms | 1,466 | 11,435 | 97,834 |
| 261,120 | 927 ms | 2,930 | 22,612 | 387,416 |

Limits that bound the work and memory (`boolean::Limits`): 2,000,000 triangles in either input,
40,000,000 overlapping pairs, 60,000,000 grid entries, and 4,000,000,000 triangle evaluations of the
winding number for parts no cut reaches. Each is covered by a test that exceeds it.

## How the numbers stay low

- Sculpt queries and picking use a triangle BVH whose boxes are padded by how far vertices have
  moved since the last refit, so a stroke never rebuilds it; it is refitted once per stroke.
- Normals are recomputed only around moved vertices.
- The GPU vertex buffer is updated in 4096-vertex chunks marked dirty by the brush.
- History evaluation reuses every step output before the first changed step.

## Not yet

Everything above is single-threaded on the CPU (see "Threads, tried and not kept"). Multithreaded
evaluation and session start are in [gaps.md](gaps.md). Session start and subdivision of million-quad meshes take about a second.
