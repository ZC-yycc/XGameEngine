//! 网格基色贴图：PNG 解码 + 端到端采样（Fox 模型从白模变回橙狐）。

use std::path::Path;
use std::sync::Mutex;

use xgc_render::{
    load_gltf, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage, TextureData,
};
use xgc_world::{Camera, GlobalTransform, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn png_decode_keeps_size_and_channels() {
    let path = Path::new("../../../projects/XGame/Resources/Texture.png");
    if !path.exists() {
        eprintln!("skip: 没有样例贴图");
        return;
    }
    let bytes = std::fs::read(path).unwrap();
    let texture = TextureData::from_png(&bytes).unwrap();
    assert_eq!((texture.width, texture.height), (1024, 1024));
    assert_eq!(texture.rgba.len(), 1024 * 1024 * 4);
    assert!(texture.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
}

#[test]
fn white_fallback_is_one_pixel() {
    let white = TextureData::white();
    assert_eq!((white.width, white.height), (1, 1));
    assert_eq!(white.rgba, vec![255; 4]);
}

/// 端到端：Fox 走 CPU 蒙皮 + 实例化路径，贴图必须真的被采样到 ——
/// 白模时整个狐狸是中性灰（R≈G≈B），有贴图后应出现明显的橙色像素。
#[test]
fn fox_renders_with_albedo_texture() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let root = Path::new("../../../projects/XGame");
    let model = "Resources/Fox.gltf";
    if !root.join(model).exists() {
        eprintln!("skip: 没有样例模型");
        return;
    }
    // 先确认模型确实带贴图，免得测试在"没贴图"时也通过。
    let mesh = load_gltf(&root.join(model)).unwrap();
    assert!(mesh.texture.is_some(), "Fox 应该有基色贴图");

    let (width, height) = (64u32, 64u32);
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "fox-texture",
            "rgba8unorm-srgb",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    });
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 0.0],
            // Fox.gltf 是厘米级模型，展示场景里同样用 0.01 缩放。
            scale: [0.01, 0.01, 0.01],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: model.to_string(),
        },
        xgc_world::Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.3, 2.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new(root));
    renderer.render(&context, texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(texture, width, height).unwrap();

    let mut orange = 0usize;
    let mut colors = std::collections::HashSet::new();
    for pixel in pixels.chunks_exact(4) {
        let (r, g, b) = (pixel[0], pixel[1], pixel[2]);
        colors.insert((r, g, b));
        if r > g && g > b && r.saturating_sub(b) > 30 {
            orange += 1;
        }
    }
    assert!(
        orange > 40,
        "贴图没有被采样：橙色像素只有 {orange}/{}，像白模",
        width * height
    );
    assert!(
        colors.len() > 8,
        "画面颜色过于单一（{} 种），贴图可能没生效",
        colors.len()
    );
}
