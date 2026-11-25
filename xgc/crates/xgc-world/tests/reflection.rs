//! 字段路径必须沿事务中的当前值解析，逆命令按同一路径还原。

use xgc_base::{diagnostics as codes, EntityId};
use xgc_world::{AnimEvent, Animator, Command, Component, Value, World};

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
