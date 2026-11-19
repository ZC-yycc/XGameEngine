//! 软阴影（PCSS）与级间混合：默认关时逐字节不变，开着时必须真的改变半影。
//!
//! 判据用"开 / 关两帧逐像素比较"：不手算投影，也不依赖具体机位。
//! 软阴影的**半影**用"变暗幅度介于全亮与全暗之间"的像素数来量：硬阴影只有
//! 一条 3×3 PCF 的窄过渡，PCSS 的过渡带明显更宽。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage, ShadowSettings};
use xgc_world::{
    GlobalTransform, LightKind, Material, MeshRenderer, SceneEnvironment, SceneLight, World,
};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const WIDTH: u32 = 192;
const HEIGHT: u32 = 108;

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
    // 俯视相机：绕 +X 转 -90°，forward 变成 (0,-1,0)。
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

/// 渲染一帧；`settings` 为 `Some` 时才调 `set_shadow_settings`（`None` = 从没设过）。
fn render(world: &World, settings: Option<ShadowSettings>) -> (Vec<u8>, usize) {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "shadow-soft",
            "rgba8unorm",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    if let Some(settings) = settings {
        renderer.set_shadow_settings(settings);
    }
    let stats = renderer.render(&context, texture, world).unwrap();
    let pixels = context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap();
    (pixels, stats.passes)
}

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner())
}

/// 每个像素相对"无阴影基线"的变暗幅度（0 = 完全受光）。
fn darkening(baseline: &[u8], shaded: &[u8]) -> Vec<i32> {
    baseline
        .chunks_exact(4)
        .zip(shaded.chunks_exact(4))
        .map(|(lit, shadow)| i32::from(lit[0]) - i32::from(shadow[0]))
        .collect()
}

/// 变暗幅度落在"过渡带"里的像素数：既不是全亮（≤5）也不是全暗（≥峰值-5）。
/// 硬阴影只有一条 3×3 PCF 的窄边（实测 24 个像素），PCSS 的过渡明显更宽。
fn midtone_pixels(deltas: &[i32], full: i32) -> usize {
    deltas
        .iter()
        .filter(|&&delta| delta > 5 && delta < full - 5)
        .count()
}

#[test]
fn default_shadow_quality_is_a_byte_identical_no_op() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let world = scene(true);
    let (never_set, never_passes) = render(&world, None);
    let (explicit_default, default_passes) = render(&world, Some(ShadowSettings::default()));
    // 画质增强默认关：帧逐字节相同、阶段数也不变（没有新 pass）。
    assert!(
        never_set == explicit_default,
        "显式设成缺省阴影画质必须与从没设过逐字节相同"
    );
    assert_eq!(never_passes, default_passes);
}

#[test]
fn illegal_shadow_values_fall_back_to_hard_shadows() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let world = scene(true);
    let (hard, _) = render(&world, None);
    // NaN / 负数都是坏配置：消费边界落回 0（= 老路径），不许把 NaN 灌进着色器。
    let (garbage, _) = render(
        &world,
        Some(ShadowSettings {
            softness: f32::NAN,
            cascade_blend: -1.0,
        }),
    );
    assert!(
        hard == garbage,
        "非法阴影画质必须落回硬阴影（与缺省逐字节相同）"
    );
}

#[test]
fn softness_widens_the_penumbra() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let lit = render(&scene(false), None).0;
    let (hard, _) = render(&scene(true), None);
    let (soft, _) = render(
        &scene(true),
        Some(ShadowSettings {
            softness: 6.0,
            cascade_blend: 0.0,
        }),
    );
    assert!(hard != soft, "软阴影必须真的改变画面");
    let hard_deltas = darkening(&lit, &hard);
    let soft_deltas = darkening(&lit, &soft);
    let full = hard_deltas.iter().copied().max().unwrap_or(0);
    let hard_mid = midtone_pixels(&hard_deltas, full);
    let soft_mid = midtone_pixels(&soft_deltas, full);
    assert!(
        soft_mid > hard_mid + 32,
        "PCSS 的过渡带必须比 3×3 PCF 宽：软 {soft_mid} vs 硬 {hard_mid}"
    );
    // 影子还在、也没有把整张图糊掉：全暗像素仍占少数。
    let dark = soft_deltas.iter().filter(|&&d| d > full / 2).count();
    assert!(dark > 0, "软阴影不能把影子整个抹掉");
    assert!(
        dark < soft_deltas.len() / 3,
        "软阴影不该盖住三分之一以上的画面（{dark}/{}）",
        soft_deltas.len()
    );
}

#[test]
fn cascade_blend_wires_the_overlap_fit() {
    let _gpu = gpu_lock();
    if GpuContext::headless().is_err() {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    }
    let (hard, _) = render(&scene(true), None);
    let (blended, _) = render(
        &scene(true),
        Some(ShadowSettings {
            softness: 0.0,
            cascade_blend: 0.05,
        }),
    );
    // 这一帧可见的接收面都落在最后一级（视线深度超过前面所有分界点），着色器里
    // 「下一级图内有值才混」的混合分支一次都不会执行；真正改变像素的是 CPU 侧按
    // `overlap` 把该级拟合框往相机方向多铺一层（采样相位随之变）。所以这条用例钉的
    // 是「旋钮真的接进了重叠拟合」，GPU 侧的 `mix` 由与旧端逐行同式的移植守；本机
    // 机位下纯混合分支改不动像素这件事写在 STATUS 的未闭环里。
    assert!(
        hard != blended,
        "级间混合必须接进 CPU 重叠拟合，否则分带仍是硬切换"
    );
    // 改动面应远小于整帧：多铺只是让这一级的采样相位微移，不是整张图重算。
    let changed = hard
        .chunks_exact(4)
        .zip(blended.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let pixels = hard.len() / 4;
    assert!(
        changed < pixels / 2,
        "级间混合接线不该整帧重算：变了 {changed}/{pixels} 个像素"
    );
}
