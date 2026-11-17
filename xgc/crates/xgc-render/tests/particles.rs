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
fn additive_particles_accumulate_in_hdr_before_tone_map() {
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
            color: [0.5, 0.0, 0.0, 1.0],
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
    let expected_once = xgc_render::aces_u8([0.5, 0.0, 0.0])[0];
    let expected_twice = xgc_render::aces_u8([1.0, 0.0, 0.0])[0];
    assert!(
        (i16::from(once) - i16::from(expected_once)).abs() <= 1,
        "单颗粒子应先贡献线性 0.5、再统一 ACES：实际 {once}，期望 {expected_once}"
    );
    assert!(
        (i16::from(twice) - i16::from(expected_twice)).abs() <= 1 && twice < 255,
        "两颗粒子应先在线性 HDR 累成 1.0、再统一 ACES：实际 {twice}，期望 {expected_twice}"
    );
}

#[test]
fn alpha_particle_blends_with_linear_scene_before_tone_map() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let desc = ResourceDesc::texture2d(
        "particle-alpha-hdr-target",
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
    let mut renderer = Renderer::new(MeshCache::new("."));
    renderer.set_clear_color([0.2, 0.0, 0.0, 1.0]);
    renderer.set_particles(vec![ParticleQuad {
        position: [0.0; 3],
        size: 0.6,
        color: [0.0, 0.4, 0.0, 0.5],
        blend: ParticleBlend::Alpha,
    }]);
    renderer.render(&context, &texture, &world).unwrap();

    let pixels = context.read_texture_rgba8(&texture, 16, 16).unwrap();
    let center = 4 * (8 * 16 + 8);
    let expected = xgc_render::aces_u8([0.1, 0.2, 0.0]);
    for (axis, want) in expected.into_iter().enumerate() {
        let got = pixels[center + axis];
        assert!(
            (i16::from(got) - i16::from(want)).abs() <= 1,
            "Alpha 粒子应先与线性清屏色合成再过曲线：通道 {axis} 实际 {got}，期望 {want}"
        );
    }
    assert_eq!(pixels[center + 3], 255, "最终显示帧必须保持不透明");
}
