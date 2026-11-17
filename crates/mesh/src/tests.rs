use super::*;
use proptest::prelude::*;

fn quad_grid(n: u32) -> PolyMesh {
    let mut pos = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            pos.push(Vec3::new(i as f32, 0.0, j as f32));
        }
    }
    let mut faces = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            faces.push(vec![a, a + n + 1, a + n + 2, a + 1]);
        }
    }
    PolyMesh::from_faces(pos, &faces).unwrap()
}

fn cube() -> PolyMesh {
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    let pos = vec![p(-1., -1., -1.), p(1., -1., -1.), p(1., 1., -1.), p(-1., 1., -1.), p(-1., -1., 1.), p(1., -1., 1.), p(1., 1., 1.), p(-1., 1., 1.)];
    let faces = vec![vec![0, 3, 2, 1], vec![4, 5, 6, 7], vec![0, 1, 5, 4], vec![2, 3, 7, 6], vec![1, 2, 6, 5], vec![0, 4, 7, 3]];
    PolyMesh::from_faces(pos, &faces).unwrap()
}

#[test]
fn closed_cube_has_euler_characteristic_two() {
    let m = cube();
    assert_eq!((m.vertex_count(), m.edge_count(), m.face_count()), (8, 12, 6));
    assert_eq!(m.half_edge_count(), 24);
    assert!((0..12).all(|e| !m.is_boundary_edge(e)));
    for v in 0..8 {
        assert_eq!(m.vertex_neighbors(v).count(), 3);
    }
    // Outward normals.
    for f in 0..6 {
        assert!(m.face_normal(f).dot(m.face_center(f)) > 0.0, "face {f} points inward");
    }
}

#[test]
fn open_grid_has_boundary_loop() {
    let m = quad_grid(3);
    assert_eq!(m.face_count(), 9);
    assert_eq!(m.edge_count(), 24);
    let boundary = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).count();
    assert_eq!(boundary, 12);
    // Corner vertex has two neighbours, interior four.
    assert_eq!(m.vertex_neighbors(0).count(), 2);
    assert_eq!(m.vertex_neighbors(5).count(), 4);
    assert!(m.is_boundary_vertex(0));
    assert!(!m.is_boundary_vertex(5));
}

#[test]
fn repairs_bad_input() {
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(f32::NAN, 0.0, 0.0)];
    let polys = vec![
        Polygon::new(vec![0, 1, 2]),
        Polygon::new(vec![0, 1, 99]), // out of range
        Polygon::new(vec![0, 0, 1]),  // degenerate
        Polygon::new(vec![0, 1, 3]),  // wrong orientation against the first
        Polygon::new(vec![0, 1, 4]),  // third face on edge 0-1: detached
    ];
    let (m, r) = PolyMesh::build(pos, polys).unwrap();
    assert_eq!(r.dropped, 2);
    assert_eq!(r.flipped, 1);
    assert_eq!(r.detached, 1);
    assert_eq!(r.fixed_positions, 1);
    assert_eq!(m.face_count(), 3);
    m.validate().unwrap();
}

#[test]
fn polygons_round_trip_with_uvs_and_patches() {
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
    let mut poly = Polygon::with_uvs(vec![0, 1, 2, 3], vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]);
    poly.patch = 7;
    let (m, _) = PolyMesh::build(pos.clone(), vec![poly.clone()]).unwrap();
    assert_eq!(m.to_polygons(), vec![poly]);
    let json = serde_json::to_string(&m).unwrap();
    let back: PolyMesh = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
    assert_eq!(m.triangulate().0.len(), 2);
}

#[test]
fn hostile_serialized_mesh_is_rejected_or_repaired() {
    let back: Result<PolyMesh, _> = serde_json::from_str(r#"{"positions":[[0,0,0]],"polygons":[{"verts":[0,5,9]}]}"#);
    let m = back.unwrap();
    assert_eq!(m.face_count(), 0);
}

#[test]
fn vertex_normals_of_grid_point_up() {
    let m = quad_grid(2);
    for n in m.vertex_normals() {
        assert!((n - Vec3::Y).length() < 1e-5 || (n + Vec3::Y).length() < 1e-5);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]
    /// Any polygon soup either builds into a valid mesh or fails cleanly.
    #[test]
    fn arbitrary_soups_build_valid_meshes(
        nverts in 0usize..20,
        faces in prop::collection::vec(prop::collection::vec(0u32..24, 0..7), 0..30),
        coords in prop::collection::vec(-10.0f32..10.0, 60),
    ) {
        let pos: Vec<Vec3> = (0..nverts).map(|i| Vec3::new(coords[i * 3], coords[i * 3 + 1], coords[i * 3 + 2])).collect();
        if let Ok((m, _)) = PolyMesh::build(pos, faces.into_iter().map(Polygon::new).collect()) {
            prop_assert!(m.validate().is_ok());
            let again = PolyMesh::build(m.positions().to_vec(), m.to_polygons()).unwrap().0;
            prop_assert_eq!(again.face_count(), m.face_count());
            for v in 0..m.vertex_count() as u32 {
                prop_assert!(m.vertex_out(v).count() <= m.half_edge_count());
            }
        }
    }
}
