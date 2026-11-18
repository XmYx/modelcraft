// The Craft path tracer as one compute kernel. It mirrors craft-render-core statement for
// statement (same sample sequence, same surface model, same estimator); the CPU crate documents
// the sources of each part. One invocation renders `params.samples` samples of one pixel.

struct Params {
    cam_origin: vec4<f32>,   // w: half width of the image plane
    cam_right: vec4<f32>,    // w: half height
    cam_up: vec4<f32>,       // w: aperture radius
    cam_forward: vec4<f32>,  // w: focus distance
    prev_origin: vec4<f32>,  // the previous frame's camera; w: half width
    prev_right: vec4<f32>,   // w: half height
    prev_up: vec4<f32>,      // w: 1 when there is a previous camera
    prev_forward: vec4<f32>, // w: 1 when it is orthographic
    env: vec4<f32>,          // rgb: constant environment radiance, w: probability of choosing the environment
    size: vec2<u32>,
    first_sample: u32,
    samples: u32,
    seed: u32,
    max_depth: u32,
    roulette_start: u32,
    light_count: u32,
    ortho: u32,
    node_count: u32,
    clamp_indirect: f32,
    strategy: u32,        // 0 both estimators (MIS), 1 light sampling only, 2 BSDF sampling only
    z_bits: u32,          // index bits of the z-curve pixel order; 255: every pixel on its own
    light_tree: u32,      // 1: lights with a position are chosen by the light tree
    positional: u32,      // how many light entries have a position
    positional_mass: f32, // probability that one of them is chosen at all
    tree_nodes: u32,      // where the light tree starts in `words` (8 words per node)
    tree_paths: u32,      // where the root-to-leaf turns of every light start in `words` (2 each)
    env_w: u32,           // size of the image of the surroundings; 0: constant environment
    env_h: u32,
    env_pixels: u32,      // where its pixels start in `blob`
    env_marginal: u32,    // where its row table starts in `words`
    env_conditional: u32, // where its per-row column tables start in `words`
    env_turn: f32,
    textures: u32,        // where the texture table starts in `words` (4 words per texture)
    texture_count: u32,
    has_cutouts: u32,
    has_env: u32,
}

struct Node {
    lo: vec3<f32>,
    a: u32,
    hi: vec3<f32>,
    count: u32,
}

struct Tri {
    p0: vec3<f32>,
    pad0: u32,
    e1: vec3<f32>,
    pad1: u32,
    e2: vec3<f32>,
    pad2: u32,
}

struct Attr {
    n0: vec3<f32>,
    material: u32,
    n1: vec3<f32>,
    light: i32, // entry in `lights`, or -1
    n2: vec3<f32>,
    prim: u32, // the triangle's index in the scene
    uv0: vec2<f32>,
    uv1: vec2<f32>,
    uv2: vec2<f32>,
    pad: vec2<f32>,
}

const FLAG_TRANSMISSIVE: u32 = 1u;
const FLAG_CUTOUTS: u32 = 2u;
const FLAG_AXES: u32 = 4u;

struct Mat {
    base: vec3<f32>, // colour × weight
    metalness: f32,
    specular_color: vec3<f32>,
    roughness: f32,
    transmission_tint: vec3<f32>,
    ior: f32,
    coat_color: vec3<f32>,
    coat: f32,
    fuzz_color: vec3<f32>,
    fuzz: f32,
    absorption: vec3<f32>,
    specular_weight: f32,
    base_color: vec3<f32>, // without the weight (the albedo feature of a path that ends at once)
    opacity: f32,
    diffuse_roughness: f32,
    anisotropy: f32,
    transmission: f32,
    coat_roughness: f32,
    coat_ior: f32,
    fuzz_roughness: f32,
    thin_film: f32,
    thin_film_thickness: f32,
    thin_film_ior: f32,
    normal_scale: f32,
    bump_scale: f32,
    flags: u32,
    base_tex: i32, // textures: an index into the texture table, or -1
    rough_tex: i32,
    metal_tex: i32,
    normal_tex: i32,
    opacity_tex: i32,
    bump_tex: i32,
    pad0: i32,
    pad1: i32,
}

struct LightEntry {
    p0: vec3<f32>,
    kind: u32, // 0 triangle, 1 two-sided triangle, 2 point, 3 directional, 4 environment, 5 spot, 6 sun
    e1: vec3<f32>,
    cdf: f32,
    e2: vec3<f32>,
    area: f32,
    radiance: vec3<f32>,
    pmf: f32,
}

