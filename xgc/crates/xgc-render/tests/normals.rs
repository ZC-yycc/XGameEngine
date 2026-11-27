//! glTF 顶点法线：`NORMAL` 必须参与着色；模型没导法线时按几何生成面积加权的
//! 顶点法线（与旧端 `compute_face_normals` 同式）。
//!
//! 背景：XGC 早先完全丢弃 `NORMAL`，片元法线一律取屏幕导数叉积 —— 平面没问题，
//! 平滑网格（狐狸、球）会被渲染成"每个三角形一个面"的硬边，与旧端
//! （`normalize(input.world_normal)`、顶点法线插值）差得肉眼可见。
//!
//! 这里用一块平面 + 一盏正对它的方向光做判别：把 `NORMAL` 故意写成 +X（侧向），
//! 若解析生效，法线与光向几乎垂直、整块变暗；若不生效，三块画面逐字节相同。
//! 另外用一块"折翼"几何把生成法线也钉住：共享边的两个面在加载期求面积加权和，
//! 生成结果写回成显式 `NORMAL` 后必须逐字节复现同一帧 —— 这正是旧端
//! "缺 NORMAL 时算平滑法线"而不是"片元取面法线"的分界。

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

/// 非索引、无 NORMAL 的平面：`face_normals` 的「按 `POSITION` 顺序展开」分支只在这类
/// 资产上执行（证据世界的 `Fox.gltf` 就是非索引的），必须单独钉住。
fn write_unindexed_quad_gltf(tag: &str) -> PathBuf {
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
    // 水平面（y = 0）：生成的法线是 -Y，与"零向量兜底 +Z"不同值，
    // 所以「展开分支真的按三角形覆盖了每个顶点」是可观察的。
    for position in [
        [-0.5f32, 0.0, -0.5],
        [0.5, 0.0, -0.5],
        [0.5, 0.0, 0.5],
        [-0.5, 0.0, -0.5],
        [0.5, 0.0, 0.5],
        [-0.5, 0.0, 0.5],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(root.join("flat.bin"), &bin).unwrap();
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "flat.bin", "byteLength": {bin_len}}}],
  "bufferViews": [{{"buffer": 0, "byteOffset": 0, "byteLength": 72}}],
  "accessors": [{{"bufferView": 0, "componentType": 5126, "count": 6, "type": "VEC3"}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}],
  "nodes": [{{"mesh": 0}}],
  "scenes": [{{"nodes": [0]}}]
}}"#,
        bin_len = bin.len(),
    );
    std::fs::write(root.join("flat.gltf"), json).unwrap();
    root.join("flat.gltf")
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
            model: model.file_name().unwrap().to_str().unwrap().to_owned(),
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
fn gltf_normals_are_parsed_or_generated() {
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

    // 没导 NORMAL：加载期按几何生成。平面（z=0）生成的必然全是 +Z，
    // 与旧端 `compute_face_normals` 同结果。
    let without = write_quad_gltf("absent", None);
    let mesh = xgc_render::load_model(&without).unwrap();
    assert_eq!(
        mesh.normals.len(),
        mesh.positions.len(),
        "没导 NORMAL 时加载期必须补齐每顶点法线（否则折角会走硬边导数路径）"
    );
    assert!(
        mesh.normals.iter().all(|normal| *normal == [0.0, 0.0, 1.0]),
        "平面生成的顶点法线必须全部是 +Z：{:?}",
        mesh.normals
    );

    // 非索引资产没有 `indices`：生成前必须按 `POSITION` 顺序展开成三角形列表，
    // 否则每个顶点都拿不到任何面、全落 (0,0,1) 之前的零向量兜底（也是 +Z，
    // 但那是巧合而不是展开对了）——这里要的是「每个顶点被自己的三角形覆盖」。
    let unindexed = write_unindexed_quad_gltf("unindexed");
    let flat = xgc_render::load_model(&unindexed).unwrap();
    assert_eq!(
        flat.normals.len(),
        flat.positions.len(),
        "非索引几何没导 NORMAL 时也必须逐顶点生成法线"
    );
    assert!(
        flat.normals
            .iter()
            .all(|normal| *normal == [0.0, -1.0, 0.0]),
        "非索引水平面生成的顶点法线必须全部是 -Y（展开分支真的跑了）：{:?}",
        flat.normals
    );
    cleanup(&[&with, &without, &unindexed]);
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
fn vertex_normals_drive_shading() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let scale = [QUAD_SCALE; 3];
    let light = [0.0, 0.0, -1.0];
    let sideways_model = write_quad_gltf("sideways", Some([[1.0, 0.0, 0.0]; 4]));
    let forward_model = write_quad_gltf("forward", Some([[0.0, 0.0, 1.0]; 4]));
    // 没导 NORMAL 的平面现在由加载期生成 +Z（见上面两条用例），这里用它当
    // 「生成值与显式 +Z 同值」的佐证；屏幕导数回退只剩内建图元 / 手工 `Mesh`。
    let generated_model = write_quad_gltf("generated-forward", None);
    let sideways = center(&sideways_model, scale, light, 3.0);
    let forward = center(&forward_model, scale, light, 3.0);
    let generated = center(&generated_model, scale, light, 3.0);

    assert!(
        luma(sideways) * 4 < luma(forward),
        "侧向法线必须让平面几乎不受光（NORMAL 真的进了着色）：侧向 {sideways:?} / 正向 {forward:?}"
    );
    let difference = luma(forward).abs_diff(luma(generated));
    assert!(
        difference <= 3,
        "生成法线必须与显式 +Z 同值：正向 {forward:?} / 生成 {generated:?}"
    );
    cleanup(&[&sideways_model, &forward_model, &generated_model]);
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

/// 折翼几何：共享边 A..B，两块翼分别朝 ±Y 折向 -Z；可选的每顶点法线。
///
/// 顶点序：A=(-1,0,0)、B=(1,0,0)、C1=(0,-1,-1)、C2=(0,1,-1)；三角形
/// `[A,B,C1]` 与 `[B,A,C2]`（第二个反绕序，两面法线都朝 -Z 一侧）。
fn write_fold_gltf(tag: &str, normals: Option<[[f32; 3]; 4]>) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_fold_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let mut bin = Vec::new();
    for position in [
        [-1.0f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, -1.0],
        [0.0, 1.0, -1.0],
    ] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let normals_offset = bin.len();
    if let Some(normals) = normals {
        for normal in normals {
            for value in normal {
                bin.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    let indices_offset = bin.len();
    for index in [0u16, 1, 2, 1, 0, 3] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    std::fs::write(root.join("fold.bin"), &bin).unwrap();

    let normal_view = normals.is_some();
    let normal_accessor = if normal_view {
        ",\n    {\"bufferView\": 1, \"componentType\": 5126, \"count\": 4, \"type\": \"VEC3\"}"
    } else {
        ""
    };
    let normal_attribute = if normal_view { r#", "NORMAL": 2"# } else { "" };
    let json = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "buffers": [{{"uri": "fold.bin", "byteLength": {bin_len}}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": 48}},
    {{"buffer": 0, "byteOffset": {normals_offset}, "byteLength": 48}},
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
        normals_offset = normals_offset,
        indices_offset = indices_offset,
        index_view = if normal_view { 2 } else { 1 },
    );
    std::fs::write(root.join("fold.gltf"), json).unwrap();
    root.join("fold.gltf")
}

/// 整帧 RGBA8 像素（逐字节对照用）。
fn frame(model: &Path, scale: [f32; 3], light_dir: [f32; 3], intensity: f32) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "normals-frame",
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
    context.read_texture_rgba8(texture, SIZE, SIZE).unwrap()
}

#[test]
fn missing_normals_are_generated_from_the_geometry() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let without = write_fold_gltf("generated", None);
    let mesh = xgc_render::load_model(&without).unwrap();
    assert_eq!(
        mesh.normals.len(),
        mesh.positions.len(),
        "折翼几何没导 NORMAL 时也必须逐顶点生成法线"
    );
    // 共享边的两个顶点吃两面的和：两个面法线的叉积和是 (0,0,-4) → (0,0,-1)。
    assert_eq!(
        mesh.normals[0],
        [0.0, 0.0, -1.0],
        "共享边顶点必须拿到相邻两面的面积加权和：{:?}",
        mesh.normals[0]
    );
    assert_eq!(
        mesh.normals[1],
        [0.0, 0.0, -1.0],
        "共享边顶点必须拿到相邻两面的面积加权和：{:?}",
        mesh.normals[1]
    );

    let tip = 0.5f32.sqrt();
    assert!(
        (mesh.normals[2][1] - tip).abs() < 1e-6 && (mesh.normals[2][2] + tip).abs() < 1e-6,
        "翼尖只应拿到自己那一面的法线：{:?}",
        mesh.normals[2]
    );
    assert!(
        (mesh.normals[3][1] + tip).abs() < 1e-6 && (mesh.normals[3][2] + tip).abs() < 1e-6,
        "翼尖只应拿到自己那一面的法线：{:?}",
        mesh.normals[3]
    );

    // 把生成结果原样写回成显式 NORMAL：同一批数值经「读取访问器」与「几何生成」两条
    // 加载路径必须导出同一帧（证明两条路径的数值与字节都一致；生成逻辑整体失效时
    // 上面的条数断言会先红，这里兜的是「数值对但路径写错」）。
    let mut round_trip = [[0.0f32; 3]; 4];
    round_trip.copy_from_slice(&mesh.normals);
    let explicit = write_fold_gltf("explicit", Some(round_trip));
    let generated_frame = frame(&without, [1.5, 1.5, 1.5], [0.0, -0.4, -0.9], 2.0);
    let explicit_frame = frame(&explicit, [1.5, 1.5, 1.5], [0.0, -0.4, -0.9], 2.0);
    assert!(
        generated_frame == explicit_frame,
        "生成法线必须与显式写回同一批数值时逐字节相同（否则片元又退回了导数路径）"
    );
    cleanup(&[&without, &explicit]);
}
