//! Renderer 帧入口回归：空世界清黑、单实体 draw list 出一次 draw。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, World};

use std::sync::{Mutex, MutexGuard};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn empty_world_clears_and_entity_emits_one_draw() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "render-target",
        "rgba8unorm",
        4,
        4,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("render target 不是纹理");
    };
    let mut renderer = Renderer::new(MeshCache::new("."));

    let empty = World::new();
    let stats = renderer.render(&context, &texture, &empty).unwrap();
    assert_eq!(stats.draws, 0);
    assert_eq!(stats.draw_calls, 0);
    assert_eq!(stats.passes, 0);
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    assert_eq!(&pixels[..4], &[0, 0, 0, 255]);

    let mut world = World::new();
    // 环境光拉满、不放方向光：本用例只看"一个实体是否出一次 draw"，不看光照。
    world.set_environment(xgc_world::SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    });
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            roughness: 0.5,
        },
    ));
    let stats = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(stats.draws, 1);
    assert_eq!(stats.draw_calls, 1);
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    let center = 4 * (2 * 4 + 2);
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);

    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 1.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let stats = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(stats.draws, 1);
    assert_eq!(stats.draw_calls, 1);
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);

    let mut depth_world = World::new();
    depth_world.set_environment(xgc_world::SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    });
    depth_world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 0.5],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color: [0.0, 1.0, 0.0, 1.0],
            roughness: 0.5,
        },
    ));
    depth_world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, -0.5],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            roughness: 0.5,
        },
    ));
    depth_world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 2.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let stats = renderer.render(&context, &texture, &depth_world).unwrap();
    assert_eq!(stats.draws, 2);
    assert_eq!(stats.draw_calls, 1, "同模型两个实体合并成一次实例化 draw");
    assert_eq!(stats.passes, 1, "两个实体应在同一个 render pass");
    assert_eq!(renderer.gpu_mesh_count(), 1, "同一模型只上传一次");
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    assert_eq!(
        &pixels[center..center + 4],
        &[0, 255, 0, 255],
        "近处绿色应挡住后画的远处红色"
    );
}

#[test]
fn ui_background_renders_over_target() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "ui-target",
        "rgba8unorm",
        4,
        4,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("UI target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            background: [1.0, 0.0, 0.0, 1.0],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 4.0, 4.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 1);
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
}

#[test]
fn ui_focus_ring_renders_border() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiInteraction, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "focus-target",
        "rgba8unorm",
        8,
        8,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("focus target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            background: [1.0, 0.0, 0.0, 1.0],
            ..UiElement::default()
        },
        UiLayout {
            rect: [2.0, 2.0, 4.0, 4.0],
            ..UiLayout::default()
        },
        UiInteraction {
            focused: true,
            ..UiInteraction::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 2);
    let pixels = context.read_texture_rgba8(&texture, 8, 8).unwrap();
    assert_eq!(&pixels[..4], &[255, 204, 51, 255]);
    let center = 4 * (4 * 8 + 4);
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);
}

#[test]
fn text_mesh_has_vertices_for_known_characters() {
    let mesh = xgc_render::text_mesh("A1", [0.0, 0.0], 4.0, [128.0, 64.0]);
    assert!(!mesh.positions.is_empty());
    let empty = xgc_render::text_mesh("中", [0.0, 0.0], 4.0, [128.0, 64.0]);
    assert!(empty.positions.is_empty());
}

#[test]
fn ui_text_renders_colored_pixels() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "text-target",
        "rgba8unorm",
        64,
        64,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("text target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            text: "A".to_string(),
            text_size: 56.0,
            text_color: [1.0, 0.0, 0.0, 1.0],
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 64.0, 64.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 1);
    let pixels = context.read_texture_rgba8(&texture, 64, 64).unwrap();
    assert!(pixels
        .chunks_exact(4)
        .any(|pixel| pixel[0] > 0 && pixel[1] == 0 && pixel[2] == 0));
}

