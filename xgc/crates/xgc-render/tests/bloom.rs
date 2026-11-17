//! HDR 光晕链回归：关闭态逐位不变、阈值门限、亮度叠加、尺寸变化与确定性。
//!
//! 场景用"整屏清屏色"构造：光晕链把常量场模糊回常量，所以合成结果仍是常量，
//! 期望值可以按旧端公式手算，不必依赖图像比对阈值。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{
    BloomSettings, GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage,
};
use xgc_world::{GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 多个用例各自建一个 headless 设备会互相抢同一个适配器；串行跑最省事。
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

struct Frame {
    pixels: Vec<u8>,
    passes: usize,
}

fn render(
    bloom: Option<BloomSettings>,
    linear: [f64; 3],
    width: u32,
    height: u32,
) -> Option<Frame> {
    let context = GpuContext::headless().ok()?;
    let mut renderer = Renderer::new(MeshCache::new("."));
    if let Some(bloom) = bloom {
        renderer.set_bloom(bloom);
    }
    let desc = ResourceDesc::texture2d(
        "bloom-target",
        "rgba8unorm",
        width,
        height,
        vec![ResourceUsage::ColorAttachment],
    );
    let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
        panic!("渲染目标不是纹理");
    };
    renderer.set_clear_color([linear[0], linear[1], linear[2], 1.0]);
    let world = World::new();
    let stats = renderer.render(&context, &texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(&texture, width, height).unwrap();
    Some(Frame {
        pixels,
        passes: stats.passes,
    })
}

/// 关闭态把 tone-map 绑定组按场景纹理缓存；换尺寸时缓存必须跟着作废，
/// 否则绑着的是上一帧尺寸的场景视图，tone-map 会读到越界的空像素。
#[test]
fn closed_state_rebuilds_its_cached_bind_group_after_resize() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let target = |width: u32, height: u32| {
        let desc = ResourceDesc::texture2d(
            "closed-resize-target",
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("渲染目标不是纹理");
        };
        texture
    };
    let world = World::new();
    let mut reused = Renderer::new(MeshCache::new("."));
    reused.set_clear_color([0.35, 0.2, 0.1, 1.0]);
    // 先在小尺寸上把关闭态的占位绑定组缓存出来，再换大尺寸复用同一个渲染器。
    let small = target(8, 8);
    reused.render(&context, &small, &world).unwrap();
    let large = target(16, 16);
    let resized_stats = reused.render(&context, &large, &world).unwrap();
    let resized = context.read_texture_rgba8(&large, 16, 16).unwrap();

    let mut fresh = Renderer::new(MeshCache::new("."));
    fresh.set_clear_color([0.35, 0.2, 0.1, 1.0]);
    let expected_target = target(16, 16);
    let fresh_stats = fresh.render(&context, &expected_target, &world).unwrap();
    let expected = context
        .read_texture_rgba8(&expected_target, 16, 16)
        .unwrap();

    assert!(
        resized_stats.passes == 2 && fresh_stats.passes == 2,
        "关闭态只该有场景 + tone-map 两个 pass：{} / {}",
        resized_stats.passes,
        fresh_stats.passes
    );
    assert_eq!(
        resized.len(),
        expected.len(),
        "两个 16×16 目标的回读长度必须一致"
    );
    assert!(
        resized == expected,
        "关闭态换尺寸后必须与新建渲染器的同尺寸画面逐字节相同"
    );
}

/// 整屏常量场的期望合成值：亮部提取 → 每级模糊都保持常量 → 按权重加起来。
fn expected_glow(linear: f32, settings: BloomSettings) -> f32 {
    let brightness = linear;
    let knee = settings.knee;
    let soft = (brightness - settings.threshold + knee).clamp(0.0, 2.0 * knee);
    let soft = soft * soft / (4.0 * knee + 0.00001);
    let contribution = soft.max(brightness - settings.threshold);
    let weight = contribution / brightness.max(0.00001);
    // 三级权重之和为 1，所以常量场的光晕就是这一次提取的结果。
    linear * weight * settings.intensity
}

