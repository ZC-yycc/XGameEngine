//! 动画状态机：参数触发转换并写回 Animator.clip。

use xgc_sim::{AnimationStateSystem, System};
use xgc_world::{
    AnimationState, Animator, StateDef, StateMachine, TransitionCondition, TransitionDef, World,
};

#[test]
fn transitions_on_parameter_and_writes_clip() {
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine: StateMachine {
                initial: "idle".into(),
                states: vec![
                    StateDef {
                        name: "idle".into(),
                        clip: "idle.ron".into(),
                        looping: true,
                        speed: 1.0,
                    },
                    StateDef {
                        name: "run".into(),
                        clip: "run.ron".into(),
                        looping: false,
                        speed: 1.5,
                    },
                ],
                transitions: vec![TransitionDef {
                    from: Some("idle".into()),
                    to: "run".into(),
                    when: Some(TransitionCondition::AtLeast {
                        parameter: "speed".into(),
                        value: 0.5,
                    }),
                    duration: 0.0,
                }],
            },
            parameters: [("speed".into(), 0.0)].into(),
            ..Default::default()
        },
        Animator::default(),
    ));
    let mut system = AnimationStateSystem;
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "idle.ron");
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");

    world
        .get_mut::<AnimationState>(key)
        .unwrap()
        .parameters
        .insert("speed".into(), 1.0);
    system.run(&mut world, 0.1);
    let animator = world.get::<Animator>(key).unwrap();
    assert_eq!(animator.clip, "run.ron");
    assert_eq!(animator.speed, 1.5);
    assert!(!animator.looping);
    let state = world.get::<AnimationState>(key).unwrap();
    assert_eq!(state.state, "run");
    assert_eq!(state.time, 0.1);
}

#[test]
fn hysteresis_band_keeps_state_and_fade_time_is_the_default_blend() {
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine: StateMachine {
                initial: "idle".into(),
                states: vec![
                    StateDef {
                        name: "idle".into(),
                        clip: "Survey".into(),
                        looping: true,
                        speed: 1.0,
                    },
                    StateDef {
                        name: "walk".into(),
                        clip: "Walk".into(),
                        looping: true,
                        speed: 1.0,
                    },
                ],
                transitions: vec![
                    TransitionDef {
                        from: Some("idle".into()),
                        to: "walk".into(),
                        when: Some(TransitionCondition::AtLeast {
                            parameter: "speed".into(),
                            value: 0.5,
                        }),
                        duration: 0.0,
                    },
                    TransitionDef {
                        from: Some("walk".into()),
                        to: "idle".into(),
                        when: Some(TransitionCondition::Below {
                            parameter: "speed".into(),
                            value: 0.3,
                        }),
                        duration: 0.0,
                    },
                ],
            },
            parameters: [("speed".into(), 0.0)].into(),
            ..Default::default()
        },
        Animator {
            clip: "Walk".into(),
            fade_time: 0.25,
            ..Default::default()
        },
    ));
    let mut system = AnimationStateSystem;
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "Survey");

    world
        .get_mut::<AnimationState>(key)
        .unwrap()
        .parameters
        .insert("speed".into(), 1.0);
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
    world
        .get_mut::<AnimationState>(key)
        .unwrap()
        .parameters
        .insert("speed".into(), 0.4);
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "walk");

    world
        .get_mut::<AnimationState>(key)
        .unwrap()
        .parameters
        .insert("speed".into(), 0.2);
    system.run(&mut world, 0.1);
    assert_eq!(world.get::<AnimationState>(key).unwrap().state, "idle");
    assert_eq!(world.get::<Animator>(key).unwrap().clip, "Survey");
}

#[test]
fn default_machine_is_disabled_and_an_initial_name_still_advances_time() {
    let mut world = World::new();
    let disabled = world.spawn((AnimationState::default(), Animator::default()));
    let initialized = world.spawn((
        AnimationState {
            machine: StateMachine {
                initial: "idle".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        Animator {
            clip: "manual".into(),
            ..Default::default()
        },
    ));

    AnimationStateSystem.run(&mut world, 0.1);

    let disabled = world.get::<AnimationState>(disabled).unwrap();
    assert!(disabled.state.is_empty());
    assert_eq!(disabled.time, 0.0);
    let state = world.get::<AnimationState>(initialized).unwrap();
    assert_eq!(state.state, "idle");
    assert_eq!(state.time, 0.1);
    assert_eq!(world.get::<Animator>(initialized).unwrap().clip, "manual");
}
