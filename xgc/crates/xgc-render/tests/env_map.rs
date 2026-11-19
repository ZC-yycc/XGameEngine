//! 环境镜面项：解析天空 → GGX 5 级预过滤立方体 + BRDF 积分表，供 split-sum 采样。
//!
//! 这一层的目的是把环境高光从"L2 球谐一大片钝光"换成旧端同款的方向性反射：
//! 判据用**金属球**（F0 = 反照率、镜面项占绝对主导），换天顶色时球面颜色必须跟着变；
//! 而天空关着时必须一趟 pass 都不跑、画面与切片前逐字节相同。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use xgc_render::{
    GpuContext, GpuResource, MeshCache, RenderStats, Renderer, ResourceDesc, ResourceUsage,
    SkySettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const SIZE: u32 = 96;
/// 首次建环境立方体：30 个 (level, face) + 1 张 BRDF 表。
const FIRST_BUILD_PASSES: usize = 31;
/// BRDF 表建好之后只重建立方体。
const ENV_ONLY_PASSES: usize = 30;

/// 临时项目根 + 一份 `metallic = 1` 的材质资产（世界组件里的 `Material` 没有金属度，
/// 金属度只来自 `.mat`，与迁入的旧项目同一路径）。
fn metal_project(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "xgc_envmap_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("metal.mat"),
        "(base_color: (1.0, 1.0, 1.0, 1.0), metallic: 1.0, roughness: 0.1)",
    )
    .unwrap();
    root
}

/// 一个金属球 + 相机；场景没有光源，画面亮度由天空 IBL 决定（环境项占主导）。
fn scene() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![],
        ambient: [0.02; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://sphere".to_string(),
        },
        Material {
            base_color: [1.0; 4],
            roughness: 0.1,
            asset: "metal.mat".to_string(),
        },
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

fn sky(zenith: [f32; 3], horizon: [f32; 3]) -> SkySettings {
    SkySettings {
        enabled: true,
        intensity: 1.0,
        zenith,
        horizon,
        ground: [0.1, 0.1, 0.1],
        // 没有方向光：太阳盘整段不出现，画面只由渐变背景决定。
        sun_size: 1.5,
        sun_intensity: 1.0,
    }
}

struct Frame {
    pixels: Vec<u8>,
    stats: RenderStats,
}

fn render_into(
    renderer: &mut Renderer,
    context: &GpuContext,
    target: &wgpu::Texture,
    world: &World,
) -> Frame {
    let stats = renderer.render(context, target, world).unwrap();
    let pixels = context.read_texture_rgba8(target, SIZE, SIZE).unwrap();
    Frame { pixels, stats }
}

