//! FXAA 回归：关闭态逐字节不变、只在边缘上动手、平坦画面不被柔化、换尺寸与确定性。
//!
//! 判据用"关闭态"当基线：FXAA 的设计约束是**只**改边缘像素，所以除了
//! "有些像素变了"之外，还要钉住"变了的像素都贴着强弱边"以及"平坦区一个字节都不动"。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{
    BloomSettings, FxaaSettings, GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc,
    ResourceUsage,
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
    width: u32,
    height: u32,
}

/// 一个硬边场景：亮 quad 压在暗背景上，四边都是 1 像素级的强对比边。
fn hard_edge_world() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.2; 3],
    });
    world.spawn((
        GlobalTransform {
            scale: [0.4, 0.3, 0.4],
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
    world
}

fn shoot(
    renderer: &mut Renderer,
    context: &GpuContext,
    world: &World,
    width: u32,
    height: u32,
    format: &str,
) -> Frame {
    let desc = ResourceDesc::texture2d(
        "fxaa-target",
        format,
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

fn render_with(
    fxaa: Option<FxaaSettings>,
    world: &World,
    width: u32,
    height: u32,
    format: &str,
    clear: [f64; 4],
) -> Option<Frame> {
    let context = GpuContext::headless().ok()?;
    let mut renderer = Renderer::new(MeshCache::new("."));
    // 这些用例的判据都建立在"场景本身是硬边"上：把场景采样数钉成 1×，
    // 让 FXAA 单独受测。场景 4× MSAA 与 FXAA 同时开的情况由 tests/msaa.rs 覆盖。
    renderer.set_sample_count(1);
    if let Some(fxaa) = fxaa {
        renderer.set_fxaa(fxaa);
    }
    renderer.set_clear_color(clear);
    Some(shoot(&mut renderer, &context, world, width, height, format))
}

fn enabled() -> FxaaSettings {
    FxaaSettings { enabled: true }
}

fn luma(pixels: &[u8], index: usize) -> i32 {
    let (r, g, b) = (
        i32::from(pixels[index]),
        i32::from(pixels[index + 1]),
        i32::from(pixels[index + 2]),
    );
    // Rec.601，与着色器同权重；这里只用来找边缘，不参与阈值判定。
    (299 * r + 587 * g + 114 * b) / 1000
}

/// 关闭态就是引入 FXAA 之前的那一帧：不调 `set_fxaa` 与显式关掉必须完全一样。
#[test]
fn disabled_fxaa_is_a_no_op() {
    let _gpu = gpu_lock();
    let world = hard_edge_world();
    let clear = [0.02, 0.02, 0.02, 1.0];
    let Some(never) = render_with(None, &world, 48, 48, "rgba8unorm", clear) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(off) = render_with(
        Some(FxaaSettings::default()),
        &world,
        48,
        48,
        "rgba8unorm",
        clear,
    ) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    assert_eq!(never.passes, 2, "场景 + tone-map，两趟");
    assert_eq!(off.passes, 2, "显式关掉也不该多跑 pass");
    assert!(never.pixels == off.pixels, "关闭态必须与从未开启逐字节相同");
}

/// 一帧画面的身份：尺寸 + 输出格式（格式必须进键，sRGB 的字节天然不同）。
type FrameKey<'a> = (u32, u32, &'a str);

/// 比较两帧：只能有边缘被改动，且改动都落在关闭态算出的边缘掩码 ±2 内。
///
/// 抽成辅助是因为 sRGB 目标要走同一套判据：形状一样、只是着色器里多一段
/// 线性↔sRGB 往返，用同一个断言才能真正钉住那条分支。
fn assert_only_edges_change(off: &Frame, on: &Frame) {
    let width_px = off.width as usize;
    let height_px = off.height as usize;
    let pixel = |x: usize, y: usize| (y * width_px + x) * 4;
    // 关闭态上的"强弱边"掩码：四邻域亮度差超过 24/255（8 位）。
    let is_edge = |x: usize, y: usize| {
        let center = luma(&off.pixels, pixel(x, y));
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width_px as i32 || ny >= height_px as i32 {
                continue;
            }
            let neighbor = luma(&off.pixels, pixel(nx as usize, ny as usize));
            if (center - neighbor).abs() > 24 {
                return true;
            }
        }
        false
    };
    let mut changed = 0usize;
    let mut far_from_edge = 0usize;
    for y in 0..height_px {
        for x in 0..width_px {
            let index = pixel(x, y);
            if off.pixels[index..index + 3] == on.pixels[index..index + 3] {
                continue;
            }
            changed += 1;
            // FXAA 最多沿边搜索 8 texel，但对 1 像素台阶的实际影响只在紧邻的两三像素。
            let mut near = false;
            'scan: for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width_px as i32 || ny >= height_px as i32 {
                        continue;
                    }
                    if is_edge(nx as usize, ny as usize) {
                        near = true;
                        break 'scan;
                    }
                }
            }
            if !near {
                far_from_edge += 1;
            }
        }
    }
    assert!(
        changed > 0,
        "开着 FXAA 必须至少改变一个边缘像素（否则着色器没跑）"
    );
    assert!(
        far_from_edge == 0,
        "FXAA 只许动边缘附近，却有 {far_from_edge} 个远离边缘的像素被改了"
    );
    // 平坦区逐字节不变：左上角（背景）与右下角各取一小块。
    for (x0, y0) in [(1usize, 1usize), (width_px - 4, height_px - 5)] {
        for y in y0..y0 + 3 {
            for x in x0..x0 + 3 {
                let index = pixel(x, y);
                assert!(
                    off.pixels[index..index + 3] == on.pixels[index..index + 3],
                    "平坦区 ({x},{y}) 不该被 FXAA 改变"
                );
            }
        }
    }
}

