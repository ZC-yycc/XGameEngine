//! 子实体的刚体位置：旧端用**世界变换**建体（`body_from_scene(def, world)`），
//! 迁移世界里子实体的 `GlobalTransform` 初始等于局部值，需要运行期按父链合成。
//!
//! 这条测试钉的是「刚体建在子实体的世界位置、而不是局部位置」：父节点在 (-5,0.5,0)，
//! 子节点局部 (0,1.05,0) / 缩放 0.8（世界 (-5,1.55,0) 半高 0.4），箱子从 (-5,5,0)
//! 落下，应当停在世界顶面 1.95 上方；若刚体错误地建在局部 (0,1.05,0)，箱子会一直
//! 往下掉（x=-5 处没有别的碰撞体）。

use xgc_sim::Simulation;
use xgc_world::{GlobalTransform, Parent, PhysicsBody, Transform, Velocity, World};

fn body(kind: &str, shape: &str) -> PhysicsBody {
    PhysicsBody {
        kind: kind.to_string(),
        shape: shape.to_string(),
        ..PhysicsBody::default()
    }
}

fn transform(position: [f32; 3], scale: [f32; 3]) -> (Transform, GlobalTransform) {
    (
        Transform {
            position,
            scale,
            ..Transform::default()
        },
        GlobalTransform {
            position,
            scale,
            ..GlobalTransform::default()
        },
    )
}

#[test]
fn child_body_is_built_at_its_world_position() {
    let mut world = World::new();
    let (parent_transform, parent_global) = transform([-5.0, 0.5, 0.0], [1.0; 3]);
    let parent = world.spawn((parent_transform, parent_global, body("static", "auto")));
    let parent_id = world.entity(parent).expect("父实体存在");
    let (child_transform, child_global) = transform([0.0, 1.05, 0.0], [0.8; 3]);
    world.spawn((
        child_transform,
        child_global,
        Parent { entity: parent_id },
        body("static", "auto"),
    ));
    let (crate_transform, crate_global) = transform([-5.0, 5.0, 0.0], [1.0; 3]);
    let falling = world.spawn((
        crate_transform,
        crate_global,
        Velocity::default(),
        body("dynamic", "auto"),
    ));

    let mut simulation = Simulation::new(world);
    for _ in 0..240 {
        simulation.step_with(Vec::new(), |_| {}).expect("步进失败");
    }
    let position = simulation
        .world()
        .get::<Transform>(falling)
        .expect("箱子存在")
        .position;
    assert!(
        (position[1] - 2.45).abs() < 0.05,
        "箱子应停在世界顶面 1.95 之上（中心 2.45），实际 {:?}",
        position
    );
}
