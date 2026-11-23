//! UI 图片渲染：着色乘子（tint）与填充裁剪（fill）与旧端语义对齐。
//!
//! 旧端 `crates/xgame-ui/src/draw.rs`：
//! - `color = image.tint`（线性乘色）；
//! - `fill < 1` 时矩形与 UV 同步收窄（从左往右裁，图案保持比例）；
//! - `fill == 0` 或路径空时一个 sprite 都不画。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{UiElement, UiLayout, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn write_ppm(label: &str, width: u32, height: u32, rgb: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "xgc_ui_image_{label}_{}_{}.ppm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut data = format!("P6\n{width} {height}\n255\n").into_bytes();
    data.extend_from_slice(rgb);
    std::fs::write(&path, &data).unwrap();
    path
}

fn render_element(element: UiElement, size: u32) -> Option<Vec<u8>> {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return None;
    };
    let desc = ResourceDesc::texture2d(
        "ui-image-target",
        "rgba8unorm",
        size,
        size,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("目标不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        element,
        UiLayout {
            rect: [0.0, 0.0, size as f32, size as f32],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render_ui(&context, &texture, &world).unwrap();
    Some(context.read_texture_rgba8(&texture, size, size).unwrap())
}

#[test]
fn ui_image_tint_multiplies_the_texture() {
    let _gpu = gpu_lock();
    let path = write_ppm(
        "tint",
        2,
        2,
        &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    );
    let element = UiElement {
        image_path: path.to_string_lossy().into_owned(),
        image_tint: [1.0, 0.0, 0.0, 1.0],
        background: [0.0; 4],
        ..UiElement::default()
    };
    let Some(pixels) = render_element(element, 16) else {
        std::fs::remove_file(&path).unwrap();
        return;
    };
    let center = 4 * (8 * 16 + 8);
    assert!(
        pixels[center] > 200,
        "红通道应保留：{:?}",
        &pixels[center..center + 4]
    );
    assert!(
        pixels[center + 1] < 10,
        "绿通道被乘成 0：{:?}",
        &pixels[center..center + 4]
    );
    assert!(
        pixels[center + 2] < 10,
        "蓝通道被乘成 0：{:?}",
        &pixels[center..center + 4]
    );
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn ui_image_fill_crops_from_the_left() {
    let _gpu = gpu_lock();
    // 2×1：左红右蓝；fill=0.5 只该看到左半（红）。
    let path = write_ppm("fill", 2, 1, &[255, 0, 0, 0, 0, 255]);
    let element = UiElement {
        image_path: path.to_string_lossy().into_owned(),
        image_fill: 0.5,
        background: [0.0; 4],
        ..UiElement::default()
    };
    let Some(pixels) = render_element(element, 16) else {
        std::fs::remove_file(&path).unwrap();
        return;
    };
    let left = 4 * (8 * 16 + 3);
    assert!(
        pixels[left] > 200 && pixels[left + 2] < 10,
        "左半是红：{:?}",
        &pixels[left..left + 4]
    );
    let right = 4 * (8 * 16 + 13);
    assert!(
        pixels[right..right + 4] == [0, 0, 0, 0],
        "右半必须被裁掉：{:?}",
        &pixels[right..right + 4]
    );
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn ui_image_fill_zero_draws_nothing() {
    let _gpu = gpu_lock();
    let path = write_ppm(
        "zero",
        2,
        2,
        &[255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255],
    );
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        std::fs::remove_file(&path).unwrap();
        return;
    };
    let desc = ResourceDesc::texture2d(
        "ui-image-zero",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("目标不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            image_path: path.to_string_lossy().into_owned(),
            image_fill: 0.0,
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 0);
    std::fs::remove_file(&path).unwrap();
}