/// 开着 FXAA 只改边缘：与关闭态相比有像素变化，但变了的像素都贴着强对比边。
#[test]
fn fxaa_smooths_a_hard_edge_without_touching_flat_areas() {
    let _gpu = gpu_lock();
    let world = hard_edge_world();
    let clear = [0.02, 0.02, 0.02, 1.0];
    let (width, height) = (64u32, 64u32);
    let Some(off) = render_with(
        Some(FxaaSettings::default()),
        &world,
        width,
        height,
        "rgba8unorm",
        clear,
    ) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(on) = render_with(Some(enabled()), &world, width, height, "rgba8unorm", clear) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    assert_eq!(off.passes, 2, "关闭态两趟");
    assert_eq!(on.passes, 3, "开着时多一趟 FXAA");
    assert_only_edges_change(&off, &on);
}

/// sRGB 目标：着色器要手动 encode / decode 往返，判据与线性目标一致。
///
/// 编辑器视口就是 sRGB 表面，这条分支不平就没人跑；只验"确定性"抓不到它。
#[test]
fn fxaa_smooths_a_hard_edge_on_an_srgb_target() {
    let _gpu = gpu_lock();
    let world = hard_edge_world();
    let clear = [0.02, 0.02, 0.02, 1.0];
    let (width, height) = (48u32, 48u32);
    let Some(off) = render_with(
        Some(FxaaSettings::default()),
        &world,
        width,
        height,
        "rgba8unorm-srgb",
        clear,
    ) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(on) = render_with(
        Some(enabled()),
        &world,
        width,
        height,
        "rgba8unorm-srgb",
        clear,
    ) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    assert_eq!(off.passes, 2, "sRGB 关闭态两趟");
    assert_eq!(on.passes, 3, "sRGB 开着时多一趟 FXAA");
    assert_only_edges_change(&off, &on);
    // 值域判据：混合必须发生在显示空间，不是线性空间。
    // 这条边亮侧 ~255、暗侧 ~40，显示空间混合出的最亮改动像素实测 luma 204；
    // 若着色器跳过 srgb encode/decode（在线性空间混合）会到 215 —— 亮侧被抬高
    // 一截，正好是线性平均比感知平均更亮的已知差异。留 4 个 8 位刻度的余量。
    let mut brightest = 0i32;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let index = (y * width as usize + x) * 4;
            if off.pixels[index..index + 3] != on.pixels[index..index + 3] {
                brightest = brightest.max(luma(&on.pixels, index));
            }
        }
    }
    assert!(
        brightest <= 208,
        "sRGB 目标的边缘混合必须发生在显示空间（线性空间会亮到 215）：{brightest}"
    );
}

