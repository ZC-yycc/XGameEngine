//! 转场淡入：状态机设置 blend 字段，播放系统按时间插值。

use xgc_sim::{
    AnimationClip, AnimationLibrary, AnimationPlaybackSystem, AnimationStateSystem, System,
    TransitionDef, TranslationKey,
};
use xgc_world::{AnimationState, Animator, Transform, World};

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
    let machine = r#"(
        initial: "idle",
        states: [
            (name: "idle", clip: "idle"),
            (name: "run", clip: "run"),
        ],
        transitions: [
            (from: Some("idle"), to: "run", parameter: "speed", at_least: 0.5, duration: 1.0),
        ],
    )"#;
    let mut world = World::new();
    let key = world.spawn((
        AnimationState {
            machine: machine.to_string(),
            parameters: "{\"speed\": 1.0}".to_string(),
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

#[test]
fn transition_defaults_are_backwards_compatible() {
    let transition: TransitionDef =
        ron::from_str(r#"(from: None, to: "run", parameter: "speed", at_least: 0.5)"#).unwrap();
    assert_eq!(transition.duration, 0.0);
}
