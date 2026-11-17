//! 方向光阴影：投射体的阴影必须真的落到地面上，且 `casts_shadow=false` 时不出现。
//!
//! 判据用"开 / 关两帧逐像素比较"：既不用手算投影，也不依赖具体相机机位。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn scene(casts_shadow: bool) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![SceneLight {
            name: "Sun".to_string(),
            kind: LightKind::Directional,
            // 斜着打：影子落在投射体旁边（正下方会被投射体自己挡住，看不到）。
            direction: [0.6, -1.0, 0.0],
            position: [0.0; 3],
            color: [1.0; 3],
            intensity: 3.0,
            range: 10.0,
            casts_shadow,
            shadow_resolution: 0,
        }],
        ambient: [0.0; 3],
    });
    // 地面：薄板，顶面在 y = 0。
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.1, 0.0],
            scale: [20.0, 0.2, 20.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    // 投射体：悬在地面上方。
    world.spawn((
        GlobalTransform {
            position: [0.0, 2.0, 0.0],
            scale: [1.5, 1.5, 1.5],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    // 俯视相机：绕着 +X 转 -90°，forward 变成 (0,-1,0)。
    world.spawn((
        GlobalTransform {
            position: [0.0, 6.0, 0.0],
            rotation: [
                -std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            ..GlobalTransform::default()
        },
        xgc_world::Camera::default(),
    ));
    world
}

fn render(world: &World) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "shadow",
            "rgba8unorm",
            128,
            72,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render(&context, texture, world).unwrap();
    context.read_texture_rgba8(texture, 128, 72).unwrap()
}

#[test]
fn caster_darkens_receiver_only_when_shadows_enabled() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let shadowed = render(&scene(true));
    let unshadowed = render(&scene(false));
    assert_eq!(shadowed.len(), unshadowed.len());

    let mut darkened = 0usize;
    let mut brightened = 0usize;
    for (on, off) in shadowed.chunks_exact(4).zip(unshadowed.chunks_exact(4)) {
        let delta = i32::from(on[0]) - i32::from(off[0]);
        if delta < -8 {
            darkened += 1;
        } else if delta > 8 {
            brightened += 1;
        }
    }
    let pixels = shadowed.len() / 4;
    assert!(
        darkened > pixels / 200,
        "阴影必须真的落在画面上：只有 {darkened}/{pixels} 个像素变暗"
    );
    assert!(
        darkened < pixels / 2,
        "阴影不应盖住半张图（{darkened}/{pixels}），怀疑拟合或偏置写错"
    );
    assert_eq!(
        brightened, 0,
        "阴影只能让像素变暗，出现 {brightened} 个变亮"
    );
}
