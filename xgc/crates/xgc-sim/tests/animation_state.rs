//! 动画状态机：参数触发转换并写回 Animator.clip。

use xgc_sim::{AnimationStateSystem, System};
use xgc_world::{AnimationState, Animator, Component, World};

const MACHINE: &str = r#"(
    initial: "idle",
    states: [
        (name: "idle", clip: "idle.ron"),
        (name: "run", clip: "run.ron", speed: 1.5),
    ],
    transitions: [
        (from: Some("idle"), to: "run", parameter: "speed", at_least: 0.5),
    ],
)"#;

#[test]
fn transitions_on_parameter_and_writes_clip() {
    let _ = xgc_sim::parse_machine(MACHINE).unwrap();
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine: MACHINE.to_string(),
            parameters: "{\"speed\": 0.0}".to_string(),
            ..Default::default()
        },
        Animator::default(),
    ));
    let mut system = AnimationStateSystem;
    system.run(&mut world, 0.1);
    let entity = world.entity(key).unwrap();
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "idle.ron");
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");

    world.get_mut::<AnimationState>(key).unwrap().parameters = "{\"speed\": 1.0}".to_string();
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "run");
    let animator = world.get::<Animator>(key).unwrap();
    assert_eq!(animator.clip, "run.ron");
    assert_eq!(animator.speed, 1.5);
    let state = world.get::<AnimationState>(key).unwrap();
    assert_eq!(state.time, 0.1);
    let _ = entity;
    let _ = AnimationState::ID;
}
