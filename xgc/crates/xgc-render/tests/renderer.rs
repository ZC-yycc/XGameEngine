//! Renderer 帧入口回归：空世界清黑、单实体 draw list 出一次 draw。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, World};

#[test]
fn empty_world_clears_and_entity_emits_one_draw() {
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
