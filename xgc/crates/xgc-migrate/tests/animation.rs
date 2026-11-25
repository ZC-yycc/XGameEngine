//! 旧 Animator 状态机 → `animation.state` 组件迁移。

use xgc_migrate::{migrate_str, MigrationError};
use xgc_world::{
    AnimationLayers, AnimationState, Animator, Component, LayerMode, TransitionCondition, Value,
    World,
};

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
                            additive: Some((
                                clip: "Run",
                                weight: 0.3,
                                speed: 1.5,
                                looping: true,
                            )),
                            root_motion: Some((
                                node: "b_Hip_01",
                                axes: (true, false, true),
                            )),
                            events: [
                                (time: 0.25, name: "footstep"),
                                (time: 0.5, name: "hit"),
                            ],
                            layers: [
                                (
                                    name: "upper",
                                    mode: Override,
                                    clip: "Walk",
                                    weight: 0.0,
                                    speed: 1.0,
                                    looping: true,
                                    machine: Some((
                                        entry: "Natural",
                                        parameters: {
                                            "survey": 0.0,
                                        },
                                        states: [
                                            (name: "Natural", clip: "Walk", speed: 1.0, looping: true),
                                            (name: "Survey", clip: "Survey", speed: 1.0, looping: true),
                                        ],
                                        transitions: [
                                            (
                                                from: "Natural",
                                                to: "Survey",
                                                when: AtLeast(param: "survey", value: 0.5),
                                                duration: Some(0.35),
                                            ),
                                        ],
                                    )),
                                    mask: ["b_Spine01_02"],
                                ),
                            ],
                            machine: Some((
                                entry: "Idle",
                                parameters: {
                                    "speed": 0.0,
                                },
                                states: [
                                    (name: "Idle", clip: "Survey", speed: 1.0, looping: true),
                                    (name: "Walk", clip: "Walk", speed: 1.5, looping: false),
                                ],
                                transitions: [
                                    (
                                        from: "Idle",
                                        to: "Walk",
                                        when: AtLeast(param: "speed", value: 0.5),
                                        duration: Some(0.35),
                                    ),
                                    (
                                        from: "Walk",
                                        to: "Idle",
                                        when: Below(param: "speed", value: 0.25),
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
    assert_eq!(world.next_id, 2);
    let entity = &world.entities[0];
    assert_eq!(entity.id.raw(), 1);
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
    let Value::Struct(machine) = machine else {
        panic!("machine 应是结构体，得到 {machine:?}");
    };
    assert_eq!(machine["initial"].as_string(), Some("Idle"));
    let Value::List(states) = &machine["states"] else {
        panic!("states 应是列表");
    };
    assert_eq!(states.len(), 2);
    let Value::List(transitions) = &machine["transitions"] else {
        panic!("transitions 应是列表");
    };
    assert_eq!(transitions.len(), 2);
    let Value::Struct(transition) = &transitions[0] else {
        panic!("转移应是结构体");
    };
    assert_eq!(
        transition["from"],
        Value::Enum {
            variant: "State".to_string(),
            payload: Box::new(Value::String("Idle".to_string())),
        }
    );
    assert_eq!(transition["to"].as_string(), Some("Walk"));
    assert_eq!(
        transition["when"],
        Value::Enum {
            variant: "AtLeast".to_string(),
            payload: Box::new(Value::Struct(
                [
                    ("parameter".to_string(), Value::String("speed".to_string())),
                    ("value".to_string(), Value::F64(0.5)),
                ]
                .into(),
            )),
        }
    );
    // 旧转移自己写了 duration 就保留，不写才回落到 Animator.fade_time。
    assert_eq!(transition["duration"].as_f32(), Some(0.35));

    let parameters = state
        .fields
        .iter()
        .find(|(name, _)| name == "parameters")
        .map(|(_, value)| value)
        .expect("缺 parameters 字段");
    assert_eq!(
        parameters,
        &Value::Map([("speed".to_string(), Value::F64(0.0))].into())
    );

    let mut restored = World::new();
    restored.restore_world_data(&world).unwrap();
    let key = restored.key(entity.id).unwrap();
    let state = restored.get::<AnimationState>(key).unwrap();
    assert_eq!(state.state, "");
    assert_eq!(state.time, 0.0);
    assert_eq!(state.parameters.get("speed"), Some(&0.0));
    let walk = &state.machine.states[1];
    assert_eq!(walk.name, "Walk");
    assert_eq!(walk.clip, "Walk");
    assert_eq!(walk.speed, 1.5);
    assert!(!walk.looping);
    let back = &state.machine.transitions[1];
    assert_eq!(back.from.as_deref(), Some("Walk"));
    assert_eq!(back.to, "Idle");
    assert_eq!(
        back.when,
        Some(TransitionCondition::Below {
            parameter: "speed".to_string(),
            value: 0.25,
        })
    );
    assert_eq!(back.duration, 0.0);

    // 叠加层配置一并搬进 Animator 组件；运行时相位（additive_time）不落盘。
    let animator = entity
        .components
        .iter()
        .find(|component| component.component == Animator::ID)
        .expect("Fox 迁移后应有 animation.animator 组件");
    let field = |name: &str| {
        animator
            .fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value)
            .unwrap_or_else(|| panic!("缺 {name} 字段"))
    };
    assert_eq!(
        field("additive_clip"),
        &xgc_world::Value::String("Run".to_string())
    );
    // 旧场景是 f32；组件字段存 f64，必须按 f32→f64 的精确值比较。
    assert_eq!(
        field("additive_weight"),
        &xgc_world::Value::F64(f64::from(0.3_f32))
    );
    assert_eq!(
        field("additive_speed"),
        &xgc_world::Value::F64(f64::from(1.5_f32))
    );
    assert_eq!(field("additive_looping"), &xgc_world::Value::Bool(true));

    // 旧 Animator.events 必须搬进组件字段，不能静默丢。
    assert_eq!(
        serde_json::to_value(field("events")).unwrap(),
        serde_json::json!({
            "kind": "list",
            "value": [
                {"kind": "struct", "value": {
                    "name": {"kind": "string", "value": "footstep"},
                    "time": {"kind": "f64", "value": 0.25},
                }},
                {"kind": "struct", "value": {
                    "name": {"kind": "string", "value": "hit"},
                    "time": {"kind": "f64", "value": 0.5},
                }},
            ],
        })
    );

    xgc_migrate::verify_migration(SCENE, &world).unwrap();

    // 根运动只存 RON 字符串，解析必须回到 world 侧唯一类型。
    let root_motion = match field("root_motion") {
        xgc_world::Value::String(text) => text.as_str(),
        other => panic!("root_motion 应是字符串，得到 {other:?}"),
    };
    let root_motion = xgc_world::RootMotion::parse(root_motion).expect("root_motion 必须能再解析");
    assert_eq!(root_motion.node, "b_Hip_01");
    assert_eq!(root_motion.axes, [true, false, true]);

    // 层栈一次性搬成独立组件：配置 / 遮罩 / 分层机器都在，运行时字段从零起。
    let layers = entity
        .components
        .iter()
        .find(|component| component.component == AnimationLayers::ID)
        .expect("Fox 迁移后应有 animation.layers 组件");
    let [(name, Value::List(layers))] = layers.fields.as_slice() else {
        panic!("迁移出的混合层应是结构化列表");
    };
    assert_eq!(name, "layers");
    assert_eq!(layers.len(), 1);
    let layer = &restored.get::<AnimationLayers>(key).unwrap().layers[0];
    assert_eq!(layer.name, "upper");
    assert_eq!(layer.mode, LayerMode::Override);
    assert_eq!(layer.clip, "Walk");
    assert_eq!(layer.weight, 0.0);
    assert_eq!(layer.mask, vec!["b_Spine01_02".to_string()]);
    let layer_machine = layer.machine.as_ref().expect("层机器必须一并迁移");
    assert_eq!(layer_machine.initial, "Natural");
    assert_eq!(layer_machine.states.len(), 2);
    assert_eq!(layer.parameters["survey"], 0.0);
    assert_eq!(layer.state, "", "运行时状态不落盘，从零起");
    assert_eq!(layer.time, 0.0);
}

#[test]
fn rejects_non_finite_base_machine_values() {
    for (original, invalid, field) in [
        (r#""speed": 0.0"#, r#""speed": NaN"#, "parameters"),
        (
            r#"when: Below(param: "speed", value: 0.25)"#,
            r#"when: Below(param: "speed", value: inf)"#,
            "machine",
        ),
    ] {
        let scene = SCENE.replacen(original, invalid, 1);
        let Err(MigrationError::Parse(message)) = migrate_str(&scene) else {
            panic!("animation.state.{field} 中的非有限值应在迁移时被拒绝");
        };
        assert!(
            message.starts_with(&format!("animation.state.{field}：")),
            "应点名出错的组件字段，得到 {message:?}"
        );
        assert!(message.contains("有限"), "应报非有限数，得到 {message:?}");
    }
}
