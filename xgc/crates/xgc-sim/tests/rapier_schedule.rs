//! 默认调度接入 rapier 的门禁：`Simulation` 走默认系统表时，动态刚体必须真的
//! 落地并翻滚（旧引擎同一条 rapier 求解路径），而不是停在半空。

use xgc_sim::{RapierWorld, Schedule, Simulation, FIXED_DT};
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

/// 运行期改 `PhysicsBody` 参数必须重建刚体：只比实体身份的话，改成 static 之后
/// 动态体仍按旧参数继续下落，编辑静默不生效。
#[test]
fn changing_a_body_parameter_rebuilds_it() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        body("static"),
    ));
    let cube = world.spawn((
        Transform {
            position: [0.0, 5.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, 5.0, 0.0],
            ..GlobalTransform::default()
        },
        Velocity::default(),
        body("dynamic"),
    ));
    let mut physics = RapierWorld::new();
    for _ in 0..10 {
        physics.step(&mut world, FIXED_DT);
    }
    let falling = world.get::<Transform>(cube).unwrap().position[1];
    assert!(falling < 5.0, "改参数前必须已经在往下掉：y={falling}");

    world.get_mut::<PhysicsBody>(cube).unwrap().kind = "static".to_string();
    physics.step(&mut world, FIXED_DT);
    let frozen = world.get::<Transform>(cube).unwrap().position[1];
    for _ in 0..10 {
        physics.step(&mut world, FIXED_DT);
    }
    let after = world.get::<Transform>(cube).unwrap().position[1];
    assert!(
        (after - frozen).abs() < 1e-6,
        "改成 static 后必须停止下落（参数改了要重建）：y {frozen} -> {after}"
    );
}

/// 稳态（同一份世界连续跑帧）不该整表重建；只有参数 / 实体集合变化才重建一次。
#[test]
fn steady_frames_do_not_rebuild_the_body_table() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        GlobalTransform::default(),
        body("static"),
    ));
    let cube = world.spawn((
        Transform {
            position: [0.0, 2.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, 2.0, 0.0],
            ..GlobalTransform::default()
        },
        Velocity::default(),
        body("dynamic"),
    ));
    let mut physics = RapierWorld::new();
    for _ in 0..10 {
        physics.step(&mut world, FIXED_DT);
    }
    assert_eq!(physics.rebuild_count(), 1, "首次建体只该发生一次");

    // 动态体每帧都在动，但位置 / 旋转不进签名 —— 不许因此重建。
    for _ in 0..20 {
        physics.step(&mut world, FIXED_DT);
    }
    assert_eq!(physics.rebuild_count(), 1, "稳态帧不该重建刚体表");

    world.get_mut::<PhysicsBody>(cube).unwrap().friction = 0.9;
    physics.step(&mut world, FIXED_DT);
    assert_eq!(physics.rebuild_count(), 2, "改摩擦必须重建一次");
    physics.step(&mut world, FIXED_DT);
    assert_eq!(physics.rebuild_count(), 2, "重建后继续复用，不重复重建");
}

/// 脚本常用缩放做脉冲动画；缩放一旦进签名，动画期间会每帧整表重建并把速度清零
/// （证据世界的展示盒就在动缩放，第一版实现因此改掉了 frame-150 的世界哈希）。
#[test]
fn animated_scale_does_not_rebuild_the_body_table() {
    let mut world = World::new();
    let cube = world.spawn((
        Transform {
            position: [0.0, 5.0, 0.0],
            scale: [0.6; 3],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, 5.0, 0.0],
            scale: [0.6; 3],
            ..GlobalTransform::default()
        },
        Velocity::default(),
        body("dynamic"),
    ));
    let mut physics = RapierWorld::new();
    physics.step(&mut world, FIXED_DT);
    assert_eq!(physics.rebuild_count(), 1);
    for frame in 1..20 {
        let scale = 0.6 - 0.01 * frame as f32;
        world.get_mut::<Transform>(cube).unwrap().scale = [scale; 3];
        world.get_mut::<GlobalTransform>(cube).unwrap().scale = [scale; 3];
        physics.step(&mut world, FIXED_DT);
    }
    assert_eq!(
        physics.rebuild_count(),
        1,
        "缩放动画不该触发刚体表重建（否则每帧清速度）"
    );
}

