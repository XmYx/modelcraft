// Mesh stats: prints vertices, faces, triangles, area and volume of every mesh, then the totals.
let total_tris = 0;
let total_area = 0.0;
let meshes = 0;
for id in object_ids() {
    if kind_of(id) != "Mesh" { continue; }
    let s = evaluate(id).stats();
    let volume = if s.volume == () { "open" } else { s.volume.round_to(3) };
    print(`${name_of(id)}: ${s.vertices} vertices, ${s.faces} faces, ${s.triangles} triangles, area ${s.area.round_to(3)}, volume ${volume}`);
    total_tris += s.triangles;
    total_area += s.area;
    meshes += 1;
}
print(`${meshes} meshes, ${total_tris} triangles, area ${total_area.round_to(3)}`);
total_tris