/// 整屏平坦（纯清屏色）时 FXAA 必须逐字节不变：局部对比度为 0，着色器原样返回中心值。
#[test]
fn fxaa_leaves_a_flat_frame_byte_identical() {
    let _gpu = gpu_lock();
    let world = World::new();
    let clear = [0.35, 0.5, 0.7, 1.0];
    let (width, height) = (32u32, 32u32);
    let Some(off) = render_with(
        Some(FxaaSettings::default()),
        &world,
        width,
        height,
        "rgba8unorm",
        clear,
    ) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let Some(on) = render_with(Some(enabled()), &world, width, height, "rgba8unorm", clear) else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    assert_eq!(on.passes, 3, "平坦画面照样跑一趟 FXAA（贴图往返必须无损）");
    assert!(
        off.pixels == on.pixels,
        "平坦画面经 FXAA 往返后必须逐字节相同"
    );
}

/// 换尺寸要重建 LDR scratch，sRGB 目标要走 encode/decode 分支；同尺寸两次必须逐字节确定。
///
/// 判据用"新建渲染器渲同一尺寸"当基准：scratch 没跟着重建的话，tone-map 会写进
/// 旧尺寸的纹理再由 FXAA 放大，pass 数仍是 3、自身也自洽，只有和新建渲染器比才看得出来。
#[test]
fn fxaa_rebuilds_on_resize_and_stays_deterministic() {
    let _gpu = gpu_lock();
    let world = hard_edge_world();
    let clear = [0.02, 0.02, 0.02, 1.0];
    let Some(context) = GpuContext::headless().ok() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    // 与 `render_with` 对齐：FXAA 用例都钉 1× 场景，别把 MSAA 的舍入混进判据。
    renderer.set_sample_count(1);
    renderer.set_fxaa(enabled());
    renderer.set_clear_color(clear);
    let mut previous: Option<(FrameKey<'_>, Vec<u8>)> = None;
    for (width, height, format) in [
        (40u32, 40u32, "rgba8unorm"),
        (64u32, 48u32, "rgba8unorm"),
        (64u32, 48u32, "rgba8unorm"),
        (64u32, 48u32, "rgba8unorm-srgb"),
        (64u32, 48u32, "rgba8unorm-srgb"),
    ] {
        let key = (width, height, format);
        let frame = shoot(&mut renderer, &context, &world, width, height, format);
        assert_eq!(
            frame.passes, 3,
            "{width}x{height}（{format}）开着 FXAA 该有三趟 pass"
        );
        match &previous {
            // 同一尺寸同一格式：确定性（两次必须逐字节相同）。
            Some((last_key, last)) if *last_key == key => {
                assert!(
                    last == &frame.pixels,
                    "{width}x{height}（{format}）两次渲染必须逐字节相同"
                );
            }
            // 换了尺寸或格式：scratch 必须重建，输出要等于新建渲染器的同键画面。
            _ => {
                let Some(fresh) =
                    render_with(Some(enabled()), &world, width, height, format, clear)
                else {
                    eprintln!("skip: 没有可用 GPU 适配器");
                    return;
                };
                assert!(
                    fresh.pixels == frame.pixels,
                    "{width}x{height}（{format}）换尺寸 / 格式后必须与新建渲染器逐字节相同"
                );
            }
        }
        previous = Some((key, frame.pixels));
    }
}

/// 光晕与 FXAA 同时开着：1 场景 + 9 光晕 + 1 tone-map + 1 FXAA = 12 趟。
///
/// 两段后处理各自有 pass 计数用例，但叠加时的顺序与计数没人钉。
#[test]
fn bloom_and_fxaa_both_on_count_twelve_passes() {
    let _gpu = gpu_lock();
    let world = hard_edge_world();
    let clear = [0.02, 0.02, 0.02, 1.0];
    let context = match GpuContext::headless() {
        Ok(context) => context,
        Err(_) => {
            eprintln!("skip: 没有可用 GPU 适配器");
            return;
        }
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_fxaa(enabled());
    renderer.set_bloom(BloomSettings {
        intensity: 1.0,
        ..BloomSettings::default()
    });
    renderer.set_clear_color(clear);
    let frame = shoot(&mut renderer, &context, &world, 48, 48, "rgba8unorm");
    assert_eq!(frame.passes, 12, "场景 + 光晕 9 趟 + tone-map + FXAA");
}
