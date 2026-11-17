//! 逐材质 metallic / roughness：金属的 F0 = 基色、漫反射归零；介电体 F0 = 0.04。
//!
//! 期望值由 CPU 镜像（`env_brdf_approx` / `sh_eval`）现算，不手抄魔数。

use std::path::Path;
use std::sync::Mutex;

use xgc_render::{
    aces_u8, env_brdf_approx, sh_eval, sky_irradiance, GpuContext, MeshCache, Renderer,
    ResourceDesc, ResourceUsage, SkySettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

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

/// 渲染一块面向相机（+Z）的四边形，返回中心像素。
fn render_asset(asset: &str) -> [u8; 4] {
    let root = Path::new("../../../projects/XGame");
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let catalog = xgc_asset::scan(root).unwrap();
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "material-pbr",
            "rgba8unorm",
            32,
            32,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
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
    renderer.set_sky(project_sky());
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

fn assert_close(label: &str, pixel: [u8; 4], expected: [u8; 3]) {
    for (axis, want) in expected.into_iter().enumerate() {
        let got = pixel[axis];
        assert!(
            (i16::from(got) - i16::from(want)).abs() <= 2,
            "{label} 通道 {axis}：实际 {pixel:?}，期望 {expected:?}"
        );
    }
    assert_eq!(pixel[3], 255);
}

#[test]
fn metallic_surface_uses_tinted_f0_and_no_diffuse() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let root = Path::new("../../../projects/XGame");
    if !root.join("Resources/Materials/PolishedMetal.mat").exists() {
        eprintln!("skip: 没有真实材质资产");
        return;
    }
    let sky = project_sky();
    let coefficients = sky_irradiance(&sky, None).expect("天空开着就有 SH");
    // PolishedMetal: base_color (0.85, 0.87, 0.9)、metallic 1、roughness 0.35；
    // 实体覆盖的 roughness 缺省 0.5 —— 旧引擎语义是"覆盖替换资产"，这里同理。
    let f0 = [0.85f32, 0.87, 0.9];
    let brdf = env_brdf_approx(f0, 0.5, 1.0);
    let env = sh_eval(&coefficients, [0.0, 0.0, 1.0]);
    let expected = aces_u8(std::array::from_fn(|channel| env[channel] * brdf[channel]));
    let pixel = render_asset("guid://315e350a8efe0e18c063dd611eba0c95");
    assert_close("金属（漫反射归零 + 彩色 F0）", pixel, expected);
}

#[test]
fn dielectric_surface_keeps_diffuse_and_neutral_f0() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let root = Path::new("../../../projects/XGame");
    if !root.join("Resources/Materials/BlueGloss.mat").exists() {
        eprintln!("skip: 没有真实材质资产");
        return;
    }
    let sky = project_sky();
    let coefficients = sky_irradiance(&sky, None).expect("天空开着就有 SH");
    // BlueGloss: base_color (0.2, 0.4, 0.8)、metallic 0；漫反射 + F0 = 0.04 的环境反射。
    let color = [0.2f32, 0.4, 0.8];
    let env = sh_eval(&coefficients, [0.0, 0.0, 1.0]);
    let brdf = env_brdf_approx([0.04; 3], 0.5, 1.0);
    let expected = aces_u8(std::array::from_fn(|channel| {
        env[channel] * color[channel] + env[channel] * brdf[channel]
    }));
    let pixel = render_asset("guid://17e23593a51c476c2f1047b0ca0c4389");
    assert_close("介电体（漫反射 + 中性 F0）", pixel, expected);
}

#[test]
fn env_brdf_scales_with_f0_and_stays_bounded() {
    // 解析拟合的两条硬性质：F0 越大反射越强；任何参数下都落在 [0,1] 里。
    for roughness in [0.05f32, 0.5, 1.0] {
        let dielectric = env_brdf_approx([0.04; 3], roughness, 0.5)[0];
        let metal = env_brdf_approx([0.9; 3], roughness, 0.5)[0];
        assert!(
            metal > dielectric,
            "F0 越大反射越强：{metal} vs {dielectric}"
        );
        for value in [dielectric, metal] {
            assert!(
                (0.0..=1.0).contains(&value),
                "环境反射必须落在 [0,1]：{value}"
            );
        }
    }
}
