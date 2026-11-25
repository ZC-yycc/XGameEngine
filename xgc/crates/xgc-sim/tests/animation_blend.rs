//! 转场淡入：状态机设置 blend 字段，播放系统按时间插值。

use xgc_sim::{
    AnimationClip, AnimationLibrary, AnimationPlaybackSystem, AnimationStateSystem, System,
    TranslationKey,
};
use xgc_world::{
    AnimationState, Animator, StateDef, StateMachine, Transform, TransitionCondition,
    TransitionDef, World,
};

fn clip(name: &str, x: f32) -> AnimationClip {
    AnimationClip::new(
        name,
        1.0,
        vec![TranslationKey {
            time: 0.0,
            value: [x, 0.0, 0.0],
        }],
    )
}

#[test]
fn state_transition_records_blend_and_playback_mixes() {
    let machine = StateMachine {
        initial: "idle".into(),
        states: ["idle", "run"]
            .into_iter()
            .map(|name| StateDef {
                name: name.into(),
                clip: name.into(),
                ..Default::default()
            })
            .collect(),
        transitions: vec![TransitionDef {
            from: Some("idle".into()),
            to: "run".into(),
            when: Some(TransitionCondition::AtLeast {
                parameter: "speed".into(),
                value: 0.5,
            }),
            duration: 1.0,
        }],
    };
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine,
            parameters: [("speed".into(), 1.0)].into(),
            ..Default::default()
        },
        Animator::default(),
        Transform::default(),
    ));
    AnimationStateSystem.run(&mut world, 0.1);
    let animator = world.get::<Animator>(key).unwrap();
    assert_eq!(animator.blend_from, "idle");
    assert_eq!(animator.blend_duration, 1.0);

    world.get_mut::<Animator>(key).unwrap().blend_elapsed = 0.25;
    let mut library = AnimationLibrary::new();
    library.insert(clip("idle", 0.0));
    library.insert(clip("run", 10.0));
    let mut playback = AnimationPlaybackSystem { library };
    playback.run(&mut world, 0.0);
    let position = world.get::<Transform>(key).unwrap().position;
    assert!((position[0] - 2.5).abs() < 1e-5, "{position:?}");
}
