//! Renderer 帧入口回归：空世界清黑、单实体 draw list 出一次 draw。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{GlobalTransform, Material, MeshRenderer, World};

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
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    let center = 4 * (2 * 4 + 2);
    assert_eq!(&pixels[center..center + 4], &[255, 0, 0, 255]);
}
