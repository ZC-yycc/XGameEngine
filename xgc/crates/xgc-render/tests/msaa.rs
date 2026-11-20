//! 场景 pass 的 MSAA：边缘平滑、与后处理链共存、确定性与采样档位。
//!
//! 判据都用"1× 与 4× 两帧对照"：4× 的几何轮廓必须出现落在两侧平台之间的
//! 中间像素，1× 必须没有——这样既钉住 MSAA 真的生效，又不会因为换 GPU 的
//! 抖动而误报。期望值不写死具体颜色，只看"有没有过渡"。

use std::sync::Mutex;

use xgc_render::{
    GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage, SsaoSettings, TaaSettings,
};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, SceneEnvironment, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> std::sync::MutexGuard<'static, ()> {
    GPU_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 一块斜放的大四边形：边缘在屏幕上既不水平也不竖直，4× MSAA 一定会采样到
/// 部分覆盖的像素；只丢环境光，画面亮度不依赖光源角度。
fn edge_scene() -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![],
        ambient: [1.1; 3],
    });
    let half = 20.0f32.to_radians() * 0.5;
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, half.sin(), half.cos()],
            scale: [1.4, 1.4, 1.4],
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material::default(),
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 6.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    world
}

struct Frame {
    pixels: Vec<u8>,
    passes: usize,
}

/// 用同一个渲染器渲一帧（可以反复调用，缓存与历史都会留着）。
fn render_into(
    renderer: &mut Renderer,
    context: &GpuContext,
    label: &str,
    world: &World,
    width: u32,
    height: u32,
) -> Frame {
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
    let stats = renderer.render(context, texture, world).unwrap();
    Frame {
        pixels: context.read_texture_rgba8(texture, width, height).unwrap(),
        passes: stats.passes,
    }
}

fn luma(pixel: &[u8]) -> u32 {
    u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2])
}

/// 扫描穿过几何边缘的一行，数出"既不像背景也不像物体"的过渡像素。
///
/// 背景取左上角、物体取画面中心；只有亮度同时离开两侧平台 3 个灰阶以上才算
/// 过渡（避免把 1 LSB 的噪声当抗锯齿）。斜放的四边形保证每行都有边缘。
fn transition_pixels(frame: &Frame, width: u32, height: u32) -> usize {
    let background = luma(&frame.pixels[0..4]);
    let row = height as usize / 2;
    let center = (row * width as usize + width as usize / 2) * 4;
    let object = luma(&frame.pixels[center..center + 4]);
    assert!(
        object > background + 60,
        "中心必须是亮的物体、背景必须是暗的：物体 {object} / 背景 {background}"
    );
    let lo = background + 3;
    let hi = object - 3;
    let mut count = 0;
    for x in 0..width as usize {
        let offset = (row * width as usize + x) * 4;
        let value = luma(&frame.pixels[offset..offset + 4]);
        if value > lo && value < hi {
            count += 1;
        }
    }
    count
}

/// 多采样档位只能是 1× 或 4×：2× / 8× 不是 wgpu 保证的档，硬开会直接建不出管线。
#[test]
fn preferred_sample_count_is_one_or_four() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let count = context.scene_sample_count();
    assert!(count == 1 || count == 4, "场景采样数只能是 1 或 4：{count}");
}

/// 4× 必须把几何轮廓混出过渡像素；显式回到 1× 时必须是硬边（一个过渡都没有）。
#[test]
fn four_samples_smooth_geometry_edges() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let scene = edge_scene();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([0.02, 0.02, 0.02, 1.0]);
    if context.scene_sample_count() < 2 {
        eprintln!("skip: 本机适配器不支持 4× MSAA");
        return;
    }
    let smooth = render_into(&mut renderer, &context, "msaa-on", &scene, 96, 96);
    let smoothed = transition_pixels(&smooth, 96, 96);
    renderer.set_sample_count(1);
    let hard = render_into(&mut renderer, &context, "msaa-off", &scene, 96, 96);
    let hard_edges = transition_pixels(&hard, 96, 96);
    assert!(smoothed >= 1, "4× MSAA 的斜边必须出现过渡像素：{smoothed}");
    assert_eq!(
        hard_edges, 0,
        "关掉 MSAA 后斜边必须是硬的、不能有过渡像素：{hard_edges}"
    );
    assert!(
        smooth.passes == hard.passes,
        "MSAA 不该改变 pass 数：{} / {}",
        smooth.passes,
        hard.passes
    );
}

/// 4× 的多采样深度要能被 TAA / SSAO 正常绑定：pass 数不变、画面有内容、不 panic。
///
/// 深度绑定写错采样数会在 `set_bind_group` 时被 wgpu 校验拦下，直接 panic。
#[test]
fn msaa_coexists_with_taa_and_ssao() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    if context.scene_sample_count() < 2 {
        eprintln!("skip: 本机适配器不支持 4× MSAA");
        return;
    }
    let scene = edge_scene();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([0.02, 0.02, 0.02, 1.0]);
    renderer.set_taa(TaaSettings {
        enabled: true,
        history_weight: 0.9,
    });
    renderer.set_ssao(SsaoSettings {
        enabled: true,
        strength: 1.5,
        radius: 0.6,
        bias: 0.02,
    });
    let smooth = render_into(&mut renderer, &context, "msaa-post-on", &scene, 96, 96);
    renderer.set_sample_count(1);
    let hard = render_into(&mut renderer, &context, "msaa-post-off", &scene, 96, 96);
    assert!(
        smooth.passes == hard.passes,
        "采样数不该改变后处理 pass 数：{} / {}",
        smooth.passes,
        hard.passes
    );
    assert!(
        smooth.passes >= 6,
        "TAA + SSAO + 场景 + tone-map 至少六趟：{}",
        smooth.passes
    );
    let center = (48 * 96 + 48) * 4;
    assert!(
        luma(&smooth.pixels[center..center + 4]) > luma(&smooth.pixels[0..4]) + 40,
        "开了 TAA / SSAO 之后画面中心仍必须有几何"
    );
}

/// 同一渲染器连渲两帧、以及新建渲染器重渲，都必须逐字节相同（截图确定性）。
#[test]
fn msaa_is_deterministic() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let scene = edge_scene();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([0.02, 0.02, 0.02, 1.0]);
    let first = render_into(&mut renderer, &context, "msaa-det-a", &scene, 64, 64);
    let second = render_into(&mut renderer, &context, "msaa-det-b", &scene, 64, 64);
    assert!(
        first.pixels == second.pixels,
        "同一渲染器两帧必须逐字节相同"
    );
    let mut fresh = Renderer::new(MeshCache::new("."));
    fresh.set_clear_color([0.02, 0.02, 0.02, 1.0]);
    let again = render_into(&mut fresh, &context, "msaa-det-c", &scene, 64, 64);
    assert!(
        first.pixels == again.pixels,
        "新建渲染器同尺寸必须逐字节相同"
    );
}
