//! 编辑器选择高亮：DrawList 只改选中实体颜色，渲染为强调黄。

use std::sync::{Mutex, MutexGuard};

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{Camera, GlobalTransform, Material, MeshRenderer, World};

static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn selected_entity_is_tinted_in_viewport() {
    let _gpu = gpu_lock();
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "select-target",
        "rgba8unorm",
        4,
        4,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("select target 不是纹理");
    };
    let mut world = World::new();
    // 环境光拉满、不放方向光：高亮色就是最终像素期望值。
    world.set_environment(xgc_world::SceneEnvironment {
        lights: Vec::new(),
        ambient: [1.0; 3],
    });
    let entity = world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://quad".to_string(),
        },
        Material {
            base_color: [1.0, 0.0, 0.0, 1.0],
            roughness: 0.5,
            ..Material::default()
        },
    ));
    let id = world.entity(entity).unwrap();
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 1.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer
        .render_selected(&context, &texture, &world, Some(id))
        .unwrap();
    let pixels = context.read_texture_rgba8(&texture, 4, 4).unwrap();
    let center = 4 * (2 * 4 + 2);
    // 高亮色同样过 ACES 曲线：期望值由 CPU 镜像现算。
    let expected = xgc_render::aces_u8(xgc_render::ambient_light(
        [1.0, 0.82, 0.2],
        0.0,
        [1.0; 3],
        1.0,
    ));
    for (axis, want) in expected.into_iter().enumerate() {
        let got = pixels[center + axis];
        assert!(
            (i16::from(got) - i16::from(want)).abs() <= 1,
            "高亮色通道 {axis} 必须等于 CPU 镜像：实际 {got}，期望 {want}"
        );
    }
    assert_eq!(pixels[center + 3], 255);
}
