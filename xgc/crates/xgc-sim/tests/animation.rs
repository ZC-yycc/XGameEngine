//! 剪辑采样回归：插值 / 边界 / 循环。

use xgc_sim::{
    AnimationClip, AnimationLibrary, AnimationPlaybackSystem, RotationKey, ScaleKey, Schedule,
    TranslationKey,
};
use xgc_world::{Animator, Transform, World};

fn clip() -> AnimationClip {
    AnimationClip::new(
        "Walk",
        2.0,
        vec![
            TranslationKey {
                time: 0.0,
                value: [0.0, 0.0, 0.0],
            },
            TranslationKey {
                time: 1.0,
                value: [2.0, 4.0, 6.0],
            },
            TranslationKey {
                time: 2.0,
                value: [0.0, 0.0, 0.0],
            },
        ],
    )
}

#[test]
fn samples_linear_and_clamps() {
    let clip = clip();
    assert_eq!(clip.sample_translation(-1.0), [0.0, 0.0, 0.0]);
    assert_eq!(clip.sample_translation(0.5), [1.0, 2.0, 3.0]);
    assert_eq!(clip.sample_translation(3.0), [0.0, 0.0, 0.0]);
}

#[test]
fn loops_with_duration_and_handles_empty() {
    let clip = clip();
    assert_eq!(clip.sample_looped(2.5), [1.0, 2.0, 3.0]);
    let empty = AnimationClip::default();
    assert_eq!(empty.sample_translation(10.0), [0.0; 3]);
}

#[test]
fn sorts_keys_and_drops_non_finite() {
    let clip = AnimationClip::new(
        "Sorted",
        1.0,
        vec![
            TranslationKey {
                time: 1.0,
                value: [1.0, 0.0, 0.0],
            },
            TranslationKey {
                time: f32::NAN,
                value: [9.0, 9.0, 9.0],
            },
            TranslationKey {
                time: 0.0,
                value: [0.0, 0.0, 0.0],
            },
        ],
    );
    assert_eq!(clip.translation.len(), 2);
    assert_eq!(clip.sample_translation(0.5), [0.5, 0.0, 0.0]);
}

#[test]
fn playback_writes_sampled_translation_onto_base() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Animator {
            clip: "Walk".to_string(),
            time: 0.5,
            base_position: [10.0, 0.0, 0.0],
            ..Animator::default()
        },
    ));
    let mut library = AnimationLibrary::new();
    library.insert(clip());
    let mut schedule = Schedule::new();
    schedule.add(AnimationPlaybackSystem { library });
    schedule.run(&mut world, 0.0);
    let transform = world.get::<Transform>(entity).unwrap();
    assert!((transform.position[0] - 11.0).abs() < 1e-5);
}

#[test]
fn samples_rotation_and_scale_and_applies_base() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Animator {
            clip: "Turn".to_string(),
            time: 0.5,
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            base_scale: [2.0, 2.0, 2.0],
            ..Animator::default()
        },
    ));
    let clip = AnimationClip::new("Turn", 2.0, Vec::new())
        .with_rotation(vec![
            RotationKey {
                time: 0.0,
                value: [0.0, 0.0, 0.0, 1.0],
            },
            RotationKey {
                time: 1.0,
                value: [0.0, 1.0, 0.0, 0.0],
            },
        ])
        .with_scale(vec![
            ScaleKey {
                time: 0.0,
                value: [1.0, 1.0, 1.0],
            },
            ScaleKey {
                time: 1.0,
                value: [3.0, 3.0, 3.0],
            },
        ]);
    assert!((clip.sample_looped_rotation(0.5)[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    assert!((clip.sample_looped_scale(0.5)[0] - 2.0).abs() < 0.01);
    let mut library = AnimationLibrary::new();
    library.insert(clip);
    let mut schedule = Schedule::new();
    schedule.add(AnimationPlaybackSystem { library });
    schedule.run(&mut world, 0.0);
    let transform = world.get::<Transform>(entity).unwrap();
    assert!((transform.scale[0] - 4.0).abs() < 0.01);
}
