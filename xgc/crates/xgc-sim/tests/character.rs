//! 角色台阶 / 阻挡回归。

use xgc_sim::{CharacterBlockSystem, CharacterStepSystem, Schedule};
use xgc_world::{CharacterController, PhysicsBody, Transform, Velocity, World};

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
fn character_blocks_on_high_wall() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.3, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((
        Transform {
            position: [1.0, 0.5, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            shape: "cuboid".to_string(),
            half_extents: [0.5, 0.5, 0.5],
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(CharacterBlockSystem);
    schedule.run(&mut world, 0.1);
    let transform = world.get::<Transform>(character).unwrap();
    let velocity = world.get::<Velocity>(character).unwrap();
    assert!((transform.position[0] - 0.2).abs() < 1e-5);
    assert_eq!(velocity.linear[0], 0.0);
}

#[test]
fn character_blocks_on_ball() {
    let mut world = World::new();
    let character = world.spawn((
        Transform {
            position: [0.3, 0.0, 0.0],
            ..Transform::default()
        },
        Velocity {
            linear: [3.0, 0.0, 0.0],
        },
        CharacterController::default(),
    ));
    world.spawn((
        Transform {
            position: [1.0, 0.5, 0.0],
            ..Transform::default()
        },
        PhysicsBody {
            shape: "ball".to_string(),
            radius: 0.5,
            ..PhysicsBody::default()
        },
    ));
    let mut schedule = Schedule::new();
    schedule.add(CharacterBlockSystem);
    schedule.run(&mut world, 0.1);
    let transform = world.get::<Transform>(character).unwrap();
    assert!((transform.position[0] - 0.2).abs() < 1e-5);
    assert_eq!(world.get::<Velocity>(character).unwrap().linear[0], 0.0);
}
