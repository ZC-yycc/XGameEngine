//! 接触求解回归：落地与堆叠必须**静得下来**（位置不再漂、速度归零）。
//!
//! 判据取自真实失效模式：只清位置不清速度时，盒子每帧带着累积的向下速度重新
//! 砸回接触面，贴地后速度持续增长、位置抖动，堆叠永远不稳。

use xgc_sim::{AabbCollisionSystem, GroundCollisionSystem, PhysicsSystem, System, VelocitySystem};
use xgc_world::{PhysicsBody, Transform, Velocity, World};

const DT: f32 = 1.0 / 60.0;

fn body(kind: &str, half: [f32; 3]) -> PhysicsBody {
    PhysicsBody {
        kind: kind.to_string(),
        shape: "cuboid".to_string(),
        half_extents: half,
        ..PhysicsBody::default()
    }
}

fn run_frame(world: &mut World) {
    let mut physics = PhysicsSystem::default();
    let mut velocity = VelocitySystem::new(1);
    let mut ground = GroundCollisionSystem::default();
    let mut aabb = AabbCollisionSystem::default();
    physics.run(world, DT);
    velocity.run(world, DT);
    ground.run(world, DT);
    aabb.run(world, DT);
}

#[test]
fn resting_box_stops_accumulating_fall_speed() {
    let mut world = World::new();
    let key = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        body("dynamic", [0.5; 3]),
    ));
    for _ in 0..240 {
        run_frame(&mut world);
    }
    let velocity = *world.get::<Velocity>(key).unwrap();
    assert_eq!(velocity.linear[1], 0.0, "贴地后竖直速度必须归零");
    let resting = world.get::<Transform>(key).unwrap().position[1];
    for _ in 0..120 {
        run_frame(&mut world);
    }
    let after = world.get::<Transform>(key).unwrap().position[1];
    assert!(
        (after - resting).abs() < 1e-4,
        "静止后位置仍在漂：{resting} → {after}"
    );
}

#[test]
fn falling_box_rests_on_a_static_box() {
    let mut world = World::new();
    world.spawn((
        Transform {
            position: [0.0, 0.5, 0.0],
            ..Transform::default()
        },
        body("static", [0.5; 3]),
    ));
    let falling = world.spawn((
        Transform {
            position: [0.0, 4.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        body("dynamic", [0.5; 3]),
    ));
    for _ in 0..300 {
        run_frame(&mut world);
    }
    let transform = *world.get::<Transform>(falling).unwrap();
    let velocity = *world.get::<Velocity>(falling).unwrap();
    // 静态盒顶面 y = 1.0；动态盒半高 0.5 → 停在 1.5（允许 1 帧的分离余量）。
    assert!(
        (transform.position[1] - 1.5).abs() < 0.05,
        "应停在静态盒顶上：y = {}（期望 1.5）",
        transform.position[1]
    );
    assert_eq!(velocity.linear[1], 0.0, "堆叠稳定后竖直速度必须归零");
    let before = transform.position[1];
    for _ in 0..120 {
        run_frame(&mut world);
    }
    let after = world.get::<Transform>(falling).unwrap().position[1];
    assert!(
        (after - before).abs() < 1e-4,
        "堆叠仍在漂：{before} → {after}"
    );
}

#[test]
fn two_dynamic_boxes_stack_without_drifting_apart() {
    let mut world = World::new();
    let lower = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        body("dynamic", [0.5; 3]),
    ));
    let upper = world.spawn((
        Transform {
            position: [0.0, 5.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        body("dynamic", [0.5; 3]),
    ));
    for _ in 0..360 {
        run_frame(&mut world);
    }
    let low = world.get::<Transform>(lower).unwrap().position;
    let high = world.get::<Transform>(upper).unwrap().position;
    assert!(low[1] < high[1], "两个动态盒应一上一下：{low:?} / {high:?}");
    assert_eq!(low[0], high[0], "竖直堆叠不应横向漂移");
    for (key, position) in [(lower, low), (upper, high)] {
        let before = position[1];
        for _ in 0..120 {
            run_frame(&mut world);
        }
        let after = world.get::<Transform>(key).unwrap().position[1];
        assert!(
            (after - before).abs() < 1e-4,
            "堆叠位置在漂：{before} → {after}"
        );
    }
}

#[test]
fn contacts_emit_enter_once_and_exit_after_separation() {
    let mut world = World::new();
    let dynamic = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        body("dynamic", [0.5; 3]),
    ));
    let mut sensor_body = body("static", [1.0; 3]);
    sensor_body.is_sensor = true;
    let sensor = world.spawn((
        Transform {
            position: [0.0, 3.0, 0.0],
            ..Transform::default()
        },
        sensor_body,
    ));
    let dynamic_id = world.entity(dynamic).unwrap();
    let sensor_id = world.entity(sensor).unwrap();

    let mut aabb = AabbCollisionSystem::default();
    aabb.run(&mut world, DT);
    let enter: Vec<_> = world.collision_events().to_vec();
    assert_eq!(enter.len(), 2, "一对接触要给双方各一条事件：{enter:?}");
    assert!(enter.iter().all(|event| event.enter));
    assert!(enter
        .iter()
        .any(|event| event.entity == dynamic_id && event.other == sensor_id));
    assert!(enter
        .iter()
        .any(|event| event.entity == sensor_id && event.other == dynamic_id));

    // 持续接触不刷新：第二帧不能再发 enter。
    world.clear_collision_events();
    aabb.run(&mut world, DT);
    assert!(
        world.collision_events().is_empty(),
        "持续接触不应重复发事件"
    );

    // 移开后发 exit；传感器不挡路，所以动态体在重叠期间没有被推走。
    world.get_mut::<Transform>(sensor).unwrap().position[0] = 10.0;
    world.clear_collision_events();
    aabb.run(&mut world, DT);
    let exit: Vec<_> = world.collision_events().to_vec();
    assert_eq!(exit.len(), 2, "离开接触要给双方各一条事件：{exit:?}");
    assert!(exit.iter().all(|event| !event.enter));
}
