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
        (from: Some("idle"), to: "run", when: Some(AtLeast(parameter: "speed", value: 0.5))),
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

const HYSTERESIS: &str = r#"(
    initial: "idle",
    states: [
        (name: "idle", clip: "Survey"),
        (name: "walk", clip: "Walk"),
    ],
    transitions: [
        (from: Some("idle"), to: "walk", when: Some(AtLeast(parameter: "speed", value: 0.5))),
        (from: Some("walk"), to: "idle", when: Some(Below(parameter: "speed", value: 0.3))),
    ],
)"#;

#[test]
fn hysteresis_band_keeps_state_and_fade_time_is_the_default_blend() {
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine: HYSTERESIS.to_string(),
            parameters: "{\"speed\": 0.0}".to_string(),
            ..Default::default()
        },
        Animator {
            clip: "Walk".to_string(),
            fade_time: 0.25,
            ..Default::default()
        },
    ));
    let mut system = AnimationStateSystem;
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "Survey");

    world.get_mut::<AnimationState>(key).unwrap().parameters = "{\"speed\": 1.0}".to_string();
    system.run(&mut world, 0.1);
    let animator = world.get::<Animator>(key).unwrap();
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "walk");
    assert_eq!(animator.clip, "Walk");
    assert_eq!(
        animator.blend_from, "Survey",
        "转移没写 duration 时用 fade_time"
    );
    assert_eq!(animator.blend_duration, 0.25);
    assert_eq!(animator.time, 0.0, "换状态后播放相位从 0 开始");

    // 0.3 ≤ speed < 0.5：两个方向的 AtLeast / Below 都不成立，停在原地。
    world.get_mut::<AnimationState>(key).unwrap().parameters = "{\"speed\": 0.4}".to_string();
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "walk");

    world.get_mut::<AnimationState>(key).unwrap().parameters = "{\"speed\": 0.2}".to_string();
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "Survey");
}
