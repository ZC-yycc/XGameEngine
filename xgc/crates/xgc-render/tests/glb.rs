//! GLB 解析回归：真实 Box.glb 存在时验证 POSITION / 索引 / 材质因子。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
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
    assert_eq!(mesh.base_color_factor, [0.8, 0.0, 0.0, 1.0]);
    // 材质 JSON 只写了 metallicFactor（其余吃 glTF spec 缺省）：
    // roughnessFactor 缺省 1.0、metallicFactor 显式 0.0 —— 与旧端 `read_material` 同口径。
    assert_eq!(mesh.metallic, 0.0);
    assert_eq!(mesh.roughness, 1.0);
    let max_index = mesh.indices.iter().copied().max().unwrap_or(0) as usize;
    assert!(max_index < mesh.positions.len());
}

#[test]
fn missing_material_or_base_color_factor_is_white() {
    // 1) primitive 不引用材质；2) 引用了材质但没写 pbrMetallicRoughness；
    // 3) 写了 pbrMetallicRoughness 但没写 baseColorFactor。三种都必须退回白色。
    for (tag, materials, material) in [
        ("no-material", None, None),
        ("no-pbr", Some(json!([{}])), Some(0)),
        (
            "no-factor",
            Some(json!([{"pbrMetallicRoughness": {"metallicFactor": 0.0}}])),
            Some(0),
        ),
    ] {
        let path = write_triangle_glb(tag, materials, material);
        let mesh = load_glb(&path).unwrap();
        assert_eq!(mesh.base_color_factor, [1.0; 4], "{tag} 缺省应为白色");
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn out_of_range_material_index_is_rejected() {
    let path = write_triangle_glb("bad-material", Some(json!([])), Some(0));
    let err = load_glb(&path).unwrap_err();
    assert!(
        err.to_string().contains("material 0"),
        "越界材质索引必须点名 material 0：{err}"
    );
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn loads_skinned_glb_joints_and_weights() {
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
    for value in xgc_render::identity() {
        bin.extend_from_slice(&value.to_le_bytes());
    }
    let json = json!({
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
            "material": 0,
            "mode": 4
        }]}],
        "materials": [{"pbrMetallicRoughness": {
            "baseColorFactor": [0.25, 0.5, 0.75, 0.8]
        }}],
        "nodes": [{"mesh": 0, "skin": 0}],
        "skins": [{"joints": [0], "inverseBindMatrices": 3}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "VEC4"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
            {"bufferView": 3, "componentType": 5126, "count": 1, "type": "MAT4"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 24},
            {"buffer": 0, "byteOffset": 60, "byteLength": 48},
            {"buffer": 0, "byteOffset": 108, "byteLength": 64}
        ]
    });
    let path = write_glb("skin", &json, bin);
    let mesh = load_glb(&path).unwrap();
    assert_eq!(mesh.positions.len(), 3);
    assert_eq!(mesh.base_color_factor, [0.25, 0.5, 0.75, 0.8]);
    // 只写了 baseColorFactor：metallicFactor / roughnessFactor 吃 glTF spec 缺省 1.0。
    assert_eq!(mesh.metallic, 1.0);
    assert_eq!(mesh.roughness, 1.0);
    assert_eq!(mesh.tint([0.8, 0.4, 0.2, 0.5]), [0.2, 0.2, 0.15, 0.4]);
    let skin = mesh.skin.as_ref().unwrap();
    assert_eq!(skin.joints.len(), 3);
    assert_eq!(skin.joints[1], [1, 0, 0, 0]);
    assert!((skin.weights[2][1] - 0.75).abs() < 1e-6);
    assert_eq!(skin.joint_nodes, vec![0]);
    assert_eq!(skin.inverse_bind.len(), 1);
    assert!((skin.inverse_bind[0][12]).abs() < 1e-6);
    std::fs::remove_file(&path).unwrap();
}

/// 合成 GLB 容器：JSON chunk 补空格、BIN chunk 补零到 4 字节对齐。
fn glb_container(json: &Value, mut bin: Vec<u8>) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(json).unwrap();
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
    glb
}

/// 临时 GLB 只需活到 `load_glb` 返回；文件名带 tag 与纳秒避免并发互相覆盖。
fn write_glb(tag: &str, json: &Value, bin: Vec<u8>) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "xgc_glb_{tag}_{}_{}.glb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, glb_container(json, bin)).unwrap();
    path
}

/// 只含 POSITION 的最小三角形；`materials` / `material` 决定材质引用形态。
fn write_triangle_glb(tag: &str, materials: Option<Value>, material: Option<usize>) -> PathBuf {
    let mut bin = Vec::new();
    for value in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for component in value {
            bin.extend_from_slice(&component.to_le_bytes());
        }
    }
    let mut primitive = json!({"attributes": {"POSITION": 0}, "mode": 4});
    if let Some(material) = material {
        primitive["material"] = json!(material);
    }
    let mut json = json!({
        "meshes": [{"primitives": [primitive]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"}],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36}]
    });
    if let Some(materials) = materials {
        json["materials"] = materials;
    }
    write_glb(tag, &json, bin)
}

/// Fox.gltf：TEXCOORD_0 与 baseColorTexture 都要真的解析出来（旧引擎 PBR 的 albedo 贴图）。
#[test]
fn fox_gltf_carries_uv_and_base_color_texture() {
    let path = std::path::Path::new("../../../projects/XGame/Resources/Fox.gltf");
    if !path.exists() {
        eprintln!("skip: 没有样例模型");
        return;
    }
    let mesh = xgc_render::load_gltf(path).unwrap();
    assert_eq!(mesh.uvs.len(), mesh.positions.len(), "每个顶点都要有 UV");
    assert!(mesh.uvs.iter().any(|uv| uv[0] != 0.0 || uv[1] != 0.0));
    let texture = mesh.texture.as_ref().expect("Fox 有基色贴图");
    assert_eq!((texture.width, texture.height), (1024, 1024));
    assert_eq!(texture.rgba.len(), 1024 * 1024 * 4);
    // 贴图不是纯白：否则"加载成功"没有意义。
    assert!(texture
        .rgba
        .chunks_exact(4)
        .any(|pixel| pixel[0] > pixel[2] + 40));
}

/// 真实 Fox.gltf 的模型材质：`metallicFactor: 0` / `roughnessFactor: 0.58`。
/// 实体没有 `render.material` 时这两个值就是有效材质（旧端 `read_material` 同口径）。
#[test]
fn fox_gltf_carries_model_metallic_and_roughness() {
    let root = Path::new("../../../projects/XGame/Resources");
    if !root.join("Fox.gltf").exists() {
        eprintln!("skip: 没有 Fox.gltf");
        return;
    }
    let mut cache = xgc_render::MeshCache::new(root);
    let mesh = cache.get("Fox.gltf").unwrap().unwrap();
    assert_eq!(mesh.metallic, 0.0);
    assert!(
        (mesh.roughness - 0.58).abs() < 1e-6,
        "roughness = {}",
        mesh.roughness
    );
}
