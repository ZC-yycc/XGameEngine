//! 蒙皮测试共用的临时 GLTF 夹具。
//!
//! 拆成 `tests/` 下的普通模块（不是测试二进制），让 `skinned_pbr` /
//! `skinned_shadow` 两个测试文件共用同一份几何构造，也把各自的行数压回软上限内。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use xgc_render::identity;

/// 四边形网格 + 可配置的关节槽位 / 骨架大小（带基色贴图）。
pub fn write_textured_skin_gltf() -> PathBuf {
    write_textured_skin_gltf_with_joint(0, 1)
}

/// 四个顶点的权重等分到 `joint` 槽；骨架声明 `joint_count` 根骨头（各一份单位逆绑定）。
/// 槽位上限（旧端 256）就是靠"骨架比槽位多一根"来钉的。
pub fn write_textured_skin_gltf_with_joint(joint: u16, joint_count: u16) -> PathBuf {
    let root = temp_root("xgc_skinned_pbr");
    let mut bin = Vec::new();
    for position in [
        [-0.8f32, -0.8, 0.0],
        [0.8, -0.8, 0.0],
        [0.8, 0.8, 0.0],
        [-0.8, 0.8, 0.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for uv in [[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]] {
        for value in uv {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..4 {
        for value in [joint; 4] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    // 总权重刻意不是 1；静态 / GPU 逐像素同值会钉住 shader 的正权重归一。
    for _ in 0..4 {
        for value in [0.25f32, 0.25, 0.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    // 每根骨头一份逆绑定（全单位阵）：palette 只认 `joint < inverse_bind.len()`。
    for _ in 0..joint_count {
        for value in identity() {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for index in [0u16, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    std::fs::write(root.join("skin.bin"), &bin).unwrap();

    let joints: Vec<String> = (1..=joint_count)
        .map(|index| format!("{{\"name\": \"joint-{index}\"}}"))
        .collect();
    let nodes = format!("{{\"mesh\": 0, \"skin\": 0}}, {}", joints.join(", "));
    let joint_slots: Vec<String> = (1..=joint_count).map(|index| index.to_string()).collect();
    let skins = format!(
        "{{\"joints\": [{}], \"inverseBindMatrices\": 4}}",
        joint_slots.join(", ")
    );
    let indices_offset = 240 + (joint_count as usize - 1) * 64;
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "skin.bin", "byteLength": {buffer_len}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": 48, "byteLength": 32}},
    {{"buffer": 0, "byteOffset": 80, "byteLength": 32}},
    {{"buffer": 0, "byteOffset": 112, "byteLength": 64}},
    {{"buffer": 0, "byteOffset": 176, "byteLength": {ibm_len}}},
    {{"buffer": 0, "byteOffset": {indices_offset}, "byteLength": 12}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"}},
    {{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC2"}},
    {{"bufferView": 2, "componentType": 5123, "count": 4, "type": "VEC4"}},
    {{"bufferView": 3, "componentType": 5126, "count": 4, "type": "VEC4"}},
    {{"bufferView": 4, "componentType": 5126, "count": {joint_count}, "type": "MAT4"}},
    {{"bufferView": 5, "componentType": 5123, "count": 6, "type": "SCALAR"}}
  ],
  "images": [{{"uri": "albedo.png"}}],
  "textures": [{{"source": 0}}],
  "materials": [{{"pbrMetallicRoughness": {{
    "baseColorFactor": [0.9, 0.8, 0.7, 1.0],
    "baseColorTexture": {{"index": 0}}
  }}}}],
  "meshes": [{{"primitives": [{{
    "attributes": {{"POSITION": 0, "TEXCOORD_0": 1, "JOINTS_0": 2, "WEIGHTS_0": 3}},
    "indices": 5,
    "material": 0,
    "mode": 4
  }}]}}],
  "nodes": [{nodes}],
  "skins": [{skins}],
  "scenes": [{{"nodes": [0]}}],
  "scene": 0
}}"#,
        buffer_len = bin.len(),
        ibm_len = joint_count as usize * 64,
    );
    std::fs::write(root.join("skin.gltf"), json).unwrap();
    write_test_png(&root.join("albedo.png"));
    root
}

/// 闭口立方体 + 单关节蒙皮：任何光照方向都有朝光的三角形，不会因背面剔除而整块
/// 从阴影图里消失（平面四边形会，那样测出来的"没有阴影"是假绿）。
pub fn write_skinned_cube_gltf() -> PathBuf {
    let root = temp_root("xgc_skinned_shadow");
    let corners = [
        [-0.5f32, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let faces = [
        [4u16, 5, 6, 4, 6, 7],
        [1, 0, 3, 1, 3, 2],
        [0, 4, 7, 0, 7, 3],
        [5, 1, 2, 5, 2, 6],
        [3, 7, 6, 3, 6, 2],
        [0, 1, 5, 0, 5, 4],
    ];
    let mut bin = Vec::new();
    for corner in corners {
        for value in corner {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..8 {
        for value in [0u16; 4] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..8 {
        for value in [1.0f32, 0.0, 0.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in identity() {
        bin.extend_from_slice(&value.to_le_bytes());
    }
    for index in faces.iter().flatten() {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    std::fs::write(root.join("skin.bin"), &bin).unwrap();

    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "skin.bin", "byteLength": {buffer_len}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 96}},
    {{"buffer": 0, "byteOffset": 96, "byteLength": 64}},
    {{"buffer": 0, "byteOffset": 160, "byteLength": 128}},
    {{"buffer": 0, "byteOffset": 288, "byteLength": 64}},
    {{"buffer": 0, "byteOffset": 352, "byteLength": 72}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 8, "type": "VEC3"}},
    {{"bufferView": 1, "componentType": 5123, "count": 8, "type": "VEC4"}},
    {{"bufferView": 2, "componentType": 5126, "count": 8, "type": "VEC4"}},
    {{"bufferView": 3, "componentType": 5126, "count": 1, "type": "MAT4"}},
    {{"bufferView": 4, "componentType": 5123, "count": 36, "type": "SCALAR"}}
  ],
  "materials": [{{"pbrMetallicRoughness": {{
    "baseColorFactor": [0.9, 0.85, 0.8, 1.0]
  }}}}],
  "meshes": [{{"primitives": [{{
    "attributes": {{"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2}},
    "indices": 4,
    "material": 0,
    "mode": 4
  }}]}}],
  "nodes": [{{"mesh": 0, "skin": 0}}, {{"name": "joint"}}],
  "skins": [{{"joints": [1], "inverseBindMatrices": 3}}],
  "scenes": [{{"nodes": [0]}}],
  "scene": 0
}}"#,
        buffer_len = bin.len(),
    );
    std::fs::write(root.join("skin.gltf"), json).unwrap();
    root
}

fn temp_root(prefix: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "{prefix}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    root
}

fn write_test_png(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(file, 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&[
            240, 40, 20, 255, 20, 220, 40, 255, 30, 60, 240, 255, 230, 190, 20, 255,
        ])
        .unwrap();
}