fn target(context: &GpuContext, label: &str) -> GpuResource {
    context
        .create_resource(&ResourceDesc::texture2d(
            label,
            "rgba8unorm-srgb",
            SIZE,
            SIZE,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap()
}

/// 画面中心像素（球心正对相机，反射方向是水平的 +Z）。
fn center(pixels: &[u8]) -> [u8; 3] {
    pixel_at(pixels, SIZE / 2, SIZE / 2)
}

/// 球面偏上的像素：反射方向朝天空上方，能看到天顶色（只换天顶色时中心像素不变，
/// 因为中心反射的是水平方向的地平线色）。
fn upper(pixels: &[u8]) -> [u8; 3] {
    pixel_at(pixels, SIZE / 4, SIZE / 2)
}

fn pixel_at(pixels: &[u8], row: u32, column: u32) -> [u8; 3] {
    let offset = ((row * SIZE + column) as usize) * 4;
    [pixels[offset], pixels[offset + 1], pixels[offset + 2]]
}

fn cleanup(root: &Path) {
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn prefiltering_runs_once_per_sky_setting() {
    let _gpu = GPU_LOCK.lock().unwrap();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = metal_project("once");
    let target = target(&context, "env-once");
    let mut renderer = Renderer::new(MeshCache::new(&root));
    renderer.set_sky(sky([0.9, 0.3, 0.2], [0.7, 0.55, 0.45]));
    let world = scene();

    let texture = target.as_texture().unwrap();
    let first = render_into(&mut renderer, &context, texture, &world);
    assert_eq!(
        first.stats.env_map_passes, FIRST_BUILD_PASSES,
        "首次必须建满 30 个级面 + 1 张 BRDF 表"
    );
    let second = render_into(&mut renderer, &context, texture, &world);
    assert_eq!(
        second.stats.env_map_passes, 0,
        "天空设置没变时不该重建环境图"
    );
    assert!(
        first.pixels == second.pixels,
        "预过滤是纯函数：同设置两帧必须逐字节相同"
    );

    // 换天顶色 = 换预过滤输入：立方体要重建，BRDF 表不用。
    renderer.set_sky(sky([0.1, 0.3, 0.95], [0.3, 0.45, 0.8]));
    let third = render_into(&mut renderer, &context, texture, &world);
    assert_eq!(
        third.stats.env_map_passes, ENV_ONLY_PASSES,
        "只换天空颜色时 BRDF 表可复用"
    );
    cleanup(&root);
}

#[test]
fn sky_disabled_skips_every_environment_pass() {
    let _gpu = GPU_LOCK.lock().unwrap();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = metal_project("off");
    let target = target(&context, "env-off");
    // 不调 `set_sky`：`SkySettings::default()` 是关。
    let mut renderer = Renderer::new(MeshCache::new(&root));
    let frame = render_into(
        &mut renderer,
        &context,
        target.as_texture().unwrap(),
        &scene(),
    );
    assert_eq!(
        frame.stats.env_map_passes, 0,
        "天空关着时一个预过滤 pass 都不许跑"
    );
    assert_eq!(
        frame.stats.passes, 2,
        "关天空的普通场景仍是 HDR + tone-map 两趟"
    );
    cleanup(&root);
}

#[test]
fn metallic_surface_follows_the_prefiltered_sky() {
    let _gpu = GPU_LOCK.lock().unwrap();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = metal_project("colour");
    let world = scene();
    let shoot = |zenith: [f32; 3], horizon: [f32; 3], label: &str| {
        let target = target(&context, label);
        let mut renderer = Renderer::new(MeshCache::new(&root));
        renderer.set_sky(sky(zenith, horizon));
        let frame = render_into(
            &mut renderer,
            &context,
            target.as_texture().unwrap(),
            &world,
        );
        (center(&frame.pixels), upper(&frame.pixels))
    };
    // 球心处的反射方向是 +Z（水平），所以判据必须落在**地平线色**上：只换天顶
    // 色时 +Z 方向的天空辐射不变，金属球当然不会变（这本身也是一条合理约束）。
    let warm = shoot([0.95, 0.35, 0.2], [0.9, 0.5, 0.3], "env-warm");
    let cool = shoot([0.1, 0.3, 0.95], [0.2, 0.35, 0.9], "env-cool");
    let off = {
        let target = target(&context, "env-dark");
        let mut renderer = Renderer::new(MeshCache::new(&root));
        let frame = render_into(
            &mut renderer,
            &context,
            target.as_texture().unwrap(),
            &world,
        );
        (center(&frame.pixels), upper(&frame.pixels))
    };

    // 金属球：F0 = 反照率，镜面项直接反映环境图内容。
    let distance = |a: [u8; 3], b: [u8; 3]| -> u32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| u32::from(*x).abs_diff(u32::from(y)))
            .sum()
    };
    let ((warm_center, warm_upper), (cool_center, cool_upper), (off_center, off_upper)) =
        (warm, cool, off);
    assert!(
        distance(warm_center, cool_center) > 24,
        "换地平线色后金属球中心的反射必须明显不同：暖 {warm_center:?} / 冷 {cool_center:?}"
    );
    assert!(
        distance(warm_upper, cool_upper) > 20,
        "球面上方反射到天顶，换天顶色必须变：暖 {warm_upper:?} / 冷 {cool_upper:?}"
    );
    assert!(
        distance(warm_center, off_center) > 24 && distance(warm_upper, off_upper) > 20,
        "开天空后金属球必须比关着亮：开 {warm_center:?}/{warm_upper:?} / 关 {off_center:?}/{off_upper:?}"
    );
    cleanup(&root);
}

#[test]
fn prefiltered_environment_is_deterministic() {
    let _gpu = GPU_LOCK.lock().unwrap();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = metal_project("deterministic");
    let world = scene();
    let mut frames = Vec::new();
    for _ in 0..2 {
        let target = target(&context, "env-deterministic");
        let mut renderer = Renderer::new(MeshCache::new(&root));
        renderer.set_sky(sky([0.4, 0.5, 0.9], [0.5, 0.6, 0.85]));
        frames.push(
            render_into(
                &mut renderer,
                &context,
                target.as_texture().unwrap(),
                &world,
            )
            .pixels,
        );
    }
    assert!(
        frames[0] == frames[1],
        "固定 Hammersley 序列必须让预过滤逐字节可复现"
    );
    cleanup(&root);
}
