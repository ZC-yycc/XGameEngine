//! 截图取景：顺光视轴、包围盒拟合与"不用场景相机"三条判据。

use xgc_render::{
    default_shot_direction, frame_bounds, frame_scene, scene_bounds, shot_direction,
    transform_point, Bounds, GpuContext, MeshCache, Renderer, ResourceDesc, ResourceUsage,
    DEFAULT_FOV_Y,
};
use xgc_world::{Camera, GlobalTransform, MeshRenderer, SceneEnvironment, SceneLight, World};

fn screen_point(
    frame: &xgc_render::ShotFrame,
    point: [f32; 3],
    width: u32,
    height: u32,
) -> [f32; 3] {
    let matrix = frame.view_projection(width, height);
    let [x, y, z, w] = transform_point(matrix, [point[0], point[1], point[2], 1.0]);
    [x / w, y / w, z / w]
}

fn direction_between(frame: &xgc_render::ShotFrame) -> [f32; 3] {
    let delta = delta_between(frame);
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    [delta[0] / length, delta[1] / length, delta[2] / length]
}

fn delta_between(frame: &xgc_render::ShotFrame) -> [f32; 3] {
    let (eye, target) = (frame.camera.eye, frame.camera.target);
    [eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[test]
fn frame_bounds_fits_every_corner_across_aspects() {
    let bounds = Bounds {
        min: [-3.0, -1.0, -2.0],
        max: [4.0, 5.0, 3.0],
    };
    for (width, height) in [(160u32, 90u32), (100, 100), (50, 100)] {
        let aspect = width as f32 / height as f32;
        let frame = frame_bounds(
            Some(&bounds),
            aspect,
            DEFAULT_FOV_Y,
            default_shot_direction(),
        );
        assert_eq!(frame.camera.target, bounds.center(), "目标必须是包围盒中心");
        for corner in bounds.corners() {
            let [x, y, z] = screen_point(&frame, corner, width, height);
            assert!(
                x.abs() <= 1.001 && y.abs() <= 1.001 && (0.0..=1.0).contains(&z),
                "角点 ({width}x{height}) 必须在视锥内：{x} {y} {z}"
            );
        }
    }
}

#[test]
fn frame_bounds_keeps_direction_and_empty_scene_still_frames() {
    let direction = default_shot_direction();
    let frame = frame_bounds(None, 16.0 / 9.0, DEFAULT_FOV_Y, direction);
    let actual = direction_between(&frame);
    for axis in 0..3 {
        assert!(
            (actual[axis] - direction[axis]).abs() < 1e-5,
            "空场景也要保持给定视轴：{actual:?} vs {direction:?}"
        );
    }
    let distance =
        (frame.camera.eye[0].powi(2) + frame.camera.eye[1].powi(2) + frame.camera.eye[2].powi(2))
            .sqrt();
    assert!(distance > 5.0, "空场景回退半径 5 之外：{distance}");
}

#[test]
fn shot_direction_follows_the_sun_and_is_lit_from_the_front() {
    let sun = SceneEnvironment {
        lights: vec![SceneLight::directional(
            "Sun",
            [0.45, -1.0, 0.35],
            [1.0, 0.97, 0.92],
            1.4,
        )],
        ambient: [0.09, 0.1, 0.13],
    };
    let direction = shot_direction(&sun);
    let to_light = {
        let [x, y, z] = sun.sun().unwrap().normalized_direction();
        [-x, -y, -z]
    };
    assert!(
        dot(direction, to_light) > 0.4,
        "视轴必须与指向光源的方向同侧（顺光）：{direction:?} vs {to_light:?}"
    );

    let overhead = SceneEnvironment {
        lights: vec![SceneLight::directional(
            "Top",
            [0.0, -1.0, 0.0],
            [1.0; 3],
            1.0,
        )],
        ambient: [0.0; 3],
    };
    let other = shot_direction(&overhead);
    let distance = ((direction[0] - other[0]).powi(2)
        + (direction[1] - other[1]).powi(2)
        + (direction[2] - other[2]).powi(2))
    .sqrt();
    assert!(distance > 0.5, "换光源方位视轴必须跟着转：{distance}");

    let dark = SceneEnvironment {
        lights: Vec::new(),
        ambient: [0.0; 3],
    };
    assert_eq!(shot_direction(&dark), default_shot_direction());
}

#[test]
fn scene_bounds_cover_scaled_cube() {
    let mut world = World::new();
    world.spawn((
        GlobalTransform {
            position: [0.0, -0.5, 0.0],
            scale: [24.0, 1.0, 24.0],
            ..GlobalTransform::default()
        },
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
    ));
    let mut meshes = MeshCache::new(".");
    let bounds = scene_bounds(&world, &mut meshes).unwrap().expect("有网格");
    let expected_min = [-12.0, -1.0, -12.0];
    let expected_max = [12.0, 0.0, 12.0];
    for axis in 0..3 {
        assert!(
            (bounds.min[axis] - expected_min[axis]).abs() < 1e-5,
            "min {axis}: {:?}",
            bounds.min
        );
        assert!(
            (bounds.max[axis] - expected_max[axis]).abs() < 1e-5,
            "max {axis}: {:?}",
            bounds.max
        );
    }
}

#[test]
fn clip_planes_stay_positive_and_cover_the_scene() {
    let frame = frame_bounds(
        Some(&Bounds {
            min: [-1.0; 3],
            max: [1.0; 3],
        }),
        16.0 / 9.0,
        DEFAULT_FOV_Y,
        default_shot_direction(),
    );
    let (near, far) = xgc_render::clip_planes(&frame.camera, frame.scene_radius);
    let delta = delta_between(&frame);
    let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    assert!(
        near > 0.0 && near < distance,
        "near {near} / dist {distance}"
    );
    assert!(far > distance + frame.scene_radius, "far {far}");
}

#[test]
fn frame_scene_uses_scene_lighting_direction() {
    let mut world = World::new();
    world.spawn((
        GlobalTransform::default(),
        MeshRenderer {
            model: "builtin://cube".to_string(),
        },
    ));
    let environment = SceneEnvironment {
        lights: vec![SceneLight::directional(
            "Sun",
            [0.45, -1.0, 0.35],
            [1.0, 0.97, 0.92],
            1.4,
        )],
        ambient: [0.09, 0.1, 0.13],
    };
    world.set_environment(environment.clone());
    let mut meshes = MeshCache::new(".");
    let frame = frame_scene(&world, &mut meshes, 1280, 720).unwrap();
    let expected = shot_direction(&environment);
    let actual = direction_between(&frame);
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 1e-5,
            "截图视轴必须来自场景光：{actual:?} vs {expected:?}"
        );
    }
}

