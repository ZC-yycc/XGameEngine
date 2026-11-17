//! 粒子 GPU 回归：billboard 软圆片、预乘 Alpha 与场景目标清除。

use xgc_render::{GpuContext, GpuResource, MeshCache, Renderer, ResourceDesc, ResourceUsage};
use xgc_world::{Camera, GlobalTransform, ParticleBlend, ParticleQuad, World};

#[test]
fn particle_billboard_draws_a_soft_premultiplied_circle() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "particle-target",
        "rgba8unorm",
        16,
        16,
        vec![ResourceUsage::ColorAttachment],
    );
    let resource = context.create_resource(&desc).unwrap();
    let GpuResource::Texture(texture) = resource else {
        panic!("particle target 不是纹理");
    };
    let mut world = World::new();
    world.spawn((
        GlobalTransform {
            position: [0.0, 0.0, 2.0],
            ..GlobalTransform::default()
        },
        Camera::default(),
    ));
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_particles(vec![ParticleQuad {
        position: [0.0; 3],
        size: 0.6,
        color: [1.0, 0.0, 0.0, 1.0],
        blend: ParticleBlend::Alpha,
    }]);
    let stats = renderer.render(&context, &texture, &world).unwrap();
    assert_eq!(stats.draw_calls, 1);
    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    let center = 4 * (8 * 16 + 8);
    assert!(
        pixels[center] > 200 && pixels[center + 1] < 5,
        "粒子中心应是经 ACES 的红色：{:?}",
        &pixels[center..center + 4]
    );
    assert_eq!(&pixels[..3], &[0, 0, 0], "软圆片外必须保持清屏色");
}

#[test]
fn additive_particles_accumulate_instead_of_alpha_blending() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let render_center = |count: usize| {
        let desc = ResourceDesc::texture2d(
            "particle-additive-target",
            "rgba8unorm",
            16,
            16,
            vec![ResourceUsage::ColorAttachment],
        );
        let GpuResource::Texture(texture) = context.create_resource(&desc).unwrap() else {
            panic!("particle target 不是纹理");
        };
        let mut world = World::new();
        world.spawn((
            GlobalTransform {
                position: [0.0, 0.0, 2.0],
                ..GlobalTransform::default()
            },
            Camera::default(),
        ));
        let quad = ParticleQuad {
            position: [0.0; 3],
            size: 0.6,
            color: [0.1, 0.0, 0.0, 0.5],
            blend: ParticleBlend::Additive,
        };
        let mut renderer = Renderer::new(MeshCache::new("."));
        renderer.set_particles(vec![quad; count]);
        let stats = renderer.render(&context, &texture, &world).unwrap();
        assert_eq!(stats.draw_calls, 1);
        let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
        pixels[4 * (8 * 16 + 8)]
    };

    let once = render_center(1);
    let twice = render_center(2);
    assert!(once > 0, "单颗加色粒子必须有贡献");
    assert!(
        twice > once,
        "两颗加色粒子应累加：once={once}, twice={twice}"
    );
    assert!(
        (i16::from(twice) - 2 * i16::from(once)).abs() <= 3,
        "加色应近似线性相加：once={once}, twice={twice}"
    );
}
