//! glTF 顶点法线：`NORMAL` 必须参与着色，模型没导法线时退回屏幕导数叉积。
//!
//! 背景：XGC 早先完全丢弃 `NORMAL`，片元法线一律取屏幕导数叉积 —— 平面没问题，
//! 平滑网格（狐狸、球）会被渲染成"每个三角形一个面"的硬边，与旧端
//! （`normalize(input.world_normal)`、顶点法线插值）差得肉眼可见。
//!
//! 这里用一块平面 + 一盏正对它的方向光做判别：把 `NORMAL` 故意写成 +X（侧向），
//! 若解析生效，法线与光向几乎垂直、整块变暗；若不生效，三块画面逐字节相同。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const SIZE: u32 = 96;
const QUAD_SCALE: f32 = 2.0;

/// 1×1 平面（z=0）+ 可选的每顶点法线；写 `.gltf` + 外部 `.bin`。
fn write_quad_gltf(tag: &str, normals: Option<[[f32; 3]; 4]>) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_normals_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let mut bin = Vec::new();
    for position in [
        [-0.5f32, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let normal_view = normals.is_some();
    if let Some(normals) = normals {
        for normal in normals {
            for value in normal {
                bin.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    let indices_offset = bin.len();
    for index in [0u16, 1, 2, 0, 2, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    std::fs::write(root.join("quad.bin"), &bin).unwrap();

    let normal_accessor = if normal_view {
        ",\n    {\"bufferView\": 1, \"componentType\": 5126, \"count\": 4, \"type\": \"VEC3\"}"
    } else {
        ""
    };
    let normal_attribute = if normal_view { r#", "NORMAL": 2"# } else { "" };
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "quad.bin", "byteLength": {bin_len}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": 48, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": {indices_offset}, "byteLength": 12}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3"}},
    {{"bufferView": {index_view}, "componentType": 5123, "count": 6, "type": "SCALAR"}}{normal_accessor}
  ],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0{normal_attribute}}}, "indices": 1}}]}}],
  "nodes": [{{"mesh": 0}}],
  "scenes": [{{"nodes": [0]}}]
}}"#,
        bin_len = bin.len(),
        indices_offset = indices_offset,
        index_view = if normal_view { 2 } else { 1 },
    );
    std::fs::write(root.join("quad.gltf"), json).unwrap();
    root.join("quad.gltf")
}

