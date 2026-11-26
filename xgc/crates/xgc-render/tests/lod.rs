//! 主绘制和方向光、点光阴影必须消费同一个 LOD 筛选结果。

use xgc_render::{GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{
    Camera, GlobalTransform, LightKind, LodGroup, LodLevel, MeshRenderer, SceneEnvironment,
    SceneLight, Transform, World,
};

#[test]
fn lod_selection_is_shared_by_scene_and_shadows() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let (width, height) = (128, 72);
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "lod",
            "rgba8unorm",
            width,
            height,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let mut renderer = Renderer::new(MeshCache::new("."));
    let mut world = World::new();
    world.set_environment(SceneEnvironment {
        lights: vec![
            SceneLight {
                direction: [0.6, -1.0, 0.0],
                casts_shadow: true,
                ..SceneLight::default()
            },
            SceneLight {
                kind: LightKind::Point,
                position: [2.0, 3.0, 0.0],
                intensity: 6.0,
                casts_shadow: true,
                ..SceneLight::default()
            },
        ],
        ambient: [0.06; 3],
    });
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.1, 0.0],
            scale: [20.0, 0.2, 20.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".into(),
        },
    ));
    let caster = world.spawn((
        Transform {
            position: [0.0, 5.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, 2.0, 0.0],
            scale: [1.5; 3],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".into(),
        },
        LodGroup {
            levels: vec![LodLevel {
                model: "builtin://cube".into(),
                max_distance: 3.0,
            }],
        },
    ));
    world.spawn((
        GlobalTransform {
            position: [0.0, 6.0, 0.0],
            rotation: [
                -std::f32::consts::FRAC_1_SQRT_2,
                0.0,
                0.0,
                std::f32::consts::FRAC_1_SQRT_2,
            ],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));

    // 世界轴心距相机 4 m，局部位置只差 1 m；后者会让投射体错误留在批次里。
    let culled = renderer.render(&context, texture, &world).unwrap();
    assert_eq!(culled.draws, 1);
    assert_eq!(culled.point_shadow_draw_calls, 6);
    let culled_pixels = context.read_texture_rgba8(texture, width, height).unwrap();
    let shot_culled = renderer.render_shot(&context, texture, &world).unwrap();
    assert_eq!(shot_culled.draws, 1);
    assert_eq!(shot_culled.point_shadow_draw_calls, 6);

    world
        .get_mut::<LodGroup>(caster)
        .unwrap()
        .levels
        .push(LodLevel {
            model: "builtin://sphere".into(),
            max_distance: 4.0,
        });
    let far = renderer.render(&context, texture, &world).unwrap();
    assert_eq!(far.draws, 2, "最大距离的端点也要绘制");
    assert_eq!(far.draw_calls, 2, "球体需要与地面的立方体分组");
    assert_eq!(far.point_shadow_draw_calls, 12);

    assert!(world.despawn(caster));
    renderer.render(&context, texture, &world).unwrap();
    let absent_pixels = context.read_texture_rgba8(texture, width, height).unwrap();
    assert!(
        culled_pixels == absent_pixels,
        "超距物体不应留下方向光或点光阴影"
    );
}
