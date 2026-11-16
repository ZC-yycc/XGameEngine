//! GLB 解析回归：真实 Box.glb 存在时验证 POSITION / 索引。

use std::path::Path;

use xgc_render::load_glb;

#[test]
fn loads_box_glb_when_available() {
    let path = Path::new("../../../projects/XGame/Resources/Box.glb");
    if !path.exists() {
        eprintln!("skip: 没有 Box.glb");
        return;
    }
    let mesh = load_glb(path).unwrap();
    assert!(!mesh.positions.is_empty());
    for position in &mesh.positions {
        assert!(position.iter().all(|value| value.is_finite()));
    }
    assert!(!mesh.indices.is_empty());
    let max_index = mesh.indices.iter().copied().max().unwrap_or(0) as usize;
    assert!(max_index < mesh.positions.len());
}
