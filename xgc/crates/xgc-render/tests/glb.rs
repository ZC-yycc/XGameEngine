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

#[test]
fn loads_skinned_glb_joints_and_weights() {
    use serde_json::json;

    let mut bin = Vec::new();
    for value in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for component in value {
            bin.extend_from_slice(&component.to_le_bytes());
        }
    }
    for joints in [[0u16, 1, 0, 0], [1, 0, 0, 0], [0, 1, 0, 0]] {
        for joint in joints {
            bin.extend_from_slice(&joint.to_le_bytes());
        }
    }
    for weights in [
        [0.5f32, 0.5, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.25, 0.75, 0.0, 0.0],
    ] {
        for weight in weights {
            bin.extend_from_slice(&weight.to_le_bytes());
        }
    }
    let json = json!({
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
            "mode": 4
        }]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "VEC4"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 24},
            {"buffer": 0, "byteOffset": 60, "byteLength": 48}
        ]
    });
    let mut json_bytes = serde_json::to_vec(&json).unwrap();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    glb.extend_from_slice(&json_bytes);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    glb.extend_from_slice(&bin);

    let path = std::env::temp_dir().join(format!(
        "xgc_skin_{}_{}.glb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, &glb).unwrap();
    let mesh = load_glb(&path).unwrap();
    assert_eq!(mesh.positions.len(), 3);
    let skin = mesh.skin.as_ref().unwrap();
    assert_eq!(skin.joints.len(), 3);
    assert_eq!(skin.joints[1], [1, 0, 0, 0]);
    assert!((skin.weights[2][1] - 0.75).abs() < 1e-6);
    std::fs::remove_file(&path).unwrap();
}
