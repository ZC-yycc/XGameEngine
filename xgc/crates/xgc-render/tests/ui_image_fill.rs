//! UI 图片渲染：着色乘子（tint）与填充裁剪（fill）与旧端语义对齐。
//!
//! 旧端 `crates/xgame-ui/src/draw.rs`：
//! - `color = image.tint`（线性乘色）；
//! - `fill < 1` 时矩形与 UV 同步收窄（从左往右裁，图案保持比例）；
//! - `fill == 0` 或路径空时一个 sprite 都不画。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use xgc_asset::{scan, AssetPaths, SearchPaths};
use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{UiElement, UiLayout, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn write_ppm(label: &str, width: u32, height: u32, rgb: &[u8]) -> PathBuf {
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

fn render_element(root: &Path, element: UiElement, size: u32) -> Option<Vec<u8>> {
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
    let mut renderer = Renderer::new(MeshCache::new(root));
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
        image_path: path.file_name().unwrap().to_str().unwrap().to_owned(),
        image_tint: [1.0, 0.0, 0.0, 1.0],
        background: [0.0; 4],
        ..UiElement::default()
    };
    let Some(pixels) = render_element(path.parent().unwrap(), element, 16) else {
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
        image_path: path.file_name().unwrap().to_str().unwrap().to_owned(),
        image_fill: 0.5,
        background: [0.0; 4],
        ..UiElement::default()
    };
    let Some(pixels) = render_element(path.parent().unwrap(), element, 16) else {
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
            image_path: path.file_name().unwrap().to_str().unwrap().to_owned(),
            image_fill: 0.0,
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new(path.parent().unwrap()));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 0);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn ui_images_use_project_paths_and_cache_results() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "xgc-hud-project-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for directory in ["Hud", "Moved", "Fallback"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    let relative = "Hud/relative.ppm";
    let late = "Hud/late.ppm";
    let fallback = "ui-only-in-search-root.ppm";
    let guid = "0123456789abcdef0123456789abcdef";
    let direct_path = root.join(relative);
    let moved_path = root.join("Moved/glyph.ppm");
    let red = b"P6\n1 1\n255\n\xff\0\0";
    let blue = b"P6\n1 1\n255\n\0\0\xff";
    std::fs::write(&direct_path, red).unwrap();
    std::fs::write(&moved_path, blue).unwrap();
    std::fs::write(
        moved_path.with_extension("ppm.meta"),
        format!("(guid: \"{guid}\")"),
    )
    .unwrap();
    std::fs::write(root.join("Fallback").join(fallback), red).unwrap();
    let assets = AssetPaths::new(
        &root,
        SearchPaths::new(&["Fallback".into()]),
        Some(scan(&root).unwrap()),
    );
    let mut renderer = Renderer::new(MeshCache::with_assets(assets));
    let mut world = World::new();
    let references = [
        relative.to_owned(),
        format!("guid://{guid}"),
        fallback.to_owned(),
        direct_path.to_str().unwrap().to_owned(),
        late.to_owned(),
    ];
    for (column, image_path) in references.into_iter().enumerate() {
        world.spawn((
            UiElement {
                image_path,
                background: [0.0; 4],
                ..UiElement::default()
            },
            UiLayout {
                rect: [column as f32 * 8.0, 0.0, 8.0, 8.0],
                ..UiLayout::default()
            },
        ));
    }
    let desc = ResourceDesc::texture2d(
        "ui-project-images",
        "rgba8unorm",
        40,
        8,
        vec![ResourceUsage::ColorAttachment],
    );
    let mut render = || {
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("目标不是纹理");
        };
        let draws = renderer.render_ui(&context, &texture, &world).unwrap();
        let pixels = context.read_texture_rgba8(&texture, 40, 8).unwrap();
        let centers: [[u8; 4]; 5] = std::array::from_fn(|column| {
            let offset = 4 * (4 * 40 + column * 8 + 4);
            pixels[offset..offset + 4].try_into().unwrap()
        });
        (draws, centers)
    };
    let first = render();
    // 同一个 Renderer 下一帧应同时复用已上传的图片和缺失结果。
    std::fs::remove_file(&direct_path).unwrap();
    std::fs::remove_file(&moved_path).unwrap();
    std::fs::write(root.join(late), blue).unwrap();
    let second = render();
    std::fs::remove_dir_all(&root).unwrap();

    assert_eq!(first.1[0], [255, 0, 0, 255], "项目根的相对图片应可见");
    assert_eq!(first.1[1], [0, 0, 255, 255], "GUID 应取到当前路径的图片");
    assert_eq!(first.1[2..], [[0; 4]; 3]);
    assert_eq!(first.0, 2);
    assert_eq!(first, second);
}
