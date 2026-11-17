//! 直接光 PBR 门禁：GGX 高光必须随粗糙度变化 —— 光滑球要出现明显高光斑，
//! 粗糙球不该有。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    Camera, GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const SIZE: u32 = 128;

fn scene(roughness: f32) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        // 太阳从相机一侧打过来：球面朝向相机的正面才有 N·L > 0。
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            // 光从相机一侧几乎正对着打：正对相机的平面中心恰好是 GGX 高光峰。
            direction: [0.1, -0.15, -1.0],
            position: [0.0; 3],
            color: [1.0; 3],
            // 强度压低：不然漫反射本身就打满、高光看不出差别。
            intensity: 0.5,
            range: 10.0,
            casts_shadow: false,
            shadow_resolution: 0,
        }],
        ambient: [0.0; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            roughness,
            ..Material::default()
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 3.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

fn render(world: &World) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "pbr-specular",
            "rgba8unorm-srgb",
            SIZE,
            SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render(&context, texture, world).unwrap();
    context.read_texture_rgba8(texture, SIZE, SIZE).unwrap()
}

fn luma(pixels: &[u8]) -> Vec<u32> {
    pixels
        .chunks_exact(4)
        .map(|pixel| u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]))
        .collect()
}

#[test]
fn smooth_surface_shows_ggx_highlight_that_rough_lacks() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let smooth = luma(&render(&scene(0.05)));
    let rough = luma(&render(&scene(0.9)));
    let smooth_max = smooth.iter().copied().max().unwrap_or(0);
    let rough_max = rough.iter().copied().max().unwrap_or(0);
    let smooth_bright = smooth.iter().filter(|luma| **luma > 600).count();
    let rough_bright = rough.iter().filter(|luma| **luma > 600).count();
    assert!(
        smooth_max > rough_max + 20,
        "光滑球应出现更亮的高光：smooth_max={smooth_max} rough_max={rough_max}"
    );
    assert!(
        smooth_bright > rough_bright,
        "高光斑像素数应更多：smooth={smooth_bright} rough={rough_bright}"
    );
}
