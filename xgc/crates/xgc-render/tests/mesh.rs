//! builtin 网格必须是真实几何，不是全屏三角形。

use xgc_render::builtin_mesh;

#[test]
fn builtin_models_have_real_geometry() {
    let cube = builtin_mesh("builtin://cube").unwrap();
    assert_eq!(cube.positions.len(), 8);
    assert_eq!(cube.indices.len(), 36);
    let sphere = builtin_mesh("builtin://sphere").unwrap();
    assert_eq!(sphere.positions.len(), 153);
    assert_eq!(sphere.indices.len(), 768);
    let cylinder = builtin_mesh("builtin://cylinder").unwrap();
    assert_eq!(cylinder.positions.len(), 36);
    assert_eq!(cylinder.indices.len(), 192);
    let plane = builtin_mesh("builtin://plane").unwrap();
    assert_eq!(plane.positions.len(), 4);
    assert_eq!(plane.indices.len(), 6);
    let triangle = builtin_mesh("builtin://triangle").unwrap();
    assert_eq!(triangle.positions.len(), 3);
    assert!(builtin_mesh("builtin://unknown").is_none());
}
