// Hardware ray queries: the third implementation of `intersect` (see bvh_binary.wgsl). Binding 1
// holds the scene's acceleration structure instead of BVH nodes. Geometry is opaque, so the
// hardware decides every hit; the kernel only reads the committed one. The triangles are the
// software BVH's, in slot order, so `primitive_index` is the slot the rest of the kernel expects.
@group(0) @binding(1) var accel: acceleration_structure;

// The closest hit (mode 0) or any hit (modes 1 and 2; cut-outs are not supported here).
fn intersect(o: vec3<f32>, d: vec3<f32>, limit: f32, mode: u32) -> Hit {
    var best = Hit(limit, 0.0, 0.0, NO_HIT);
    var rq: ray_query;
    // Flag 4: stop at the first hit found (an any-hit query).
    let flags = select(0u, 4u, mode != 0u);
    rayQueryInitialize(&rq, accel, RayDesc(flags, 0xffu, 0.0, limit, o, d));
    while rayQueryProceed(&rq) {
    }
    let hit = rayQueryGetCommittedIntersection(&rq);
    // Kind 1: a triangle.
    if hit.kind == 1u && hit.t > 0.0 {
        best = Hit(hit.t, hit.barycentrics.x, hit.barycentrics.y, hit.primitive_index);
    }
    return best;
}