/// 相机优先级与旧端 `--screenshot` 一致：**有场景相机就用场景相机**，
/// 只有场景没有相机时才顺光自动取景。
#[test]
fn render_shot_prefers_scene_camera_then_frames_the_scene() {
    let Ok(context) = GpuContext::headless() else {
        eprintln!("skip: 没有可用 GPU 适配器");
        return;
    };
    let target = context
        .create_resource(&ResourceDesc::texture2d(
            "shot-frame",
            "rgba8unorm",
            64,
            64,
            vec![ResourceUsage::ColorAttachment],
        ))
        .unwrap();
    let texture = target.as_texture().unwrap();
    let build_world = || {
        let mut world = World::new();
        world.spawn((
            GlobalTransform {
                position: [0.0, 0.0, 5.0],
                scale: [4.0, 4.0, 4.0],
                ..GlobalTransform::default()
            },
            MeshRenderer {
                model: "builtin://cube".to_string(),
            },
        ));
        world
    };
    let mut renderer = Renderer::new(MeshCache::new("."));
    let center = 4 * (32 * 64 + 32);

    // 没有相机：顺光自动取景把整套场景拍进画面。
    let mut world = build_world();
    let stats = renderer.render_shot(&context, texture, &world).unwrap();
    assert_eq!(stats.draws, 1);
    let framed = context.read_texture_rgba8(texture, 64, 64).unwrap();
    assert_ne!(
        &framed[center..center + 4],
        &[0, 0, 0, 255],
        "缺相机时必须自动取景，而不是只剩清屏色"
    );

    // 有场景相机：按相机看，立方体在它背后 → 画面只有清屏色。
    world.spawn((GlobalTransform::default(), Camera::default()));
    renderer.render_shot(&context, texture, &world).unwrap();
    let behind = context.read_texture_rgba8(texture, 64, 64).unwrap();
    assert_eq!(
        &behind[center..center + 4],
        &[0, 0, 0, 255],
        "有场景相机时不得改用自动取景"
    );
}