// What a pixel accumulates: the image and the feature images a denoiser reads.
struct Pixel {
    radiance: vec4<f32>, // w: samples
    albedo: vec4<f32>,   // w: distance to the first surface
    normal: vec4<f32>,   // w: squared luminance
    motion: vec4<f32>,   // xy: screen motion in pixels
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> tris: array<Tri>;
@group(0) @binding(3) var<storage, read> attrs: array<Attr>;
@group(0) @binding(4) var<storage, read> materials: array<Mat>;
@group(0) @binding(5) var<storage, read> lights: array<LightEntry>;
// Four-float rows: the albedo table first, then texture and environment pixels.
@group(0) @binding(6) var<storage, read> blob: array<vec4<f32>>;
// Plain words: the light tree, the environment's sampling tables and the texture table.
@group(0) @binding(7) var<storage, read> words: array<u32>;
@group(0) @binding(8) var<storage, read_write> accum: array<Pixel>;

const PI: f32 = 3.14159265358979;
const INF: f32 = 3.0e38;
const NO_HIT: u32 = 0xffffffffu;
const TABLE: u32 = 32u;
const ALPHA_MIN: f32 = 1e-3;
const MU_82: f32 = 0.142857142857;
const BELOW_ONE: f32 = 0.99999994;

fn wordf(i: u32) -> f32 {
    return bitcast<f32>(words[i]);
}

fn finite(x: f32) -> bool {
    // NaN fails every comparison.
    return x == x && abs(x) < INF;
}

fn finite3(v: vec3<f32>) -> bool {
    return all(v == v) && all(abs(v) < vec3<f32>(INF));
}

// The unit vector along `v`, or `fallback` when `v` has no direction.
fn normalize_or(v: vec3<f32>, fallback: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    if l > 0.0 && l < INF {
        let r = v / l;
        if finite3(r) {
            return r;
        }
    }
    return fallback;
}

// ---- sampling ---------------------------------------------------------------------------

var<private> s_index: u32;
var<private> s_seed: u32;
var<private> s_dim: u32;

fn hash32(v: u32) -> u32 {
    var x = v;
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return x;
}

fn hash_combine(seed: u32, v: u32) -> u32 {
    return hash32(seed ^ (v * 0x9e3779b9u + 0x85ebca6bu + (seed << 6u) + (seed >> 2u)));
}

fn laine_karras(v: u32, seed: u32) -> u32 {
    var x = v + seed;
    x ^= x * 0x6c50b47cu;
    x ^= x * 0xb82f1e52u;
    x ^= x * 0xc7afe638u;
    x ^= x * 0x8d22f6e6u;
    return x;
}

fn owen(x: u32, seed: u32) -> u32 {
    return reverseBits(laine_karras(reverseBits(x), seed));
}

fn sobol1(index: u32) -> u32 {
    var i = index;
    var v = 0x80000000u;
    var x = 0u;
    while i != 0u {
        if (i & 1u) != 0u {
            x ^= v;
        }
        v ^= v >> 1u;
        i >>= 1u;
    }
    return x;
}

fn spread_bits(value: u32) -> u32 {
    var v = value & 0xffffu;
    v = (v | (v << 8u)) & 0x00ff00ffu;
    v = (v | (v << 4u)) & 0x0f0f0f0fu;
    v = (v | (v << 2u)) & 0x33333333u;
    return (v | (v << 1u)) & 0x55555555u;
}

fn next2() -> vec2<f32> {
    let s = hash_combine(s_seed, s_dim);
    s_dim += 1u;
    let i = owen(s_index, s);
    let x = owen(reverseBits(i), hash32(s ^ 0x68bc21ebu));
    let y = owen(sobol1(i), hash32(s ^ 0x02e5be93u));
    return vec2<f32>(f32(x >> 8u), f32(y >> 8u)) * (1.0 / 16777216.0);
}

fn next1() -> f32 {
    return next2().x;
}

fn concentric_disk(u: vec2<f32>) -> vec2<f32> {
    let o = u * 2.0 - vec2<f32>(1.0);
    if o.x == 0.0 && o.y == 0.0 {
        return vec2<f32>(0.0);
    }
    var r: f32;
    var theta: f32;
    if abs(o.x) > abs(o.y) {
        r = o.x;
        theta = PI / 4.0 * (o.y / o.x);
    } else {
        r = o.y;
        theta = PI / 2.0 - PI / 4.0 * (o.x / o.y);
    }
    return vec2<f32>(cos(theta), sin(theta)) * r;
}

fn cosine_hemisphere(u: vec2<f32>) -> vec3<f32> {
    let d = concentric_disk(u);
    return vec3<f32>(d.x, d.y, sqrt(max(1.0 - dot(d, d), 0.0)));
}

fn uniform_sphere(u: vec2<f32>) -> vec3<f32> {
    let z = 1.0 - 2.0 * u.x;
    let r = sqrt(max(1.0 - z * z, 0.0));
    let phi = 2.0 * PI * u.y;
    return vec3<f32>(r * cos(phi), r * sin(phi), z);
}

fn power_heuristic(a: f32, b: f32) -> f32 {
    if a >= INF {
        return 1.0;
    }
    let a2 = a * a;
    let b2 = b * b;
    if a2 + b2 > 0.0 {
        return a2 / (a2 + b2);
    }
    return 0.0;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn max3(v: vec3<f32>) -> f32 {
    return max(v.x, max(v.y, v.z));
}

fn sanitize(c: vec3<f32>) -> vec3<f32> {
    // NaN fails every comparison, so `c == c` is false exactly for NaN.
    if !(all(c == c) && all(abs(c) < vec3<f32>(INF))) {
        return vec3<f32>(0.0);
    }
    return max(c, vec3<f32>(0.0));
}

// ---- textures ---------------------------------------------------------------------------

fn tex_texel(t: u32, x: i32, y: i32) -> vec4<f32> {
    let w = i32(words[t + 1u]);
    let h = i32(words[t + 2u]);
    let xi = ((x % w) + w) % w;
    let yi = ((y % h) + h) % h;
    return blob[words[t] + u32(yi) * u32(w) + u32(xi)];
}

// Bilinear, repeating; pixel centres sit at (i + 0.5) / size.
fn tex_sample(id: i32, uv: vec2<f32>) -> vec4<f32> {
    if !(finite(uv.x) && finite(uv.y)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let t = params.textures + u32(id) * 4u;
    let u = min(uv.x - floor(uv.x), BELOW_ONE);
    let v = min(uv.y - floor(uv.y), BELOW_ONE);
    let fx = u * f32(words[t + 1u]) - 0.5;
    let fy = v * f32(words[t + 2u]) - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    return tex_texel(t, xi, yi) * ((1.0 - tx) * (1.0 - ty)) + tex_texel(t, xi + 1, yi) * (tx * (1.0 - ty))
        + tex_texel(t, xi, yi + 1) * ((1.0 - tx) * ty) + tex_texel(t, xi + 1, yi + 1) * (tx * ty);
}

fn tex_gray(id: i32, uv: vec2<f32>) -> f32 {
    let p = tex_sample(id, uv);
    if words[params.textures + u32(id) * 4u + 3u] != 0u {
        return p.x;
    }
    return luminance(p.rgb);
}

fn mat_opacity(m: Mat, uv: vec2<f32>) -> f32 {
    if m.opacity_tex < 0 {
        return m.opacity;
    }
    let s = tex_sample(m.opacity_tex, uv);
    var a = s.w;
    if words[params.textures + u32(m.opacity_tex) * 4u + 3u] != 0u {
        a = s.x;
    }
    return m.opacity * clamp(a, 0.0, 1.0);
}

// ---- intersection -----------------------------------------------------------------------

struct Hit {
    t: f32,
    u: f32,
    v: f32,
    slot: u32,
}

fn box_entry(index: u32, o: vec3<f32>, inv: vec3<f32>, tmax: f32) -> f32 {
    let t0 = (nodes[index].lo - o) * inv;
    let t1 = (nodes[index].hi - o) * inv;
    let lo = min(t0, t1);
    let hi = max(t0, t1);
    let near = max(max(lo.x, max(lo.y, lo.z)), 0.0);
    let far = min(min(hi.x, min(hi.y, hi.z)), tmax);
    if near <= far {
        return near;
    }
    return INF;
}

// Whether the surface at a hit stops a shadow ray. Cut-outs block in proportion to their
// opacity; a fixed pattern per point decides, so the answer is repeatable.
fn blocks(slot: u32, u: f32, v: f32) -> bool {
    let at = attrs[slot];
    let m = materials[at.material];
    if (m.flags & FLAG_CUTOUTS) == 0u {
        return true;
    }
    let opacity = mat_opacity(m, at.uv0 * (1.0 - u - v) + at.uv1 * u + at.uv2 * v);
    if opacity >= 1.0 {
        return true;
    }
    let vb = bitcast<u32>(v);
    let pattern = hash_combine(at.prim, bitcast<u32>(u) ^ ((vb << 16u) | (vb >> 16u)));
    return opacity > 0.0 && f32(pattern >> 8u) / 16777216.0 < opacity;
}

// The closest hit (mode 0), any hit (mode 1) or any hit that blocks light (mode 2).
fn intersect(o: vec3<f32>, d: vec3<f32>, limit: f32, mode: u32) -> Hit {
    var best = Hit(limit, 0.0, 0.0, NO_HIT);
    if params.node_count == 0u {
        return best;
    }
    let inv = vec3<f32>(1.0) / d;
    if box_entry(0u, o, inv, best.t) >= INF {
        return best;
    }
    var stack_node: array<u32, 64>;
    var stack_t: array<f32, 64>;
    var sp = 0u;
    var at = 0u;
    loop {
        let count = nodes[at].count;
        let a = nodes[at].a;
        var descend = false;
        if count > 0u {
            for (var slot = a; slot < a + count; slot++) {
                let tri = tris[slot];
                let p = cross(d, tri.e2);
                let det = dot(tri.e1, p);
                if det == 0.0 {
                    continue;
                }
                let idet = 1.0 / det;
                let s = o - tri.p0;
                let u = dot(s, p) * idet;
                if !(u >= 0.0 && u <= 1.0) {
                    continue;
                }
                let q = cross(s, tri.e1);
                let v = dot(d, q) * idet;
                if !(v >= 0.0 && u + v <= 1.0) {
                    continue;
                }
                let t = dot(tri.e2, q) * idet;
                if t > 0.0 && t < best.t {
                    if mode == 2u && !blocks(slot, u, v) {
                        continue;
                    }
                    best = Hit(t, u, v, slot);
                    if mode != 0u {
                        return best;
                    }
                }
            }
        } else {
            let l = at + 1u;
            let r = at + a;
            let tl = box_entry(l, o, inv, best.t);
            let tr = box_entry(r, o, inv, best.t);
            if min(tl, tr) < INF {
                var near = l;
                var far = r;
                var tfar = tr;
                if tr < tl {
                    near = r;
                    far = l;
                    tfar = tl;
                }
                if tfar < INF && sp < 64u {
                    stack_node[sp] = far;
                    stack_t[sp] = tfar;
                    sp += 1u;
                }
                at = near;
                descend = true;
            }
        }
        if !descend {
            var found = false;
            while sp > 0u {
                sp -= 1u;
                if stack_t[sp] <= best.t {
                    at = stack_node[sp];
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }
    }
    return best;
}

// The two ray queries the kernel makes. Everything above is one implementation of them (BVH
// traversal in the shader); another backend, such as hardware ray queries, replaces these two.
fn trace_closest(o: vec3<f32>, d: vec3<f32>) -> Hit {
    return intersect(o, d, INF, 0u);
}

fn trace_any(o: vec3<f32>, d: vec3<f32>, tmax: f32) -> bool {
    return intersect(o, d, tmax, select(1u, 2u, params.has_cutouts != 0u)).slot != NO_HIT;
}

fn spawn_origin(p: vec3<f32>, ng: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let eps = 4e-5 * (1.0 + max3(abs(p)));
    let side = select(-1.0, 1.0, dot(dir, ng) >= 0.0);
    return p + ng * (eps * side);
}

fn occluded(p: vec3<f32>, ng: vec3<f32>, dir: vec3<f32>, distance: f32) -> bool {
    let o = spawn_origin(p, ng, dir);
    if distance >= INF {
        return trace_any(o, dir, INF);
    }
    // Aim from the offset origin at the point on the light and stop just short of it.
    let to = p + dir * distance - o;
    let len = length(to);
    let tmax = len * (1.0 - 2e-4) - 1e-5;
    if tmax <= 0.0 {
        return false;
    }
    return trace_any(o, to / len, tmax);
}

// ---- surface model ----------------------------------------------------------------------

fn ggx_d(h: vec3<f32>, ax: f32, ay: f32) -> f32 {
    let d = h.x * h.x / (ax * ax) + h.y * h.y / (ay * ay) + h.z * h.z;
    return 1.0 / (PI * ax * ay * d * d);
}

fn ggx_lambda(w: vec3<f32>, ax: f32, ay: f32) -> f32 {
    let z2 = w.z * w.z;
    if z2 <= 0.0 {
        return INF;
    }
    let t2 = (ax * ax * w.x * w.x + ay * ay * w.y * w.y) / z2;
    return (sqrt(1.0 + t2) - 1.0) * 0.5;
}

fn sample_visible_normal(wo: vec3<f32>, ax: f32, ay: f32, u: vec2<f32>) -> vec3<f32> {
    let vh = normalize(vec3<f32>(ax * wo.x, ay * wo.y, wo.z));
    let len2 = vh.x * vh.x + vh.y * vh.y;
    var t1 = vec3<f32>(1.0, 0.0, 0.0);
    if len2 > 0.0 {
        t1 = vec3<f32>(-vh.y, vh.x, 0.0) / sqrt(len2);
    }
    let t2 = cross(vh, t1);
    let r = sqrt(u.x);
    let phi = 2.0 * PI * u.y;
    let p1 = r * cos(phi);
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * sqrt(max(1.0 - p1 * p1, 0.0)) + s * r * sin(phi);
    let nh = t1 * p1 + t2 * p2 + vh * sqrt(max(1.0 - p1 * p1 - p2 * p2, 0.0));
    return normalize(vec3<f32>(ax * nh.x, ay * nh.y, max(nh.z, 1e-6)));
}

fn schlick_weight(c: f32) -> f32 {
    let m = clamp(1.0 - c, 0.0, 1.0);
    let m2 = m * m;
    return m2 * m2 * m;
}

// The F82-tint edge term μ (1 − μ)^6.
fn edge_weight(cos_in: f32) -> f32 {
    let c = clamp(cos_in, 0.0, 1.0);
    let m = 1.0 - c;
    let m2 = m * m;
    return c * m2 * m2 * m2;
}

// Unpolarized Fresnel reflectance of a dielectric interface (1 under total internal reflection).
fn fresnel_dielectric(cos_i: f32, eta: f32) -> f32 {
    let c = clamp(cos_i, 0.0, 1.0);
    let s2 = (1.0 - c * c) / (eta * eta);
    if s2 >= 1.0 {
        return 1.0;
    }
    let ct = sqrt(1.0 - s2);
    let rs = (c - eta * ct) / (c + eta * ct);
    let rp = (eta * c - ct) / (eta * c + ct);
    return 0.5 * (rs * rs + rp * rp);
}

fn airy(a: f32, b: f32, cp: f32) -> f32 {
    return (a * a + b * b + 2.0 * a * b * cp) / (1.0 + a * a * b * b + 2.0 * a * b * cp);
}

// Reflectance of a base under a thin film, seen from air: the Airy formula averaged over
// polarization and three wavelengths per channel.
fn thin_film_reflectance(cos_i: f32, n_film: f32, n_base: f32, thickness: f32) -> vec3<f32> {
    let c1 = clamp(cos_i, 1e-4, 1.0);
    let s1 = max(1.0 - c1 * c1, 0.0);
    let s2 = s1 / (n_film * n_film);
    let s3 = s1 / (n_base * n_base);
    if s2 >= 1.0 || s3 >= 1.0 {
        return vec3<f32>(1.0);
    }
    let c2 = sqrt(1.0 - s2);
    let c3 = sqrt(1.0 - s3);
    let r12s = (c1 - n_film * c2) / (c1 + n_film * c2);
    let r12p = (n_film * c1 - c2) / (n_film * c1 + c2);
    let r23s = (n_film * c2 - n_base * c3) / (n_film * c2 + n_base * c3);
    let r23p = (n_base * c2 - n_film * c3) / (n_base * c2 + n_film * c3);
    let path = 4.0 * PI * n_film * thickness * c2;
    var lambdas = array<vec3<f32>, 3>(vec3<f32>(0.580, 0.610, 0.640), vec3<f32>(0.520, 0.545, 0.570), vec3<f32>(0.440, 0.460, 0.480));
    var out = vec3<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch++) {
        var acc = 0.0;
        for (var k = 0u; k < 3u; k++) {
            let cp = cos(path / lambdas[ch][k]);
            acc += airy(r12s, r23s, cp);
            acc += airy(r12p, r23p, cp);
        }
        out[ch] = acc / 6.0;
    }
    return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn fuzz_alpha(roughness: f32) -> f32 {
    return clamp(roughness, 0.05, 1.0);
}

// The fuzz lobe without colour or weight: D · V.
fn fuzz_dv(wo: vec3<f32>, wi: vec3<f32>, alpha: f32) -> f32 {
    let h = normalize_or(wo + wi, vec3<f32>(0.0));
    let sin2 = max(1.0 - h.z * h.z, 0.0);
    let inv = 1.0 / alpha;
    var shape = 0.0;
    if sin2 > 0.0 {
        shape = pow(sin2, 0.5 * inv);
    }
    let d = (2.0 + inv) * shape / (2.0 * PI);
    return d / max(4.0 * (wi.z + wo.z - wi.z * wo.z), 1e-4);
}

// [a, b, c, fuzz] at a cosine and a roughness, bilinearly interpolated.
fn albedo_lookup(c: f32, roughness: f32) -> vec4<f32> {
    let m = f32(TABLE - 1u);
    let x = clamp(c, 0.0, 1.0) * m;
    let y = clamp(roughness, 0.0, 1.0) * m;
    let x0 = min(u32(x), TABLE - 2u);
    let y0 = min(u32(y), TABLE - 2u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    let p00 = blob[y0 * TABLE + x0];
    let p10 = blob[y0 * TABLE + x0 + 1u];
    let p01 = blob[(y0 + 1u) * TABLE + x0];
    let p11 = blob[(y0 + 1u) * TABLE + x0 + 1u];
    return (p00 * (1.0 - fx) + p10 * fx) * (1.0 - fy) + (p01 * (1.0 - fx) + p11 * fx) * fy;
}

// The numbers the scattering model reads at one point, after textures.
struct Surf {
    base: vec3<f32>,
    diffuse_roughness: f32,
    specular_color: vec3<f32>,
    metalness: f32,
    transmission_tint: vec3<f32>,
    specular_weight: f32,
    coat_color: vec3<f32>,
    roughness: f32,
    fuzz_color: vec3<f32>,
    anisotropy: f32,
    ior: f32,
    transmission: f32,
    coat: f32,
    coat_roughness: f32,
    coat_ior: f32,
    fuzz: f32,
    fuzz_roughness: f32,
    thin_film: f32,
    thin_film_thickness: f32,
    thin_film_ior: f32,
}

fn surface_params(m: Mat, uv: vec2<f32>) -> Surf {
    var s: Surf;
    s.base = m.base;
    if m.base_tex >= 0 {
        s.base *= clamp(tex_sample(m.base_tex, uv).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    s.roughness = m.roughness;
    if m.rough_tex >= 0 {
        s.roughness = clamp(m.roughness * tex_sample(m.rough_tex, uv).y, 0.0, 1.0);
    }
    s.metalness = m.metalness;
    if m.metal_tex >= 0 {
        s.metalness = clamp(m.metalness * tex_sample(m.metal_tex, uv).z, 0.0, 1.0);
    }
    s.diffuse_roughness = m.diffuse_roughness;
    s.specular_weight = m.specular_weight;
    s.specular_color = m.specular_color;
    s.anisotropy = m.anisotropy;
    s.ior = m.ior;
    s.transmission = m.transmission;
    s.transmission_tint = m.transmission_tint;
    s.coat = m.coat;
    s.coat_color = m.coat_color;
    s.coat_roughness = m.coat_roughness;
    s.coat_ior = m.coat_ior;
    s.fuzz = m.fuzz;
    s.fuzz_color = m.fuzz_color;
    s.fuzz_roughness = m.fuzz_roughness;
    s.thin_film = m.thin_film;
    s.thin_film_thickness = m.thin_film_thickness;
    s.thin_film_ior = m.thin_film_ior;
    return s;
}

const FUZZ: u32 = 0u;
const COAT: u32 = 1u;
const SPEC: u32 = 2u;
const DIFFUSE: u32 = 3u;
const TRANSMIT: u32 = 4u;

// A material resolved at one point for one outgoing direction.
struct Bsdf {
    wo: vec3<f32>,
    ax: f32,
    metal_f0: vec3<f32>,
    ay: f32,
    metal_edge: vec3<f32>,
    eta: f32,
    metal_scale: vec3<f32>,
    sigma: f32,
    diel_scale: vec3<f32>,
    coat: f32,
    diffuse: vec3<f32>,
    coat_f0: f32,
    transmit: vec3<f32>,
    coat_alpha: f32,
    fuzz: vec3<f32>,
    fuzz_alpha: f32,
    film: vec4<f32>, // weight, thickness, film index, base index
    p: array<f32, 5>,
}

fn bsdf_new(m: Surf, wo_in: vec3<f32>, inside: bool) -> Bsdf {
    var wo = wo_in;
    if !(wo.z > 1e-4) {
        wo = normalize(vec3<f32>(wo.x, wo.y, 1e-4));
    }
    let alpha = max(m.roughness * m.roughness, ALPHA_MIN);
    let thin = 1.0 - m.anisotropy;
    let stretch = sqrt(2.0 / (1.0 + thin * thin));
    let ax = min(alpha * stretch, 1.0);
    let ay = max(thin * ax, ALPHA_MIN);
    var eta = m.ior;
    if inside {
        eta = 1.0 / m.ior;
    }
    if abs(eta - 1.0) < 1e-4 {
        eta = 1.0001;
    }
    var b: Bsdf;
    b.wo = wo;
    b.ax = ax;
    b.ay = ay;
    b.metal_f0 = vec3<f32>(0.0);
    b.metal_edge = vec3<f32>(0.0);
    b.metal_scale = vec3<f32>(0.0);
    b.diel_scale = vec3<f32>(1.0);
    b.eta = eta;
    b.film = vec4<f32>(0.0);
    b.diffuse = vec3<f32>(0.0);
    b.sigma = 0.0;
    b.transmit = vec3<f32>(1.0);
    b.coat = 0.0;
    b.coat_f0 = 0.0;
    b.coat_alpha = ALPHA_MIN;
    b.fuzz = vec3<f32>(0.0);
    b.fuzz_alpha = 1.0;
    b.p = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
    if inside {
        let f = clamp(fresnel_dielectric(wo.z, eta), 0.1, 0.9);
        b.p[SPEC] = f;
        b.p[TRANSMIT] = 1.0 - f;
        return b;
    }
    let t = albedo_lookup(wo.z, sqrt(sqrt(ax * ay)));
    let e = clamp(t.x + t.y, 1e-4, 1.0);
    let k = (1.0 - e) / e;

    // Fuzz on top.
    b.fuzz_alpha = fuzz_alpha(m.fuzz_roughness);
    let e_fuzz_lobe = albedo_lookup(wo.z, m.fuzz_roughness).w;
    let fuzz_norm = 1.0 / max(e_fuzz_lobe, 1.0);
    let e_fuzz = clamp(m.fuzz * e_fuzz_lobe * fuzz_norm, 0.0, 1.0);
    b.fuzz = m.fuzz_color * (m.fuzz * fuzz_norm);
    let under_fuzz = 1.0 - e_fuzz;

    // Coat.
    let rc = (m.coat_ior - 1.0) / (m.coat_ior + 1.0);
    b.coat_f0 = rc * rc;
    b.coat_alpha = max(m.coat_roughness * m.coat_roughness, ALPHA_MIN);
    let ct = albedo_lookup(wo.z, m.coat_roughness);
    let ce = clamp(ct.x + ct.y, 1e-4, 1.0);
    let coat_ms = 1.0 + b.coat_f0 * (1.0 - ce) / ce;
    let e_coat = clamp(m.coat * (b.coat_f0 * ct.x + ct.y) * coat_ms, 0.0, 1.0);
    b.coat = under_fuzz * m.coat * coat_ms;
    let base_scale = (vec3<f32>(1.0) + (m.coat_color - vec3<f32>(1.0)) * m.coat) * (under_fuzz * (1.0 - e_coat));

    // Metal.
    let mt = m.metalness;
    let dl = 1.0 - m.metalness;
    let metal_ms = vec3<f32>(1.0) + m.base * k;
    let schlick_82 = m.base + (vec3<f32>(1.0) - m.base) * schlick_weight(MU_82);
    let tint = m.specular_color * m.specular_weight + vec3<f32>(1.0 - m.specular_weight);
    b.metal_f0 = m.base;
    b.metal_edge = schlick_82 * (vec3<f32>(1.0) - tint) / edge_weight(MU_82);
    b.metal_scale = metal_ms * base_scale * mt;
    let metal_e = clamp((m.base * t.x + vec3<f32>(t.y) - b.metal_edge * t.z) * metal_ms, vec3<f32>(0.0), vec3<f32>(1.0));

    // Dielectric interface.
    let r = (m.ior - 1.0) / (m.ior + 1.0);
    var d0 = r * r;
    if m.thin_film > 0.0 {
        b.film = vec4<f32>(m.thin_film, m.thin_film_thickness, m.thin_film_ior, m.ior);
        let film0 = luminance(thin_film_reflectance(1.0, m.thin_film_ior, m.ior, m.thin_film_thickness));
        d0 += (film0 - d0) * m.thin_film;
    }
    d0 = d0 * m.specular_weight;
    let diel_ms = 1.0 + d0 * k;
    let diel_e = clamp((d0 * t.x + m.specular_weight * t.y) * diel_ms, 0.0, 1.0);
    b.diel_scale = m.specular_color * base_scale * (m.specular_weight * diel_ms * dl);

    // Under the interface: diffuse or transmission.
    let below = base_scale * (dl * (1.0 - diel_e));
    b.diffuse = m.base * below * (1.0 - m.transmission);
    b.sigma = m.diffuse_roughness;
    b.transmit = m.transmission_tint * base_scale * (dl * m.transmission);

    let scale = luminance(base_scale);
    var w = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
    w[FUZZ] = e_fuzz * luminance(m.fuzz_color);
    w[COAT] = under_fuzz * e_coat;
    w[SPEC] = scale * (mt * luminance(metal_e) + dl * luminance(m.specular_color) * diel_e);
    w[DIFFUSE] = luminance(b.diffuse);
    w[TRANSMIT] = luminance(b.transmit) * (1.0 - diel_e);
    if w[TRANSMIT] > 0.0 {
        // Glass seen at a grazing angle is mostly mirror; keep that lobe well sampled.
        w[SPEC] = max(w[SPEC], 0.1 * w[TRANSMIT]);
    }
    let total = w[0] + w[1] + w[2] + w[3] + w[4];
    if total > 0.0 {
        for (var i = 0u; i < 5u; i++) {
            b.p[i] = w[i] / total;
        }
    }
    return b;
}

fn dielectric_fresnel(b: Bsdf, c: f32) -> vec3<f32> {
    let exact = vec3<f32>(fresnel_dielectric(c, b.eta));
    if b.film.x > 0.0 {
        return exact + (thin_film_reflectance(c, b.film.z, b.film.w, b.film.y) - exact) * b.film.x;
    }
    return exact;
}

// rgb: f·|cos|, w: pdf, for the incoming direction `wi` (below the surface for transmission).
fn bsdf_eval(b: Bsdf, wi: vec3<f32>) -> vec4<f32> {
    let wo = b.wo;
    var f = vec3<f32>(0.0);
    var pdf = 0.0;
    if wi.z > 0.0 {
        let cos_pdf = wi.z / PI;
        if max3(b.fuzz) > 0.0 {
            f += b.fuzz * (fuzz_dv(wo, wi, b.fuzz_alpha) * wi.z);
            pdf += b.p[FUZZ] * cos_pdf;
        }
        if max3(b.diffuse) > 0.0 {
            var shape = 1.0;
            if b.sigma > 0.0 {
                let a = 1.0 / (1.0 + (0.5 - 2.0 / (3.0 * PI)) * b.sigma);
                let s = dot(wi, wo) - wi.z * wo.z;
                var t = 1.0;
                if s > 0.0 {
                    t = max(wi.z, wo.z);
                }
                shape = a * (1.0 + b.sigma * s / t);
            }
            f += b.diffuse * (cos_pdf * shape);
            pdf += b.p[DIFFUSE] * cos_pdf;
        }
        let h = normalize_or(wo + wi, vec3<f32>(0.0));
        if h.z > 0.0 {
            let cos_h = dot(wo, h);
            if b.coat > 0.0 {
                let a = b.coat_alpha;
                let lo = ggx_lambda(wo, a, a);
                let dv = ggx_d(h, a, a) / (4.0 * wo.z);
                if finite(dv) {
                    let fr = b.coat_f0 + (1.0 - b.coat_f0) * schlick_weight(cos_h);
                    f += vec3<f32>(b.coat * fr * dv / (1.0 + lo + ggx_lambda(wi, a, a)));
                    pdf += b.p[COAT] * dv / (1.0 + lo);
                }
            }
            if b.p[SPEC] > 0.0 {
                let lo = ggx_lambda(wo, b.ax, b.ay);
                let dv = ggx_d(h, b.ax, b.ay) / (4.0 * wo.z);
                if finite(dv) {
                    var fr = b.diel_scale * dielectric_fresnel(b, cos_h);
                    if max3(b.metal_scale) > 0.0 {
                        let metal = b.metal_f0 + (vec3<f32>(1.0) - b.metal_f0) * schlick_weight(cos_h) - b.metal_edge * edge_weight(cos_h);
                        fr += b.metal_scale * max(metal, vec3<f32>(0.0));
                    }
                    f += fr * (dv / (1.0 + lo + ggx_lambda(wi, b.ax, b.ay)));
                    pdf += b.p[SPEC] * dv / (1.0 + lo);
                }
            }
        }
    } else if wi.z < 0.0 && b.p[TRANSMIT] > 0.0 {
        // Refraction: the half vector that bends `wo` into `wi`.
        var h = -normalize_or(wo + wi * b.eta, vec3<f32>(0.0));
        if h.z < 0.0 {
            h = -h;
        }
        let co = dot(wo, h);
        let ci = dot(wi, h);
        if h.z > 0.0 && co > 0.0 && ci < 0.0 {
            let denom = co + b.eta * ci;
            let jac = b.eta * b.eta * abs(ci) / (denom * denom);
            let lo = ggx_lambda(wo, b.ax, b.ay);
            let dvis = ggx_d(h, b.ax, b.ay) * co / (wo.z * (1.0 + lo));
            let g_ratio = (1.0 + lo) / (1.0 + lo + ggx_lambda(wi, b.ax, b.ay));
            let through = vec3<f32>(1.0) - dielectric_fresnel(b, co);
            let v = dvis * jac;
            if finite(v) {
                f += b.transmit * through * (v * g_ratio);
                pdf += b.p[TRANSMIT] * v;
            }
        }
    }
    return vec4<f32>(f, pdf);
}

struct BsdfSample {
    wi: vec3<f32>,
    pdf: f32, // 0 when no direction was produced
    weight: vec3<f32>,
}

fn bsdf_sample(b: Bsdf, lobe: f32, u: vec2<f32>) -> BsdfSample {
    var none = BsdfSample(vec3<f32>(0.0), 0.0, vec3<f32>(0.0));
    var p = b.p;
    var acc = 0.0;
    var pick = DIFFUSE;
    for (var i = 0u; i < 5u; i++) {
        acc += p[i];
        if p[i] > 0.0 {
            pick = i;
            if lobe < acc {
                break;
            }
        }
    }
    let wo = b.wo;
    var wi: vec3<f32>;
    if pick == COAT {
        let h = sample_visible_normal(wo, b.coat_alpha, b.coat_alpha, u);
        wi = h * (2.0 * dot(wo, h)) - wo;
    } else if pick == SPEC {
        let h = sample_visible_normal(wo, b.ax, b.ay, u);
        wi = h * (2.0 * dot(wo, h)) - wo;
    } else if pick == TRANSMIT {
        let h = sample_visible_normal(wo, b.ax, b.ay, u);
        let c = dot(wo, h);
        let k = 1.0 - (1.0 - c * c) / (b.eta * b.eta);
        if k <= 0.0 {
            // Total internal reflection: this microfacet only reflects.
            return none;
        }
        wi = normalize(-wo / b.eta + h * (c / b.eta - sqrt(k)));
    } else {
        wi = cosine_hemisphere(u);
    }
    let fe = bsdf_eval(b, wi);
    if fe.w > 0.0 && finite3(fe.rgb) {
        return BsdfSample(wi, fe.w, fe.rgb / fe.w);
    }
    return none;
}

// ---- lights -----------------------------------------------------------------------------

fn env_uv(dir: vec3<f32>) -> vec2<f32> {
    let u = atan2(dir.z, dir.x) / (2.0 * PI) + 0.5 - params.env_turn;
    return vec2<f32>(u - floor(u), acos(clamp(dir.y, -1.0, 1.0)) / PI);
}

fn env_cell(uv: vec2<f32>) -> vec2<u32> {
    return vec2<u32>(min(u32(uv.x * f32(params.env_w)), params.env_w - 1u), min(u32(uv.y * f32(params.env_h)), params.env_h - 1u));
}

// Radiance arriving from `dir` from outside the scene.
fn env_radiance(dir: vec3<f32>) -> vec3<f32> {
    if params.env_w == 0u {
        return params.env.rgb;
    }
    let cell = env_cell(env_uv(dir));
    let c = blob[params.env_pixels + cell.y * params.env_w + cell.x].rgb;
    if finite3(c) {
        return max(c, vec3<f32>(0.0));
    }
    return vec3<f32>(0.0);
}

fn table_step(base: u32, i: u32, first: u32) -> f32 {
    var below = 0.0;
    if i > first {
        below = wordf(base + i - 1u);
    }
    return wordf(base + i) - below;
}

// Density over solid angle with which the image of the surroundings is sampled towards `dir`.
fn env_map_pdf(dir: vec3<f32>) -> f32 {
    let s = sqrt(max(1.0 - dir.y * dir.y, 0.0));
    if s <= 1e-6 {
        return 0.0;
    }
    let cell = env_cell(env_uv(dir));
    let row = cell.y * params.env_w;
    let pdf_uv = table_step(params.env_marginal, cell.y, 0u) * table_step(params.env_conditional, row + cell.x, row) * f32(params.env_w * params.env_h);
    return pdf_uv / (2.0 * PI * PI * s);
}

// Density with which light sampling picks direction `dir` leaving the scene.
fn pdf_environment(dir: vec3<f32>) -> f32 {
    if params.has_env == 0u {
        return 0.0;
    }
    if params.env_w == 0u {
        return params.env.w / (4.0 * PI);
    }
    return params.env.w * env_map_pdf(dir);
}

// The first entry of `count` table entries from `base` that is above `r`, and where `r` lies
// within it (x: index, y: fraction).
fn table_pick(base: u32, count: u32, r: f32) -> vec2<f32> {
    var lo = 0u;
    var hi = count;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if wordf(base + mid) <= r {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let i = min(lo, count - 1u);
    var below = 0.0;
    if i > 0u {
        below = wordf(base + i - 1u);
    }
    let span = wordf(base + i) - below;
    var frac = 0.5;
    if span > 0.0 {
        frac = clamp((r - below) / span, 0.0, 0.999999);
    }
    return vec2<f32>(f32(i), frac);
}

fn env_map_sample(u: vec2<f32>) -> vec3<f32> {
    let y = table_pick(params.env_marginal, params.env_h, u.y);
    let x = table_pick(params.env_conditional + u32(y.x) * params.env_w, params.env_w, u.x);
    let uv = vec2<f32>((x.x + x.y) / f32(params.env_w), (y.x + y.y) / f32(params.env_h));
    let theta = uv.y * PI;
    let phi = (uv.x + params.env_turn - 0.5) * 2.0 * PI;
    return vec3<f32>(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
}

// How much a node of the light tree matters at `p`: its power over its squared distance.
fn tree_importance(node: u32, p: vec3<f32>) -> f32 {
    let b = params.tree_nodes + node * 8u;
    let d = vec3<f32>(wordf(b), wordf(b + 1u), wordf(b + 2u)) - p;
    // Inside the node's sphere distance says nothing; treat the lights as one radius away.
    return wordf(b + 4u) / max(max(dot(d, d), wordf(b + 3u)), 1e-12);
}

// Probability of going left at an interior node when shading `p`.
fn tree_left(b: u32, p: vec3<f32>) -> f32 {
    let l = tree_importance(words[b + 5u], p);
    let r = tree_importance(words[b + 6u], p);
    if l + r > 0.0 && l + r < INF {
        return l / (l + r);
    }
    return 0.5;
}

// x: the light, y: the probability with which it was chosen (0: none).
fn tree_sample(p: vec3<f32>, u_in: f32) -> vec2<f32> {
    var u = u_in;
    var node = 0u;
    var pdf = 1.0;
    for (var i = 0u; i < 64u; i++) {
        let b = params.tree_nodes + node * 8u;
        if words[b + 6u] == NO_HIT {
            return vec2<f32>(f32(words[b + 5u]), pdf);
        }
        let pl = tree_left(b, p);
        if u < pl {
            u /= pl;
            pdf *= pl;
            node = words[b + 5u];
        } else {
            u = (u - pl) / (1.0 - pl);
            pdf *= 1.0 - pl;
            node = words[b + 6u];
        }
    }
    return vec2<f32>(0.0);
}

// Probability with which `tree_sample` returns `light` when shading `p`.
fn tree_pdf(light: u32, p: vec3<f32>) -> f32 {
    if light >= params.positional {
        return 0.0;
    }
    var lo = words[params.tree_paths + light * 2u];
    var hi = words[params.tree_paths + light * 2u + 1u];
    var node = 0u;
    var pdf = 1.0;
    for (var i = 0u; i < 64u; i++) {
        let b = params.tree_nodes + node * 8u;
        if words[b + 6u] == NO_HIT {
            if words[b + 5u] == light {
                return pdf;
            }
            return 0.0;
        }
        let pl = tree_left(b, p);
        if (lo & 1u) == 0u {
            pdf *= pl;
            node = words[b + 5u];
        } else {
            pdf *= 1.0 - pl;
            node = words[b + 6u];
        }
        lo = (lo >> 1u) | (hi << 31u);
        hi = hi >> 1u;
    }
    return 0.0;
}

// Probability of choosing light entry `entry` (one with a position) when shading `p`.
fn select_pdf(entry: u32, p: vec3<f32>) -> f32 {
    if params.light_tree != 0u {
        return params.positional_mass * tree_pdf(entry, p);
    }
    return lights[entry].pmf;
}

const SUN_DISK_MIN: f32 = 1e-6;

// Density over solid angle of a point of a sun's disk, `c` (a cosine) away from its axis.
fn sun_density(tan_radius: f32, c: f32) -> f32 {
    return 1.0 / (PI * tan_radius * tan_radius * c * c * c);
}

// rgb: radiance of the sun disks around `dir` (a direction that leaves the scene), w: the density
// with which light sampling picks `dir` on them. Lights without a position follow those with one.
fn sun_radiance(dir: vec3<f32>) -> vec4<f32> {
    var out = vec4<f32>(0.0);
    for (var i = params.positional; i < params.light_count; i++) {
        let l = lights[i];
        if l.kind == 6u && l.area > SUN_DISK_MIN {
            let c = dot(dir, l.p0);
            if c > 0.0 && max(1.0 - c * c, 0.0) <= l.area * l.area * c * c {
                let density = sun_density(l.area, c);
                out += vec4<f32>(l.radiance * density, l.pmf * density);
            }
        }
    }
    return out;
}

struct LightSample {
    wi: vec3<f32>,
    distance: f32,
    radiance: vec3<f32>,
    pdf: f32, // 0 when no sample was produced
    delta: bool,
}

fn sample_light(p: vec3<f32>, pick: f32, u: vec2<f32>) -> LightSample {
    var out = LightSample(vec3<f32>(0.0), 0.0, vec3<f32>(0.0), 0.0, false);
    var entry = 0u;
    var pmf = 0.0;
    if params.light_tree != 0u && pick < params.positional_mass {
        let chosen = tree_sample(p, pick / params.positional_mass);
        entry = u32(chosen.x);
        pmf = chosen.y * params.positional_mass;
    } else {
        var lo = 0u;
        var hi = params.light_count;
        while lo < hi {
            let mid = (lo + hi) / 2u;
            if lights[mid].cdf <= pick {
                lo = mid + 1u;
            } else {
                hi = mid;
            }
        }
        entry = min(lo, params.light_count - 1u);
        pmf = lights[entry].pmf;
    }
    if !(pmf > 0.0) {
        return out;
    }
    let l = lights[entry];
    if l.kind <= 1u {
        let s = sqrt(max(u.x, 0.0));
        let q = l.p0 + l.e1 * (1.0 - s) + l.e2 * (u.y * s);
        let d = q - p;
        let dist2 = dot(d, d);
        if dist2 <= 0.0 {
            return out;
        }
        let dist = sqrt(dist2);
        let wi = d / dist;
        let c = -dot(wi, normalize_or(cross(l.e1, l.e2), vec3<f32>(0.0)));
        if c == 0.0 || (c < 0.0 && l.kind == 0u) || l.area <= 0.0 {
            return out;
        }
        return LightSample(wi, dist, l.radiance, pmf * dist2 / (l.area * abs(c)), false);
    }
    if l.kind == 2u {
        let d = l.p0 - p;
        let dist2 = dot(d, d);
        if dist2 <= 0.0 {
            return out;
        }
        return LightSample(d / sqrt(dist2), sqrt(dist2), l.radiance / dist2, pmf, true);
    }
    if l.kind == 3u {
        return LightSample(l.p0, INF, l.radiance, pmf, true);
    }
    if l.kind == 5u {
        let d = l.p0 - p;
        let dist2 = dot(d, d);
        if dist2 <= 0.0 {
            return out;
        }
        let wi = d / sqrt(dist2);
        let c = -dot(wi, l.e1);
        let co = l.area;
        let ci = l.e2.x;
        var falloff = 1.0;
        if c <= co {
            return out;
        } else if c < ci && ci - co >= 1e-6 {
            let t = (c - co) / (ci - co);
            falloff = t * t * (3.0 - 2.0 * t);
        }
        return LightSample(wi, sqrt(dist2), l.radiance * (falloff / dist2), pmf, true);
    }
    if l.kind == 6u {
        let disk = concentric_disk(u) * l.area;
        let n = l.p0;
        let sign = select(-1.0, 1.0, n.z >= 0.0);
        let fa = -1.0 / (sign + n.z);
        let fb = n.x * n.y * fa;
        let tx = vec3<f32>(1.0 + sign * n.x * n.x * fa, sign * fb, -sign * n.x);
        let ty = vec3<f32>(fb, sign + n.y * n.y * fa, -n.y);
        let local = normalize(vec3<f32>(disk.x, disk.y, 1.0));
        let wi = tx * local.x + ty * local.y + n * local.z;
        if l.area > SUN_DISK_MIN {
            // A disk in the sky: radiance and a density over solid angle, whose ratio is the irradiance.
            let density = sun_density(l.area, local.z);
            return LightSample(wi, INF, l.radiance * density, pmf * density, false);
        }
        return LightSample(wi, INF, l.radiance, pmf, true);
    }
    if params.env_w != 0u {
        let wi = env_map_sample(u);
        let pdf = env_map_pdf(wi);
        if !(pdf > 0.0) {
            return out;
        }
        return LightSample(wi, INF, env_radiance(wi), pmf * pdf, false);
    }
    return LightSample(uniform_sphere(u), INF, l.radiance, pmf / (4.0 * PI), false);
}

// ---- transport --------------------------------------------------------------------------

// What the path saw at its first surface (the feature images).
var<private> f_albedo: vec3<f32>;
var<private> f_normal: vec3<f32>;
var<private> f_depth: f32;
var<private> f_position: vec3<f32>;
var<private> f_has_position: bool;

struct Frame {
    t: vec3<f32>,
    b: vec3<f32>,
    n: vec3<f32>,
}

// An orthonormal basis around `n` (Duff et al. 2017).
fn frame_new(n: vec3<f32>) -> Frame {
    let sign = select(-1.0, 1.0, n.z >= 0.0);
    let fa = -1.0 / (sign + n.z);
    let fb = n.x * n.y * fa;
    return Frame(vec3<f32>(1.0 + sign * n.x * n.x * fa, sign * fb, -sign * n.x), vec3<f32>(fb, sign + n.y * n.y * fa, -n.y), n);
}

fn to_local(f: Frame, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(v, f.t), dot(v, f.b), dot(v, f.n));
}

fn to_world(f: Frame, v: vec3<f32>) -> vec3<f32> {
    return f.t * v.x + f.b * v.y + f.n * v.z;
}

// Apply the material's normal map, bump map and anisotropy direction to the shading frame.
fn shading_frame(m: Mat, tri: Tri, at: Attr, uv: vec2<f32>, ng: vec3<f32>, frame: Frame) -> Frame {
    // The directions in which the texture coordinates grow across the triangle.
    let d1 = at.uv1 - at.uv0;
    let d2 = at.uv2 - at.uv0;
    let det = d1.x * d2.y - d1.y * d2.x;
    if !(abs(det) >= 1e-12) || !finite(det) {
        return frame;
    }
    let dpdu = (tri.e1 * d2.y - tri.e2 * d1.y) / det;
    let dpdv = (tri.e2 * d1.x - tri.e1 * d2.x) / det;
    if !(finite3(dpdu) && finite3(dpdv) && dot(dpdu, dpdu) > 0.0 && dot(dpdv, dpdv) > 0.0) {
        return frame;
    }
    var n = frame.n;
    let tangent = normalize_or(dpdu - n * dot(n, dpdu), vec3<f32>(0.0));
    if all(tangent == vec3<f32>(0.0)) {
        return frame;
    }
    var bitangent = cross(n, tangent);
    if dot(bitangent, dpdv) < 0.0 {
        bitangent = -bitangent;
    }
    if m.normal_tex >= 0 {
        let s = tex_sample(m.normal_tex, uv);
        let x = (s.x * 2.0 - 1.0) * m.normal_scale;
        let y = (s.y * 2.0 - 1.0) * m.normal_scale;
        let z = max(s.z * 2.0 - 1.0, 1e-3);
        n = normalize_or(tangent * x + bitangent * y + n * z, n);
    }
    if m.bump_tex >= 0 {
        // Finite differences of the height, one texel apart (Blinn 1978).
        let t = params.textures + u32(m.bump_tex) * 4u;
        let du = 1.0 / f32(max(words[t + 1u], 1u));
        let dv = 1.0 / f32(max(words[t + 2u], 1u));
        let h0 = tex_gray(m.bump_tex, uv);
        let dhdu = (tex_gray(m.bump_tex, uv + vec2<f32>(du, 0.0)) - h0) / du * m.bump_scale;
        let dhdv = (tex_gray(m.bump_tex, uv + vec2<f32>(0.0, dv)) - h0) / dv * m.bump_scale;
        n = normalize_or(n - dpdu * (dhdu / dot(dpdu, dpdu)) - dpdv * (dhdv / dot(dpdv, dpdv)), n);
    }
    if dot(n, ng) <= 0.0 {
        // A map may not turn the surface away from the side it is seen from.
        n = normalize_or(n - ng * (dot(n, ng) - 1e-3), ng);
    }
    let t = normalize_or(tangent - n * dot(n, tangent), vec3<f32>(0.0));
    if all(t == vec3<f32>(0.0)) {
        return frame_new(n);
    }
    return Frame(t, cross(n, t), n);
}

fn radiance(origin: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
    var o = origin;
    var d = direction;
    var total = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    var prev_pdf = INF;
    var clamp_to = INF;
    if params.clamp_indirect > 0.0 {
        clamp_to = params.clamp_indirect;
    }
    var first = true;
    f_albedo = vec3<f32>(0.0);
    f_normal = vec3<f32>(0.0);
    f_depth = 0.0;
    f_position = vec3<f32>(0.0);
    f_has_position = false;
    // Absorption of the object the path is inside, if any.
    var medium = vec3<f32>(0.0);
    var in_medium = false;
    // Cut-outs passed without spending a bounce (bounded, so a stack of them cannot loop forever).
    var passes = 0u;
    var depth = 0u;
    loop {
        if depth > params.max_depth {
            break;
        }
        let hit = trace_closest(o, d);
        if hit.slot == NO_HIT {
            if first {
                f_albedo = min(env_radiance(d), vec3<f32>(1.0));
            }
            let sun = sun_radiance(d);
            if sun.w > 0.0 {
                // The path found a sun's disk by itself; light sampling could have aimed at it too.
                var w = 1.0;
                if prev_pdf < INF {
                    if params.strategy == 0u {
                        w = power_heuristic(prev_pdf, sun.w);
                    } else if params.strategy == 1u {
                        w = 0.0;
                    }
                }
                var c = sanitize(throughput * sun.rgb * w);
                if depth > 0u {
                    c = min(c, vec3<f32>(clamp_to));
                }
                total += c;
            }
            if params.has_env != 0u {
                var w = 1.0;
                if prev_pdf < INF {
                    if params.strategy == 0u {
                        w = power_heuristic(prev_pdf, pdf_environment(d));
                    } else if params.strategy == 1u {
                        w = 0.0;
                    }
                }
                var c = sanitize(throughput * env_radiance(d) * w);
                if depth > 0u {
                    c = min(c, vec3<f32>(clamp_to));
                }
                total += c;
            }
            break;
        }
        let tri = tris[hit.slot];
        let at = attrs[hit.slot];
        let bw = 1.0 - hit.u - hit.v;
        let p = tri.p0 + tri.e1 * hit.u + tri.e2 * hit.v;
        let gn = cross(tri.e1, tri.e2);
        let gl = length(gn);
        if !(gl > 0.0) {
            break;
        }
        var ng = gn / gl;
        let front = dot(ng, d) < 0.0;
        if !front {
            ng = -ng;
        }
        var ns = normalize_or(at.n0 * bw + at.n1 * hit.u + at.n2 * hit.v, ng);
        if dot(ns, ng) < 0.0 {
            ns = -ns;
        }
        let uv = at.uv0 * bw + at.uv1 * hit.u + at.uv2 * hit.v;
        let m = materials[at.material];
        if in_medium {
            throughput *= exp(-medium * hit.t);
        }
        if (m.flags & FLAG_CUTOUTS) != 0u && passes < 256u {
            let opacity = mat_opacity(m, uv);
            if opacity < 1.0 && next1() >= opacity {
                passes += 1u;
                o = spawn_origin(p, ng, d);
                continue;
            }
        }
        if at.light >= 0 {
            let l = lights[u32(at.light)];
            if front || l.kind == 1u {
                var w = 1.0;
                if prev_pdf < INF {
                    if params.strategy == 0u {
                        let c = abs(dot(ng, d));
                        var pdf_light = 0.0;
                        if c > 0.0 && l.area > 0.0 {
                            pdf_light = select_pdf(u32(at.light), o) * hit.t * hit.t / (l.area * c);
                        }
                        w = power_heuristic(prev_pdf, pdf_light);
                    } else if params.strategy == 1u {
                        w = 0.0;
                    }
                }
                var c = sanitize(throughput * l.radiance * w);
                if depth > 0u {
                    c = min(c, vec3<f32>(clamp_to));
                }
                total += c;
            }
        }
        var frame = frame_new(ns);
        if depth == params.max_depth {
            if first {
                f_albedo = m.base_color;
                f_normal = frame.n;
                f_depth = hit.t;
                f_position = p;
                f_has_position = true;
            }
            break;
        }
        if (m.flags & FLAG_AXES) != 0u {
            frame = shading_frame(m, tri, at, uv, ng, frame);
        }
        let transmissive = (m.flags & FLAG_TRANSMISSIVE) != 0u;
        let inside = transmissive && !front;
        let surf = surface_params(m, uv);
        if first {
            first = false;
            f_albedo = surf.base;
            f_normal = frame.n;
            f_depth = hit.t;
            f_position = p;
            f_has_position = true;
        }
        let bsdf = bsdf_new(surf, to_local(frame, -d), inside);

        // Direct light from one chosen light.
        if params.strategy != 2u && params.light_count > 0u {
            let pick = next1();
            let u = next2();
            let ls = sample_light(p, pick, u);
            if ls.pdf > 0.0 {
                let wi = to_local(frame, ls.wi);
                // The shading and the real surface must agree on which side the light is.
                let side = dot(ls.wi, ng);
                if side * wi.z > 0.0 && (side > 0.0 || transmissive) {
                    let fe = bsdf_eval(bsdf, wi);
                    if max3(fe.rgb) > 0.0 && !occluded(p, ng, ls.wi, ls.distance) {
                        var w = 1.0;
                        if !ls.delta && params.strategy == 0u {
                            w = power_heuristic(ls.pdf, fe.w);
                        }
                        total += min(sanitize(throughput * fe.rgb * ls.radiance * (w / ls.pdf)), vec3<f32>(clamp_to));
                    }
                }
            }
        }

        // Continue the path.
        let lobe = next1();
        let u = next2();
        let bs = bsdf_sample(bsdf, lobe, u);
        if !(bs.pdf > 0.0) {
            break;
        }
        let wi = to_world(frame, bs.wi);
        let side = dot(wi, ng);
        if side * bs.wi.z <= 0.0 {
            // The shading normal sent the direction to the other side of the real surface.
            break;
        }
        if side < 0.0 {
            // Crossed the surface: entering sets the object's absorption, leaving clears it.
            in_medium = front && max3(m.absorption) > 0.0;
            medium = m.absorption;
        }
        throughput *= bs.weight;
        prev_pdf = bs.pdf;
        if depth >= params.roulette_start {
            let q = min(max3(throughput), 0.95);
            if q <= 0.0 || next1() >= q {
                break;
            }
            throughput /= q;
        }
        if !finite3(throughput) {
            break;
        }
        o = spawn_origin(p, ng, wi);
        d = wi;
        depth += 1u;
    }
    return total;
}

// Where a world point appeared in the previous frame, in the coordinates of `ndc`
// (z: 0 when it was behind the camera).
fn project_previous(p: vec3<f32>) -> vec3<f32> {
    let d = p - params.prev_origin.xyz;
    let x = dot(d, params.prev_right.xyz);
    let y = dot(d, params.prev_up.xyz);
    let z = dot(d, params.prev_forward.xyz);
    if params.prev_forward.w != 0.0 {
        return vec3<f32>(x / params.prev_origin.w, y / params.prev_right.w, 1.0);
    }
    if z > 1e-6 {
        return vec3<f32>(x / (z * params.prev_origin.w), y / (z * params.prev_right.w), 1.0);
    }
    return vec3<f32>(0.0);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.size.x || gid.y >= params.size.y {
        return;
    }
    let seed = hash_combine(hash_combine(params.seed, gid.x), gid.y);
    var acc = vec3<f32>(0.0);
    var albedo = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0);
    var depth = 0.0;
    var luminance_sq = 0.0;
    var motion = vec2<f32>(0.0);
    for (var s = 0u; s < params.samples; s++) {
        s_index = params.first_sample + s;
        s_seed = seed;
        s_dim = 0u;
        // Pixels share one sequence along the z-curve while its 32-bit index lasts.
        if params.z_bits < 32u && (params.z_bits == 0u || s_index < (1u << (32u - params.z_bits))) {
            s_index = (s_index << params.z_bits) | spread_bits(gid.x) | (spread_bits(gid.y) << 1u);
            s_seed = hash32(params.seed ^ 0x5bd1e995u);
        }
        let jitter = next2();
        let lens = next2();
        let ndc = vec2<f32>(
            (f32(gid.x) + jitter.x) / f32(params.size.x) * 2.0 - 1.0,
            1.0 - (f32(gid.y) + jitter.y) / f32(params.size.y) * 2.0,
        );
        let sx = ndc.x * params.cam_origin.w;
        let sy = ndc.y * params.cam_right.w;
        var o = params.cam_origin.xyz;
        var d = params.cam_forward.xyz;
        if params.ortho != 0u {
            o += params.cam_right.xyz * sx + params.cam_up.xyz * sy;
        } else {
            let dir = params.cam_forward.xyz + params.cam_right.xyz * sx + params.cam_up.xyz * sy;
            d = normalize(dir);
            if params.cam_up.w > 0.0 {
                let focus = o + dir * params.cam_forward.w;
                let l = concentric_disk(lens) * params.cam_up.w;
                o += params.cam_right.xyz * l.x + params.cam_up.xyz * l.y;
                d = normalize(focus - o);
            }
        }
        let l = radiance(o, d);
        acc += l;
        let lum = luminance(l);
        luminance_sq += lum * lum;
        albedo += f_albedo;
        normal += f_normal;
        depth += f_depth;
        if f_has_position && params.prev_up.w != 0.0 {
            let before = project_previous(f_position);
            if before.z != 0.0 {
                motion += vec2<f32>((ndc.x - before.x) * 0.5 * f32(params.size.x), (before.y - ndc.y) * 0.5 * f32(params.size.y));
            }
        }
    }
    let pixel = gid.y * params.size.x + gid.x;
    accum[pixel].radiance += vec4<f32>(acc, f32(params.samples));
    accum[pixel].albedo += vec4<f32>(albedo, depth);
    accum[pixel].normal += vec4<f32>(normal, luminance_sq);
    accum[pixel].motion += vec4<f32>(motion, 0.0, 0.0);
}
