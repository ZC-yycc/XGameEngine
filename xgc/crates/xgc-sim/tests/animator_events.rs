//! 动画事件：AnimatorSystem 跨帧 diff 只发一次，循环跨圈能命中，非循环不重复。

use xgc_sim::{AnimatorSystem, System};
use xgc_world::{AnimEvent, Animator, Transform, World};

fn animator(events: Vec<AnimEvent>, looping: bool) -> Animator {
    Animator {
        duration: 1.0,
        looping,
        autoplay: true,
        events,
        ..Animator::default()
    }
}

#[test]
fn events_fire_once_on_the_crossing_frame() {
    let mut world = World::new();
    let entity = world.spawn((
        Transform::default(),
        animator(
            vec![
                AnimEvent::new("footstep", 0.25),
                AnimEvent::new("hit", 0.75),
            ],
            true,
        ),
    ));
    let mut system = AnimatorSystem::default();

    system.run(&mut world, 0.1);
    system.run(&mut world, 0.1);
    assert!(world.animation_events().is_empty(), "还没到 0.25");

    system.run(&mut world, 0.1);
    let fired: Vec<_> = world
        .animation_events()
        .iter()
        .map(|event| event.name.as_str())
        .collect();
    assert_eq!(fired, ["footstep"]);
    assert_eq!(
        world.animation_events()[0].entity,
        world.entity(entity).unwrap()
    );

    // 事件只属于本步：下一步没有新穿越就不该再看到 footstep。
    system.run(&mut world, 0.1);
    assert!(world.animation_events().is_empty());

    system.run(&mut world, 0.4);
    let fired: Vec<_> = world
        .animation_events()
        .iter()
        .map(|event| event.name.as_str())
        .collect();
    assert_eq!(fired, ["hit"]);
}

#[test]
fn looping_clip_fires_the_event_on_the_next_lap() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        Animator {
            time: 0.9,
            ..animator(vec![AnimEvent::new("lap", 0.4)], true)
        },
    ));
    let mut system = AnimatorSystem::default();

    system.run(&mut world, 0.1);
    assert!(world.animation_events().is_empty());
    system.run(&mut world, 0.5);
    let fired: Vec<_> = world
        .animation_events()
        .iter()
        .map(|event| event.name.as_str())
        .collect();
    assert_eq!(fired, ["lap"], "0.9→1.0 无事，下一圈 0→0.5 跨过 0.4");
}

#[test]
fn non_looping_clip_does_not_repeat_an_event() {
    let mut world = World::new();
    world.spawn((
        Transform::default(),
        animator(vec![AnimEvent::new("once", 0.25)], false),
    ));
    let mut system = AnimatorSystem::default();

    system.run(&mut world, 0.5);
    assert_eq!(world.animation_events().len(), 1);
    system.run(&mut world, 0.5);
    assert!(world.animation_events().is_empty(), "到片尾后不再重复");
}
