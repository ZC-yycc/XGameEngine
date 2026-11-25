//! 混合层按本层的参数、时钟与淡入进度独立推进。

use xgc_base::EntityKey;
use xgc_sim::{AnimationLayerSystem, System};
use xgc_world::{
    AnimLayer, AnimationLayers, Animator, StateDef, StateMachine, TransitionCondition,
    TransitionDef, World,
};

fn spawn(world: &mut World, autoplay: bool, duration: f32) -> EntityKey {
    world.spawn((
        AnimationLayers {
            layers: vec![AnimLayer {
                name: "upper".into(),
                machine: Some(StateMachine {
                    initial: "Natural".into(),
                    states: vec![
                        StateDef {
                            name: "Natural".into(),
                            clip: "Walk".into(),
                            ..StateDef::default()
                        },
                        StateDef {
                            name: "Survey".into(),
                            clip: "Survey".into(),
                            ..StateDef::default()
                        },
                    ],
                    transitions: vec![TransitionDef {
                        from: Some("Natural".into()),
                        to: "Survey".into(),
                        when: Some(TransitionCondition::AtLeast {
                            parameter: "survey".into(),
                            value: 0.5,
                        }),
                        duration,
                    }],
                }),
                ..AnimLayer::default()
            }],
        },
        Animator {
            autoplay,
            ..Animator::default()
        },
    ))
}

fn layer(world: &World, key: EntityKey) -> &AnimLayer {
    &world.get::<AnimationLayers>(key).unwrap().layers[0]
}

fn select_survey(world: &mut World, key: EntityKey) {
    world.get_mut::<AnimationLayers>(key).unwrap().layers[0]
        .parameters
        .insert("survey".into(), 1.0);
}

#[test]
fn layer_machine_switches_and_freezes_the_old_clock() {
    let mut world = World::new();
    let key = spawn(&mut world, true, 0.35);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    assert_eq!(layer(&world, key).state, "Natural");
    assert!((layer(&world, key).time - 0.1).abs() < 1e-6);

    select_survey(&mut world, key);
    system.run(&mut world, 0.1);
    let layer = layer(&world, key);
    assert_eq!(layer.state, "Survey");
    assert!((layer.fade_duration - 0.35).abs() < 1e-6);
    assert_eq!(layer.fade_from, "Walk");
    // 旧状态以切换时的播放速度继续走，从 0.1 推到 0.2。
    assert!((layer.fade_from_time - 0.2).abs() < 1e-6);
    assert!((layer.time - 0.1).abs() < 1e-6);
    assert!((layer.fade_elapsed - 0.1).abs() < 1e-6);
}

#[test]
fn layer_fade_ends_after_its_default_duration() {
    let mut world = World::new();
    let key = spawn(&mut world, true, 0.0);
    let mut system = AnimationLayerSystem;
    system.run(&mut world, 0.1);
    select_survey(&mut world, key);
    system.run(&mut world, 0.1);
    assert_eq!(layer(&world, key).fade_duration, 0.25);
    system.run(&mut world, 0.2);
    let layer = layer(&world, key);
    assert!(layer.fade_from.is_empty(), "淡入结束应清掉旧半边");
    assert_eq!(layer.fade_duration, 0.0);
}

#[test]
fn non_autoplay_layers_do_not_advance() {
    let mut world = World::new();
    let key = spawn(&mut world, false, 0.35);
    select_survey(&mut world, key);
    AnimationLayerSystem.run(&mut world, 0.1);
    let layer = layer(&world, key);
    assert_eq!(layer.state, "Natural");
    assert_eq!(layer.time, 0.0);
    assert!(layer.fade_from.is_empty());
}
