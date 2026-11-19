//! SSAO 环境光遮蔽：默认关逐字节不变、四趟 pass、强度 0 等价关、接触区变暗。
//!
//! 场景只丢环境光（没有光源），这样画面亮度完全由 ambient 决定：AO 一乘就能
//! 在像素上看出"角落变暗"，而不用先算 PBR。判据尽量用"开 / 关两帧比较"。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage, SsaoSettings};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 地板 + 立在地板上的立方体：立方体与地板的接触线是 SSAO 该出现遮蔽的地方。
fn scene() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![],
        ambient: [0.6; 3],
    });
    world.spawn((
        GlobalTransform {
            position: [0.0, -1.5, 0.0],
            rotation: quat_x_degrees(-90.0),
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, -1.0, 0.0],
            scale: [1.0; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.4, 6.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

/// 绕 X 轴转 `degrees` 度（与 `GlobalTransform::rotation` 的欧拉约定一致）。
fn quat_x_degrees(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() * 0.5;
    [half.sin(), 0.0, 0.0, half.cos()]
}

/// 把场景渲进一张 `width × height` 的纹理并回读像素（同一个渲染器可反复调用）。
fn render_into(
    renderer: &mut Renderer,
    context: &GpuContext,
    label: &str,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            label,
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    renderer.render(context, texture, &scene()).unwrap();
    context.read_texture_rgba8(texture, width, height).unwrap()
}

struct Frame {
    pixels: Vec<u8>,
    passes: usize,
    ssao_target: Option<(u32, u32)>,
}

fn shoot(width: u32, height: u32, settings: Option<SsaoSettings>) -> Frame {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "ssao",
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    if let Some(settings) = settings {
        renderer.set_ssao(settings);
    }
    let stats = renderer.render(&context, texture, &scene()).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, width, height).unwrap(),
        passes: stats.passes,
        ssao_target: renderer.ssao_target_size(),
    }
}

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn occlusion_on() -> SsaoSettings {
    SsaoSettings {
        enabled: true,
        strength: 2.0,
        radius: 0.6,
        bias: 0.02,
    }
}

#[test]
fn default_ssao_is_a_byte_identical_no_op() {
    let _gpu = gpu_lock();
    let plain = shoot(64, 64, None);
    let explicit = shoot(64, 64, Some(SsaoSettings::default()));
    assert!(
        plain.pixels == explicit.pixels,
        "显式关闭 SSAO 必须与从没设过逐字节相同"
    );
    assert_eq!(plain.passes, explicit.passes);
    assert_eq!(explicit.ssao_target, None, "关着不该建 AO 目标");
}

#[test]
fn enabling_ssao_adds_four_passes_and_allocates_targets() {
    let _gpu = gpu_lock();
    let frame = shoot(64, 64, Some(occlusion_on()));
    // 场景 1 + AO 1 + 模糊横 1 + 模糊纵 1 + 乘算 1 + tone-map 1。
    assert_eq!(frame.passes, 6, "SSAO 必须真的跑满四趟");
    assert_eq!(
        frame.ssao_target,
        Some((64, 64)),
        "SSAO 输出目标按全分辨率申请（AO 本身半分辨率）"
    );
}

#[test]
fn strength_zero_is_a_byte_identical_no_op() {
    let _gpu = gpu_lock();
    let plain = shoot(64, 64, None);
    let zero = shoot(
        64,
        64,
        Some(SsaoSettings {
            strength: 0.0,
            ..occlusion_on()
        }),
    );
    assert!(
        plain.pixels == zero.pixels,
        "强度 0 与关掉等价：乘 1 不该改任何像素"
    );
    assert_eq!(plain.passes, zero.passes);
    assert_eq!(zero.ssao_target, None, "强度 0 不该建 AO 目标");
}

#[test]
fn occlusion_darkens_contact_areas() {
    let _gpu = gpu_lock();
    let plain = shoot(64, 64, None).pixels;
    let shaded = shoot(64, 64, Some(occlusion_on())).pixels;
    let mut darkened = 0usize;
    for (lit, dim) in plain.chunks_exact(4).zip(shaded.chunks_exact(4)) {
        let delta = i32::from(lit[0]) - i32::from(dim[0]);
        if delta > 3 {
            darkened += 1;
        }
    }
    let total = plain.len() / 4;
    assert!(
        darkened > 0,
        "环境光遮蔽必须让接触区变暗：0 / {total} 个像素变暗"
    );
    assert!(
        darkened < total / 2,
        "遮蔽必须是局部的，不该把整帧压黑：{darkened} / {total}"
    );
}

#[test]
fn ssao_sequence_is_deterministic() {
    let _gpu = gpu_lock();
    let first = shoot(48, 48, Some(occlusion_on()));
    let second = shoot(48, 48, Some(occlusion_on()));
    assert!(
        first.pixels == second.pixels,
        "同样的场景与设置必须逐字节相同（采样核 / 噪声都是纯函数）"
    );
    assert_eq!(first.passes, second.passes);
}

#[test]
fn resizing_rebuilds_and_disabling_releases() {
    let _gpu = gpu_lock();
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_ssao(occlusion_on());

    let render_at = |renderer: &mut Renderer, width: u32, height: u32| {
        render_into(renderer, &context, "ssao-resize", width, height);
    };

    render_at(&mut renderer, 32, 32);
    assert_eq!(renderer.ssao_target_size(), Some((32, 32)));
    render_at(&mut renderer, 8, 8);
    assert_eq!(renderer.ssao_target_size(), Some((8, 8)));

    renderer.set_ssao(SsaoSettings::default());
    render_at(&mut renderer, 8, 8);
    assert_eq!(renderer.ssao_target_size(), None, "关掉必须释放 AO 目标");
}

/// 关掉会释放 AO 目标（只留管线）：再打开必须重建出与全新渲染器逐字节相同的画面。
#[test]
fn re_enabling_after_release_matches_a_fresh_renderer() {
    let _gpu = gpu_lock();
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let mut reused = Renderer::new(MeshCache::new("."));

    reused.set_ssao(occlusion_on());
    render_into(&mut reused, &context, "ssao-reopen", 32, 32);
    assert_eq!(reused.ssao_target_size(), Some((32, 32)));

    reused.set_ssao(SsaoSettings::default());
    render_into(&mut reused, &context, "ssao-reopen", 32, 32);
    assert_eq!(reused.ssao_target_size(), None, "关掉必须释放 AO 目标");

    reused.set_ssao(occlusion_on());
    let reopened = render_into(&mut reused, &context, "ssao-reopen", 32, 32);
    assert_eq!(
        reused.ssao_target_size(),
        Some((32, 32)),
        "再开必须重建目标"
    );

    let fresh = shoot(32, 32, Some(occlusion_on()));
    assert!(
        reopened == fresh.pixels,
        "释放后重开必须与全新渲染器逐字节相同（管线与 uniform 复用无残留状态）"
    );
}