fn scene(model: &Path, scale: [f32; 3], light_dir: [f32; 3], intensity: f32) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            // 光沿 `direction` 传播（`to_light = -direction`）：主用例正对平面打，
            // 逆转换置用例沿斜法线打，两者都拿满 N·L 才有判别力。
            direction: light_dir,
            position: [0.0; 3],
            color: [1.0; 3],
            intensity,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        }],
        ambient: [0.03; 3],
    });
    world.spawn((
        GlobalTransform {
            scale,
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: model.to_string_lossy().into_owned(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 4.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

/// 返回中心像素的 RGB（平面铺满画面中央）。
fn center(model: &Path, scale: [f32; 3], light_dir: [f32; 3], intensity: f32) -> [u8; 3] {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "normals-probe",
            "rgba8unorm-srgb",
            SIZE,
            SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let root = model.parent().unwrap().to_path_buf();
    let mut renderer = Renderer::new(MeshCache::new(&root));
    renderer
        .render(
            &context,
            texture,
            &scene(model, scale, light_dir, intensity),
        )
        .unwrap();
    let pixels = context.read_texture_rgba8(texture, SIZE, SIZE).unwrap();
    let center = ((SIZE / 2) * SIZE + SIZE / 2) as usize * 4;
    [pixels[center], pixels[center + 1], pixels[center + 2]]
}

fn luma(pixel: [u8; 3]) -> u32 {
    pixel.iter().map(|value| u32::from(*value)).sum()
}

/// 删掉临时模型目录；测试失败也不该在 `%TEMP%` 里积垃圾。
fn cleanup(models: &[&Path]) {
    for model in models {
        if let Some(root) = model.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

/// 把 `.gltf` 里一段 JSON 换掉（只用于构造坏资产）。
fn rewrite_gltf(path: &Path, old: &str, new: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(old), "测试夹具里没有这段 JSON：{old}");
    std::fs::write(path, text.replace(old, new)).unwrap();
}

#[test]
fn gltf_normals_are_parsed_and_optional() {
    // 用长度 2 的法线而不是单位向量：解析阶段若偷偷归一化，这里必须变红
    // （归一只在片元里做，提前归一会让非均匀缩放的逆转置结果被带偏）。
    let with = write_quad_gltf("parsed", Some([[2.0, 0.0, 0.0]; 4]));
    let mesh = xgc_render::load_model(&with).unwrap();
    assert_eq!(
        mesh.normals.len(),
        mesh.positions.len(),
        "有 NORMAL 必须逐顶点一条"
    );
    assert!(
        mesh.normals
            .iter()
            .all(|normal| (normal[0] - 2.0).abs() < 1e-6 && normal[1] == 0.0 && normal[2] == 0.0),
        "NORMAL 必须原样读入（不提前归一）：{:?}",
        mesh.normals[0]
    );

    let without = write_quad_gltf("absent", None);
    let mesh = xgc_render::load_model(&without).unwrap();
    assert!(
        mesh.normals.is_empty(),
        "模型没导 NORMAL 时保持空，交给片元回退"
    );
    cleanup(&[&with, &without]);
}

#[test]
fn malformed_normals_are_rejected() {
    // 非 float VEC3：不许悄悄当没有法线（否则着色会在错误的空间里算）。
    let wrong_type = write_quad_gltf("wrong-type", Some([[0.0, 0.0, 1.0]; 4]));
    rewrite_gltf(
        &wrong_type,
        r#"{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"}"#,
        r#"{"bufferView": 1, "componentType": 5123, "count": 4, "type": "VEC3"}"#,
    );
    let error = xgc_render::load_model(&wrong_type).unwrap_err().to_string();
    assert!(
        error.contains("NORMAL 需要 float VEC3"),
        "非法 NORMAL 类型必须报错：{error}"
    );

    // 条数与 POSITION 不一致：顶点流会整体错位，必须报错而不是截断。
    let short = write_quad_gltf("short", Some([[0.0, 0.0, 1.0]; 4]));
    rewrite_gltf(
        &short,
        r#"{"bufferView": 1, "componentType": 5126, "count": 4, "type": "VEC3"}"#,
        r#"{"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}"#,
    );
    let error = xgc_render::load_model(&short).unwrap_err().to_string();
    assert!(
        error.contains("NORMAL 数量与 POSITION 不一致"),
        "条数不一致必须报错：{error}"
    );
    cleanup(&[&wrong_type, &short]);
}

#[test]
fn vertex_normals_drive_shading_and_fall_back_to_derivatives() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let scale = [QUAD_SCALE; 3];
    let light = [0.0, 0.0, -1.0];
    let sideways_model = write_quad_gltf("sideways", Some([[1.0, 0.0, 0.0]; 4]));
    let forward_model = write_quad_gltf("forward", Some([[0.0, 0.0, 1.0]; 4]));
    let fallback_model = write_quad_gltf("fallback", None);
    let sideways = center(&sideways_model, scale, light, 3.0);
    let forward = center(&forward_model, scale, light, 3.0);
    let fallback = center(&fallback_model, scale, light, 3.0);

    assert!(
        luma(sideways) * 4 < luma(forward),
        "侧向法线必须让平面几乎不受光（NORMAL 真的进了着色）：侧向 {sideways:?} / 正向 {forward:?}"
    );
    let difference = luma(forward).abs_diff(luma(fallback));
    assert!(
        difference <= 3,
        "正向法线与导数叉积应得到同一面法线：正向 {forward:?} / 回退 {fallback:?}"
    );
    cleanup(&[&sideways_model, &forward_model, &fallback_model]);
}

#[test]
fn non_uniform_scale_uses_the_inverse_transpose() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // 斜法线 + 非均匀缩放是唯一能区分"逆转置"和"直接乘 model"的输入：
    // 逆转置把 (0.6, 0.8, 0) 变成 (0.075, 0.8, 0) → 归一后 (0.093, 0.996, 0)，
    // N·L 约 0.853；直接乘会得到 (4.8, 0.8, 0) → 归一后 (0.986, 0.164, 0)，
    // N·L 约 0.723（差 15% 左右漫反射）。所以拿"缩放 8 倍 + 原法线"与"不缩放 +
    // 已按逆转置预变形的法线"对照，两者的 N·L 必须一致；这也正是旧端
    // `normal_mat = transpose(mat3_inverse(model))`。
    //
    // 光强刻意压到 0.8：ACES 在 248 附近已经压平，亮度接近饱和时两种法线的
    // 8 位输出只差 2 个灰阶，红证据会太薄。
    let scale_x = 8.0_f32;
    let light = [-0.6, -0.8, 0.0];
    let intensity = 0.8;
    let skewed_model = write_quad_gltf("skewed", Some([[0.6, 0.8, 0.0]; 4]));
    let skewed = center(&skewed_model, [scale_x, 1.0, 1.0], light, intensity);
    let expected_normal = [0.6 / scale_x, 0.8, 0.0];
    let length = (expected_normal[0] * expected_normal[0] + 0.64_f32).sqrt();
    let reference_model = write_quad_gltf(
        "reference",
        Some([[expected_normal[0] / length, 0.8 / length, 0.0]; 4]),
    );
    let reference = center(&reference_model, [1.0, 1.0, 1.0], light, intensity);
    let difference = luma(skewed).abs_diff(luma(reference));
    assert!(
        difference <= 4,
        "非均匀缩放下法线必须走逆转置：缩放 {scale_x} 倍后 {skewed:?} / 预变形参考 {reference:?}"
    );
    cleanup(&[&skewed_model, &reference_model]);
}