#[test]
fn ppm_image_loads_and_renders_cells() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let path = std::env::temp_dir().join(format!(
        "xgc_image_{}_{}.ppm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut data = b"P6\n2 2\n255\n".to_vec();
    data.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255]);
    std::fs::write(&path, &data).unwrap();
    let image = xgc_render::ImageData::load_ppm(&path).unwrap();
    assert_eq!(image.width, 2);
    assert_eq!(image.height, 2);
    assert_eq!(image.cells(1, 1)[0], [0.5, 0.5, 0.5, 1.0]);

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        std::fs::remove_file(&path).unwrap();
        return;
    };
    let desc = ResourceDesc::texture2d(
        "image-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("image target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            image_path: path.to_string_lossy().into_owned(),
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 1);
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[0] > 0));
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn png_image_loads_and_renders_texture() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let path = std::env::temp_dir().join(format!(
        "xgc_png_{}_{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(file, 2, 2);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer
        .write_image_data(&[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ])
        .unwrap();
    writer.finish().unwrap();
    let image = xgc_render::ImageData::load(&path).unwrap();
    assert_eq!(image.width, 2);
    assert_eq!((image.cells(1, 1)[0][0] * 255.0) as u8, 127);
    if let Ok(context) = GpuContext::headless() {
        let desc = ResourceDesc::texture2d(
            "png-target",
            "rgba8unorm",
            16,
            16,
            vec![ResourceUsage::ColorAttachment],
        );
        let resource = context.create_resource(&desc).unwrap();
        let GpuResource::Texture(texture) = resource else {
            panic!("png target 不是纹理");
        };
        let mut world = World::new();
        world.spawn((
            UiElement {
                image_path: path.to_string_lossy().into_owned(),
                background: [0.0; 4],
                ..UiElement::default()
            },
            UiLayout {
                rect: [0.0, 0.0, 16.0, 16.0],
                ..UiLayout::default()
            },
        ));
        let mut renderer = Renderer::new(MeshCache::new("."));
        assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 1);
    }
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn ui_image_preserve_keeps_aspect() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let path = std::env::temp_dir().join(format!(
        "xgc_preserve_{}_{}.ppm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut data = b"P6\n2 1\n255\n".to_vec();
    data.extend_from_slice(&[255, 0, 0, 0, 255, 0]);
    std::fs::write(&path, &data).unwrap();
    let desc = ResourceDesc::texture2d(
        "preserve-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("preserve target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            image_path: path.to_string_lossy().into_owned(),
            image_mode: "preserve".to_string(),
            background: [0.0; 4],
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    assert_eq!(renderer.render_ui(&context, &texture, &world).unwrap(), 1);
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    assert_eq!(&pixels[..4], &[0, 0, 0, 0], "上下应留黑边");
    let center = 4 * (8 * 16 + 8);
    eprintln!(
        "pixels center {:?} top {:?} y4 {:?}",
        &pixels[center..center + 4],
        &pixels[..4],
        &pixels[4 * (4 * 16 + 8)..4 * (4 * 16 + 8) + 4]
    );
    assert!(pixels[center] > 0 || pixels[center + 1] > 0);
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn cover_uv_crops_wide_source() {
    let uv = xgc_render::cover_uv([4.0, 1.0], [16.0, 16.0]);
    assert!((uv[0] - 0.375).abs() < 1e-6);
    assert!((uv[2] - 0.25).abs() < 1e-6);
    let tall = xgc_render::cover_uv([1.0, 4.0], [16.0, 16.0]);
    assert!((tall[1] - 0.375).abs() < 1e-6);
    assert!((tall[3] - 0.25).abs() < 1e-6);
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

#[test]
fn ui_clip_scissors_background() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "clip-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("clip target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            background: [1.0, 0.0, 0.0, 1.0],
            corner_radius: 0.0,
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            clip: [0.0, 0.0, 8.0, 8.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render_ui(&context, &texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    let outside = 4 * (12 * 16 + 12);
    assert_eq!(&pixels[outside..outside + 4], &[0, 0, 0, 0]);
}

#[test]
fn text_wraps_when_width_is_tight() {
    let mesh = xgc_render::text_mesh_wrapped("AB", [0.0, 0.0], 8.0, [128.0, 128.0], 20.0);
    assert!(!mesh.positions.is_empty());
    let min = mesh
        .positions
        .iter()
        .fold(f32::MAX, |min, position| min.min(position[1]));
    let max = mesh
        .positions
        .iter()
        .fold(f32::MIN, |max, position| max.max(position[1]));
    assert!(max - min > 0.05, "换行后应出现两行高度：{min}..{max}");
}

#[test]
fn ui_background_rounds_corners() {
    let _gpu = gpu_lock();
    use xgc_world::{UiElement, UiLayout};

    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "round-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("round target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        UiElement {
            background: [1.0, 0.0, 0.0, 1.0],
            corner_radius: 8.0,
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.render_ui(&context, &texture, &world).unwrap();
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    assert_eq!(&pixels[..4], &[0, 0, 0, 0], "圆角外应透明");
    let center = 4 * (8 * 16 + 8);
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);
}
