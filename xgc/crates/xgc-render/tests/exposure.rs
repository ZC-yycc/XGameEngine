//! 曝光回归：缺省 1.0 逐字节不变、加倍变亮、与 CPU ACES 镜像对表、
//! 光晕的亮部提取也用同一个乘子、非法值落回缺省、改了曝光要重建光晕链。
//!
//! 判据都用"同一渲染器的另一档"或 CPU 镜像当基线：曝光是纯函数，不涉及随机
//! 与时间，所以比较可以直接到逐字节 / 逐通道。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{
    aces_u8, BloomSettings, GpuContext, GpuResource, MeshCache, RenderQuality, RenderSettings,
    Renderer, ResourceDesc, ResourceUsage, ToneMapMode,
};
use xgc_world::World;

static GPU_LOCK: Mutex<()> = Mutex::new(());

/// 多个用例各自建 headless 设备会互相抢适配器；串行跑最省事。
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

struct Frame {
    pixels: Vec<u8>,
    passes: usize,
    width: u32,
    height: u32,
}

fn shoot(
    renderer: &mut Renderer,
    context: &GpuContext,
    world: &World,
    width: u32,
    height: u32,
) -> Frame {
    let desc = ResourceDesc::texture2d(
        "exposure-target",
        "rgba8unorm",
        width,
        height,
        vec![ResourceUsage::ColorAttachment],
    );
    let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
        panic!("渲染目标不是纹理");
    };
    let stats = renderer.render(context, &texture, world).unwrap();
    let pixels = context.read_texture_rgba8(&texture, width, height).unwrap();
    Frame {
        pixels,
        passes: stats.passes,
        width,
        height,
    }
}

/// 中心像素的偏移（clear 色是常量场，中心与四角同值）。
fn center(frame: &Frame) -> usize {
    (((frame.height / 2) * frame.width + frame.width / 2) * 4) as usize
}

/// 纯清屏场景：没有几何、没有光，tone-map 的输入就是 `clear`。
fn render_with(
    exposure: Option<f32>,
    bloom: BloomSettings,
    clear: [f32; 3],
    width: u32,
    height: u32,
) -> Option<Frame> {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return None;
    };
    let world = World::new();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([
        f64::from(clear[0]),
        f64::from(clear[1]),
        f64::from(clear[2]),
        1.0,
    ]);
    renderer.set_bloom(bloom);
    if let Some(exposure) = exposure {
        renderer.set_exposure(exposure);
    }
    Some(shoot(&mut renderer, &context, &world, width, height))
}

/// 硬阈值光晕：软膝为 0 时低于阈值的像素贡献恰好为 0，判据最干净。
fn hard_bloom() -> BloomSettings {
    BloomSettings {
        intensity: 1.0,
        threshold: 1.0,
        knee: 0.0,
    }
}

#[test]
fn default_exposure_is_a_byte_identical_no_op() {
    let Some(plain) = render_with(None, BloomSettings::default(), [0.35, 0.2, 0.1], 16, 16) else {
        return;
    };
    let Some(explicit) = render_with(
        Some(1.0),
        BloomSettings::default(),
        [0.35, 0.2, 0.1],
        16,
        16,
    ) else {
        return;
    };
    assert!(
        plain.passes == 2 && explicit.passes == 2,
        "曝光缺省也不该多出 pass：{} / {}",
        plain.passes,
        explicit.passes
    );
    assert!(
        plain.pixels == explicit.pixels,
        "显式写 1.0 必须与不设曝光逐字节相同"
    );
}

#[test]
fn doubling_exposure_brightens_the_linear_scene() {
    let Some(base) = render_with(None, BloomSettings::default(), [0.2, 0.2, 0.2], 16, 16) else {
        return;
    };
    let Some(bright) = render_with(Some(2.0), BloomSettings::default(), [0.2, 0.2, 0.2], 16, 16)
    else {
        return;
    };
    let (a, b) = (center(&base), center(&bright));
    assert!(
        bright.pixels[b] > base.pixels[a],
        "曝光加倍必须变亮：{} -> {}",
        base.pixels[a],
        bright.pixels[b]
    );
    // 曝光是曲线之前的乘子：期望值 = ACES(清屏色 × 2)，走 CPU 镜像。
    let want = aces_u8([0.4; 3])[0];
    assert!(
        bright.pixels[b].abs_diff(want) <= 1,
        "曝光 2.0 的中心像素应对上 ACES(0.4)：实际 {}，期望 {want}",
        bright.pixels[b]
    );
}

