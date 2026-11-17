//! `.mat` 材质资产：解析、乘子语义与"读不到就退回模型材质"。

use std::path::Path;

use xgc_render::{
    aces_u8, GpuContext, MaterialAsset, MeshCache, Renderer, ResourceDesc, ResourceUsage,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

/// 只看材质乘子：环境光拉满、不放方向光，着色输出等于颜色本身过曲线。
fn flat_light() -> SceneEnvironment {
    SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    }
}

fn render_asset(asset: &str, base_color: [f32; 4]) -> [u8; 4] {
    let root = Path::new("../../../projects/XGame");
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let catalog = xgc_asset::scan(root).unwrap();
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "material-asset",
            "rgba8unorm",
            32,
            32,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(flat_light());
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color,
            asset: asset.to_string(),
            ..Material::default()
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 1.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::with_catalog(root, catalog));
    renderer.render(&context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, 32, 32).unwrap();
    let center = 4 * (16 * 32 + 16);
    [
        pixels[center],
        pixels[center + 1],
        pixels[center + 2],
        pixels[center + 3],
    ]
}

#[test]
fn real_material_asset_multiplies_base_color() {
    let root = Path::new("../../../projects/XGame");
    if !root.join("Resources/Materials/Concrete.mat").exists() {
        eprintln!("skip: 没有真实项目材质资产");
        return;
    }
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    // Concrete.mat: base_color (0.55, 0.58, 0.62)；实体覆盖 0.5 → 有效反照率 (0.275, 0.29, 0.31)。
    let pixel = render_asset(
        "guid://235b50780baf2866df65bddf31c31470",
        [0.5, 0.5, 0.5, 1.0],
    );
    let expected = aces_u8([0.275, 0.29, 0.31]);
    for (axis, want) in expected.into_iter().enumerate() {
        let got = pixel[axis];
        assert!(
            (i16::from(got) - i16::from(want)).abs() <= 1,
            "材质资产基色必须乘进最终颜色：实际 {pixel:?}，期望 {expected:?}"
        );
    }
    assert_eq!(pixel[3], 255);
}

#[test]
fn missing_material_asset_falls_back_to_model_material() {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let pixel = render_asset("guid://0000000000000000000000000000dead", [0.5; 4]);
    let expected = aces_u8([0.5, 0.5, 0.5]);
    for (axis, want) in expected.into_iter().enumerate() {
        assert_eq!(pixel[axis], want, "读不到资产应退回模型材质：{pixel:?}");
    }
}

#[test]
fn parses_real_material_files_and_sanitizes() {
    let root = Path::new("../../../projects/XGame/Resources/Materials");
    if !root.exists() {
        eprintln!("skip: 没有真实项目材质目录");
        return;
    }
    let text = std::fs::read_to_string(root.join("Wood.mat")).unwrap();
    let wood = MaterialAsset::from_ron(&text).unwrap();
    assert_eq!(wood.base_color, [0.52, 0.34, 0.2, 1.0]);
    assert!((wood.roughness - 0.8).abs() < 1e-6);
    assert!((wood.metallic).abs() < 1e-6);

    // 缺字段回落到旧引擎缺省值（白 / 绝缘 / 0.45），坏数值不变量化。
    let partial = MaterialAsset::from_ron("(base_color: (1.0, 0.0, 0.0, 1.0))").unwrap();
    assert_eq!(partial.roughness, 0.45);
    let broken = MaterialAsset::from_ron("(roughness: 0.4)").unwrap();
    let sanitized = MaterialAsset {
        base_color: [f32::NAN, 2.0, -1.0, 1.0],
        metallic: f32::INFINITY,
        roughness: 5.0,
    }
    .sanitized();
    assert_eq!(sanitized.base_color[0], 1.0);
    assert_eq!(sanitized.metallic, 0.0);
    assert_eq!(sanitized.roughness, 1.0);
    assert_eq!(broken.base_color, [1.0; 4]);
}
