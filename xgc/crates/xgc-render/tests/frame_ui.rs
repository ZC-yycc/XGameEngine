//! 正式帧入口必须把屏幕空间 UI 合成在三维场景之后。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{GlobalTransform, Material, MeshRenderer, UiElement, UiLayout, World};

#[test]
fn frame_composites_ui_after_scene() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "frame-ui-target",
        "rgba8unorm-srgb",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("frame UI target 不是纹理");
    };
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
    world.spawn((
        UiElement {
            background: [0.0, 1.0, 0.0, 0.5],
            corner_radius: 0.0,
            ..UiElement::default()
        },
        UiLayout {
            rect: [0.0, 0.0, 16.0, 16.0],
            ..UiLayout::default()
        },
    ));

    let mut renderer = Renderer::new(MeshCache::new("."));
    let stats = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(stats.draws, 1);
    assert_eq!(stats.draw_calls, 1);
    assert_eq!(stats.ui_draws, 1);
    assert_eq!(stats.passes, 2);
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    let center = 4 * (8 * 16 + 8);
    let pixel = &pixels[center..center + 4];
    assert!(
        (187..=188).contains(&pixel[0])
            && (187..=188).contains(&pixel[1])
            && pixel[2] == 0
            && pixel[3] == 255,
        "HUD 必须在三维场景之后按 alpha 合成，实际 {pixel:?}"
    );
}