/// 中心像素的字节偏移；不能用 `len() / 2` 顶替 —— 那会落在最后一行开头。
fn center_offset(width: u32, height: u32) -> usize {
    (((height / 2) * width + width / 2) * 4) as usize
}

#[test]
fn disabled_bloom_is_a_no_op() {
    let _gpu = gpu_lock();
    let Some(off) = render(None, [0.9, 0.9, 0.9], 8, 8) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let explicit = BloomSettings {
        intensity: 0.0,
        ..BloomSettings::default()
    };
    let Some(zero) = render(Some(explicit), [0.9, 0.9, 0.9], 8, 8) else {
        return;
    };
    assert!(
        off.pixels == zero.pixels,
        "intensity = 0 必须与不调 set_bloom 逐字节相同"
    );
    assert_eq!(off.passes, 2, "关闭态只有 HDR 场景与 tone-map 两个阶段");
    assert_eq!(zero.passes, 2, "关闭态不该跑光晕链");
}

#[test]
fn bloom_brightens_a_scene_above_threshold() {
    let _gpu = gpu_lock();
    let settings = BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    };
    let Some(off) = render(None, [1.6, 1.6, 1.6], 8, 8) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(on) = render(Some(settings), [1.6, 1.6, 1.6], 8, 8) else {
        return;
    };
    assert_eq!(on.passes, 11, "场景 1 + 光晕链 9 + tone-map 1");
    let glow = expected_glow(1.6, settings);
    assert!(glow > 0.5, "亮部就该提取出非零光晕，实际 {glow}");
    let center = center_offset(8, 8);
    let lit = on.pixels[center];
    let plain = off.pixels[center];
    assert!(
        lit > plain,
        "开着光晕的像素必须更亮：开着 {lit}，关着 {plain}"
    );
    // 合成在 ACES 之前，所以"先加光晕再映射"与各自映射不是同一件事。
    let want = xgc_render::aces_u8([1.6 + glow; 3])[0];
    assert!(
        (i16::from(lit) - i16::from(want)).abs() <= 2,
        "合成值应等于 aces(场景 + 光晕)：实际 {lit}，期望 {want}"
    );
}

#[test]
fn bloom_ignores_a_scene_below_threshold() {
    let _gpu = gpu_lock();
    let settings = BloomSettings {
        intensity: 4.0,
        ..BloomSettings::default()
    };
    let Some(off) = render(None, [0.05, 0.05, 0.05], 8, 8) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(on) = render(Some(settings), [0.05, 0.05, 0.05], 8, 8) else {
        return;
    };
    assert!(
        off.pixels == on.pixels,
        "低于阈值的场景不该被光晕改变（强度给到 4.0 也不该亮）"
    );
}

#[test]
fn bloom_chain_rebuilds_on_resize_and_stays_deterministic() {
    let _gpu = gpu_lock();
    let settings = BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    };
    let context = GpuContext::headless();
    let Ok(context) = context else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_bloom(settings);
    renderer.set_clear_color([1.6, 1.6, 1.6, 1.0]);
    let world = World::new();
    let mut previous: Option<(u32, u32, Vec<u8>)> = None;
    for (width, height) in [(8, 8), (16, 16), (16, 16), (5, 3)] {
        let desc = ResourceDesc::texture2d(
            format!("bloom-{width}x{height}"),
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("渲染目标不是纹理");
        };
        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert_eq!(stats.passes, 11, "每帧都要重跑 9 个光晕阶段");
        let pixels = context.read_texture_rgba8(&texture, width, height).unwrap();
        let lit = pixels[center_offset(width, height)];
        let want = xgc_render::aces_u8([1.6 + expected_glow(1.6, settings); 3])[0];
        assert!(
            (i16::from(lit) - i16::from(want)).abs() <= 2,
            "{width}x{height} 上光晕合成值不对：实际 {lit}，期望 {want}"
        );
        if let Some((last_width, last_height, last)) = &previous {
            if (*last_width, *last_height) == (width, height) {
                assert!(last == &pixels, "{width}x{height} 两次渲染必须逐字节相同");
            }
        }
        previous = Some((width, height, pixels));
    }
}

