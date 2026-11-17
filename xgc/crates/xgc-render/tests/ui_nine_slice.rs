//! 九宫格图片：一张图切成 9 个单元格各画一次（与场景 / 文本无关，单独成文件守行数预算）。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::World;

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn ui_image_nine_slice_draws_nine_cells() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let path = std::env::temp_dir().join(format!(
        "xgc_slice_{}_{}.ppm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut data = b"P6\n4 4\n255\n".to_vec();
    for _ in 0..16 {
        data.extend_from_slice(&[255, 0, 0]);
    }
    std::fs::write(&path, &data).unwrap();
    let desc = ResourceDesc::texture2d(
        "slice-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("slice target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            image_path: path.to_string_lossy().into_owned(),
            image_mode: "nine_slice".to_string(),
            image_slice: 1.0,
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 9);
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    let center = 4 * (8 * 16 + 8);
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);
    std::fs::remove_file(&path).unwrap();
}
