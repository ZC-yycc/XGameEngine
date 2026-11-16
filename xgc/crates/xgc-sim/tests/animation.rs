//! 剪辑采样回归：插值 / 边界 / 循环。

use xgc_sim::{AnimationClip, TranslationKey};

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
