// ModelCraft viewport shading: a studio-style key/fill/rim light set over a soft hemisphere,
// with sculpt mask darkening, patch tints and a selection rim. Output is gamma-encoded because
// the target is a plain (non-sRGB) texture that the UI samples as sRGB colour.

struct Camera {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
};

struct Draw {
    model: mat4x4<f32>,
    color: vec4<f32>,
    // x: selected, y: show mask, z: patch tint, w: roughness
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> cam: Camera;
@group(1) @binding(0) var<uniform> draw: Draw;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    @location(3) patch_id: f32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    // Flat: a triangle takes the patch of its first vertex, so shared vertices do not blend ids.
    @location(3) @interpolate(flat) patch_id: f32,
};

@vertex
fn vs_mesh(v: VIn) -> VOut {
    var o: VOut;
    let w = draw.model * vec4<f32>(v.pos, 1.0);
    o.world = w.xyz;
    o.clip = cam.view_proj * w;
    // Uniform scale assumed for normals; non-uniform scale is renormalised (good enough for a viewport).
    o.normal = (draw.model * vec4<f32>(v.normal, 0.0)).xyz;
    o.mask = v.mask;
    o.patch_id = v.patch_id;
    return o;
}

fn hash(n: f32) -> vec3<f32> {
    let p = vec3<f32>(n * 12.9898, n * 78.233, n * 37.719);
    return fract(sin(p) * 43758.5453);
}

@fragment
fn fs_mesh(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.normal);
    if (!front) {
        n = -n;
    }
    let v = normalize(cam.eye.xyz - i.world);
    var base = draw.color.rgb;
    if (draw.flags.z > 0.5) {
        base = mix(base, 0.35 + 0.6 * hash(i.patch_id + 1.0), 0.55);
    }
    // Light rig in view-relative terms so the model always reads well.
    let key = normalize(v + vec3<f32>(0.45, 0.65, 0.2));
    let fill = normalize(vec3<f32>(-0.6, 0.2, 0.4));
    let rough = clamp(draw.flags.w, 0.05, 1.0);
    let diff = max(dot(n, key), 0.0) * 0.62 + max(dot(n, fill), 0.0) * 0.12;
    let hemi = mix(vec3<f32>(0.07, 0.065, 0.06), vec3<f32>(0.20, 0.22, 0.25), n.y * 0.5 + 0.5);
    let h = normalize(key + v);
    let shin = mix(96.0, 6.0, rough);
    let spec = pow(max(dot(n, h), 0.0), shin) * (1.0 - rough) * 0.35;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.12;
    var c = base * (diff + hemi) + vec3<f32>(spec) + vec3<f32>(rim);
    if (draw.flags.y > 0.5) {
        c = mix(c, c * vec3<f32>(0.25, 0.27, 0.32), clamp(i.mask, 0.0, 1.0));
    }
    if (draw.flags.x > 0.5) {
        c = c + vec3<f32>(1.0, 0.55, 0.15) * pow(1.0 - max(dot(n, v), 0.0), 2.0) * 0.55;
    }
    if (!front) {
        c = c * vec3<f32>(0.55, 0.45, 0.45);
    }
    return vec4<f32>(pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), 1.0);
}

struct LIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(v: LIn) -> LOut {
    var o: LOut;
    var c = cam.view_proj * vec4<f32>(v.pos, 1.0);
    // Pull lines slightly towards the eye so wireframes sit on top of their surface.
    c.z = c.z - 0.00002 * c.w;
    o.clip = c;
    o.color = v.color;
    return o;
}

@fragment
fn fs_line(i: LOut) -> @location(0) vec4<f32> {
    return vec4<f32>(pow(i.color.rgb, vec3<f32>(1.0 / 2.2)), i.color.a);
}