/// 建体签名必须覆盖**全部**构建参数：逐个只改一个字段都要恰好重建一次。
///
/// 只测 `kind` / `friction` 的话，签名里另外几个槽（尺寸 / 半径 / 半高 / 弹性 /
/// 质量 / 重力缩放 / 传感器 / 形状）写错、漏掉都不会变红，「改字段静默不生效」
/// 这个旧缺陷就会在没人测的字段上复活。
#[test]
fn every_build_parameter_is_part_of_the_signature() {
    type Mutate = fn(&mut PhysicsBody);
    let cases: [(&str, Mutate); 12] = [
        ("kind", |body| body.kind = "static".to_string()),
        ("shape", |body| body.shape = "cuboid".to_string()),
        // 三个分量各自一条：一次改三个的话，签名只错一个分量时测试仍绿。
        ("half_extents.x", |body| body.half_extents[0] = 0.7),
        ("half_extents.y", |body| body.half_extents[1] = 0.8),
        ("half_extents.z", |body| body.half_extents[2] = 0.9),
        ("radius", |body| body.radius = 0.61),
        ("half_height", |body| body.half_height = 0.62),
        ("friction", |body| body.friction = 0.63),
        ("restitution", |body| body.restitution = 0.64),
        ("mass", |body| body.mass = 2.5),
        ("gravity_scale", |body| body.gravity_scale = 0.65),
        ("is_sensor", |body| body.is_sensor = true),
    ];
    for (field, mutate) in cases {
        let mut world = World::new();
        world.spawn((
            Transform::default(),
            GlobalTransform::default(),
            body("static"),
        ));
        let cube = world.spawn((
            Transform {
                position: [0.0, 3.0, 0.0],
                ..Transform::default()
            },
            GlobalTransform {
                position: [0.0, 3.0, 0.0],
                ..GlobalTransform::default()
            },
            Velocity::default(),
            body("dynamic"),
        ));
        let mut rapier = RapierWorld::new();
        rapier.step(&mut world, FIXED_DT);
        assert_eq!(rapier.rebuild_count(), 1, "{field}: 首帧只该建一次表");
        let def = world.get_mut::<PhysicsBody>(cube).unwrap();
        mutate(def);
        rapier.step(&mut world, FIXED_DT);
        assert_eq!(
            rapier.rebuild_count(),
            2,
            "{field}: 改了这个构建参数必须重建刚体表"
        );
        rapier.step(&mut world, FIXED_DT);
        assert_eq!(rapier.rebuild_count(), 2, "{field}: 参数没再变就不该重建");
    }
}

/// `kind` 拼错（不在闭集里）必须与 `build_body` 同口径按动态体处理：
/// 建出动态体却不回写 Transform / Velocity 的话，实体会静默冻在半空。
#[test]
fn an_unknown_body_kind_still_writes_back_like_a_dynamic_body() {
    let mut world = World::new();
    let cube = world.spawn((
        Transform {
            position: [0.0, 4.0, 0.0],
            ..Transform::default()
        },
        GlobalTransform {
            position: [0.0, 4.0, 0.0],
            ..GlobalTransform::default()
        },
        Velocity::default(),
        body("Dynamic"),
    ));
    let mut rapier = RapierWorld::new();
    for _ in 0..60 {
        rapier.step(&mut world, FIXED_DT);
    }
    let y = world.get::<Transform>(cube).unwrap().position[1];
    assert!(y < 3.0, "拼错的 kind 也必须按动态体回写：y={y}");
}
