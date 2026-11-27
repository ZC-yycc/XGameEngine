//! 刚体、简化接触和角色查询须使用同一份项目掩码。

use xgc_base::layer::{CollisionLayers, LayerIndex};
use xgc_sim::{AabbCollisionSystem, Schedule, System, FIXED_DT};
use xgc_world::{CharacterController, CollisionLayer, PhysicsBody, Transform, Velocity, World};

fn isolated_layer() -> CollisionLayer {
    CollisionLayer {
        index: LayerIndex::new(31).unwrap(),
    }
}

fn block_layer() -> CollisionLayers {
    CollisionLayers {
        masks: [(31, 0)].into(),
        ..CollisionLayers::default()
    }
}

fn platform(y: f32, is_sensor: bool) -> (Transform, PhysicsBody, CollisionLayer) {
    (
        Transform {
            position: [0.0, y, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            kind: "static".into(),
            shape: "cuboid".into(),
            half_extents: [2.0, 0.5, 2.0],
            is_sensor,
            ..PhysicsBody::default()
        },
        isolated_layer(),
    )
}

#[test]
fn aabb_reports_exit_when_either_membership_starts_blocking() {
    let mut world = World::new();
    let moving = world.spawn((
        Transform::default(),
        PhysicsBody::default(),
        CollisionLayer::default(),
    ));
    let sensor = world.spawn(platform(0.0, true));
    let moving_id = world.entity(moving).unwrap();
    let sensor_id = world.entity(sensor).unwrap();
    let mut aabb = AabbCollisionSystem::default();

    for (layers, entering) in [
        (CollisionLayers::default(), true),
        (block_layer(), false),
        (CollisionLayers::default(), true),
    ] {
        world.set_collision_layers(&layers);
        world.clear_collision_events();
        aabb.run(&mut world, FIXED_DT);
        let events = world.collision_events();
        assert_eq!(events.len(), 2, "静止重叠随矩阵跨越接触边界：{events:?}");
        assert!(events.iter().all(|event| event.enter == entering));
        assert!(events
            .iter()
            .any(|event| event.entity == moving_id && event.other == sensor_id));
        assert!(events
            .iter()
            .any(|event| event.entity == sensor_id && event.other == moving_id));
    }
    assert_eq!(world.get::<Transform>(moving).unwrap().position, [0.0; 3]);
}

#[test]
fn character_ignores_both_ground_and_trigger_on_a_blocked_layer() {
    let run = |layers: &CollisionLayers| {
        let mut world = World::new();
        world.set_collision_layers(layers);
        world.spawn(platform(1.5, false));
        let sensor = world.spawn(platform(3.25, true));
        let character = world.spawn((
            Transform {
                position: [0.0, 3.0, 0.0],
                ..Transform::default()
            },
            Velocity::default(),
            CharacterController::default(),
        ));
        let sensor_id = world.entity(sensor).unwrap();
        let character_id = world.entity(character).unwrap();
        let mut enters = 0;
        let mut schedule = Schedule::default_schedule(1);
        for _ in 0..90 {
            world.clear_collision_events();
            schedule.run(&mut world, FIXED_DT);
            enters += world
                .collision_events()
                .iter()
                .filter(|event| {
                    event.entity == character_id && event.other == sensor_id && event.enter
                })
                .count();
        }
        assert!(
            world
                .get::<CharacterController>(character)
                .unwrap()
                .grounded
        );
        (
            world.get::<Transform>(character).unwrap().position[1],
            enters,
        )
    };
    let (allowed_y, allowed_enters) = run(&CollisionLayers::default());
    let (blocked_y, blocked_enters) = run(&block_layer());
    assert!((allowed_y - 2.0).abs() < 0.03, "应站在平台上：{allowed_y}");
    assert!(blocked_y.abs() < 1e-3, "应掉到隐式地面：{blocked_y}");
    assert_eq!((allowed_enters, blocked_enters), (1, 0));
}