#[test]
fn turning_bloom_off_restores_the_disabled_frame() {
    let _gpu = gpu_lock();
    let Some(context) = GpuContext::headless().ok() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let settings = BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    };
    let (width, height) = (8u32, 8u32);
    let desc = ResourceDesc::texture2d(
        "bloom-toggle",
        "rgba8unorm",
        width,
        height,
        vec![ResourceUsage::ColorAttachment],
    );
    let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
        panic!("渲染目标不是纹理");
    };
    let world = World::new();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([1.6, 1.6, 1.6, 1.0]);

    let never = {
        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert_eq!(stats.passes, 2, "没开过光晕只有场景与 tone-map");
        context.read_texture_rgba8(&texture, width, height).unwrap()
    };
    let still_off = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(still_off.passes, 2, "还没 set_bloom，仍是关闭态");
    renderer.set_bloom(settings);
    let on = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(on.passes, 11, "打开后多跑 9 个光晕阶段");

    // 开过再关：链必须整份释放（旧端同合同），画面回到"从没开过"的逐字节状态。
    renderer.set_bloom(BloomSettings {
        intensity: 0.0,
        ..BloomSettings::default()
    });
    let off = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(off.passes, 2, "关掉光晕后不该再跑链");
    let back = context.read_texture_rgba8(&texture, width, height).unwrap();
    assert!(back == never, "开过再关必须回到关闭态的逐字节像素");
}

#[test]
fn bloom_spreads_from_bright_geometry_without_lifting_the_corner() {
    let _gpu = gpu_lock();
    let settings = BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    };
    // 环境光调高，中心 quad 直接被照到远高于阈值；四周清屏色保持很暗。
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.4; 3],
    });
    world.spawn((
        // 缩小的 quad = 画面中央一小块亮斑，边缘留给衰减去检验。
        GlobalTransform {
            scale: [0.15, 0.15, 0.15],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color: [1.0, 1.0, 1.0, 1.0],
            roughness: 0.5,
            ..Material::default()
        },
    ));
    let (width, height) = (96u32, 96u32);
    // 无适配器的机器（CI）要跳过而不是 panic：这条用例本来就只在能出图的机器上有意义。
    let Some(context) = GpuContext::headless().ok() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let shoot = |bloom: Option<BloomSettings>| {
        let mut renderer = Renderer::new(MeshCache::new("."));
        if let Some(bloom) = bloom {
            renderer.set_bloom(bloom);
        }
        renderer.set_clear_color([0.02, 0.02, 0.02, 1.0]);
        let desc = ResourceDesc::texture2d(
            "bloom-spread",
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("渲染目标不是纹理");
        };
        renderer.render(&context, &texture, &world).unwrap();
        context.read_texture_rgba8(&texture, width, height).unwrap()
    };
    let plain = shoot(None);
    let lit_pixels = shoot(Some(settings));
    let at = |pixels: &[u8], x: u32, y: u32| {
        let offset = ((y * width + x) * 4) as usize;
        i16::from(pixels[offset])
    };
    let center = (width / 2, height / 2);
    let corner = (1, 1);
    let center_lift = at(&lit_pixels, center.0, center.1) - at(&plain, center.0, center.1);
    let corner_lift = at(&lit_pixels, corner.0, corner.1) - at(&plain, corner.0, corner.1);
    assert!(
        center_lift > 0,
        "亮几何附近必须被光晕抬亮：中心 {center_lift}"
    );
    assert!(
        center_lift >= 2 * corner_lift.max(0),
        "光晕必须衰减：中心抬 {center_lift}，角落只该抬 {corner_lift}"
    );
}
