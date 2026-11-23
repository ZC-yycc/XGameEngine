//! builtin 网格必须是真实几何，且与旧端 `crates/xgame-render/src/mesh.rs` 的生成器逐顶点一致。

use xgc_render::builtin_mesh;

#[test]
fn builtin_models_have_real_geometry() {
    let cube = builtin_mesh("builtin://cube").unwrap();
    // 旧端 cube 每面 4 个角（24 顶点）、6 面各 2 个三角形。
    assert_eq!(cube.positions.len(), 24);
    assert_eq!(cube.indices.len(), 36);
    let sphere = builtin_mesh("builtin://sphere").unwrap();
    // 旧端 sphere(0.5, 24, 32)：(rings+1) × (sectors+1) 个顶点。
    assert_eq!(sphere.positions.len(), 25 * 33);
    assert_eq!(sphere.indices.len(), 24 * 32 * 6);
    let cylinder = builtin_mesh("builtin://cylinder").unwrap();
    // 旧端 cylinder(0.5, 1.0, 32)：侧面 33 × 2 + 顶盖 34 + 底盖 34。
    assert_eq!(cylinder.positions.len(), 66 + 34 + 34);
    assert_eq!(cylinder.indices.len(), 32 * 6 + 32 * 3 + 32 * 3);
    let plane = builtin_mesh("builtin://plane").unwrap();
    assert_eq!(plane.positions.len(), 4);
    assert_eq!(plane.indices.len(), 6);
    let triangle = builtin_mesh("builtin://triangle").unwrap();
    assert_eq!(triangle.positions.len(), 3);
    assert!(builtin_mesh("builtin://unknown").is_none());
}

/// 内建图元与旧端一样自带顶点法线与 UV：缺法线会退回屏幕导数（分面着色），
/// 红球就会重新出现「高光位置不对 + 横向条带」。
#[test]
fn builtin_models_carry_vertex_normals_and_uvs() {
    for model in [
        "builtin://cube",
        "builtin://sphere",
        "builtin://plane",
        "builtin://cylinder",
    ] {
        let mesh = builtin_mesh(model).unwrap();
        assert_eq!(
            mesh.normals.len(),
            mesh.positions.len(),
            "{model} 每顶点都要有法线"
        );
        assert_eq!(
            mesh.uvs.len(),
            mesh.positions.len(),
            "{model} 每顶点都要有 UV"
        );
        for normal in &mesh.normals {
            let length = normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2];
            assert!(
                (length - 1.0).abs() < 1e-4,
                "{model} 法线必须归一：{normal:?}"
            );
        }
        for uv in &mesh.uvs {
            assert!(
                (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                "{model} UV 越界：{uv:?}"
            );
        }
    }
}

/// 旧端 cube 的 24 个顶点按面分组：每个面的 4 个顶点共享同一法线，6 个面法线覆盖 ±X/Y/Z。
#[test]
fn cube_faces_are_axis_aligned_with_four_shared_corners() {
    let cube = builtin_mesh("builtin://cube").unwrap();
    let mut normals = Vec::new();
    for face in 0..6 {
        let normal = cube.normals[face * 4];
        for corner in 0..4 {
            let vertex = face * 4 + corner;
            assert_eq!(
                cube.normals[vertex], normal,
                "第 {face} 面第 {corner} 个角法线不共享"
            );
            for (axis, value) in cube.positions[vertex].iter().enumerate() {
                assert!(value.abs() <= 0.500_001, "第 {face} 面第 {corner} 个角越界");
                assert!(normal[axis] == 0.0 || (value - normal[axis] * 0.5).abs() < 1e-6);
            }
        }
        normals.push(normal);
    }
    for expected in [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ] {
        assert!(normals.contains(&expected), "cube 缺少法线 {expected:?}");
    }
}

/// 旧端球体的顶点法线就是归一化位置（平滑着色），UV 按经纬度铺满 0..1。
#[test]
fn sphere_normals_follow_positions_and_uvs_span_the_sphere() {
    let sphere = builtin_mesh("builtin://sphere").unwrap();
    for (position, normal) in sphere.positions.iter().zip(&sphere.normals) {
        let scaled = [position[0] * 2.0, position[1] * 2.0, position[2] * 2.0];
        for axis in 0..3 {
            assert!(
                (scaled[axis] - normal[axis]).abs() < 1e-5,
                "球面法线必须等于归一化位置"
            );
        }
    }
    // 第一圈（北极）UV 的 v = 0，最后一行 v = 1；经度从 0 到 1。
    assert_eq!(sphere.uvs.first().copied(), Some([0.0, 0.0]));
    assert_eq!(sphere.uvs.last().copied(), Some([1.0, 1.0]));
}

/// 旧端圆柱：侧面法线沿径向、顶盖 +Y、底盖 -Y；UV 也在 0..1。
#[test]
fn cylinder_side_normals_are_radial_and_caps_are_flat() {
    let cylinder = builtin_mesh("builtin://cylinder").unwrap();
    // 侧面：前 66 个顶点是 33 组（顶 / 底）交替，法线水平且与位置同向。
    for pair in 0..33 {
        let top = pair * 2;
        let bottom = top + 1;
        for vertex in [top, bottom] {
            let normal = cylinder.normals[vertex];
            assert!(normal[1].abs() < 1e-6, "侧面法线不能有竖直分量");
            let position = cylinder.positions[vertex];
            let radial = [position[0] * 2.0, position[2] * 2.0];
            assert!((radial[0] - normal[0]).abs() < 1e-5 && (radial[1] - normal[2]).abs() < 1e-5);
        }
    }
    assert_eq!(cylinder.normals[66], [0.0, 1.0, 0.0], "顶盖中心法线 +Y");
    assert_eq!(cylinder.normals[100], [0.0, -1.0, 0.0], "底盖中心法线 -Y");
}

/// 内建图元的模型材质 = 旧端 `MaterialParams::default()`（metallic 0.0 / roughness 0.45）；
/// 改了会让"没有 `render.material` 的实体"画面漂移。
#[test]
fn builtin_meshes_use_old_engine_default_material_params() {
    for model in [
        "builtin://cube",
        "builtin://sphere",
        "builtin://cylinder",
        "builtin://plane",
        "builtin://quad",
        "builtin://triangle",
    ] {
        let mesh = builtin_mesh(model).unwrap();
        assert_eq!(mesh.metallic, 0.0, "{model}");
        assert_eq!(mesh.roughness, 0.45, "{model}");
    }
}
