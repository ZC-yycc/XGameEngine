//! 相机矩阵回归：active 相机、view-projection 中心与 near/far。

use xgc_render::{camera_view_projection, transform_point};
use xgc_world::{Camera, GlobalTransform, World};

fn camera_world(position: [f32; 3], active: bool) -> World {
    let mut world = World::new();
    world.spawn((
        GlobalTransform {
            position,
            ..GlobalTransform::default()
        },
        Camera {
            active,
            ..Camera::default()
        },
    ));
    world
}

#[test]
fn active_camera_projects_origin_to_center() {
    let world = camera_world([0.0, 0.0, 5.0], true);
    let matrix = camera_view_projection(&world, 16, 9).unwrap();
    let clip = transform_point(matrix, [0.0, 0.0, 0.0, 1.0]);
    assert!(clip[0].abs() < 1e-5, "{clip:?}");
    assert!(clip[1].abs() < 1e-5, "{clip:?}");
    assert!((clip[3] - 5.0).abs() < 1e-5, "{clip:?}");
    let depth = clip[2] / clip[3];
    assert!((0.0..=1.0).contains(&depth), "depth={depth}");
}

#[test]
fn inactive_or_camera_without_global_is_ignored() {
    let world = camera_world([0.0, 0.0, 5.0], false);
    assert!(camera_view_projection(&world, 16, 9).is_none());

    let mut world = World::new();
    world.spawn((Camera::default(),));
    assert!(camera_view_projection(&world, 16, 9).is_none());
}