#[test]
fn project_tone_map_switch_preserves_exposure_and_clamps_hdr() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let world = World::new();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([0.125, 0.375, 0.75, 1.0]);
    let quality = RenderQuality {
        msaa: Some(1),
        ..Default::default()
    };
    let mut render = RenderSettings {
        exposure: 2.0,
        ..Default::default()
    };
    renderer.apply_project_settings(&context, render, quality);
    let aces = shoot(&mut renderer, &context, &world, 8, 8);
    let offset = center(&aces);
    for (actual, expected) in aces.pixels[offset..][..3]
        .iter()
        .zip(aces_u8([0.25, 0.75, 1.5]))
    {
        assert!(actual.abs_diff(expected) <= 1);
    }

    render.tone_map = ToneMapMode::Off;
    renderer.apply_project_settings(&context, render, quality);
    let linear = shoot(&mut renderer, &context, &world, 8, 8);
    assert_eq!(aces.passes, linear.passes);
    assert_eq!(&linear.pixels[offset..][..4], &[64, 191, 255, 255]);

    render.tone_map = ToneMapMode::Aces;
    renderer.apply_project_settings(&context, render, quality);
    let restored = shoot(&mut renderer, &context, &world, 8, 8);
    assert_eq!(restored.pixels, aces.pixels);
}

#[test]
fn illegal_exposures_fall_back_to_the_default_frame() {
    let Some(base) = render_with(None, BloomSettings::default(), [0.35, 0.2, 0.1], 8, 8) else {
        return;
    };
    for (label, value) in [("0", 0.0), ("负", -2.0), ("NaN", f32::NAN)] {
        let Some(frame) = render_with(
            Some(value),
            BloomSettings::default(),
            [0.35, 0.2, 0.1],
            8,
            8,
        ) else {
            return;
        };
        assert!(
            frame.pixels == base.pixels,
            "曝光 {label} 必须落回缺省 1.0，帧输出应逐字节不变"
        );
    }
}

#[test]
fn bloom_threshold_reads_the_exposed_value() {
    // 清屏 0.4：曝光 1.0 时低于阈值 1.0（不发光）；曝光 3.0 时 1.2 过阈值。
    let Some(dark_off) = render_with(Some(1.0), BloomSettings::default(), [0.4; 3], 48, 48) else {
        return;
    };
    let Some(dark_on) = render_with(Some(1.0), hard_bloom(), [0.4; 3], 48, 48) else {
        return;
    };
    assert!(
        dark_on.passes == 11,
        "开着光晕必须跑 9 趟链（1 场景 + 9 光晕 + 1 tone-map）：{}",
        dark_on.passes
    );
    assert!(
        dark_on.pixels == dark_off.pixels,
        "曝光后仍低于阈值时不该发光，画面应与关光晕逐字节相同"
    );

    let Some(lit_off) = render_with(Some(3.0), BloomSettings::default(), [0.4; 3], 48, 48) else {
        return;
    };
    let Some(lit_on) = render_with(Some(3.0), hard_bloom(), [0.4; 3], 48, 48) else {
        return;
    };
    let (a, b) = (center(&lit_off), center(&lit_on));
    assert!(
        lit_on.pixels[b] > lit_off.pixels[a],
        "曝光把亮度抬过阈值后必须有光晕：{} -> {}",
        lit_off.pixels[a],
        lit_on.pixels[b]
    );
}

#[test]
fn changing_exposure_rebuilds_the_bloom_chain() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let world = World::new();
    let mut reused = Renderer::new(MeshCache::new("."));
    reused.set_clear_color([0.4, 0.4, 0.4, 1.0]);
    reused.set_bloom(hard_bloom());
    let first = shoot(&mut reused, &context, &world, 48, 48);
    assert!(
        first.pixels == {
            // 曝光 1.0 时 0.4 不过阈值：初始链不发光。
            let desc = ResourceDesc::texture2d(
                "exposure-first-target",
                "rgba8unorm",
                48,
                48,
                vec![ResourceUsage::ColorAttachment],
            );
            let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
                panic!("渲染目标不是纹理");
            };
            let mut plain = Renderer::new(MeshCache::new("."));
            plain.set_clear_color([0.4, 0.4, 0.4, 1.0]);
            plain.render(&context, &texture, &world).unwrap();
            context.read_texture_rgba8(&texture, 48, 48).unwrap()
        },
        "曝光 1.0 时清屏 0.4 不该发光"
    );

    // 同一个渲染器把曝光抬到 3.0：链里缓存着 exposure 1.0，必须重建。
    reused.set_exposure(3.0);
    let lifted = shoot(&mut reused, &context, &world, 48, 48);

    let mut fresh = Renderer::new(MeshCache::new("."));
    fresh.set_clear_color([0.4, 0.4, 0.4, 1.0]);
    fresh.set_bloom(hard_bloom());
    fresh.set_exposure(3.0);
    let expected = shoot(&mut fresh, &context, &world, 48, 48);

    assert!(
        lifted.pixels == expected.pixels,
        "改曝光后光晕链必须按新曝光重建，与新建渲染器逐字节相同"
    );
    assert!(
        lifted.pixels[center(&lifted)] > first.pixels[center(&first)],
        "曝光抬过阈值后中心像素必须被光晕抬亮"
    );
}
