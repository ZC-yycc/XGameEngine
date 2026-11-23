//! 首帧建体口径：刚体表必须在**本帧速度积分之前**建立，否则新建的刚体会把
//! 本帧 ECS 积分（`VelocitySystem` 的 `transform += velocity * dt`）当成场景位姿，
//! 整条轨迹比旧端多出 g·dt²。旧端 `sync_bodies()` 在固定步内先建体、再推进物理，
//! 这里用默认调度把它钉死。

use xgc_sim::{default_system_names, Simulation};
use xgc_world::{GlobalTransform, PhysicsBody, Transform, Velocity, World};

const DT: f32 = 1.0 / 60.0;
const G: f32 = 9.81;

fn dynamic_cube(y: f32) -> (World, xgc_base::EntityKey) {
    let mut world = World::new();
    let key = world.spawn((
        Transform {
            position: [0.0, y, 0.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, y, 0.0],
            ..GlobalTransform::default()
        },
        Velocity::default(),
        PhysicsBody {
            kind: "dynamic".to_string(),
            shape: "auto".to_string(),
            ..PhysicsBody::default()
        },
    ));
    (world, key)
}

/// 解析口径：旧端 `PhysicsWorld::step(1/60)` 的位姿等于每个固定步 4 个子步
/// symplectic Euler 的结果，第 N 帧累计位移 = g·dt²·N(4N+1)/8。
fn expected_drop(frame: i32) -> f32 {
    let k = G * DT * DT;
    k * (frame as f32) * (4.0 * frame as f32 + 1.0) / 8.0
}

#[test]
fn first_fixed_step_starts_from_the_scene_pose() {
    // 保护性断言：默认调度里确实有 Rapier 系统（否则本文件没有意义）。
    assert!(default_system_names().contains(&"rapier"));

    let (world, key) = dynamic_cube(7.0);
    let mut simulation = Simulation::with_fixed_dt(world, DT);
    for frame in 1..=6 {
        simulation.step(Vec::new()).unwrap();
        let world = simulation.world();
        let y = world.get::<Transform>(key).unwrap().position[1];
        let expected = 7.0 - expected_drop(frame);
        assert!(
            (y - expected).abs() < 1e-5,
            "第 {frame} 帧 y 应为 {expected}（旧端口径），实际 {y}：建体时可能吃到了本帧的速度积分"
        );
        let velocity = world.get::<Velocity>(key).unwrap();
        assert!(
            (velocity.linear[1] + G * DT * frame as f32).abs() < 1e-4,
            "第 {frame} 帧竖直速度应为 {}，实际 {}",
            -G * DT * frame as f32,
            velocity.linear[1]
        );
    }
}
