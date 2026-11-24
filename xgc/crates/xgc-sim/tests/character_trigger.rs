//! 角色 vs 传感器（trigger）：角色没有 `physics.body`，AABB 看不到它，接触由
//! `RapierWorld::move_characters` 里的胶囊求交产生。

use xgc_sim::{Schedule, FIXED_DT};
use xgc_world::{CharacterController, PhysicsBody, Transform, Velocity, World};

#[test]
fn a_character_entering_a_sensor_emits_enter_and_exit() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [1.6, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    let mut sensor_body = PhysicsBody {
        kind: "static".to_string(),
        shape: "cuboid".to_string(),
        half_extents: [1.0, 1.0, 1.0],
        ..PhysicsBody::default()
    };
    sensor_body.is_sensor = true;
    let sensor = world.spawn((
        Transform {
            position: [1.6, 1.0, 0.0],
            ..Transform::default()
        },
        sensor_body,
    ));
    let character_id = world.entity(character).unwrap();
    let sensor_id = world.entity(sensor).unwrap();

    let mut schedule = Schedule::default_schedule(1);
    world.clear_collision_events();
    schedule.run(&mut world, FIXED_DT);
    let enter: Vec<_> = world.collision_events().to_vec();
    assert_eq!(
        enter.len(),
        2,
        "角色进入触发区要给双方各一条事件：{enter:?}"
    );
    assert!(enter.iter().all(|event| event.enter));
    assert!(enter
        .iter()
        .any(|event| event.entity == character_id && event.other == sensor_id));
    assert!(enter
        .iter()
        .any(|event| event.entity == sensor_id && event.other == character_id));

    // 持续接触不重复发。
    world.clear_collision_events();
    schedule.run(&mut world, FIXED_DT);
    assert!(world.collision_events().is_empty());

    // 删掉触发区，下一帧发 exit（静态体的 Transform 改动不会推给 rapier，删除才会
    // 让 `ensure_built` 把碰撞体从表里摘掉；真实游戏里触发区也不会每帧挪位置）。
    world.despawn(sensor);
    world.clear_collision_events();
    schedule.run(&mut world, FIXED_DT);
    let exit: Vec<_> = world.collision_events().to_vec();
    assert_eq!(exit.len(), 2, "离开触发区要给双方各一条事件：{exit:?}");
    assert!(exit.iter().all(|event| !event.enter));
}
