//! UI 真字体渲染门禁：文本必须真的画出字形（有覆盖率渐变、不是实心方块），
//! 且 `center` 对齐要把整行摆到内容框中间。

use std::sync::Mutex;

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{SceneEnvironment, UiElement, UiLayout, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

const WIDTH: u32 = 240;
const HEIGHT: u32 = 80;

fn world_with(text: &str, align: &str) -> World {
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    });
    // 透明背景：只留文字，方便统计"墨迹"。
    world.spawn((
        UiLayout {
            rect: [0.0, 0.0, WIDTH as f32, HEIGHT as f32],
            clip: [0.0, 0.0, WIDTH as f32, HEIGHT as f32],
            visible: true,
        },
        UiElement {
            text: text.to_string(),
            text_size: 20.0,
            text_color: [1.0, 1.0, 1.0, 1.0],
            text_align: align.to_string(),
            text_valign: "middle".to_string(),
            wrap: false,
            ..UiElement::default()
        },
    ));
    world
}

fn render(world: &World) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要 GPU 适配器");
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "ui-text",
            "rgba8unorm-srgb",
            WIDTH,
            HEIGHT,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render_ui(&context, texture, world).unwrap();
    context.read_texture_rgba8(texture, WIDTH, HEIGHT).unwrap()
}

fn ink_bbox(pixels: &[u8]) -> Option<(u32, u32, u32, u32)> {
    let mut bbox: Option<(u32, u32, u32, u32)> = None;
    for (index, pixel) in pixels.chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = (index as u32) % WIDTH;
        let y = (index as u32) / WIDTH;
        bbox = Some(match bbox {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    bbox
}

fn assert_font_available() -> bool {
    match xgc_render::FontAtlas::load() {
        Some(_) => true,
        None => {
            eprintln!("skip: 没有系统字体");
            false
        }
    }
}

#[test]
fn ui_text_draws_anti_aliased_glyphs() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() || !assert_font_available() {
        return;
    }
    let pixels = render(&world_with("XG", "left"));
    let bbox = ink_bbox(&pixels).expect("文字必须有墨迹");
    assert!(bbox.2 - bbox.0 > 10, "字宽太窄：{bbox:?}");
    assert!(bbox.3 - bbox.1 > 8, "字高太矮：{bbox:?}");
    // 有半透明的抗锯齿像素（覆盖率不是 0/1 两档）。
    let partial = pixels
        .chunks_exact(4)
        .filter(|pixel| pixel[3] > 0 && pixel[3] < 250)
        .count();
    assert!(partial > 20, "抗锯齿像素太少：{partial}");
    // 灰度层次足够（不是位图方块）。
    let levels: std::collections::BTreeSet<u8> =
        pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
    assert!(levels.len() > 4, "覆盖率层次太少：{}", levels.len());
}

#[test]
fn center_aligned_text_is_horizontally_centered() {
    let _guard = GPU_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    if GpuContext::headless().is_err() || !assert_font_available() {
        return;
    }
    let left = render(&world_with("Hello", "left"));
    let center = render(&world_with("Hello", "center"));
    let left_box = ink_bbox(&left).unwrap();
    let center_box = ink_bbox(&center).unwrap();
    assert!(center_box.0 > left_box.0, "居中对齐应把整行右移");
    let left_center = (left_box.0 + left_box.2) as f32 * 0.5;
    let center_center = (center_box.0 + center_box.2) as f32 * 0.5;
    assert!(
        (center_center - WIDTH as f32 * 0.5).abs() < 4.0,
        "居中后墨迹中心应落在内容框中心：{center_center}"
    );
    assert!(
        (center_center - left_center) > 20.0,
        "左对齐与居中应有明显位移"
    );
}
