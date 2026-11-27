//! 角色查询和普通刚体碰撞体共用项目掩码。

use xgc_base::layer::{CollisionLayers, LayerIndex};
use xgc_sim::{Schedule, FIXED_DT};
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
