//! 叠加层时钟：基础层与叠加层各自推进（旧端 additive 与基础共享 autoplay 总闸，
//! 但时间按各自的 speed 走）。

use xgc_sim::{AnimatorSystem, System};
use xgc_world::{Animator, Transform, World};

#[test]
fn additive_layer_has_its_own_clock() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Animator {
            additive_clip: "Run".to_string(),
            additive_speed: 1.5,
            ..Animator::default()
        },
    ));
    let mut system = AnimatorSystem;
    system.run(&mut world, 1.0 / 60.0);
    let animator = world.get::<Animator>(entity).unwrap();
    // 基础剪辑 speed 缺省 1.0；叠加层按自己的 1.5 推进。
    assert!((animator.time - 1.0 / 60.0).abs() < 1e-6);
    assert!(
        (animator.additive_time - 0.025).abs() < 1e-6,
        "additive_time={}",
        animator.additive_time
    );
}

#[test]
fn additive_clock_keeps_running_when_base_speed_is_zero() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        Animator {
            speed: 0.0,
            additive_clip: "Run".to_string(),
            additive_speed: 2.0,
            ..Animator::default()
        },
    ));
    let mut system = AnimatorSystem;
    system.run(&mut world, 0.5);
    let animator = world.get::<Animator>(entity).unwrap();
    assert_eq!(animator.time, 0.0);
    assert!((animator.additive_time - 1.0).abs() < 1e-6);
}

#[test]
fn empty_clip_zero_speed_or_no_autoplay_freeze_the_additive_clock() {
    let mut world = World::new();
    let empty = world.spawn((
        Transform::default(),
        Animator {
            additive_clip: String::new(),
            additive_speed: 5.0,
            ..Animator::default()
        },
    ));
    let stopped = world.spawn((
        Transform::default(),
        Animator {
            additive_clip: "Run".to_string(),
            additive_speed: 0.0,
            ..Animator::default()
        },
    ));
    let manual = world.spawn((
        Transform::default(),
        Animator {
            autoplay: false,
            additive_clip: "Run".to_string(),
            additive_speed: 1.0,
            ..Animator::default()
        },
    ));
    let mut system = AnimatorSystem;
    system.run(&mut world, 1.0);
    for entity in [empty, stopped, manual] {
        assert_eq!(world.get::<Animator>(entity).unwrap().additive_time, 0.0);
    }
}
