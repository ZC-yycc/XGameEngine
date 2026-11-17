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
    let coefficients = sky_irradiance(&sky, None).expect("天空开着就要有 SH");
    // 四边形正面法线朝相机（+Z），着色器把面法线朝向相机作为受光方向；
    // 入射视线 = -Z，反射方向 = +Z，N·V = 1。
    let env = sh_eval(&coefficients, [0.0, 0.0, 1.0]);
    let specular = xgc_render::env_specular(env, 1.0);
    let expected_linear = std::array::from_fn(|channel| env[channel] + specular[channel]);
    let expected = aces_u8(expected_linear);
    assert!(
        expected[0] > 0,
        "天空环境光必须非零（否则这个门禁什么也没测到）：{expected:?}"
    );
    let pixel = render(sky);
    for (axis, want) in expected.into_iter().enumerate() {
        let got = pixel[axis];
        assert!(
            (i16::from(got) - i16::from(want)).abs() <= 1,
            "天空 IBL 必须按 SH 镜像照亮正面：实际 {pixel:?}，期望 {expected:?}"
        );
    }
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
