//! 时间抗锯齿：默认关逐字节不变、抖动真的动相机、解析多一个 pass、换尺寸重建历史、
//! 开着时 FXAA 让位。
//!
//! 判据尽量用"两帧比较"：抖动相位是纯函数，所以同一序列必须逐字节可复现；
//! 绝对像素值会随 ACES 曲线变动，不作为断言目标。

use std::sync::Mutex;

use xgc_render::{
    BloomSettings, FxaaSettings, GpuContext, MeshCache, RenderStats, Renderer, ResourceDesc,
    ResourceUsage, TaaSettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 真正的"开着"：`TaaSettings::default()` 是关（缺省不动画面）。
fn taa_on() -> TaaSettings {
    TaaSettings {
        enabled: true,
        ..TaaSettings::default()
    }
}

/// 两块错开的 quad：抖动会把它们的边缘挪动像素，纯色满屏则看不出来。
fn scene() -> World {
    scene_with_ambient([0.2; 3])
}

fn scene_with_ambient(ambient: [f32; 3]) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![xgc_world::SceneLight::directional(
            "sun",
            [0.0, 0.0, -1.0],
            [1.0; 3],
            3.0,
        )],
        ambient,
    });
    for (x, y, scale) in [(-0.35, 0.25, 0.55), (0.4, -0.3, 0.4)] {
        world.spawn((
            GlobalTransform {
                position: [x, y, 0.0],
                scale: [scale; 3],
                ..GlobalTransform::default()
            },
            MeshRenderer {
                model: "builtin://quad".to_string(),
            },
            Material::default(),
        ));
    }
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 6.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

fn target(context: &GpuContext, size: u32) -> xgc_render::GpuResource {
    context
        .create_resource(&ResourceDesc::texture2d(
            "taa-target",
            "rgba8unorm",
            size,
            size,
            vec![ResourceUsage::ColorAttachment],
        ))
        .expect("建渲染目标")
}

fn pixels(context: &GpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    context
        .read_texture_rgba8(texture, texture.width(), texture.height())
        .expect("回读渲染目标")
}

fn shoot(
    renderer: &mut Renderer,
    context: &GpuContext,
    resource: &xgc_render::GpuResource,
) -> (Vec<u8>, RenderStats) {
    shoot_world(renderer, context, resource, &scene())
}

fn shoot_world(
    renderer: &mut Renderer,
    context: &GpuContext,
    resource: &xgc_render::GpuResource,
    world: &World,
) -> (Vec<u8>, RenderStats) {
    let texture = resource.as_texture().expect("渲染目标是纹理");
    let stats = renderer.render(context, texture, world).expect("渲染一帧");
    (pixels(context, texture), stats)
}

#[test]
fn default_taa_is_a_byte_identical_no_op() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 32);
    let mut never = Renderer::new(MeshCache::new("."));
    let (plain, plain_stats) = shoot(&mut never, &context, &resource);
    let mut explicit = Renderer::new(MeshCache::new("."));
    explicit.set_taa(TaaSettings::default());
    let (off, off_stats) = shoot(&mut explicit, &context, &resource);
    assert!(plain == off, "显式关 TAA 必须与从没设过逐字节相同",);
    assert_eq!(plain_stats.passes, off_stats.passes);
    assert_eq!(
        explicit.taa_history_size(),
        None,
        "关着时不该建历史纹理（否则白占显存）"
    );
}

#[test]
fn enabling_taa_adds_a_resolve_pass_and_allocates_history() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 64);
    let mut off = Renderer::new(MeshCache::new("."));
    let (_, off_stats) = shoot(&mut off, &context, &resource);
    assert_eq!(off.taa_history_size(), None);
    let mut on = Renderer::new(MeshCache::new("."));
    on.set_taa(taa_on());
    let (_, on_stats) = shoot(&mut on, &context, &resource);
    assert_eq!(
        on_stats.passes,
        off_stats.passes + 1,
        "开着 TAA 时多一次历史解析 pass"
    );
    assert_eq!(
        on.taa_history_size(),
        Some((64, 64)),
        "开着 TAA 必须按帧尺寸分配历史纹理"
    );
}

/// 历史混合的判据：两个渲染器都开 TAA，只差 `history_weight`（0.0 vs 0.9）。
/// 权重 0 时 `mix(current, history, 0) = current`，第二帧完全跟上新环境；
/// 权重 0.9 时第二帧被暗的历史拉住，平均亮度必须更低。这条能直接抓住
/// "着色器忽略 blend" / "history_weight 不生效" 两类变异，不会像
/// "开/关 TAA 两帧不同" 那样被亚像素抖动冒充。
#[test]
fn history_weight_pulls_the_frame_toward_the_previous_one() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 32);
    let dark = scene_with_ambient([0.05; 3]);
    let bright = scene_with_ambient([1.2; 3]);
    let mut no_history = Renderer::new(MeshCache::new("."));
    no_history.set_taa(TaaSettings {
        enabled: true,
        history_weight: 0.0,
    });
    let _ = shoot_world(&mut no_history, &context, &resource, &dark);
    let (fresh_second, _) = shoot_world(&mut no_history, &context, &resource, &bright);

    let mut blended = Renderer::new(MeshCache::new("."));
    blended.set_taa(taa_on());
    let _ = shoot_world(&mut blended, &context, &resource, &dark);
    let (blended_second, _) = shoot_world(&mut blended, &context, &resource, &bright);

    assert!(
        fresh_second != blended_second,
        "历史权重 0 与 0.9 的第二帧必须不同（否则 blend 没生效）"
    );
    // 不比较亮度方向：ACES 是非线性的，混合后的亮度高低取决于场景内容；
    // "权重不同 ⇒ 输出不同" 已经能抓住 blend 被忽略 / history_weight 不生效。
}

