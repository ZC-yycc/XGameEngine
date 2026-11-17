//! 默认调度接入 rapier 的门禁：`Simulation` 走默认系统表时，动态刚体必须真的
//! 落地并翻滚（旧引擎同一条 rapier 求解路径），而不是停在半空。

use xgc_sim::Simulation;
use xgc_world::{GlobalTransform, PhysicsBody, Transform, Velocity, World};

fn body(kind: &str) -> PhysicsBody {
    PhysicsBody {
        kind: kind.to_string(),
        shape: "auto".to_string(),
        ..PhysicsBody::default()
    }
}

#[test]
fn default_schedule_lands_a_dynamic_cube_with_rapier() {
    let mut world = World::new();
    world.spawn((
        Transform {
            position: [0.0, -0.5, 0.0],
            scale: [20.0, 1.0, 20.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, -0.5, 0.0],
            scale: [20.0, 1.0, 20.0],
            ..GlobalTransform::default()
        },
        body("static"),
    ));
    let angle = 0.3f32;
    let cube = world.spawn((
        Transform {
            position: [0.0, 4.0, 0.0],
            rotation: [angle.sin(), 0.0, 0.0, angle.cos()],
            ..Transform::default()
        },
        GlobalTransform::default(),
        Velocity::default(),
        body("dynamic"),
    ));
    let mut simulation = Simulation::with_fixed_dt(world, 1.0 / 60.0);
    for _ in 0..240 {
        simulation.step(Vec::new()).unwrap();
    }
    let world = simulation.world();
    let transform = *world.get::<Transform>(cube).unwrap();
    assert!(
        (transform.position[1] - 0.5).abs() < 0.05,
        "动态立方体应停在 y = 0.5，实际 {}",
        transform.position[1]
    );
    let rotation = transform.rotation;
    assert!(
        (rotation[0] - angle.sin()).abs() > 1e-3 || rotation[2].abs() > 1e-3,
        "落地后姿态应被接触力矩改变：{rotation:?}"
    );
}
