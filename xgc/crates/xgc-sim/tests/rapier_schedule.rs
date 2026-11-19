//! 默认调度接入 rapier 的门禁：`Simulation` 走默认系统表时，动态刚体必须真的
//! 落地并翻滚（旧引擎同一条 rapier 求解路径），而不是停在半空。

use xgc_sim::{Schedule, Simulation, FIXED_DT};
use xgc_world::{Command, GlobalTransform, PhysicsBody, Transform, Velocity, World};

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

/// 实体键换代（`Replace` / 组件增删 / 快照恢复）后动态刚体必须继续被回写。
///
/// `xgc-world` 的 `replace_entity_raw` 会 `despawn + restore`，**实体 ID 不变、`EntityKey`
/// 换代**。刚体表若只按 `physics.body` 数量判重建就会提前返回，`BodyLink.key` 停在过期
/// 代际 → `World::get_mut` 拒绝过期句柄，动态体的 `Transform` / `Velocity` 回写静默冻结
/// （角色表的 `character_signature` 是同一个坑）。
#[test]
fn a_dynamic_body_keeps_moving_after_its_entity_key_changes() {
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
    // 刻意**不给** `Velocity`：默认调度里的 `VelocitySystem` 会给任何带
    // `Velocity + Transform` 的实体积分位置，有它的话"y 在下降"根本分不清是 rapier
    // 回写还是 ECS 积分。没有 `Velocity` 时只有 rapier 的 `BodyLink` 会写这个 Transform，
    // 换代后回写是否还生效就看这条断言。
    let cube = world.spawn((
        Transform {
            position: [0.0, 6.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform::default(),
        body("dynamic"),
    ));
    let id = world.entity(cube).unwrap();
    // 全程同一个 `Schedule`：换一个新的会把刚体表整份重建，掩盖"签名没变"的过期句柄。
    let mut schedule = Schedule::default_schedule(1);
    for _ in 0..10 {
        schedule.run(&mut world, FIXED_DT);
    }
    let before = world.get::<Transform>(cube).unwrap().position[1];
    assert!(before < 6.0, "动态体没被 rapier 推进：y={before}");

    let data = world.entity_data(id).unwrap();
    let plan = world.plan(vec![Command::Replace { data }]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    world.apply(&plan).unwrap();
    let replaced = world.key(id).unwrap();
    assert_ne!(replaced, cube, "`Replace` 必须换代");

    for _ in 0..10 {
        schedule.run(&mut world, FIXED_DT);
    }
    let after = world.get::<Transform>(replaced).unwrap().position[1];
    assert!(
        after < before - 0.05,
        "换代后动态刚体回写被冻结：y={after}（换代前 {before}）"
    );
}
