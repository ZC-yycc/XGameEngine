//! 角色台阶回归（横向阻挡已改由 rapier 角色控制器负责，见 `character_kcc`）。

use xgc_sim::{CharacterStepSystem, CharacterSystem, Schedule};
use xgc_world::{CharacterController, PhysicsBody, Slope, Transform, Velocity, World};

#[test]
fn character_steps_onto_low_box() {
    let mut world = World::new();
    let character = world.spawn((
        Transform::default(),
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((
        Transform {
            position: [0.8, 0.1, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            shape: "cuboid".to_string(),
            half_extents: [0.5, 0.1, 0.5],
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(CharacterStepSystem);
    schedule.run(&mut world, 0.1);
    let transform = world.get::<Transform>(character).unwrap();
    assert!((transform.position[1] - 0.2).abs() < 1e-5);
}

#[test]
fn character_grounds_on_slope() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [5.0, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity::default(),
        CharacterController::default(),
    ));
    world.spawn((Slope {
        origin: [0.0, 0.0, 0.0],
        size: [10.0, 10.0],
        rise: 2.0,
    },));
    let mut schedule = Schedule::new();
    schedule.add(CharacterSystem::default());
    schedule.run(&mut world, 0.016);
    let transform = world.get::<Transform>(character).unwrap();
    assert!((transform.position[1] - 1.0).abs() < 1e-5);
    assert!(
        world
            .get::<CharacterController>(character)
            .unwrap()
            .grounded
    );
}
