// Compressed 8-wide BVH traversal (craft-render-bvh::wide), the other implementation of `intersect`.
// A node is 28 words, seven rows of four: the corner `p` (3 floats) and the scale exponents (one
// signed byte per axis); eight child kinds (bytes: 0 empty, 1 interior, 0x80|count leaf); the
// quantised child boxes (bytes: x, y, z lows then highs); eight words (interior: child node; leaf:
// first triangle); two words of padding.
@group(0) @binding(1) var<storage, read> nodes: array<vec4<u32>>;

const NODE_ROWS: u32 = 7u;
const WIDE_STACK: u32 = 128u;

fn pick_byte(a: u32, b: u32, k: u32) -> f32 {
    // Byte k (0..8) of the eight bytes held in words a and b.
    let word = select(a, b, k >= 4u);
    return f32((word >> ((k & 3u) * 8u)) & 0xffu);
}

fn pow2_exp(byte: u32) -> f32 {
    // A signed byte e: 2^e.
    let e = i32(byte << 24u) >> 24u;
    return bitcast<f32>(u32(e + 127) << 23u);
}

fn intersect(o: vec3<f32>, d: vec3<f32>, limit: f32, mode: u32) -> Hit {
    var best = Hit(limit, 0.0, 0.0, NO_HIT);
    if params.node_count == 0u {
        return best;
    }
    let inv = vec3<f32>(1.0) / d;
    var stack_node: array<u32, 128>;
    var stack_t: array<f32, 128>;
    var sp = 0u;
    var at = 0u;
    loop {
        let row = at * NODE_ROWS;
        let r0 = nodes[row];
        let r1 = nodes[row + 1u];
        let r2 = nodes[row + 2u];
        let r3 = nodes[row + 3u];
        let r4 = nodes[row + 4u];
        let p = vec3<f32>(bitcast<f32>(r0.x), bitcast<f32>(r0.y), bitcast<f32>(r0.z));
        let s = vec3<f32>(pow2_exp(r0.w & 0xffu), pow2_exp((r0.w >> 8u) & 0xffu), pow2_exp((r0.w >> 16u) & 0xffu));
        // Interior hits of this node, farthest first by insertion (at most eight).
        var hit_node: array<u32, 8>;
        var hit_t: array<f32, 8>;
        var hits = 0u;
        for (var k = 0u; k < 8u; k++) {
            let kind = (select(r1.x, r1.y, k >= 4u) >> ((k & 3u) * 8u)) & 0xffu;
            if kind == 0u {
                continue;
            }
            let lo = p + vec3<f32>(pick_byte(r1.z, r1.w, k), pick_byte(r2.x, r2.y, k), pick_byte(r2.z, r2.w, k)) * s;
            let hi = p + vec3<f32>(pick_byte(r3.x, r3.y, k), pick_byte(r3.z, r3.w, k), pick_byte(r4.x, r4.y, k)) * s;
            let t0 = (lo - o) * inv;
            let t1 = (hi - o) * inv;
            let a = min(t0, t1);
            let b = max(t0, t1);
            let near = max(max(a.x, max(a.y, a.z)), 0.0);
            let far = min(min(b.x, min(b.y, b.z)), best.t);
            if near > far {
                continue;
            }
            // The child's word: data 0..8 start at word 18 (row 4, component 2).
            let dw = 18u + k;
            let child = nodes[row + (dw >> 2u)][dw & 3u];
            if (kind & 0x80u) != 0u {
                if leaf_hit(child, kind & 0x7fu, o, d, mode, &best) {
                    return best;
                }
            } else {
                var j = hits;
                while j > 0u && hit_t[j - 1u] < near {
                    hit_t[j] = hit_t[j - 1u];
                    hit_node[j] = hit_node[j - 1u];
                    j -= 1u;
                }
                hit_t[j] = near;
                hit_node[j] = child;
                hits += 1u;
            }
        }
        // The farthest goes on the stack first, so the nearest is popped first.
        for (var j = 0u; j < hits; j++) {
            if hit_t[j] <= best.t && sp < WIDE_STACK {
                stack_node[sp] = hit_node[j];
                stack_t[sp] = hit_t[j];
                sp += 1u;
            }
        }
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
    return best;
}
