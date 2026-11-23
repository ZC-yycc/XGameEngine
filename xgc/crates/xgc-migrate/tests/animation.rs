//! 旧 Animator 状态机 → `animation.state` 组件迁移。

use xgc_migrate::migrate_str;
use xgc_world::{parse_machine, parse_parameters, AnimationState, Component, StateMachine};

const SCENE: &str = r#"(
    format_version: 2,
    name: "Fox",
    lights: [],
    ambient: (0.0, 0.0, 0.0),
    entities: [
        (
            id: (1),
            name: "Fox",
            parent: None,
            layer: 0,
            transform: (
                position: (0.0, 0.0, 1.8),
                rotation: (0.0, 0.0, 0.0, 1.0),
                scale: (0.01, 0.01, 0.01),
            ),
            transform_identity: (id: 1),
            components: (
                next_component_id: (next_id: 2),
                entries: [
                    (
                        identity: (id: 1),
                        data: Animator((
                            clip: "Walk",
                            speed: 1.0,
                            looping: true,
                            autoplay: true,
                            fade_time: 0.25,
                            machine: Some((
                                entry: "Idle",
                                parameters: {
                                    "speed": 0.0,
                                },
                                states: [
                                    (name: "Idle", clip: "Survey", speed: 1.0, looping: true),
                                    (name: "Walk", clip: "Walk", speed: 1.0, looping: true),
                                ],
                                transitions: [
                                    (
                                        from: "Idle",
                                        to: "Walk",
                                        when: AtLeast(param: "speed", value: 0.5),
                                        duration: Some(0.35),
                                    ),
                                ],
                            )),
                        )),
                    ),
                ],
            ),
        ),
    ],
)"#;

#[test]
fn animator_machine_becomes_animation_state_component() {
    let (world, report) = migrate_str(SCENE).unwrap();
    assert_eq!(report.entities, 1);
    let entity = &world.entities[0];
    let state = entity
        .components
        .iter()
        .find(|component| component.component == AnimationState::ID)
        .expect("Fox 迁移后应有 animation.state 组件");

    let machine = state
        .fields
        .iter()
        .find(|(name, _)| name == "machine")
        .map(|(_, value)| value)
        .expect("缺 machine 字段");
    let machine = match machine {
        xgc_world::Value::String(text) => text,
        other => panic!("machine 应是字符串，得到 {other:?}"),
    };
    let machine: StateMachine = parse_machine(machine).expect("迁移出的机器必须能再解析");
    assert_eq!(machine.initial, "Idle");
    assert_eq!(machine.states.len(), 2);
    assert_eq!(machine.transitions.len(), 1);
    let transition = &machine.transitions[0];
    assert_eq!(transition.from.as_deref(), Some("Idle"));
    assert_eq!(transition.to, "Walk");
    // 旧转移自己写了 duration 就保留，不写才回落到 Animator.fade_time。
    assert_eq!(transition.duration, 0.35);

    let parameters = state
        .fields
        .iter()
        .find(|(name, _)| name == "parameters")
        .map(|(_, value)| value)
        .expect("缺 parameters 字段");
    let parameters = match parameters {
        xgc_world::Value::String(text) => text,
        other => panic!("parameters 应是字符串，得到 {other:?}"),
    };
    let parameters = parse_parameters(parameters);
    assert_eq!(parameters.get("speed"), Some(&0.0));
}