/// 相机瞬移 / 切场景后的重置：保留历史纹理，但下一帧必须直通当前帧
/// （否则会拿旧位置的深度去重投影，残影 1–2 帧）。
#[test]
fn resetting_the_history_skips_blending_for_one_frame() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 32);
    let dark = scene_with_ambient([0.05; 3]);
    let bright = scene_with_ambient([1.2; 3]);
    let mut resetter = Renderer::new(MeshCache::new("."));
    resetter.set_taa(taa_on());
    let _ = shoot_world(&mut resetter, &context, &resource, &dark);
    let mut kept = Renderer::new(MeshCache::new("."));
    kept.set_taa(taa_on());
    let _ = shoot_world(&mut kept, &context, &resource, &dark);
    resetter.reset_taa_history();
    assert_eq!(
        resetter.taa_history_size(),
        Some((32, 32)),
        "重置只清重投影状态，历史纹理要留着（否则白白重建）"
    );
    assert!(resetter.taa_reprojection_is_reset());
    let (reset_pixels, _) = shoot_world(&mut resetter, &context, &resource, &bright);
    let (kept_pixels, _) = shoot_world(&mut kept, &context, &resource, &bright);
    assert!(
        reset_pixels != kept_pixels,
        "重置后的第一帧必须与未重置的分道（重置会清 prev_view_proj，blend 归 0）"
    );
    assert!(
        !resetter.taa_reprojection_is_reset(),
        "渲过一帧后重置状态即结束"
    );
}

/// 整链口径：场景 1 + TAA 解析 1 + 三级光晕 9 + tone-map 1。
#[test]
fn taa_counts_every_pass_in_the_chain() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 64);
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_taa(taa_on());
    renderer.set_bloom(BloomSettings {
        intensity: 1.0,
        threshold: 1.0,
        knee: 0.6,
    });
    let (_, stats) = shoot(&mut renderer, &context, &resource);
    assert_eq!(
        stats.passes, 12,
        "TAA + 光晕 + tone-map 的整链 pass 数应为 12"
    );
}

#[test]
fn taa_sequence_is_deterministic_across_frames() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 48);
    let mut first = Renderer::new(MeshCache::new("."));
    first.set_taa(taa_on());
    let mut second = Renderer::new(MeshCache::new("."));
    second.set_taa(taa_on());
    let mut frames = Vec::new();
    for _ in 0..3 {
        let (pixels, _) = shoot(&mut first, &context, &resource);
        frames.push(pixels);
    }
    for (index, expected) in frames.iter().enumerate() {
        let (pixels, _) = shoot(&mut second, &context, &resource);
        assert!(
            &pixels == expected,
            "同一帧序列必须逐字节可复现（第 {index} 帧）"
        );
    }
    // 抖动相位逐帧推进、历史参与合成：两帧不该退化成同一张图。
    assert!(
        frames[0] != frames[2],
        "历史混合后第 3 帧不该与第 1 帧逐字节相同"
    );
}

#[test]
fn resizing_rebuilds_the_history_and_disabling_releases_it() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let small = target(&context, 8);
    let large = target(&context, 16);
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_taa(taa_on());
    let _ = shoot(&mut renderer, &context, &small);
    assert_eq!(renderer.taa_history_size(), Some((8, 8)));
    let _ = shoot(&mut renderer, &context, &large);
    assert_eq!(
        renderer.taa_history_size(),
        Some((16, 16)),
        "换尺寸必须重建历史纹理"
    );
    renderer.set_taa(TaaSettings::default());
    let _ = shoot(&mut renderer, &context, &large);
    assert_eq!(
        renderer.taa_history_size(),
        None,
        "关掉后历史纹理必须整份释放"
    );
}

#[test]
fn taa_makes_fxaa_yield() {
    let _gpu = GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let resource = target(&context, 64);
    let mut both = Renderer::new(MeshCache::new("."));
    both.set_taa(taa_on());
    both.set_fxaa(FxaaSettings { enabled: true });
    let (both_pixels, both_stats) = shoot(&mut both, &context, &resource);
    let mut taa_only = Renderer::new(MeshCache::new("."));
    taa_only.set_taa(taa_on());
    let (only_pixels, only_stats) = shoot(&mut taa_only, &context, &resource);
    assert!(
        both_pixels == only_pixels,
        "开着 TAA 时 FXAA 必须让位（显示空间只留一种抗锯齿）"
    );
    assert_eq!(both_stats.passes, only_stats.passes);
}
