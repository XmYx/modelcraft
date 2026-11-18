// Binary BVH traversal (32-byte nodes), one of two implementations of `intersect`.

struct Node {
    lo: vec3<f32>,
    a: u32,
    hi: vec3<f32>,
    count: u32,
}

@group(0) @binding(1) var<storage, read> nodes: array<Node>;

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
            if leaf_hit(a, count, o, d, mode, &best) {
                return best;
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

