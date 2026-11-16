//! 骨架姿态回归。

use xgc_sim::{parse_pose, Schedule, SkeletonPoseSystem};
use xgc_world::{Bone, Parent, Skeleton, SkeletonPose, Transform, World};

#[test]
fn skeleton_pose_composes_parent_chain() {
    let mut world = World::new();
    let root = world.spawn((
        Skeleton { joint_count: 2 },
        SkeletonPose::default(),
        Transform::default(),
    ));
    let root_id = world.entity(root).unwrap();
    let bone0 = world.spawn((
        Parent { entity: root_id },
        Bone { index: 0 },
        Transform {
            position: [1.0, 0.0, 0.0],
            ..Transform::default()
        },
    ));
    let bone0_id = world.entity(bone0).unwrap();
    world.spawn((
        Parent { entity: bone0_id },
        Bone { index: 1 },
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(SkeletonPoseSystem);
    schedule.run(&mut world, 0.016);
    let pose = world.get::<SkeletonPose>(root).unwrap();
    let matrices = parse_pose(&pose.matrices).unwrap();
    assert_eq!(matrices.len(), 2);
    assert!((matrices[0][12] - 1.0).abs() < 1e-5);
    assert!((matrices[1][12] - 1.0).abs() < 1e-5);
    assert!((matrices[1][13] - 2.0).abs() < 1e-5);
}
