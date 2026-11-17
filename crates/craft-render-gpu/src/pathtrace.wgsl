// The Craft path tracer as one compute kernel. It mirrors craft-render-core statement for
// statement (same sample sequence, same surface model, same estimator); the CPU crate documents
// the sources of each part. One invocation renders `params.samples` samples of one pixel.

struct Params {
    cam_origin: vec4<f32>,  // w: half width of the image plane
    cam_right: vec4<f32>,   // w: half height
    cam_up: vec4<f32>,      // w: aperture radius
    cam_forward: vec4<f32>, // w: focus distance
    env: vec4<f32>,         // rgb: environment radiance, w: its density over solid angle
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
    strategy: u32, // 0 both estimators (MIS), 1 light sampling only, 2 BSDF sampling only
    z_bits: u32,   // index bits of the z-curve pixel order; 255: every pixel on its own
    pad0: u32,
    pad1: u32,
    pad2: u32,
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
    pad: u32,
}

struct Mat {
    base: vec3<f32>,
    metalness: f32,
    tint: vec3<f32>,
    roughness: f32,
    d0: f32,
    d90: f32,
    ior: f32,
    pad1: f32,
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

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> nodes: array<Node>;
@group(0) @binding(2) var<storage, read> tris: array<Tri>;
@group(0) @binding(3) var<storage, read> attrs: array<Attr>;
@group(0) @binding(4) var<storage, read> materials: array<Mat>;
@group(0) @binding(5) var<storage, read> lights: array<LightEntry>;
@group(0) @binding(6) var<storage, read> albedo: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> accum: array<vec4<f32>>;

const PI: f32 = 3.14159265358979;
const INF: f32 = 3.0e38;
const NO_HIT: u32 = 0xffffffffu;
const TABLE: u32 = 32u;
const ALPHA_MIN: f32 = 1e-3;

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

// The closest hit, or any hit when `any` is set (shadow rays).
fn intersect(o: vec3<f32>, d: vec3<f32>, limit: f32, any: bool) -> Hit {
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
                    best = Hit(t, u, v, slot);
                    if any {
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
    return intersect(o, d, INF, false);
}

fn trace_any(o: vec3<f32>, d: vec3<f32>, tmax: f32) -> bool {
    return intersect(o, d, tmax, true).slot != NO_HIT;
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

fn ggx_d(h: vec3<f32>, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = (h.x * h.x + h.y * h.y) / a2 + h.z * h.z;
    return 1.0 / (PI * a2 * d * d);
}

fn ggx_lambda(w: vec3<f32>, alpha: f32) -> f32 {
    let z2 = w.z * w.z;
    if z2 <= 0.0 {
        return INF;
    }
    let t2 = alpha * alpha * (w.x * w.x + w.y * w.y) / z2;
    return (sqrt(1.0 + t2) - 1.0) * 0.5;
}

fn sample_visible_normal(wo: vec3<f32>, alpha: f32, u: vec2<f32>) -> vec3<f32> {
    let vh = normalize(vec3<f32>(alpha * wo.x, alpha * wo.y, wo.z));
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
    return normalize(vec3<f32>(alpha * nh.x, alpha * nh.y, max(nh.z, 1e-6)));
}

fn schlick_weight(c: f32) -> f32 {
    let m = clamp(1.0 - c, 0.0, 1.0);
    let m2 = m * m;
    return m2 * m2 * m;
}

fn albedo_lookup(c: f32, roughness: f32) -> vec2<f32> {
    let m = f32(TABLE - 1u);
    let x = clamp(c, 0.0, 1.0) * m;
    let y = clamp(roughness, 0.0, 1.0) * m;
    let x0 = min(u32(x), TABLE - 2u);
    let y0 = min(u32(y), TABLE - 2u);
    let fx = x - f32(x0);
    let fy = y - f32(y0);
    let p00 = albedo[y0 * TABLE + x0].xy;
    let p10 = albedo[y0 * TABLE + x0 + 1u].xy;
    let p01 = albedo[(y0 + 1u) * TABLE + x0].xy;
    let p11 = albedo[(y0 + 1u) * TABLE + x0 + 1u].xy;
    return mix(mix(p00, p10, fx), mix(p01, p11, fx), fy);
}

struct Bsdf {
    wo: vec3<f32>,
    alpha: f32,
    diffuse: vec3<f32>,
    p_spec: f32,
    metal_f0: vec3<f32>,
    eta: f32,
    metal_scale: vec3<f32>,
    diel_scale: vec3<f32>,
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

fn bsdf_new(m: Mat, wo_in: vec3<f32>) -> Bsdf {
    var wo = wo_in;
    if wo.z <= 1e-4 {
        wo = normalize(vec3<f32>(wo.x, wo.y, 1e-4));
    }
    let alpha = max(m.roughness * m.roughness, ALPHA_MIN);
    let ab = albedo_lookup(wo.z, sqrt(alpha));
    let e = clamp(ab.x + ab.y, 1e-4, 1.0);
    let k = (1.0 - e) / e;
    let metal_ms = vec3<f32>(1.0) + m.base * k;
    let metal_e = (m.base * ab.x + vec3<f32>(ab.y)) * metal_ms;
    let diel_ms = 1.0 + m.d0 * k;
    let diel_e = clamp((m.d0 * ab.x + m.d90 * ab.y) * diel_ms, 0.0, 1.0);
    let mt = m.metalness;
    let dl = 1.0 - m.metalness;
    var b: Bsdf;
    b.wo = wo;
    b.alpha = alpha;
    b.metal_f0 = m.base;
    b.metal_scale = metal_ms * mt;
    b.diel_scale = m.tint * (m.d90 * diel_ms * dl);
    b.eta = m.ior;
    if abs(b.eta - 1.0) < 1e-4 {
        b.eta = 1.0001;
    }
    b.diffuse = m.base * (dl * (1.0 - diel_e));
    let spec_albedo = luminance(metal_e * mt + m.tint * (diel_e * dl));
    let diff_albedo = luminance(b.diffuse);
    let total = spec_albedo + diff_albedo;
    b.p_spec = 0.0;
    if total > 0.0 {
        b.p_spec = clamp(spec_albedo / total, 0.0, 1.0);
    }
    return b;
}

// rgb: f·cos, w: pdf
fn bsdf_eval(b: Bsdf, wi: vec3<f32>) -> vec4<f32> {
    if wi.z <= 0.0 {
        return vec4<f32>(0.0);
    }
    var f = b.diffuse * (wi.z / PI);
    var pdf = (1.0 - b.p_spec) * wi.z / PI;
    if b.p_spec > 0.0 {
        let hs = b.wo + wi;
        let hl = length(hs);
        if hl > 0.0 {
            let h = hs / hl;
            if h.z > 0.0 {
                let d = ggx_d(h, b.alpha);
                let lo = ggx_lambda(b.wo, b.alpha);
                let li = ggx_lambda(wi, b.alpha);
                let ch = dot(b.wo, h);
                let fr = b.diel_scale * fresnel_dielectric(ch, b.eta) + b.metal_scale * (b.metal_f0 + (vec3<f32>(1.0) - b.metal_f0) * schlick_weight(ch));
                let dv = d / (4.0 * b.wo.z);
                if dv < INF {
                    f += fr * (dv / (1.0 + lo + li));
                    pdf += b.p_spec * dv / (1.0 + lo);
                }
            }
        }
    }
    return vec4<f32>(f, pdf);
}

fn bsdf_sample_dir(b: Bsdf, lobe: f32, u: vec2<f32>) -> vec3<f32> {
    if lobe < b.p_spec {
        let h = sample_visible_normal(b.wo, b.alpha, u);
        return h * (2.0 * dot(b.wo, h)) - b.wo;
    }
    return cosine_hemisphere(u);
}

// ---- lights -----------------------------------------------------------------------------

struct LightSample {
    wi: vec3<f32>,
    distance: f32,
    radiance: vec3<f32>,
    pdf: f32, // 0 when no sample was produced
    delta: bool,
}

fn sample_light(p: vec3<f32>, pick: f32, u: vec2<f32>) -> LightSample {
    var out = LightSample(vec3<f32>(0.0), 0.0, vec3<f32>(0.0), 0.0, false);
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
    let l = lights[min(lo, params.light_count - 1u)];
    if l.pmf <= 0.0 {
        return out;
    }
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
        let c = -dot(wi, normalize(cross(l.e1, l.e2)));
        if c == 0.0 || (c < 0.0 && l.kind == 0u) || l.area <= 0.0 {
            return out;
        }
        return LightSample(wi, dist, l.radiance, l.pmf * dist2 / (l.area * abs(c)), false);
    }
    if l.kind == 2u {
        let d = l.p0 - p;
        let dist2 = dot(d, d);
        if dist2 <= 0.0 {
            return out;
        }
        return LightSample(d / sqrt(dist2), sqrt(dist2), l.radiance / dist2, l.pmf, true);
    }
    if l.kind == 3u {
        return LightSample(l.p0, INF, l.radiance, l.pmf, true);
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
        return LightSample(wi, sqrt(dist2), l.radiance * (falloff / dist2), l.pmf, true);
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
        return LightSample(tx * local.x + ty * local.y + n * local.z, INF, l.radiance, l.pmf, true);
    }
    return LightSample(uniform_sphere(u), INF, l.radiance, l.pmf / (4.0 * PI), false);
}

// ---- transport --------------------------------------------------------------------------

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
    for (var depth = 0u; depth <= params.max_depth; depth++) {
        let hit = trace_closest(o, d);
        if hit.slot == NO_HIT {
            if max3(params.env.rgb) > 0.0 {
                var w = 1.0;
                if prev_pdf < INF {
                    if params.strategy == 0u {
                        w = power_heuristic(prev_pdf, params.env.w);
                    } else if params.strategy == 1u {
                        w = 0.0;
                    }
                }
                var c = sanitize(throughput * params.env.rgb * w);
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
        if gl <= 0.0 {
            break;
        }
        var ng = gn / gl;
        let front = dot(ng, d) < 0.0;
        if !front {
            ng = -ng;
        }
        var ns = at.n0 * bw + at.n1 * hit.u + at.n2 * hit.v;
        let nl = length(ns);
        if nl > 0.0 {
            ns = ns / nl;
        } else {
            ns = ng;
        }
        if dot(ns, ng) < 0.0 {
            ns = -ns;
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
                            pdf_light = l.pmf * hit.t * hit.t / (l.area * c);
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
        if depth == params.max_depth {
            break;
        }
        // Shading frame (Duff et al. 2017).
        let sign = select(-1.0, 1.0, ns.z >= 0.0);
        let fa = -1.0 / (sign + ns.z);
        let fb = ns.x * ns.y * fa;
        let tx = vec3<f32>(1.0 + sign * ns.x * ns.x * fa, sign * fb, -sign * ns.x);
        let ty = vec3<f32>(fb, sign + ns.y * ns.y * fa, -ns.y);
        let wo_world = -d;
        let bsdf = bsdf_new(materials[at.material], vec3<f32>(dot(wo_world, tx), dot(wo_world, ty), dot(wo_world, ns)));

        if params.strategy != 2u && params.light_count > 0u {
            let pick = next1();
            let u = next2();
            let ls = sample_light(p, pick, u);
            if ls.pdf > 0.0 && dot(ls.wi, ng) > 0.0 {
                let fe = bsdf_eval(bsdf, vec3<f32>(dot(ls.wi, tx), dot(ls.wi, ty), dot(ls.wi, ns)));
                if max3(fe.rgb) > 0.0 && !occluded(p, ng, ls.wi, ls.distance) {
                    var w = 1.0;
                    if !ls.delta && params.strategy == 0u {
                        w = power_heuristic(ls.pdf, fe.w);
                    }
                    total += min(sanitize(throughput * fe.rgb * ls.radiance * (w / ls.pdf)), vec3<f32>(clamp_to));
                }
            }
        }

        let lobe = next1();
        let u = next2();
        let wi_local = bsdf_sample_dir(bsdf, lobe, u);
        let fe = bsdf_eval(bsdf, wi_local);
        if !(fe.w > 0.0) {
            break;
        }
        let wi = tx * wi_local.x + ty * wi_local.y + ns * wi_local.z;
        if dot(wi, ng) <= 0.0 {
            break;
        }
        throughput *= fe.rgb / fe.w;
        prev_pdf = fe.w;
        if depth >= params.roulette_start {
            let q = min(max3(throughput), 0.95);
            if q <= 0.0 || next1() >= q {
                break;
            }
            throughput /= q;
        }
        if !(all(throughput == throughput) && all(throughput < vec3<f32>(INF))) {
            break;
        }
        o = spawn_origin(p, ng, wi);
        d = wi;
    }
    return total;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.size.x || gid.y >= params.size.y {
        return;
    }
    let seed = hash_combine(hash_combine(params.seed, gid.x), gid.y);
    var acc = vec3<f32>(0.0);
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
        acc += radiance(o, d);
    }
    let pixel = gid.y * params.size.x + gid.x;
    accum[pixel] = accum[pixel] + vec4<f32>(acc, f32(params.samples));
}
