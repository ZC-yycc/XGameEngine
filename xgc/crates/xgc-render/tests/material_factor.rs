//! glTF PBR 基色因子必须从模型资产贯通到最终像素。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, Skeleton, SkeletonPose,
    World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 只看材质因子：环境光拉满、不放方向光，着色输出就等于颜色本身。
fn flat_light() -> SceneEnvironment {
    SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    }
}

#[test]
fn gltf_base_color_factor_tints_rendered_mesh() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let assets = Path::new("../../../projects/XGame/Resources");
    if !assets.join("Box.glb").exists() {
        eprintln!("skip: 没有真实项目 Box.glb");
        return;
    }
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "material-factor",
            "rgba8unorm",
            64,
            64,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(flat_light());
    let mesh = world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "Box.glb".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 3.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));

    let selected = world.entity(mesh).unwrap();
    let mut renderer = Renderer::new(MeshCache::new(assets));
    renderer.render(&context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, 64, 64).unwrap();
    let center = 4 * (32 * 64 + 32);
    let pixel = &pixels[center..center + 4];
    let [red, green, blue] = [pixel[0], pixel[1], pixel[2]].map(u16::from);
    assert!(
        red > 100 && red > green * 4 && red > blue * 4,
        "Box.glb 的 baseColorFactor=[0.8,0,0,1] 应生成显著红色，实际 {pixel:?}"
    );

    renderer
        .render_selected(&context, texture, &world, Some(selected))
        .unwrap();
    let selected_pixels = context.read_texture_rgba8(texture, 64, 64).unwrap();
    let selected_pixel = &selected_pixels[center..center + 4];
    assert!(
        selected_pixel[1] > 100 && selected_pixel[2] > 20,
        "选择高亮应替换最终颜色，而非再乘红色资产因子：{selected_pixel:?}"
    );
}

#[test]
fn gltf_base_color_factor_tints_skinned_mesh() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let root = write_skinned_glb();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        let _ = std::fs::remove_dir_all(root);
        return;
    };
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "skinned-material-factor",
            "rgba8unorm",
            64,
            64,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(flat_light());
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "skin.glb".to_string(),
        },
        Skeleton { joint_count: 1 },
        SkeletonPose {
            matrices: ron::to_string(&vec![xgc_render::identity()]).unwrap(),
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 3.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));

    // 先把网格读进缓存，临时目录就能在断言前删掉——旧实现上跑红也不留残渣。
    let mut cache = MeshCache::new(&root);
    cache.get("skin.glb").unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    let mut renderer = Renderer::new(cache);
    renderer.render(&context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, 64, 64).unwrap();
    let center = 4 * (32 * 64 + 32);
    let pixel = &pixels[center..center + 4];
    let expected = xgc_render::aces_u8(xgc_render::ambient_light(
        [0.0, 0.7, 0.0],
        0.0,
        [1.0; 3],
        1.0,
    ));
    for (axis, want) in expected.into_iter().enumerate() {
        assert!(
            (i16::from(pixel[axis]) - i16::from(want)).abs() <= 1,
            "GPU 蒙皮必须复用静态 PBR/ACES：通道 {axis} 实际 {}，期望 {want}",
            pixel[axis]
        );
    }
}

#[test]
fn gltf_base_color_factor_multiplies_entity_color_once() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let assets = Path::new("../../../projects/XGame/Resources");
    if !assets.join("Box.glb").exists() {
        eprintln!("skip: 没有真实项目 Box.glb");
        return;
    }
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let white = box_center(&context, assets, Material::default());
    let halved = box_center(
        &context,
        assets,
        Material {
            base_color: [0.5, 0.5, 0.5, 1.0],
            ..Material::default()
        },
    );
    let ratio = f32::from(halved[0]) / f32::from(white[0]);
    // 曲线之后的像素仍要逐通道对表：白 = 曲线(0.8+0.04,0.04,0.04)，灰 = 曲线(0.4+0.04,0.04,0.04)；
    // ambient 的菲涅尔项与环境色无关（恒 +0.04），所以比值不是 0.5 而是约 0.62。
    // 重复相乘会得到曲线(0.32,0,0) 的红通道 ≈ 103，与 120 明显不同。
    let expected_white = xgc_render::aces_u8(xgc_render::ambient_light(
        [0.8, 0.0, 0.0],
        0.0,
        [1.0; 3],
        1.0,
    ));
    let expected_halved = xgc_render::aces_u8(xgc_render::ambient_light(
        [0.4, 0.0, 0.0],
        0.0,
        [1.0; 3],
        1.0,
    ));
    for (label, pixel, expected) in [
        ("白模", white, expected_white),
        ("0.5 灰", halved, expected_halved),
    ] {
        for (axis, want) in expected.into_iter().enumerate() {
            let got = pixel[axis];
            assert!(
                (i16::from(got) - i16::from(want)).abs() <= 1,
                "{label} 通道 {axis} 必须等于曲线与 CPU 镜像的对表值：实际 {got}，期望 {want}"
            );
        }
    }
    assert!(
        white[0] > 150 && (0.58..=0.68).contains(&ratio),
        "模型因子与实体材质只许乘一次：白 {white:?} → 灰 0.5 {halved:?}，ratio={ratio}"
    );
}

/// 单个 Box.glb 的画面中心像素；每次用独立纹理，避免上一次的附件残留。
fn box_center(context: &GpuContext, assets: &Path, material: Material) -> [u8; 4] {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "material-factor-sample",
            "rgba8unorm",
            64,
            64,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(flat_light());
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "Box.glb".to_string(),
        },
        material,
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 3.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new(assets));
    renderer.render(context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, 64, 64).unwrap();
    let center = 4 * (32 * 64 + 32);
    [
        pixels[center],
        pixels[center + 1],
        pixels[center + 2],
        pixels[center + 3],
    ]
}

fn write_skinned_glb() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_material_factor_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let mut bin = Vec::new();
    for position in [[-1.0f32, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]] {
        for value in position {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..3 {
        for value in [0u16; 4] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    for _ in 0..3 {
        for value in [1.0f32, 0.0, 0.0, 0.0] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
    }
    let json = serde_json::json!({
        "meshes": [{"primitives": [{
            "attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2},
            "material": 0,
            "mode": 4
        }]}],
        "materials": [{"pbrMetallicRoughness": {
            "baseColorFactor": [0.0, 0.7, 0.0, 1.0],
            // 显式钉住模型材质：glTF 省略因子时 spec 缺省是 metallic 1.0 / roughness 1.0，
            // 这条用例只看"蒙皮也走同一套 PBR/ACES"，不希望被缺省值变化牵动。
            "metallicFactor": 0.0,
            "roughnessFactor": 1.0
        }}],
        "nodes": [{"mesh": 0, "skin": 0}],
        "skins": [{"joints": [0]}],
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
    let mut json = serde_json::to_vec(&json).unwrap();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    glb.extend_from_slice(&bin);
    std::fs::write(root.join("skin.glb"), glb).unwrap();
    root
}
