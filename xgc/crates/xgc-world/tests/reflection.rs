//! 字段路径必须沿事务中的当前值解析，逆命令按同一路径还原。

use xgc_base::{diagnostics as codes, EntityId};
use xgc_world::{AnimEvent, AnimationState, Animator, Command, Component, LodGroup, Value, World};

#[test]
fn edits_machine_conditions_and_parameters_in_one_transaction() {
    let mut world = World::new();
    let key = world.spawn((AnimationState::default(),));
    let entity = world.entity(key).unwrap();
    let get = |world: &World, path| world.get_field(entity, AnimationState::ID, path);
    assert_eq!(
        get(&world, "machine.initial"),
        Some(Value::String(String::new()))
    );
    let initial = world.world_data();
    let hash = world.state_hash();
    let set = |path: &str, value| Command::SetField {
        entity,
        component: AnimationState::ID,
        field: path.into(),
        value,
    };
    let enum_value = |variant: &str, payload| Value::Enum {
        variant: variant.into(),
        payload: Box::new(payload),
    };
    let transition = Value::Struct(
        [
            (
                "from".into(),
                enum_value("Any", Value::Struct(Default::default())),
            ),
            ("to".into(), Value::String("Walk".into())),
            (
                "when".into(),
                enum_value(
                    "AtLeast",
                    Value::Struct(
                        [
                            ("parameter".into(), Value::String("speed:main".into())),
                            ("value".into(), Value::F64(0.5)),
                        ]
                        .into(),
                    ),
                ),
            ),
            ("duration".into(), Value::F64(0.0)),
        ]
        .into(),
    );
    let condition = "machine.transitions[0].when.AtLeast.value";
    let parameter = r#"parameters["speed:main"]"#;
    let plan = world.plan(vec![
        set("machine.transitions", Value::List(vec![transition])),
        set(
            "parameters",
            Value::Map([("speed:main".into(), Value::F64(0.0))].into()),
        ),
        set(condition, Value::F64(1.25)),
        set(parameter, Value::F64(2.0)),
    ]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    let applied = world.apply(&plan).unwrap();
    assert_eq!(get(&world, condition), Some(Value::F64(1.25)));
    assert_eq!(get(&world, parameter), Some(Value::F64(2.0)));
    assert_ne!(world.state_hash(), hash);
    let mut restored = World::new();
    restored.restore_world_data(&world.world_data()).unwrap();
    assert_eq!(get(&restored, condition), Some(Value::F64(1.25)));
    assert_eq!(restored.state_hash(), world.state_hash());

    let before = world.world_data();
    for (path, value) in [(condition, f64::MAX), (parameter, f64::INFINITY)] {
        let rejected = world.plan(vec![set(path, Value::F64(value))]);
        assert!(!rejected.is_valid(), "{path}");
        assert!(world.apply(&rejected).is_err());
    }
    assert_eq!(world.world_data(), before);
    world.revert(&applied).unwrap();
    assert_eq!(
        world.world_data().entities[0].components,
        initial.entities[0].components
    );
    assert_eq!(world.state_hash(), hash);
}

#[test]
fn edits_and_reverts_lod_levels() {
    let mut world = World::new();
    let key = world.spawn((LodGroup::default(),));
    let entity = world.entity(key).unwrap();
    let levels = |distance| {
        Value::List(vec![Value::Struct(
            [
                ("model".into(), Value::String("guid://aabbccdd".into())),
                ("max_distance".into(), Value::F64(distance)),
            ]
            .into(),
        )])
    };
    let set = |field: &str, value| Command::SetField {
        entity,
        component: LodGroup::ID,
        field: field.into(),
        value,
    };
    let before = world.world_data();
    let plan = world.plan(vec![
        set("levels", levels(30.0)),
        set("levels[0].max_distance", Value::F64(-1.0)),
    ]);
    assert!(plan.is_valid(), "{:?}", plan.diagnostics);
    let applied = world.apply(&plan).unwrap();
    assert_eq!(
        world.get_field(entity, LodGroup::ID, "levels"),
        Some(levels(-1.0))
    );
    let current = world.world_data();
    let invalid = world.plan(vec![set(
        "levels[0].max_distance",
        Value::F64(f64::INFINITY),
    )]);
    assert_eq!(invalid.diagnostics[0].code, codes::EDIT_TYPE_MISMATCH);
    assert!(world.apply(&invalid).is_err());
    assert_eq!(world.world_data(), current);
    world.revert(&applied).unwrap();
    assert_eq!(
        world.get_field(entity, LodGroup::ID, "levels"),
        Some(Value::List(Vec::new()))
    );
    // 撤销恢复组件值，同时给实体记下新的修订号。
    let after = world.world_data();
    assert_eq!(after.entities.len(), 1);
    assert_eq!(after.entities[0].id, before.entities[0].id);
    assert_eq!(after.entities[0].components, before.entities[0].components);
    assert!(after.entities[0].revision > current.entities[0].revision);
}

fn event_world() -> (World, EntityId) {
    let mut world = World::new();
    let key = world.spawn((Animator {
        events: vec![AnimEvent::new("footstep", 0.25)],
        ..Animator::default()
    },));
    let entity = world.entity(key).unwrap();
    (world, entity)
}

fn edit(entity: EntityId, field: &str, value: Value) -> Command {
    Command::SetField {
        entity,
        component: Animator::ID,
        field: field.to_string(),
        value,
    }
}

#[test]
fn reads_writes_and_reverts_an_animation_event_path() {
    let (mut world, entity) = event_world();
    let hash = world.state_hash();
    assert_eq!(
        world.get_field(entity, Animator::ID, "events[0].time"),
        Some(Value::F64(0.25))
    );
    let plan = world.plan(vec![edit(entity, "events[0].time", Value::F64(0.5))]);
    let applied = world.apply(&plan).unwrap();
    let animator = world.get::<Animator>(world.key(entity).unwrap()).unwrap();
    assert_eq!(animator.events, [AnimEvent::new("footstep", 0.5)]);
    assert_eq!(world.state_hash(), hash);
    world.revert(&applied).unwrap();
    assert_eq!(
        world.get_field(entity, Animator::ID, "events[0].time"),
        Some(Value::F64(0.25))
    );
}

#[test]
fn rejects_invalid_nested_writes_without_changing_the_world() {
    let (mut world, entity) = event_world();
    let before = world.world_data();
    for (field, value, code) in [
        (
            "events[0].time",
            Value::F64(-0.5),
            codes::EDIT_TYPE_MISMATCH,
        ),
        (
            "events[0].time",
            Value::F64(f64::INFINITY),
            codes::EDIT_TYPE_MISMATCH,
        ),
        (
            "events[0].time",
            Value::String("now".into()),
            codes::EDIT_TYPE_MISMATCH,
        ),
        ("events[1].time", Value::F64(0.5), codes::EDIT_UNKNOWN_FIELD),
        ("events[0]time", Value::F64(0.5), codes::EDIT_UNKNOWN_FIELD),
    ] {
        let plan = world.plan(vec![
            edit(entity, "clip", Value::String("Walk".into())),
            edit(entity, field, value),
        ]);
        assert!(!plan.is_valid());
        assert_eq!(plan.diagnostics[0].code, code, "{field}");
        assert!(world.apply(&plan).is_err());
        assert_eq!(world.world_data(), before);
    }
}

#[test]
fn data_validation_reports_the_path_of_an_out_of_range_leaf() {
    let (world, _) = event_world();
    let mut data = world.world_data();
    let fields = &mut data.entities[0].components[0].fields;
    let (_, events) = fields
        .iter_mut()
        .find(|(name, _)| name == "events")
        .unwrap();
    xgc_reflect::FieldPath::parse("events[0].time")
        .unwrap()
        .replace(events, Value::F64(-0.5))
        .unwrap();
    let diagnostics = World::validate_data(&data, &xgc_world::builtin_registry());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, codes::WORLD_RANGE);
    assert!(diagnostics[0]
        .subject
        .to_string()
        .contains("events[0].time"));
}

#[test]
fn resolves_paths_against_preceding_edits_in_the_same_plan() {
    let (mut world, entity) = event_world();
    let entry = world.get_field(entity, Animator::ID, "events[0]").unwrap();
    let plan = world.plan(vec![
        edit(entity, "events", Value::List(vec![entry.clone(), entry])),
        edit(entity, "events[1].time", Value::F64(0.75)),
    ]);
    let applied = world.apply(&plan).unwrap();
    assert_eq!(
        world.get_field(entity, Animator::ID, "events[1].time"),
        Some(Value::F64(0.75))
    );
    world.revert(&applied).unwrap();
    assert_eq!(
        world.get_field(entity, Animator::ID, "events[1].time"),
        None
    );
}
