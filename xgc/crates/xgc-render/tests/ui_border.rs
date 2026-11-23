//! UI 覆盖层的圆角抗锯齿与边框：旧端 `ui_renderer` 的圆角矩形用 1px 过渡带做
//! 抗锯齿，并在 `border_width > 0` 时按 SDF 内缩把边框色混进填充色。XGC 之前
//! 只有"距离 < 0 就画、否则 discard"的硬边，也没有边框——血条 / 按钮圆角边缘
//! 与旧端差 1–2 px 的 AA，就是这条路径造成的。这里钉住这两条语义。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{UiElement, UiLayout, World};

const SIZE: u32 = 32;

fn render(world: &World, label: &str) -> Vec<u8> {
    let context = GpuContext::headless().expect("需要可用 GPU 适配器");
    let desc = ResourceDesc::texture2d(
        label,
        "rgba8unorm-srgb",
        SIZE,
        SIZE,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).expect("创建离屏目标");
    let GpuResource::Texture(texture) = resource else {
        panic!("{label} 不是纹理");
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer
        .render(&context, &texture, world)
        .expect("渲染成功");
    context
        .read_texture_rgba8(&texture, SIZE, SIZE)
        .expect("回读像素")
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let index = 4 * ((y * SIZE + x) as usize);
    [
        pixels[index],
        pixels[index + 1],
        pixels[index + 2],
        pixels[index + 3],
    ]
}

fn rounded(background: [f32; 4], border_width: f32) -> World {
    let mut world = World::new();
    world.spawn((
        UiElement {
            background,
            corner_radius: 6.0,
            border_width,
            border_color: [0.0, 1.0, 0.0, 1.0],
            ..UiElement::default()
        },
        UiLayout {
            rect: [4.0, 4.0, 24.0, 24.0],
            ..UiLayout::default()
        },
    ));
    world
}

#[test]
fn rounded_border_ring_uses_the_border_color() {
    let empty = render(&World::new(), "ui-border-empty");
    let pixels = render(&rounded([1.0, 0.0, 0.0, 1.0], 2.0), "ui-border-ring");
    let ring = pixel(&pixels, 5, 16);
    let center = pixel(&pixels, 16, 16);
    assert_ne!(ring, pixel(&empty, 5, 16), "有边框的元素必须画出来");
    assert!(
        ring[1] > 200 && ring[0] < 60,
        "边框那一圈应是边框色（绿），实际 {ring:?}"
    );
    assert!(
        center[0] > 200 && center[1] < 60,
        "填充内部应是背景色（红），实际 {center:?}"
    );
}

#[test]
fn transparent_background_with_border_is_drawn() {
    let empty = render(&World::new(), "ui-border-clear-empty");
    let pixels = render(&rounded([0.0, 0.0, 0.0, 0.0], 2.0), "ui-border-clear");
    let ring = pixel(&pixels, 5, 16);
    assert_ne!(
        ring,
        pixel(&empty, 5, 16),
        "只有边框、背景透明的元素也要画（旧端判据是 background.a > 0 || border_width > 0）"
    );
    assert!(ring[1] > 200, "边框色应可见，实际 {ring:?}");
}

#[test]
fn rounded_corner_is_antialiased() {
    let empty = render(&World::new(), "ui-aa-empty");
    let pixels = render(&rounded([0.0, 0.0, 1.0, 1.0], 0.0), "ui-aa-corner");
    // 像素中心 (5.5, 6.5) 到圆角圆心 (10, 10) 的距离 ≈ 5.70，比半径 6 小 0.3 ⇒
    // 落在 1px 过渡带里，覆盖率 ≈ 0.9（旧端会按覆盖度混 alpha，XGC 之前是硬边）。
    let partial = pixel(&pixels, 5, 6);
    let full = pixel(&pixels, 16, 16);
    assert_ne!(partial, pixel(&empty, 6, 7), "圆角弧上的像素必须被部分覆盖");
    assert_ne!(
        partial, full,
        "圆角弧上的像素应只覆盖一部分（1px 过渡带），实际与实心同色 {partial:?}"
    );
}
