# Craft render engine: research survey and adoption plan

Status: research note, 2026-11-17. It contains no engine code. It is input for the render-engine
workstream (`crates/craft-render-*`), and it records what the published literature offers, at what
cost, and in what order to adopt it.

The goal is to match or beat V-Ray and Arnold on quality per unit of compute. In this note,
"quality per compute" means *efficiency*: 1 / (error × time), with error measured against a
converged reference (see [benchmark protocol](#8-benchmark-protocol-proving-beats-v-rayarnold-honestly)).

Rules that shape every recommendation below:

- **Clean room.** We implement from papers and open specifications only. Reference code is listed
  so we know what exists and what licence it carries, **not** so anyone reads it. Nobody on the
  render workstream opens it. Blender/Cycles, PBRT, Mitsuba and Embree samples stay off-limits
  even where their licences are permissive (see `WORKSTREAM_RULES.md`).
- **Pure Rust, no `unsafe`.** The CPU path is plain Rust. The GPU path is WGSL compute through
  wgpu 30.
- **CI has no GPU.** Every GPU technique needs a CPU reference that the GPU path is tested against.

Numbers quoted from papers are the authors' own measurements, on their scenes and hardware. They
show what a technique *can* buy. They are not predictions for our engine; each adoption slice
has to re-measure them on our scenes.

---

## 1. Platform facts that constrain the design

### 1.1 wgpu 30 and hardware ray tracing

I checked the shipped source of `wgpu-types` 30.0.2 in the local cargo registry. This is the
version the workspace pins (`wgpu = "30"`).

- `Features::EXPERIMENTAL_RAY_QUERY` exists, with acceleration-structure (BLAS/TLAS) build and
  inline ray queries from any shader stage, including compute. The feature docs list **Vulkan**.
  `wgpu-hal` also sets the flag on DX12 and Metal adapters that report ray-tracing support. WGSL
  needs `enable wgpu_ray_query;`, and vertex-position fetch needs a second enable
  ([wgpu ray-tracing docs](https://wgpu.rs/doc/wgpu/documentation/extensions/ray_tracing/index.html),
  [gfx-rs/wgpu#8545](https://github.com/gfx-rs/wgpu/pull/8545)).
- Ray-tracing *pipelines* (raygen/hit/miss shaders, shader binding tables) are still in
  development. Ray queries are the only hardware path.
- **Enabling any `EXPERIMENTAL_*` feature requires `unsafe`.** Device creation must pass
  `ExperimentalFeatures`, whose enabling constructor is an `unsafe fn` whose safety contract
  says there "may be UB-containing bugs". The workspace has a no-`unsafe` rule, so **hardware ray
  queries are not available to us without a rules exception.**

Consequence for the plan: the GPU path uses **software BVH traversal in WGSL compute**. That is
the path the render agent is already building (`craft-render-bvh` emits `bytemuck::Pod` nodes
for a GPU buffer). Keep traversal behind one interface, `trace_closest` / `trace_any`, so that
a ray-query backend can be added later behind a cargo feature if the user grants the exception.
Every ranking below assumes software traversal. That makes BVH quality and node layout
(section 3.9) matter *more* than in RTX-based papers, and it makes techniques that save rays
worth more to us than their papers suggest.

`Features::SUBGROUP` (not experimental) is available on Vulkan/DX12/Metal and is worth using for
wavefront compaction, reservoir merging and warp-level BVH builds.

### 1.2 Where things slot in the current engine

From the render worktree (`craft-render-core`, `craft-render-bvh`, `craft-render-gpu`):

| Slot | Today | Techniques that plug in here |
|---|---|---|
| `craft-render-bvh` | binned SAH binary BVH, 32-byte nodes | wide/compressed BVH, PLOC-family GPU builder, curve and instance support |
| `craft-render-core::sampler` | Owen-scrambled Sobol' (Burley 2020) with padding | z-ordered / blue-noise pixel seeding, guided dimension allocation |
| `craft-render-core::integrator` | unidirectional PT + NEE + MIS + RR | light tree, MIS compensation, ReSTIR, guiding, EARS, MNEE |
| *new* `craft-render-lights` | — | light BVH, environment importance map, ReGIR grid |
| *new* `craft-render-guide` | — | spatio-directional guiding structure, radiance/hash caches |
| *new* `craft-render-denoise` | — | SVGF-class filter, NFOR-class regression, small CNN |
| `craft-render-core::material`/`bsdf` | OpenPBR-style | random-walk SSS, hair BCSDF, spectral upsampling |
| *new* volume module | — | null-scattering tracking, majorant grids |

Each new crate stays UI-free with unit and property tests, as the workstream rules require. All
of them are candidates for craft-libs later.

---

## 2. Summary table

Gains are as reported by the cited paper, against the paper's own baseline. "CPU"/"GPU" give the
rough per-sample cost. Complexity is our estimate of the work in pure Rust plus WGSL, with no
hardware ray tracing.

| # | Technique | Reported gain | Per-sample cost | Complexity (Rust+WGSL) | Depends on |
|---|---|---|---|---|---|
| 1 | Light BVH / many-light tree (Conty Estevez & Kulla 2018) | large on many-light scenes (orders of magnitude vs uniform/power sampling) | O(log L) traversal per NEE | M | — |
| 2 | MIS compensation (Karlík et al. 2019) | cheap variance cut for env-map + BSDF MIS | ~0 | S | env map sampling |
| 3 | ReSTIR DI (Bitterli et al. 2020) | equal error **6–60×** faster (unbiased), 35–65× (biased) | 1–2 extra shadow rays, screen buffers | M | light sampling (#1) for candidates |
| 4 | Blue-noise error distribution (z-sampler, Ahmed & Wonka 2020; Heitz & Belcour 2019) | same MSE, perceptually much better at low spp | ~0 | S | Owen-Sobol' sampler (exists) |
| 5 | Practical path guiding family (Müller et al. 2017; Rath et al. 2020; Ruppert et al. 2020) | large on indirect-dominated interiors; Müller 2017 is the production baseline | tree lookup + mixture sample per bounce | M–L | — |
| 6 | EARS: efficiency-aware RR and splitting (Rath et al. 2022) | large efficiency gains on top of guiding | needs a radiance/variance cache | M | #5 or a cache |
| 7 | ReSTIR GI (Ouyang et al. 2021) | MSE **9.3–166×** lower than PT at 1 spp | 1 extra path vertex reuse; reservoirs | M | #3 |
| 8 | ReSTIR PT / GRIS (Lin et al. 2022) + reservoir splatting (Liu 2025) | MAPE 0.39 vs PT 1.63 (Carousel) | shift-mapping cost; heavy bookkeeping | L | #3, #7 |
| 9 | Compressed wide BVH (Ylitie et al. 2017) | **1.9–2.1×** incoherent traversal; 35–60 % memory | — | M | BVH builder |
| 10 | H-PLOC GPU BVH builder (Benthin et al. 2024) | **1.1–3.6×** faster build than PLOC++/ATRBVH at similar quality | build only | M | — |
| 11 | Denoiser: SVGF-class (interactive) + NFOR-class or small CNN (final) | 1 spp interactive; final-frame time ÷ several | ms per frame | M (filter) / L (CNN) | AOV buffers |
| 12 | Null-scattering volumes + ratio tracking + unbiased ray-marching transmittance (Miller et al. 2019; Kettunen et al. 2021) | "up to several orders of magnitude" lower transmittance variance | density lookups | M | majorant grid |
| 13 | Random-walk SSS (Chiang et al. 2016; Wrenninge et al. 2017) | production standard; replaces diffusion profiles | 10s of steps per entry | M | #12 machinery |
| 14 | Hair BCSDF (Chiang et al. 2016; Huang et al. 2022) | production standard | lobe eval + curve intersection | M | curve primitives in BVH |
| 15 | MNEE / SMS / specular polynomials (Hanika 2015; Zeltner 2020; Fan 2024) | caustics through glass that PT cannot converge | Newton solve per connection | L | — |
| 16 | Spectral (hero wavelength, sigmoid upsampling) | correctness (dispersion, metamerism), not speed | ~1.1–1.3× of RGB (our estimate) | M | — |
| 17 | Neural radiance cache (Müller et al. 2021) | **2.6 ms**/frame overhead at 1080p on an RTX GPU | MLP eval at path end | L (hand-written WGSL MLP) | — |
| 18 | Neural path guiding (Dong et al. 2023; Huang et al. 2024) | better than tree guiding at equal samples | MLP eval per bounce | L | #17 machinery |

S = days, M = 1–2 weeks, L = 3+ weeks, for one agent with tests.

---

## 3. Survey by area

### 3.1 ReSTIR family (resampled importance sampling with spatiotemporal reuse)

**What it is.** Resampled importance sampling (RIS) picks one sample from M cheap candidates in
proportion to a target function (for example the unshadowed contribution). It does this in
streaming form with a single-sample *reservoir*. Reservoirs are then merged across neighbouring
pixels and frames. The cost of tracing one shadow ray is amortised over the information gathered
by many candidates.

- **ReSTIR DI.** [Bitterli et al. 2020, *Spatiotemporal reservoir resampling for real-time ray
  tracing with dynamic direct lighting*](https://cs.dartmouth.edu/wjarosz/publications/bitterli20spatiotemporal.html).
  The authors report equal error **6–60× faster** than the prior state of the art with the
  unbiased estimator (35–65× with the biased one), up to 3.4 M emissive triangles in under 50 ms
  per frame. Cost: one candidate-generation pass, a temporal and a spatial reuse pass, and one or
  two visibility rays. Complexity M. The difficult parts are unbiased MIS weights for reuse and
  temporal reprojection. Reference code: NVIDIA RTXDI SDK (NVIDIA licence; not to be read).
- **GRIS / ReSTIR PT.** [Lin et al. 2022, *Generalized Resampled Importance Sampling: Foundations
  of ReSTIR*](https://d1qx31qr3h6wln.cloudfront.net/publications/sig22_GRIS.pdf)
  ([project](https://dqlin.xyz/pubs/2022-sig-GRS/)). This is the theory that makes reuse across
  correlated samples correct (shift mappings, generalized balance heuristic) and extends it to full
  paths. Example from the paper: MAPE 0.39 for ReSTIR PT, 0.45 for ReSTIR GI and 1.63 for PT on
  the Carousel scene. Complexity L, because shift mappings (hybrid reconnection/random replay)
  must be implemented for every BSDF lobe.
- **ReSTIR GI.** [Ouyang et al. 2021](https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing).
  Resamples the secondary vertex of indirect paths. The authors report MSE **9.3–166×** lower than
  PT at 1 spp. It is simpler than PT and a good second step after DI.
- **Area ReSTIR.** Zhang et al. 2024, *Area ReSTIR: Resampling for Real-Time Defocus and
  Antialiasing* (TOG 43(4)) extends reuse to depth of field and
  sub-pixel detail. **Reservoir splatting.** [Liu et al. 2025](https://research.nvidia.com/labs/rtr/publication/liu2025splatting)
  forward-projects reservoirs, which fixes temporal reuse failures and supports motion blur. The
  authors report better resampling quality than Area ReSTIR at up to 10 % lower cost. A 2026
  multi-layer follow-up handles disocclusion
  ([Hong et al.](https://research.nvidia.com/labs/rtr/publication/hong2026multilayer/)).
- **Conditional ReSTIR.** Kettunen et al. 2023 apply resampling to final gathering after specular
  bounces and fast camera motion ([overview table](https://en.wikipedia.org/wiki/Spatiotemporal_reservoir_resampling)).
- **ReSTIR BDPT.** [Hedstrom et al. 2025, *Bidirectional ReSTIR Path Tracing with Caustics*](https://dl.acm.org/doi/10.1145/3744898).
  Adds light subpaths so that resampling also covers caustics. The authors report MAPE 0.312 in
  70 ms at 1080p with 1 M light subpaths, lower than ReSTIR PT.
- **Volumes.** Lin et al. 2021, *Fast Volume Rendering with Spatiotemporal Reservoir Resampling*
  (SIGGRAPH Asia), extends ReSTIR DI to volumetric NEE.

**Offline vs interactive.** ReSTIR was designed for real time, where it is biased in practice
(temporal reuse). For final-frame rendering, its unbiased variants are still a win on many-light
direct lighting, because each spatial reuse pass is cheap compared with a shadow ray. Our use:
interactive preview (R5) and the GPU final-frame path, with a mode that disables temporal reuse
for still frames and keeps spatial reuse with unbiased weights.

### 3.2 Many-light sampling

- **Light BVH with orientation cones.** Conty Estevez & Kulla 2018, *Importance Sampling of Many
  Lights with Adaptive Tree Splitting* (HPG / PACMCGIT) (Sony Imageworks
  production renderer). A BVH over emitters stores bounds, power and an orientation cone, and NEE
  descends stochastically using an importance bound. Adaptive splitting traverses both children
  near the shading point. This is the production baseline. Complexity M on the CPU; the GPU
  version uses the same flat node array.
- **Dynamic many-light sampling on GPUs.** [Moreau, Pharr & Clarberg 2019](https://research.nvidia.com/sites/default/files/pubs/2019-07_Dynamic-Many-Light-Sampling/MPC19.pdf)
  covers a GPU light BVH that is rebuilt and refitted every frame.
- **Accurate SG lighting for the tree.** [Tokuyoshi et al., SIGGRAPH Asia 2024, *Hierarchical
  Light Sampling with Accurate Spherical Gaussian Lighting*](https://gpuopen.com/download/publications/Hierarchical_Light_Sampling_with_Accurate_Spherical_Gaussian_Lighting.pdf).
  Replaces the cone importance with a spherical-Gaussian estimate of each cluster's
  contribution, including glossy BSDFs, while staying a valid importance function. This is the
  best published tree importance; it is a drop-in upgrade to the node importance function.
- **ReGIR** (world-space reservoir grid, Boksansky et al., *Ray Tracing Gems II*, 2021) gives
  world-space candidate lists that also serve secondary bounces where screen-space ReSTIR cannot
  reach.
- **Learned light selection.** Vévoda et al. 2018, *Bayesian online regression for adaptive direct
  illumination sampling* (SIGGRAPH), learns per-region light-selection probabilities online. It is
  reported as used in Corona, a V-Ray competitor. It pairs well with the light tree as a
  correction on top of it.

**Slot:** new `craft-render-lights` crate (tree build, importance, sample/pdf), consumed by the
integrator's NEE and by ReSTIR candidate generation. Without the tree, ReSTIR candidates come from
power sampling and lose most of their advantage in scenes with many small lights. **The tree comes
before, or together with, ReSTIR DI.**

### 3.3 MIS refinements (cheap wins)

- **MIS compensation.** Karlík, Šik, Vévoda, Skřivan, Křivánek 2019, *MIS compensation:
  optimizing sampling techniques in multiple importance sampling* (SIGGRAPH Asia). Subtracting the
  mean from the environment-map sampling density before MIS gives a sharper light-sampling
  technique, at almost no cost. It is a few lines in the environment importance map builder.
- **Optimal MIS.** Kondapaneni et al. 2019, *Optimal multiple importance sampling* (SIGGRAPH),
  shows weights that can be negative and estimated online. Worth it only where two techniques are
  both poor, for example glossy surfaces under a large area light. Medium priority.

### 3.4 Path guiding

- **Practical path guiding (PPG).** Müller, Gross & Novák 2017 (EGSR) use an SD-tree: a spatial
  binary tree with directional quadtrees, refined over power-of-two iterations. Production renderers
  use it today, and it is robust and simple. Follow-ups: Müller's 2019 SIGGRAPH course notes (a
  better spatial filter and DS-MIS), Rath et al. 2020, *Variance-aware path guiding* (SIGGRAPH)
  (guide toward the variance-optimal density, not the radiance), and Ruppert, Herholz & Lensch
  2020 (*Robust fitting of parallax-aware mixtures for path guiding*, vMF mixtures with parallax
  compensation). Intel's Open PGL (Apache-2.0) implements this family; it is C++ and we neither
  read nor link it.
- **Product guiding** with the BSDF, using mixtures of von Mises–Fisher lobes or normalized
  anisotropic spherical Gaussians, avoids guiding into directions the BSDF rejects.
- **Neural guiding.** [Dong, Wang & Li 2023, *Neural Parametric Mixtures for Path Guiding*](https://neuropara.github.io)
  ([arXiv 2504.04315](https://arxiv.org/abs/2504.04315)) uses a hash-grid encoding with a tiny MLP
  that outputs vMF mixture parameters, trained online from noisy MC estimates. The authors report
  it beats tree-based guiding at equal sample count. [Huang et al. 2024, *Online Neural Path
  Guiding with Normalized Anisotropic Spherical Gaussians*](https://arxiv.org/abs/2303.08064)
  (TOG) trains with no warm-up phase on a wavefront GPU path tracer. [Rath et al., EGSR 2025,
  *Neural Resampling with Optimized Candidate Allocation*](https://diglib.eg.org/bitstreams/453bbb95-6f11-4e2b-aeb2-7b3335e0fb2a/download)
  learns the incident integrand and is designed to use idle GPU resources.
- **Real-time guiding.** [Lu et al. 2024, *Real-Time Path Guiding Using Bounding Voxel Sampling*
  (VXPG)](https://par.nsf.gov/servlets/purl/10584751) builds an irradiance voxel structure and
  reports significantly lower perceptual error than other real-time guiding and VPL methods at
  equal time.

**Cost.** Tree guiding costs one tree lookup and a mixture sample per bounce, plus a training
splat. CPU memory is tens of MB. On the GPU, the SD-tree build is awkward (per-iteration
rebuild); a hash-grid of vMF mixtures is more natural. Neural guiding needs fused-MLP training
in WGSL, which is feasible but is the single hardest kernel in this plan.

**Slot:** a new `craft-render-guide` crate with a `Guide` trait (`sample(x, n, wo) → (wi, pdf)`,
`pdf`, `splat(x, wi, L)`), used by the integrator's BSDF-sampling branch with one-sample MIS
between BSDF and guide.

**Path guiding vs ReSTIR GI/PT.** They overlap but are not substitutes. Guiding improves *every*
sample everywhere and suits offline final frames on the CPU and GPU. ReSTIR GI/PT amortises across
pixels and suits interactive and GPU work. For "beat V-Ray/Arnold on final frames", guiding is
the one that matters (both competitors ship some form of path guiding or light caching). For the
interactive viewport, ReSTIR matters more. **Recommendation: guiding first for final frames,
ReSTIR GI second for the viewport, ReSTIR PT only if measurements show the gap is worth its
complexity.**

### 3.5 Online caches without an ML runtime

- **Neural radiance caching (NRC).** [Müller et al. 2021](https://arxiv.org/abs/2106.12372). Paths
  terminate early into a small MLP that is trained every frame from longer training paths. The
  authors report about **2.6 ms** per frame at 1080p for updates and queries, with little induced
  bias. They relied on a fully fused CUDA MLP. In WGSL we would hand-write a 64-wide MLP using
  workgroup memory. That is doable but slow without tensor cores, because wgpu exposes no
  cooperative-matrix path for us (any such feature would be experimental, so it would need `unsafe`).
- **Hash-grid radiance caches (no network).** Binder, Fricke & Keller 2019, *Massively parallel
  path space filtering* (MCQMC proceedings) and NVIDIA's spatially hashed radiance cache (SHaRC,
  RTX SDK) store radiance in a jittered spatial hash keyed by position, normal and level of detail,
  and look it up at path termination or for guiding. This gives most of the NRC benefit with plain
  atomics and no training. **Recommended before NRC.** It also provides the per-region radiance and
  variance estimates that EARS (3.6) and guiding need.
- **No ML runtime needed.** Everything above runs as WGSL compute plus CPU Rust. If a framework
  becomes desirable later, [Burn](https://github.com/tracel-ai/burn) (MIT/Apache-2.0, pure Rust,
  wgpu backend) is the candidate. Check its licence and the craft-libs allowlist first.

### 3.6 Variance reduction: RR/splitting, control variates

- **EARS.** Rath, Grittmann, Herholz, Weier, Slusallek 2022, *EARS: Efficiency-Aware Russian
  Roulette and Splitting* (SIGGRAPH). Chooses the RR/splitting factor per vertex to maximise
  efficiency using cached estimates of the pixel's variance and cost, and reports large
  efficiency gains over standard RR on top of guiding. It needs the cache from 3.5. **High value
  for the effort once a cache exists.**
- **Control variates.** Müller et al. 2020, *Neural Control Variates* (SIGGRAPH Asia)
  is heavy (learned CV plus learned sampling). Salaün et al. 2022, *Regression-based Monte Carlo
  integration* (SIGGRAPH), fits a polynomial CV per pixel from the samples themselves, which is
  cheap but helps mostly at low dimension. The CV that pays off for us is a narrow one: the ratio
  estimator of Heitz et al. 2018, *Combining analytic direct illumination and stochastic shadows*
  (I3D). It computes unshadowed area-light shading analytically (LTC) and only estimates the
  shadow ratio stochastically, so it is great for the viewport. CVs also appear inside the
  transmittance estimator in 3.11.
- **Firefly handling.** Zirr et al. 2018, *Re-weighting firefly samples for improved finite-sample
  Monte Carlo estimates* (CGF), is a consistent, bounded-bias outlier treatment. It beats
  clamping, which is biased and non-converging.

### 3.7 Spectral rendering

- **Hero wavelength sampling.** Wilkie et al. 2014 (EGSR) carry 4 wavelengths per path; the first
  drives sampling, and the others are combined with spectral MIS. The cost is a `vec4` instead of
  an `Rgb`, about the same in SIMD lanes. Dispersive interfaces collapse to the hero wavelength.
- **RGB→spectrum upsampling.** Jakob & Hanika 2019, *A Low-Dimensional Function Space for
  Efficient Spectral Upsampling* (CGF), stores 3 sigmoid-polynomial coefficients per texel, from a
  precomputed table. Evaluation is a few FMAs. This is what makes spectral practical with RGB
  textures.
- **Cost (our estimate, to be measured):** 10–30 % over RGB per sample, from upsampling and
  per-wavelength Fresnel. Colour noise at very low spp is the real cost; hero wavelength with four
  wavelengths largely hides it. Weta's Manuka shows spectral transport is production-viable
  (Fascione et al. 2018, *Manuka: A batch-shading architecture for spectral path tracing*, TOG).
- **What it buys:** dispersion, thin-film and iridescence, fluorescence later, and correct
  colour under coloured lights. Arnold and V-Ray are RGB renderers by default, so this is a
  quality differentiator, not a speed one. Make it a compile-time integrator variant sharing all
  code through a `Spectrum` trait.

### 3.8 Sampling: QMC, blue noise, lattices

- **Owen-scrambled Sobol'.** The engine already uses Burley 2020 (*Practical hash-based Owen
  scrambling*, JCGT) with padding. Keep it. It is the right default: randomized, progressive,
  and convergence close to O(N^-1.5) on smooth 2D integrands.
- **Screen-space blue-noise error.** Ahmed & Wonka 2020, *Screen-space blue-noise diffusion of
  Monte Carlo sampling error via hierarchical ordering of pixels* (SIGGRAPH Asia, the "z-sampler")
  maps pixels along a Morton curve with a scrambled shared sequence. This gives blue-noise error at
  zero runtime cost and all sample counts. Heitz & Belcour 2019 (*Distributing Monte Carlo errors as
  a blue noise in screen space by permuting pixel seeds temporally*, EGSR) is an alternative
  approach. Georgiev & Fajardo 2016 (*Blue-noise dithered sampling*, SIGGRAPH Talks) is the Arnold
  team's version. MSE is unchanged, but perceived noise and denoiser input quality improve a lot
  at 1–16 spp. **Cheap, do it early.**
- **Rank-1 lattices.** Liu, Han & Jiang 2021, [*Rank-1 lattices for efficient path integral
  estimation*](https://diglib.eg.org:443/handle/10.1111/cgf142617) (CGF). The authors report
  variance comparable to or lower than Sobol', with lattices found by an offline search. That is
  not enough reason to switch, but it is a cheap alternative sampler to keep for experiments.
  pmj02 (Christensen, Kensler & Kilpatrick 2018) is another validated progressive alternative.
- **Learned point sets.** See section 5: [MPMC (Rusch et al., PNAS 2024)](https://arxiv.org/abs/2405.15059)
  and [LLM-evolved Sobol' direction numbers (Sadikov 2025)](https://arxiv.org/abs/2510.03650).
  Both are candidates to test in our sampler harness, not to adopt blind.

### 3.9 BVH construction and traversal

- **Wide, compressed nodes.** [Ylitie, Karras & Laine 2017, *Efficient Incoherent Ray Traversal on
  GPUs Through Compressed Wide BVHs*](https://research.nvidia.com/publication/2017-07_efficient-incoherent-ray-traversal-gpus-through-compressed-wide-bvhs)
  use 8-wide nodes with quantized child boxes and an ordered traversal. The authors report
  **1.9–2.1×** faster incoherent traversal than the prior state of the art, with 35–60 % of the
  memory. Because we trace in software (1.1), this is the most direct GPU speedup available. The
  CPU equivalent is a 4- or 8-wide BVH with SIMD slab tests in portable Rust (`std::simd` is not
  stable; write lane-parallel scalar code that autovectorises, or use the `wide` crate if the
  allowlist permits).
- **GPU builders.** PLOC (Meister & Bittner 2018), PLOC++ (Benthin et al. 2022) and
  [**H-PLOC** (Benthin et al. 2024)](https://gpuopen.com/download/publications/HPLOC.pdf). H-PLOC
  is a single-kernel hierarchical build guided by an LBVH, reported **1.1–3.6×** faster than
  PLOC++/ATRBVH for the whole build at similar BVH quality. It relies on subgroup operations, which
  is a good fit for `Features::SUBGROUP`. It matters for interactive edits and animation (rebuild
  every frame). Final frames can keep the CPU binned-SAH plus spatial-split builder (Stich et al.
  2009, SBVH).
- **Two-level.** Instancing (R8) needs a TLAS over BLASes in our own format. Plan the node format
  so that the same traversal kernel handles both levels.
- **Curves.** Hair (3.14) needs curve primitives in the BVH: ribbons or tubes over cubic Bézier
  segments, with oriented bounds per segment. Reshetov & Luebke 2018, *Phantom Ray-Hair
  Intersector* (HPG), is the published GPU curve intersector.

### 3.10 Denoising without vendor SDKs

- **Interactive:** SVGF (Schied et al. 2017, HPG) is an à-trous wavelet filter steered by temporal
  variance and G-buffer edges; A-SVGF (Schied et al. 2018) adds adaptive temporal accumulation.
  Both are pure compute shaders, cost a few ms at 1080p, and need no training. They are the right
  first denoiser for R5.
- **Final frames, non-neural:** Bitterli et al. 2016, *Nonlinearly Weighted First-order Regression
  for Denoising Monte Carlo Renderings* (NFOR, EGSR), uses feature regression with no training data.
  It is competitive with early CNN denoisers on offline frames, runs on the CPU, and is
  embarrassingly parallel.
- **Final frames, neural:** kernel-predicting CNNs (Bako et al. 2017; Vogels et al. 2018), and
  lighter recent designs such as Balint et al. 2023, *Neural Partitioning Pyramids for Denoising
  Monte Carlo Renderings* (SIGGRAPH), which is fast and high quality. Inference in hand-written
  WGSL (conv + pool + upsample) is a contained piece of work; training happens offline on renders
  of **our own** scenes. Intel Open Image Denoise is Apache-2.0, and so are its
  [trained weights](https://github.com/RenderKit/oidn). Using another product's weights is a
  clean-room question for the user to decide. This plan assumes we train our own.
- **Slot:** `craft-render-denoise` takes beauty plus albedo, normal, depth, motion and a variance
  AOV. The integrator must output first-hit albedo and normal AOVs; that is cheap, so add it now.

### 3.11 Volumes: null scattering, delta and ratio tracking

- **Framework.** Novák et al. 2018, *Monte Carlo methods for volumetric light transport simulation*
  (Eurographics STAR), and Miller, Georgiev & Jarosz 2019, *A null-scattering path integral
  formulation of light transport* (SIGGRAPH). Delta tracking samples free paths against a majorant.
  Ratio tracking estimates transmittance for NEE. The null-scattering formulation makes both
  MIS-compatible, including spectral MIS for chromatic media.
- **Transmittance.** [Kettunen et al. 2021, *An unbiased ray-marching transmittance
  estimator*](https://arxiv.org/abs/2102.10294) is mostly ray marching with a rarely evaluated
  power-series correction and a control variate. The authors report variance "up to several orders
  of magnitude" lower than prior unbiased estimators at equal cost, and zero variance for constant
  extinction. **Use it for all shadow-ray transmittance.**
- **Majorants.** A coarse min/max grid over the density (our own sparse format; OpenVDB is an open
  specification we can read the *docs* of) keeps majorants tight. Tight majorants decide delta
  tracking's speed.
- **Spectral and chromatic media:** spectral MIS (Miller et al. 2019) or hero-wavelength tracking.
- **Guiding in volumes:** Herholz et al. 2019, *Volume path guiding based on zero-variance random
  walk theory* (TOG), after surface guiding works.

### 3.12 Subsurface scattering

- **Random walk.** Chiang, Kutz & Burley 2016, *Practical and controllable subsurface scattering
  for production path tracing* (SIGGRAPH Talks), maps user-facing albedo and radius to volume
  coefficients by a fitted inversion and runs a volumetric random walk inside the closed mesh.
  Wrenninge, Villemin & Hery 2017 (Pixar technical memo 17-07) adds anisotropic phase functions and
  non-exponential free flights for better control. It is the current production standard; Arnold
  uses a random-walk SSS mode. OpenPBR specifies subsurface parameters in this style.
- **Cost:** tens of steps per entry. Use Dwivedi-style guided walks (Křivánek & d'Eon 2014,
  *A zero-variance-based sampling scheme for Monte Carlo subsurface scattering*, SIGGRAPH Talks)
  to cut walk length in high-albedo media.
- **Slot:** reuses the volume tracking code (3.11) with a homogeneous medium bounded by the mesh.
  Needs robust inside/outside handling, for example on non-manifold meshes, which ModelCraft can
  produce, so validate with the half-edge checker first.

### 3.13 Caustics

- **MNEE.** Hanika, Droske & Fascione 2015, *Manifold Next Event Estimation* (EGSR), used at Weta.
  It connects NEE through one or a few refractive interfaces by Newton iteration on the specular
  constraint, which covers shadow caustics through glass and water.
- **SMS.** Zeltner, Georgiev & Jakob 2020, *Specular Manifold Sampling for Rendering
  High-Frequency Caustics and Glints* (SIGGRAPH), gives unbiased manifold walks with random
  initialisation and probability estimation. Manifold path guiding (Fan et al. 2023, TOG) guides
  the seeds.
- **Deterministic.** [Fan et al. 2024, *Specular Polynomials*](https://arxiv.org/abs/2405.13409)
  (SIGGRAPH) finds *all* admissible specular chains between two endpoints via resultants, with
  nearly noise-free caustics, at high cost per query.
  [Bernstein Bounds for Caustics (2025)](https://arxiv.org/abs/2504.19163) cuts the cost of that
  deterministic search.
- **Photon-based:** SPPM (Hachisuka & Jensen 2009) and VCM/UPS (Georgiev et al. 2012;
  Hachisuka et al. 2012) are the robust general solutions. V-Ray offers photon-mapped caustics.
- **Recommendation:** MNEE for single-interface shadow caustics (most of the visible cases: glass
  objects, pools), then SPPM as an opt-in caustics pass. Specular polynomials are research-grade
  for now.

### 3.14 Hair

- **BCSDF.** Chiang et al. 2016, *A Practical and Controllable Hair and Fur Model for Production
  Path Tracing* (EGSR; Disney): R/TT/TRT plus residual lobes, with a fitted azimuthal roughness
  and artist parameters built on d'Eon et al. 2011's energy-conserving model. Huang et al. 2022,
  *A microfacet-based hair scattering model* (EGSR), adds elliptical fibres and microfacet
  reflection. Pick Chiang 2016 first. Its parameters match what users of Arnold's standard hair
  expect.
- **Cost:** BCSDF evaluation is cheap. The cost is geometry (millions of curve segments) and
  multiple scattering in light hair. Path guiding helps there too.

### 3.15 Path-space reuse at scale (for later)

Photon mapping, VCM and MCMC methods (for example [path-space partitioning and guided image
sampling for MCMC, 2025](https://arxiv.org/pdf/2501.06214)) are robust on pathological transport
but complex. They are out of scope until the core plan is measured.

---

## 4. Reference-code licensing (not to be read; recorded for provenance)

| Technique | Known reference code | Licence | Status for us |
|---|---|---|---|
| ReSTIR DI/GI | NVIDIA RTXDI SDK | NVIDIA RTX SDK licence | do not read |
| Path guiding | Intel Open PGL | Apache-2.0 | do not read, do not link (C++) |
| PPG original | Müller's code on a Mitsuba base | Mitsuba-family licence | banned by rules |
| VXPG | [authors' repo](https://github.com/SuikaSibyl/vxpg) | not checked | do not read |
| Neural guiding (NPM) | project page | not checked | do not read |
| NRC | tiny-cuda-nn (NVIDIA) | NVIDIA licence, not checked | do not read |
| H-PLOC | AMD GPUOpen paper; code status not checked | — | do not read |
| Denoiser | Intel OIDN code and weights | Apache-2.0 | do not read; using the weights needs the user's decision |
| MPMC | [tk-rusch/MPMC](https://github.com/tk-rusch/MPMC) | not checked | do not read; regenerate points ourselves |
| LLM-evolved QMC | [openevolve-star-discrepancy](https://github.com/hockeyguy123/openevolve-star-discrepancy) | not checked | tables are data; check licence before using any table |
| openai/math | [openai/math](https://github.com/openai/math) | Apache-2.0 | mathematical manuscripts, not renderer code; fine to read |

"Not checked" means nobody looked at the repository, by design. If anyone ever needs a licence
answer for one of them, a person outside the render workstream should check it.

---

## 5. AI math and reasoning models: what is real

The user asked for recent results from AI math and reasoning models that could make rendering
cheaper. Each item is labelled **verified** (primary source exists and the claim was checked by
named humans or a proof checker) or **rumour** (claimed, retracted, or not traceable to a primary
source). "Relevance" is my judgement for this engine.

| Finding | Status | Source | Relevance to rendering |
|---|---|---|---|
| AlphaEvolve (Google DeepMind, May 2025): 4×4 complex matrix multiplication with 48 scalar multiplications; kissing-number lower bound in 11 dimensions raised from 592 to 593; improved Gemini kernel tiling | **Verified** for the mathematical constructions: they are explicit and checkable. The kernel-speed figures are Google's own and unverified by us | [DeepMind blog](https://deepmind.google/blog/alphaevolve-a-gemini-powered-coding-agent-for-designing-advanced-algorithms/) | None directly. Our 4×4 transforms are real-valued, and naive SIMD beats 48-mult schemes at this size. The *method* (LLM-guided program search against an automatic evaluator) is relevant; see the last two rows |
| Georgiev, Gómez-Serrano, Tao, Wagner, *Mathematical exploration and discovery at scale* (Nov 2025): AlphaEvolve on 67 problems; matched most, improved several | **Verified** (arXiv paper with Tao as co-author) | [arXiv 2511.02864](https://arxiv.org/abs/2511.02864) | Indirect. Includes packing and point-configuration problems. No rendering result |
| GPT-5 Pro improves the step-size condition for a gradient-descent convergence property on smooth convex functions from 1/L to 1.5/L (Aug 2025) | **Verified** correct by Bubeck and reported in OpenAI's paper. A later human revision of the source paper went further, so it is not the final word | [Early science acceleration experiments with GPT-5, arXiv 2511.16072](https://arxiv.org/abs/2511.16072) | Marginal. Step-size theory for convex solvers (UV/LSCM energy minimisation), not rendering |
| "GPT-5 solved 10 open Erdős problems" (Oct 2025) | **Rumour, retracted.** The model had found existing published solutions, which is literature search, not new proofs. The claim post was deleted | [eWeek summary](https://www.eweek.com/news/openai-math-hype/) | None |
| GPT-5.2 Pro solutions to Erdős problems #728 and #397 (Jan 2026), formalised in Lean with Harmonic's Aristotle | **Verified** (Lean proof; acknowledged by Tao, who called them low-hanging fruit) | [The Decoder](https://the-decoder.com/terence-tao-says-gpt-5-2-pro-cracked-an-erdos-problem-but-warns-the-win-says-more-about-speed-than-difficulty/) | None (number theory) |
| Later claims (GPT-5.4 Pro on FrontierMath, an OpenAI "planar unit distance" proof) | **Rumour**: secondary coverage only | [The Neuron round-up](https://www.theneuron.ai/explainer-articles/from-erdos-to-axiom-the-open-problems-ai-has-actually-solved/) | None known |
| openai/math (Oct 2026): 721 manuscripts in 373 families from an unreleased OpenAI model; Lean formalisation for part of them | **Partly verified.** Formalised main results are machine-checked. The README itself warns that unformalised results may have issues | [github.com/openai/math](https://github.com/openai/math) | See section 6. Nothing directly usable; three results are informative |
| MPMC: graph neural networks generate low-discrepancy point sets (PNAS 2024). Reported optimal or near-optimal discrepancy for small N in low dimensions | **Verified** (peer-reviewed). This is ML, not a reasoning model | [arXiv 2405.15059](https://arxiv.org/abs/2405.15059) | **Testable.** Fixed small-N 2D tables (for example 16–256 points for pixel-filter or area-light stratification). Not progressive, so not a Sobol' replacement |
| LLM-guided evolutionary search (OpenEvolve, an open AlphaEvolve-style system) finds 2D/3D point sets with low star discrepancy and Sobol' direction numbers that reduce randomized-QMC error (Oct 2025; AAAI 2026 workshop) | **Published, not independently reproduced.** Not checked by us | [arXiv 2510.03650](https://arxiv.org/abs/2510.03650) | **Most relevant item.** Direction numbers are drop-in data for our Sobol' generator. Test them: measure error on our integrand suite against Joe–Kuo numbers before adopting |

**Bottom line.** No AI-model result to date gives a rendering algorithm that is cheaper than the
human literature above. The real, usable contributions are in **sampling tables** (Sobol'
direction numbers, small point sets), which are cheap to test because they are data, not code.
The methodology is also usable: we could run an evaluator-in-the-loop search ourselves, for
example for light-tree importance heuristics or BVH build heuristics, scored by our own
benchmark harness. That is a research project in its own right, not part of the core plan.

---

## 6. openai/math: applicability

Scope checked: the repository's manuscript map (`CONTENTS.md`, 373 families) was searched for
discrepancy, quasi-Monte Carlo, lattice rules, quadrature and cubature, importance sampling,
Markov-chain mixing, random walks, packing and covering, convex geometry, optimal transport,
least squares and linear solvers, meshes and geodesics, blue noise and point processes. Lean
status comes from [`lean/formalization.yaml`](https://github.com/openai/math/blob/main/lean/formalization.yaml)
and the per-family scope files under [`lean/docs/`](https://github.com/openai/math/tree/main/lean/docs).
For the two most promising families (090, 238) the manuscript text itself was read, not only the
abstract.

**Plainly: nothing in the collection is directly implementable in a renderer, a sampler, a
geometry-processing tool or a UV packer.** There are no results on discrepancy, QMC, lattice
integration rules, cubature error bounds, importance sampling, sparse least squares or mesh
processing. Three results are informative, meaning they justify or warn against a design choice.
The rest of the candidates are not applicable.

| Family | Result | Lean status | Verdict | Use |
|---|---|---|---|---|
| [090](https://github.com/openai/math/blob/main/lean/docs/090.md) | Triangular lattice is universally energy-optimal in the plane (every completely monotone potential of squared distance, including Gaussians and inverse powers); sharp Cohn–Elkies certificate for circle packing; Coulomb and Riesz renormalised-energy minimality and the spherical log-energy linear term | Formalised: universal energy minimality, Gaussian minorants, packing certificate. **Not** in the formalised scope: the Coulomb/Sandier–Serfaty and spherical log-energy parts | **Informative** | Blue-noise generators that minimise Gaussian or Riesz pair energies (for example Gaussian blue noise, Ahmed et al. 2022) have the hexagonal lattice as their infinite-N optimum. Optimising those energies further drives point sets toward a regular hex pattern, which aliases. Keep randomization (Owen scrambling, jitter) rather than chasing lower energy. The theorem is about infinite configurations only; it gives no finite-N construction or bound |
| [238](https://github.com/openai/math/blob/main/lean/docs/238.md) | Thorp shuffle on 2^d items mixes the full permutation in Θ(d) rounds (upper bound 1600·d, asymptotic as d → ∞; lower bound 2d − O(1)) | Formalised (main theorem and supporting results) | **Informative** | A Thorp round is a GPU-friendly keyed permutation: each round pairs index i with i + N/2 and swaps on a hash bit. It could serve as a stateless random-access shuffle of sample or light indices. The theorem proves O(log N) rounds suffice in principle, but the constant (1600·d) and the asymptotic form give no practical round count. Rendering needs only statistical decorrelation, which the existing hash-based Owen scrambling already gives. No expected gain |
| [374](https://github.com/openai/math/blob/main/lean/docs/374.md) (with [360](https://github.com/openai/math/blob/main/lean/docs/360.md)) | Brenier (optimal transport) maps from a uniform convex source are only 1/3-Hölder stable in W2, and the exponent is sharp; 360: weak MTW curvature gives convex injectivity domains and bi-Hölder OT maps on manifolds | Formalised (both) | **Informative** | OT-based sampling (blue noise through optimal transport, sliced-OT sampling) and OT-based UV/parameterisation methods: a small change in the target density can move the transport map by about the cube root of the change. Expect temporal flicker if OT-warped sample sets are recomputed per frame from a slowly changing density; prefer fixed sets |
| [118](https://github.com/openai/math/blob/main/lean/docs/118.md) | Bin packing: integral optimum can exceed the configuration-LP value by any additive constant; fixed-additive approximation is NP-hard | Formalised | **Not applicable** (mildly informative) | It concerns 1D bin packing. UV island packing is 2D irregular packing and is heuristic anyway. It confirms that LP lower bounds cannot certify near-optimal packings, so judge packers by measured utilisation, not optimality gaps |
| [092](https://github.com/openai/math/blob/main/lean/docs/092.md) | Optimal worst-case covering density of convex bodies is Θ(n log n) (lattice and translative) | Formalised | Not applicable | Asymptotic in dimension n. BVH and partition bounds live in n = 3 with constants that matter |
| [130](https://github.com/openai/math/blob/main/lean/docs/130.md) | Exact DFT in O(n (log n)^(1−10⁻¹³)) operations | Formalised | Not applicable | δ = 10⁻¹³ in an exact-arithmetic model; it gives no speedup at any practical n. FFT convolution in denoisers stays standard |
| [107](https://github.com/openai/math/blob/main/lean/docs/107.md) | Matrix multiplication exponent ω ≤ 9/4 over ℂ | Formalised | Not applicable | Asymptotic ("galactic") result; nothing for small dense matrices or for GPU kernels |
| [139](https://github.com/openai/math/blob/main/lean/docs/139.md) | Well-conditioned log-concave sampling needs only C_ε·d^ε gradient queries | Formalised | Not applicable | Light-transport integrands are not log-concave, and the oracle model does not match MC rendering |
| [113](https://github.com/openai/math/blob/main/lean/docs/113.md), [115](https://github.com/openai/math/blob/main/lean/docs/115.md), [131](https://github.com/openai/math/blob/main/lean/docs/131.md) | Approximate counting and exact uniform sampling of perfect matchings, contingency tables and degree-constrained graphs; switch-chain mixing | 113, 115 formalised; 131: chain mixing formalised, the exact sampler is not | Not applicable | Discrete combinatorial sampling, with no continuous-integration analogue we use |
| [104](https://github.com/openai/math/blob/main/lean/docs/104.md) | Quasipolynomial algorithms for mean-payoff, stochastic and parity games | Formalised (randomized mean-payoff algorithm) | Not applicable | Verification and game theory |
| [211](https://github.com/openai/math/blob/main/lean/docs/211.md) | Random walks on random planar maps converge to Liouville Brownian motion | Only the continuum-random-tree part is formalised; the random-walk results are not | Not applicable | Unrelated to random-walk SSS or walk-on-spheres, which concern walks in Euclidean media with known coefficients |

Searches that found nothing usable: discrepancy (one hit, in algebraic geometry), quasi-Monte
Carlo or Monte Carlo (none), quadrature or cubature (none), importance sampling (none), least
squares, preconditioning or linear solvers (none), meshes (only lattice-model probability),
geodesics (existence theorems on Riemannian manifolds, family 345, no Lean file).

---

## 7. Ranked adoption plan

Ranking key: (expected efficiency gain on our target scenes) ÷ (effort), with dependencies
respected. Phases are ordered; items inside a phase can run in parallel. The spine follows the
expected path, ReSTIR DI → light tree → guiding / ReSTIR GI → denoiser, with two corrections from
this survey: **the light tree comes before ReSTIR DI, not after**, because RIS candidates are only
as good as their source distribution; and **cheap sampler and MIS fixes come first**.

### Phase 0: foundations (days; enables honest measurement)

1. **Benchmark harness and reference images** (section 8). This must exist before any technique
   is "adopted". Every later item merges with an error-vs-time plot.
2. **AOV outputs** (first-hit albedo, normal, depth, motion, per-pixel variance). Needed by the
   denoiser and by ReSTIR reprojection.
3. **z-sampler blue-noise pixel ordering** on the existing Owen-Sobol' sampler. Zero cost, large
   perceptual gain at low spp.
4. **Environment-map importance sampling with MIS compensation.** Small effort, a direct win on
   every exterior and studio-lit scene.

### Phase 1: direct lighting (1–3 weeks)

5. **Light BVH** (Conty Estevez & Kulla 2018) with SG importance (Tokuyoshi et al. 2024) as a
   later refinement, shared CPU/GPU node format. Expected: orders-of-magnitude variance reduction
   on scenes with hundreds or more emitters, and neutral on few-light scenes.
6. **ReSTIR DI** on the GPU path: candidates from the light tree, spatial reuse with unbiased
   weights, temporal reuse only in the interactive preview. Expected: the paper's 6–60× at equal
   error over light-tree-only NEE should shrink (the tree is a stronger baseline than the paper's),
   so treat 2–10× as the realistic target and measure it.

### Phase 2: indirect lighting (3–6 weeks)

7. **Hash-grid radiance cache** (path-space filtering style). It is a prerequisite for EARS and
   useful for guiding warm-up and early termination.
8. **Path guiding** (SD-tree or hash-grid vMF mixtures with variance-aware targets), CPU first
   with a GPU port. This is the biggest final-frame win on interiors, which is the classic
   V-Ray/Arnold battleground.
9. **EARS** RR/splitting using the cache. A multiplier on top of guiding.
10. **ReSTIR GI** for the interactive viewport.

### Phase 3: speed of the substrate (in parallel with phase 2)

11. **Compressed 8-wide BVH** for GPU traversal and 4/8-wide for the CPU. Reported 1.9–2.1× on
    incoherent rays, which multiplies with everything above.
12. **H-PLOC GPU builder** for interactive rebuilds and animation.

### Phase 4: denoising (2–4 weeks)

13. **SVGF/A-SVGF** for the viewport.
14. **NFOR-class regression denoiser** for final frames (CPU). Later, a **small CNN** trained on our
    own scenes with WGSL inference.

### Phase 5: material and medium breadth (R7; ordered by user value)

15. **Null-scattering volumes** with ratio tracking and the unbiased ray-marching transmittance
    estimator.
16. **Random-walk SSS** (reuses 15).
17. **Hair**: curve primitives in the BVH plus the Chiang 2016 BCSDF.
18. **MNEE** for refractive shadow caustics, then an optional SPPM caustics pass.
19. **Spectral mode** (hero wavelength plus sigmoid upsampling) as an integrator variant.

### Phase 6: research bets (only after phases 0–4 are measured)

20. **ReSTIR PT** (with reservoir splatting) if viewport quality on glossy interreflection still
    lags.
21. **Neural radiance cache / neural guiding** in WGSL, if the hash-grid cache and tree guiding
    plateau.
22. **Evaluated QMC tables**: LLM-evolved Sobol' direction numbers and MPMC small-N sets, A/B
    tested in the sampler harness. Cheap; it can run at any time as a side task.

### Top five by quality per compute per effort

1. **Light BVH** (with environment MIS compensation): the largest gain per line of code on
   realistic lighting.
2. **Path guiding + EARS** (on a hash-grid cache): the largest final-frame gain on indirect-heavy
   scenes.
3. **ReSTIR DI**: large gains in the many-light and interactive cases, built on the tree.
4. **Compressed wide BVH**: about 2× on all GPU rays, because we trace in software.
5. **Denoiser** (SVGF now, regression/CNN for finals): converts all of the above into an earlier
   "done" for the user.

---

## 8. Benchmark protocol: proving "beats V-Ray/Arnold" honestly

### 8.1 What we can and cannot claim

- **Without running V-Ray or Arnold** we can claim only measurements of *our* engine: error vs
  time, our own baselines, and "implements technique X, which is described in the vendor's
  public documentation". We **cannot** claim to match or beat them. No numbers from vendor
  marketing material go next to ours.
- **With licensed copies** run by the user on the same machine: we can publish equal-time and
  equal-quality results on **our own scenes**, with every setting disclosed. First, someone must
  read each product's EULA for benchmark-publication clauses. Some commercial licences restrict
  publishing benchmarks. If publication is restricted, results stay internal.
- A claim is scene-specific: "on scenes A–F, at equal time, our RMSE is lower by N×". There is no
  general "beats" claim.

### 8.2 Scenes (built by us, in the repo, versioned)

Each scene isolates one transport difficulty and can be expressed identically in all three
renderers. That means OpenPBR-class materials (both vendors document OpenPBR support in recent
versions; check before relying on it), standard light types, and no renderer-specific features.

1. Cornell variants: diffuse, glossy, and with a small bright light (MIS stress, after Veach).
2. Sunlit interior through a small window (indirect-dominated; the classic guiding case).
3. Many lights: a night street with 10 k emissive triangles plus 500 point/spot lights.
4. Studio: an HDR dome plus three area lights on glossy, metallic and coated objects.
5. Glass: a refractive object on a diffuse floor under a small light (caustics).
6. Media: a heterogeneous cloud (procedural density) and a skin-like SSS object.
7. Hair: a procedurally generated fur ball with 1 M+ segments.
8. Scale: one of our large meshes (2 M triangles) instanced 1 000 times.

### 8.3 References and metrics

- **Reference per renderer.** Each renderer is measured against **its own** converged image,
  rendered at very high sample counts with all biasing options off. Material and light models
  differ slightly between renderers. Measuring a competitor against *our* reference would
  penalise model differences, not noise. Separately, a side-by-side of the converged images
  documents those model differences.
- **Our reference** is checked for convergence by comparing two independent half-sample renders
  (their difference must be far below the error levels being measured).
- **Metrics:** relative MSE (with a small epsilon in the denominator), and
  FLIP (Andersson et al. 2020, *FLIP: A Difference Evaluator for Alternating Images*, HPG;
  implemented from the paper) for perceptual error. SSIM is reported as a
  secondary metric only. Denoised results are scored separately from raw results, never mixed.
- **Equal time:** error at fixed wall-clock budgets (for example 10 s, 60 s, 5 min), plotted as
  error vs time on log-log axes. This curve is the main artifact.
- **Equal quality:** time to reach a fixed relMSE and a fixed FLIP threshold.
- **Efficiency:** 1 / (relMSE × time) at each budget.
- **Hygiene:** same machine (i9-12900K, RTX 4090), no other workstream builds running, warm caches,
  at least 5 seeds with mean and spread reported, and BVH build time reported separately from
  rendering. Every command and setting goes into `docs/performance.md`. CPU-vs-CPU and GPU-vs-GPU
  results are reported separately; a CPU result is never compared with a GPU result as if they were
  like for like.
- **Regression gate:** each adoption slice from section 7 must show its error-vs-time curve on
  the affected scenes against the previous main. A technique that does not improve the curve on at
  least one target scene without regressing the others does not merge as default-on.

---

## 9. Open decisions for the user

1. **Hardware ray queries.** These require `unsafe` to enable wgpu's experimental feature. Should
   the workspace allow a single audited `unsafe` call behind a cargo feature? The plan does not
   depend on it.
2. **Vendor licences and EULAs** for benchmarking against V-Ray/Arnold (section 8.1).
3. **Denoiser weights.** Train our own (assumed), or allow Apache-2.0 OIDN weights as data.
