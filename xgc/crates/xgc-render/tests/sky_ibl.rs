//! 天空漫反射 IBL：解析天空投到 L2 球谐后必须真的照亮物体。
//!
//! 判据用 CPU 镜像对表（与旧引擎 `sky.rs` 的 `sh_project` / `sh_eval` 同式），
//! 不手抄像素魔数。天空关着时画面必须与"只有 ambient"一致。

use xgc_render::{
    aces_u8, sh_eval, sky_irradiance, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage,
    SkySettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

/// 项目 `project.ron` 里那份天空参数（太阳关掉，只看环境半球）。
fn project_sky() -> SkySettings {
    SkySettings {
        enabled: true,
        intensity: 1.0,
        zenith: [0.09, 0.26, 0.60],
        horizon: [0.62, 0.72, 0.88],
        ground: [0.15, 0.14, 0.13],
        sun_size: 2.0,
        sun_intensity: 0.0,
    }
}

fn render(sky: SkySettings) -> [u8; 4] {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "sky-ibl",
            "rgba8unorm",
            32,
            32,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    // 只留天空：ambient 与方向光都清零，像素亮起来就只能是天空 IBL。
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 1.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_sky(sky);
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
fn sky_irradiance_lights_the_object() {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let sky = project_sky();
    let coefficients = sky_irradiance(&sky, None).expect("天空开启就要有 SH");
    // 立方体法线朝向 +Z（相机沿 -Z 看，视线反向 = +Z，N·V = 1）。
    let env = sh_eval(&coefficients, [0.0, 0.0, 1.0]);
    let kd = xgc_render::sky_kd_env(0.5, 0.0, 1.0);
    // 漫反射项仍由 SH 镜像现算；环境镜面项现在来自预过滤立方体 + BRDF 积分表
    // （CPU 侧没有同口径镜像，而且只会**叠加**在漫反射之上），所以判据是"不低于纯漫反射、
    // 且镜面增益有界"，再钉一条"蓝天照亮的中性面偏蓝"。
    let diffuse = aces_u8(std::array::from_fn(|channel| env[channel] * kd));
    assert!(
        diffuse[0] > 0,
        "天空开启必须照亮物体（否则说明什么都没测到）：{diffuse:?}"
    );
    let pixel = render(sky);
    for (axis, want) in diffuse.into_iter().enumerate() {
        let got = i16::from(pixel[axis]);
        let floor = i16::from(want);
        assert!(
            got + 2 >= floor,
            "天空 IBL 的漫反射不得低于 SH 期望：实际 {pixel:?}，纯漫反射 {diffuse:?}"
        );
        assert!(
            got <= floor + 40,
            "环境镜面项必须有界（预过滤反射不该把漫反射面抬爆）：实际 {pixel:?}，纯漫反射 {diffuse:?}"
        );
    }
    assert!(pixel[2] > pixel[0], "蓝天照亮的中性面应当偏蓝：{pixel:?}");
    assert_eq!(pixel[3], 255);
}

#[test]
fn sky_off_leaves_only_ambient() {
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let sky = SkySettings {
        enabled: false,
        ..project_sky()
    };
    // ambient = 0、没有方向光、天空关：画面必须是黑的。
    assert_eq!(render(sky), [0, 0, 0, 255]);
}
