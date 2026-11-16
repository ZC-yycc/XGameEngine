//! GLB 骨骼层级：skin.joints 的名字与父索引。

use xgc_render::load_glb_skeleton;

#[test]
fn reads_joint_names_and_parents() {
    let json = serde_json::json!({
        "nodes": [
            {"name": "root", "children": [1]},
            {"name": "bone", "children": [2]},
            {"name": "hand"}
        ],
        "skins": [{"joints": [0, 1, 2]}]
    });
    let mut json_bytes = serde_json::to_vec(&json).unwrap();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8;
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&0u32.to_le_bytes());
    glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    let path = std::env::temp_dir().join(format!(
        "xgc_skeleton_{}_{}.glb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &glb).unwrap();
    let graph = load_glb_skeleton(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(graph.joints.len(), 3);
    assert_eq!(graph.joints[0].name, "root");
    assert_eq!(graph.joints[0].parent, None);
    assert_eq!(graph.joints[1].name, "bone");
    assert_eq!(graph.joints[1].parent, Some(0));
    assert_eq!(graph.joints[2].parent, Some(1));
}
